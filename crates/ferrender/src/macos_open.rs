//! Receive Finder/Open With document events without replacing winit's app delegate.
//! Register before launch and again at will-finish-launching, after AppKit installs
//! its defaults but before it delivers the startup document event.
use std::{ffi::CStr, os::unix::ffi::OsStrExt, path::PathBuf};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2::rc::Retained;
use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter, NSObject, NSObjectProtocol};
use crate::open_requests::OpenRequests;

const CORE: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

define_class!(
    // NSObject has no subclassing requirements. All callbacks run on the main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = OpenRequests]
    struct DocumentEvents;
    unsafe impl NSObjectProtocol for DocumentEvents {}
    impl DocumentEvents {
        #[unsafe(method(applicationWillFinishLaunching:))]
        fn will_launch(&self, _notification: &NSNotification) { self.register(); }

        // NSAppleEventManager's documented two-descriptor handler signature.
        #[unsafe(method(openDocuments:withReplyEvent:))]
        fn open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            self.ivars().push(document_path(event));
        }
    }
);

impl DocumentEvents {
    fn register(&self) {
        // AEEventClass/AEEventID are 32-bit four-character codes. The Foundation
        // binding gates these selectors on optional CoreServices bindings; using
        // their documented ABI avoids adding that framework just for the codes.
        let _: () = unsafe { msg_send![&*NSAppleEventManager::sharedAppleEventManager(),
            setEventHandler: self, andSelector: sel!(openDocuments:withReplyEvent:),
            forEventClass: CORE, andEventID: OPEN_DOCUMENTS] };
    }
}

fn document_path(event: &NSAppleEventDescriptor) -> Result<PathBuf, String> {
    // The non-owning Foundation accessor is retained by msg_send's return conversion.
    let list: Option<Retained<NSAppleEventDescriptor>> = unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
    let list = list.ok_or("macOS did not provide a document to open.")?;
    if list.numberOfItems() != 1 { return Err("Open one document at a time in Ferrender.".into()); }
    let descriptor = list.descriptorAtIndex(1).ok_or("macOS did not provide a valid document.")?;
    // fileURLValue also coerces arbitrary strings into paths. Accept only the
    // file descriptor types used by Launch Services, never text or remote URLs.
    let kind: u32 = unsafe { msg_send![&*descriptor, descriptorType] };
    if ![u32::from_be_bytes(*b"furl"), u32::from_be_bytes(*b"bmrk"),
         u32::from_be_bytes(*b"alis"), u32::from_be_bytes(*b"fsrf")].contains(&kind) {
        return Err("macOS did not provide a local file descriptor.".into());
    }
    let url = descriptor.fileURLValue().ok_or("macOS did not provide a local file URL.")?;
    if !url.isFileURL() { return Err("Ferrender can only open local documents.".into()); }
    // NSURL owns this NUL-terminated filesystem representation. Copy the bytes
    // while the URL is alive, preserving spaces, Unicode and non-UTF8 filenames.
    let bytes = unsafe { CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) }.to_bytes();
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(bytes));
    if !path.is_absolute() { return Err("macOS did not provide an absolute file path.".into()); }
    Ok(path)
}

pub struct Receiver(Retained<DocumentEvents>);

pub fn install(requests: OpenRequests) -> Receiver {
    let main = MainThreadMarker::new().expect("Install document events on the application main thread");
    let receiver: Retained<DocumentEvents> = unsafe { msg_send![super(DocumentEvents::alloc(main).set_ivars(requests)), init] };
    unsafe {
        NSNotificationCenter::defaultCenter().addObserver_selector_name_object(
            &receiver, sel!(applicationWillFinishLaunching:), Some(NSApplicationWillFinishLaunchingNotification), None);
    }
    receiver.register();
    Receiver(receiver)
}

impl Drop for Receiver {
    fn drop(&mut self) {
        // Kept alive by main until the event loop exits; unregister before release.
        unsafe {
            NSNotificationCenter::defaultCenter().removeObserver(&self.0);
            let _: () = msg_send![&*NSAppleEventManager::sharedAppleEventManager(),
                removeEventHandlerForEventClass: CORE, andEventID: OPEN_DOCUMENTS];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::ClassType;
    use objc2_foundation::{NSString, NSURL, NSURLBookmarkCreationOptions};

    fn event(items: &[Retained<NSAppleEventDescriptor>]) -> Retained<NSAppleEventDescriptor> {
        let event: Retained<NSAppleEventDescriptor> = unsafe { msg_send![NSAppleEventDescriptor::class(),
            appleEventWithEventClass: CORE, eventID: OPEN_DOCUMENTS,
            targetDescriptor: None::<&NSAppleEventDescriptor>, returnID: -1_i16, transactionID: 0_i32] };
        let list = NSAppleEventDescriptor::listDescriptor();
        for (index, item) in items.iter().enumerate() { list.insertDescriptor_atIndex(item, index as isize + 1); }
        let _: () = unsafe { msg_send![&*event, setParamDescriptor: &*list, forKeyword: DIRECT_OBJECT] };
        event
    }

    #[test]
    fn finder_file_url_preserves_spaces_unicode_and_literal_percent() {
        let expected = "/tmp/a folder/tracing 100% 雪.ferr";
        let url = NSURL::fileURLWithPath(&NSString::from_str(expected));
        let descriptor = NSAppleEventDescriptor::descriptorWithFileURL(&url);
        assert_eq!(document_path(&event(&[descriptor])).unwrap(), PathBuf::from(expected));
    }

    #[test]
    fn finder_bookmark_resolves_to_local_file() {
        let expected = std::env::current_exe().unwrap().canonicalize().unwrap();
        let url = NSURL::fileURLWithPath(&NSString::from_str(expected.to_str().unwrap()));
        // Current Launch Services sends bookmarks (bmrk), not plain file URLs.
        let data = url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
            NSURLBookmarkCreationOptions::empty(), None, None).unwrap();
        let bookmark: Option<Retained<NSAppleEventDescriptor>> = unsafe {
            msg_send![NSAppleEventDescriptor::class(),
                descriptorWithDescriptorType: u32::from_be_bytes(*b"bmrk"), data: &*data]
        };
        assert_eq!(document_path(&event(&[bookmark.unwrap()])).unwrap(), expected);
    }

    #[test]
    fn malformed_or_multiple_desktop_documents_are_not_silently_opened() {
        assert!(document_path(&event(&[])).is_err());
        let descriptor = NSAppleEventDescriptor::descriptorWithString(&NSString::from_str("not a file"));
        assert!(document_path(&event(&[descriptor])).is_err());
        let url = NSURL::fileURLWithPath(&NSString::from_str("/tmp/design.ferr"));
        let descriptor = NSAppleEventDescriptor::descriptorWithFileURL(&url);
        assert!(document_path(&event(&[descriptor.clone(), descriptor])).unwrap_err().contains("one document"));
    }
}
