//! Read the desktop preference independently of a window's appearance override.
//! Native window events alone omit Linux settings and can be stale on macOS.

use egui::{Context, Theme};

pub struct NativeTheme {
    #[cfg(target_os = "linux")]
    value: std::sync::Arc<std::sync::atomic::AtomicU8>,
    #[cfg(target_os = "linux")]
    stop: Option<std::sync::mpsc::Sender<()>>,
}

impl NativeTheme {
    pub fn new(_ctx: &Context) -> Self {
        #[cfg(target_os = "linux")]
        {
            use std::sync::{Arc, atomic::AtomicU8, mpsc};
            let value = Arc::new(AtomicU8::new(0));
            // Headless UI tests supply their own RawInput system theme.
            if cfg!(test) { return Self { value, stop: None }; }
            let (send, receive) = mpsc::channel();
            let current = value.clone();
            let context = _ctx.clone();
            let started = std::thread::Builder::new().name("ferrender-appearance".into())
                .spawn(move || linux::watch(current, context, receive)).is_ok();
            Self { value, stop: started.then_some(send) }
        }
        #[cfg(not(target_os = "linux"))]
        Self {}
    }

    /// Cheap, nonblocking on the UI thread. Unknown preferences use the app fallback.
    pub fn current(&self) -> Option<Theme> {
        if cfg!(test) { return None; }
        #[cfg(target_os = "macos")]
        {
            use objc2::MainThreadMarker;
            use objc2_app_kit::{NSApplication, NSAppearanceNameAqua, NSAppearanceNameDarkAqua};
            use objc2_foundation::NSArray;
            let main = MainThreadMarker::new()?;
            // Query NSApplication, not NSWindow: a forced window appearance must not
            // hide OS changes while the user has selected Light or Dark in Ferrender.
            let appearance = NSApplication::sharedApplication(main).effectiveAppearance();
            // AppKit's immutable appearance names are safe to read on the main thread.
            let (light, dark) = unsafe { (NSAppearanceNameAqua, NSAppearanceNameDarkAqua) };
            let matched = appearance.bestMatchFromAppearancesWithNames(&NSArray::from_slice(&[light, dark]))?;
            Some(if *matched == *dark { Theme::Dark } else { Theme::Light })
        }
        #[cfg(target_os = "linux")]
        {
            decode(self.value.load(std::sync::atomic::Ordering::Relaxed))
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        None
    }
}

#[cfg(target_os = "linux")]
impl Drop for NativeTheme {
    fn drop(&mut self) {
        // Wake the worker immediately if sleeping. In-flight I/O has a short timeout;
        // never join it here and make closing a window wait on a desktop service.
        if let Some(stop) = &self.stop { let _ = stop.send(()); }
    }
}

#[cfg(any(target_os = "linux", test))]
fn decode(value: u8) -> Option<Theme> {
    match value { 1 => Some(Theme::Dark), 2 => Some(Theme::Light), _ => None }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicU8, Ordering}, mpsc::{Receiver, RecvTimeoutError, TryRecvError}};
    use std::time::Duration;
    use zbus::{blocking::Connection, zvariant::{OwnedValue, Value}};

    const TIMEOUT: Duration = Duration::from_secs(1);

    fn connect() -> zbus::Result<Connection> {
        let builder = zbus::connection::Builder::session()?.method_timeout(TIMEOUT);
        // method_timeout bounds calls, but not socket connection/authentication.
        let connection = async_io::block_on(futures_lite::future::race(builder.build(), async {
            async_io::Timer::after(TIMEOUT).await;
            Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "Desktop settings connection timed out").into())
        }))?;
        Ok(connection.into())
    }

    fn setting(connection: &Connection) -> zbus::Result<u8> {
        let read = |method| connection.call_method(Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop", Some("org.freedesktop.portal.Settings"),
            method, &("org.freedesktop.appearance", "color-scheme"));
        // ReadOne (portal v2) has one variant layer; old Read replies have two.
        // https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Settings.html
        let reply = match read("ReadOne") {
            Err(zbus::Error::MethodError(name, _, _)) if name.as_str() == "org.freedesktop.DBus.Error.UnknownMethod" => read("Read")?,
            result => result?,
        };
        let value: OwnedValue = reply.body().deserialize()?;
        Ok(portal_value(&value))
    }

    fn portal_value(mut value: &Value<'_>) -> u8 {
        // Bound unwrapping even if a nonconforming portal returns nested variants.
        for _ in 0..4 {
            match value {
                Value::U32(1) => return 1,
                Value::U32(2) => return 2,
                Value::Value(inner) => value = inner,
                _ => return 0,
            }
        }
        0
    }

    pub fn watch(value: Arc<AtomicU8>, context: Context, stop: Receiver<()>) {
        let mut connection = None;
        loop {
            if !matches!(stop.try_recv(), Err(TryRecvError::Empty)) { break; }
            if connection.is_none() { connection = connect().ok(); }
            let result = connection.as_ref().map(setting);
            let (next, delay) = match result {
                Some(Ok(value)) => (value, Duration::from_secs(1)),
                _ => {
                    connection = None;
                    // A portal may appear later or restart; retry without flooding it.
                    (0, Duration::from_secs(5))
                }
            };
            if value.swap(next, Ordering::Relaxed) != next { context.request_repaint(); }
            match stop.recv_timeout(delay) {
                Err(RecvTimeoutError::Timeout) => {},
                _ => break,
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn portal_values_accept_legacy_variants_and_reject_unknown_values() {
            assert_eq!(portal_value(&Value::U32(1)), 1);
            assert_eq!(portal_value(&Value::U32(2)), 2);
            assert_eq!(portal_value(&Value::Value(Box::new(Value::U32(1)))), 1);
            assert_eq!(portal_value(&Value::U32(0)), 0);
            assert_eq!(portal_value(&Value::U32(99)), 0);
            assert_eq!(portal_value(&Value::Bool(true)), 0);
            assert_eq!(portal_value(&Value::from("dark")), 0);
            let deep = (0..8).fold(Value::U32(1), |inner, _| Value::Value(Box::new(inner)));
            assert_eq!(portal_value(&deep), 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_desktop_values_have_no_preference() {
        assert_eq!(decode(0), None);
        assert_eq!(decode(1), Some(Theme::Dark));
        assert_eq!(decode(2), Some(Theme::Light));
        assert_eq!(decode(255), None);
    }
}
