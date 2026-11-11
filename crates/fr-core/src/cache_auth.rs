//! Local cache authentication. The key never leaves the private configuration
//! directory. A MAC proves only that this installation saved these bytes, not
//! publisher identity or that another installation validated the design.

use std::path::PathBuf;
use ring::{hmac, rand::{SecureRandom, SystemRandom}};

const COMMENT: &[u8] = b"Ferrender-local-cache-HMAC-SHA256-v1:";
const DOMAIN: &[u8] = b"Ferrender native ZIP cache authentication v1\0";
const KEY_BYTES: usize = 32;

pub(crate) struct Store { directory: PathBuf }

impl Store {
    pub(crate) fn local() -> Option<Self> {
        let set = |name| std::env::var_os(name).filter(|s| !s.is_empty()).map(PathBuf::from);
        // Match the app configuration precedence. Refuse relative paths rather
        // than accidentally creating a secret in a project or working tree.
        let base = set("FERRENDER_CONFIG_DIR")
            .or_else(|| set("XDG_CONFIG_HOME").map(|p| p.join("ferrender")))
            .or_else(|| set("HOME").or_else(|| set("USERPROFILE")).map(|p| p.join(".config/ferrender")))?;
        base.is_absolute().then(|| Self { directory: base.join("cache-auth") })
    }

    #[cfg(unix)]
    fn key(&self, create: bool) -> Option<hmac::Key> {
        use std::{fs::{DirBuilder, OpenOptions}, io::{Read, Write}, os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt}};
        let uid = unsafe { libc::geteuid() };
        let base = self.directory.parent()?;
        if create { std::fs::create_dir_all(base).ok()?; }
        let parent = std::fs::metadata(base).ok()?;
        if !parent.is_dir() || parent.uid() != uid || parent.mode() & 0o022 != 0 { return None; }
        if create {
            match DirBuilder::new().mode(0o700).create(&self.directory) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(_) => return None,
            }
        }
        let directory = std::fs::symlink_metadata(&self.directory).ok()?;
        if !directory.is_dir() || directory.uid() != uid || directory.mode() & 0o077 != 0 { return None; }
        let path = self.directory.join("key");
        if create && std::fs::symlink_metadata(&path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
            let mut secret = [0; KEY_BYTES];
            SystemRandom::new().fill(&mut secret).ok()?;
            // A temporary private file + exclusive hard-link publication avoids
            // readers seeing a partial key and never replaces an existing key.
            let mut suffix = [0; 8];
            SystemRandom::new().fill(&mut suffix).ok()?;
            let temporary = self.directory.join(format!("key-{:016x}.tmp", u64::from_le_bytes(suffix)));
            let mut output = OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary).ok()?;
            let written = output.write_all(&secret).and_then(|_| output.sync_all());
            drop(output);
            let published = written.and_then(|_| std::fs::hard_link(&temporary, &path));
            let _ = std::fs::remove_file(&temporary);
            match published {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(_) => return None,
            }
        }
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        if !metadata.is_file() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 || metadata.len() != KEY_BYTES as u64 || metadata.nlink() != 1 { return None; }
        let mut file = OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(&path).ok()?;
        let opened = file.metadata().ok()?;
        if opened.dev() != metadata.dev() || opened.ino() != metadata.ino() { return None; }
        let mut secret = [0; KEY_BYTES];
        file.read_exact(&mut secret).ok()?;
        // Derive a domain-specific key, then use ring's constant-time verifier
        // directly on the original ZIP slice without copying a large archive.
        let root = hmac::Key::new(hmac::HMAC_SHA256, &secret);
        let derived = hmac::sign(&root, DOMAIN);
        Some(hmac::Key::new(hmac::HMAC_SHA256, derived.as_ref()))
    }

    // No ACL-backed key store is implemented for other targets. Rebuilding is
    // safe and fully functional; never create a loosely protected key instead.
    #[cfg(not(unix))]
    fn key(&self, _: bool) -> Option<hmac::Key> { None }

    pub(crate) fn seal(&self, bytes: &mut Vec<u8>) {
        let Some(key) = self.key(true) else { return };
        if bytes.len() < 22 || &bytes[bytes.len()-22..bytes.len()-18] != b"PK\x05\x06" || !bytes.ends_with(&[0, 0]) { return; }
        // Only the EOCD comment-length field is omitted from the MAC. Its value
        // is fixed by the versioned comment format and checked by verify(). All
        // ZIP entries, directory records and offsets are authenticated as bytes.
        let end = bytes.len() - 2;
        let tag = hmac::sign(&key, &bytes[..end]);
        bytes[end..].copy_from_slice(&((COMMENT.len() + tag.as_ref().len()) as u16).to_le_bytes());
        bytes.extend_from_slice(COMMENT);
        bytes.extend_from_slice(tag.as_ref());
    }

    pub(crate) fn verify(&self, bytes: &[u8], comment: &[u8]) -> bool {
        if comment.len() != COMMENT.len() + 32 || !comment.starts_with(COMMENT) || bytes.len() < 22 + comment.len() { return false; }
        let end = bytes.len() - comment.len() - 2;
        if &bytes[end-20..end-16] != b"PK\x05\x06" || bytes[end..end+2] != (comment.len() as u16).to_le_bytes() || &bytes[end+2..] != comment { return false; }
        let Some(key) = self.key(false) else { return false };
        hmac::verify(&key, &bytes[..end], &comment[COMMENT.len()..]).is_ok()
    }
}

#[cfg(test)]
impl Store {
    pub(crate) fn in_directory(directory: PathBuf) -> Self { Self { directory } }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    fn store(name: &str) -> Store {
        let base = std::env::temp_dir().join(format!("ferrender-cache-auth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::DirBuilder::new().mode(0o700).create(&base).unwrap();
        Store::in_directory(base.join("private"))
    }
    fn empty_zip() -> Vec<u8> { zip::ZipWriter::new(std::io::Cursor::new(Vec::new())).finish().unwrap().into_inner() }
    fn verifies(store: &Store, bytes: &[u8]) -> bool {
        let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        store.verify(bytes, archive.comment())
    }

    #[test]
    fn private_local_key_authenticates_bytes_not_other_installations() {
        let local = store("local");
        let foreign = store("foreign");
        let mut bytes = empty_zip();
        local.seal(&mut bytes);
        assert!(verifies(&local, &bytes));
        assert!(!verifies(&foreign, &bytes));
        assert!(!foreign.directory.exists(), "verification never creates a key");
        let secret = std::fs::read(local.directory.join("key")).unwrap();
        assert!(!bytes.windows(secret.len()).any(|w| w == secret), "designs never carry the key");
        assert_eq!(std::fs::metadata(local.directory.join("key")).unwrap().permissions().mode() & 0o777, 0o600);
        std::fs::remove_dir_all(local.directory.parent().unwrap()).unwrap();
        std::fs::remove_dir_all(foreign.directory.parent().unwrap()).unwrap();
    }

    #[test]
    fn missing_rotated_or_exposed_keys_fail_closed() {
        let local = store("rotate");
        let mut old = empty_zip();
        local.seal(&mut old);
        let key = local.directory.join("key");
        std::fs::remove_file(&key).unwrap();
        assert!(!verifies(&local, &old));
        assert!(!key.exists());
        let mut new = empty_zip();
        local.seal(&mut new);
        assert!(verifies(&local, &new));
        assert!(!verifies(&local, &old));
        std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!verifies(&local, &new));
        let mut unsigned = empty_zip();
        local.seal(&mut unsigned);
        assert_eq!(unsigned, empty_zip());
        std::fs::remove_dir_all(local.directory.parent().unwrap()).unwrap();
    }

    #[test]
    fn linked_keys_and_appended_or_changed_authentication_are_refused() {
        let local = store("links");
        let mut bytes = empty_zip();
        local.seal(&mut bytes);
        let comment = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap().comment().to_vec();
        let mut changed = bytes.clone();
        changed[4] ^= 1;
        assert!(!local.verify(&changed, &comment));
        changed = bytes.clone();
        changed.push(0);
        assert!(!local.verify(&changed, &comment));
        let key = local.directory.join("key");
        let outside = local.directory.parent().unwrap().join("key-outside");
        std::fs::rename(&key, &outside).unwrap();
        std::os::unix::fs::symlink(&outside, &key).unwrap();
        assert!(!verifies(&local, &bytes));
        let before = std::fs::read(&outside).unwrap();
        let mut unsigned = empty_zip();
        local.seal(&mut unsigned);
        assert_eq!(unsigned, empty_zip());
        assert_eq!(std::fs::read(&outside).unwrap(), before);
        std::fs::remove_dir_all(local.directory.parent().unwrap()).unwrap();
    }
}
