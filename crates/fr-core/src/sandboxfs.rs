//! File access for scripts that does not trust a path between checking it and
//! using it. A [`Dir`] is an open directory handle reached from an allowed
//! root by walking one component at a time without following links, so a
//! link or a swapped folder planted between the check and the use is refused
//! instead of followed. Files are created and renamed relative to that
//! handle. Another process running as the same user can still do whatever
//! the user can; this keeps the script itself, and anything racing its
//! entries, inside the allowed folders.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::path::{Component, Path, PathBuf};

type R<T> = Result<T, String>;

/// An open directory inside an allowed root.
pub struct Dir {
    #[cfg(unix)]
    file: File,
    path: PathBuf,
}

impl Dir {
    /// The directory's path, for messages and for listing.
    pub fn path(&self) -> &Path { &self.path }
}

/// Opens `root/rel` by descending one component at a time without following
/// links. `rel` must be relative and may not climb (`..`).
pub fn open_beneath(root: &Path, rel: &Path) -> R<Dir> {
    let mut dir = Dir::open_root(root)?;
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(name) => dir = dir.descend(name)?,
            _ => return Err(format!("{} climbs out of its folder", rel.display())),
        }
    }
    Ok(dir)
}

fn describe(what: &str, path: &Path, e: std::io::Error) -> String {
    match e.raw_os_error() {
        #[cfg(unix)]
        Some(libc::ELOOP) | Some(libc::EMLINK) => format!("{what} {}: it is a link, and links inside the allowed folders are not followed", path.display()),
        _ => format!("{what} {}: {e}", path.display()),
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::{AsRawFd, FromRawFd};

    fn cstr(name: &OsStr) -> std::io::Result<CString> {
        CString::new(name.as_bytes()).map_err(|_| std::io::Error::other("a file name contains a NUL byte"))
    }

    fn open_at(dir: &File, name: &OsStr, flags: i32, mode: u32) -> std::io::Result<File> {
        let c = cstr(name)?;
        let fd = unsafe { libc::openat(dir.as_raw_fd(), c.as_ptr(), flags | libc::O_CLOEXEC | libc::O_NOFOLLOW, mode as libc::c_uint) };
        if fd < 0 { return Err(std::io::Error::last_os_error()); }
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    fn check(rc: i32) -> std::io::Result<()> {
        if rc < 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
    }

    impl Dir {
        pub fn try_clone(&self) -> R<Dir> {
            Ok(Dir { file: self.file.try_clone().map_err(|e| e.to_string())?, path: self.path.clone() })
        }

        pub(super) fn open_root(root: &Path) -> R<Dir> {
            let c = cstr(root.as_os_str()).map_err(|e| e.to_string())?;
            let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC) };
            if fd < 0 { return Err(describe("could not open the folder", root, std::io::Error::last_os_error())); }
            Ok(Dir { file: unsafe { File::from_raw_fd(fd) }, path: root.to_path_buf() })
        }

        pub(super) fn descend(&self, name: &OsStr) -> R<Dir> {
            let path = self.path.join(name);
            let file = open_at(&self.file, name, libc::O_RDONLY | libc::O_DIRECTORY, 0).map_err(|e| describe("could not open the folder", &path, e))?;
            Ok(Dir { file, path })
        }

        /// Opens an existing regular file for reading; links and devices are refused.
        pub fn open_read(&self, name: &OsStr) -> R<File> {
            let path = self.path.join(name);
            // A FIFO must not block before fstat can reject it. O_NONBLOCK has
            // no effect on regular files.
            let file = open_at(&self.file, name, libc::O_RDONLY | libc::O_NONBLOCK, 0).map_err(|e| describe("could not open", &path, e))?;
            if !file.metadata().map_err(|e| e.to_string())?.is_file() { return Err(format!("{} is not a regular file", path.display())); }
            Ok(file)
        }

        /// Creates a file that did not exist; an existing entry of any kind is an error.
        pub fn create_new(&self, name: &OsStr) -> std::io::Result<File> {
            open_at(&self.file, name, libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL, 0o600)
        }

        /// Renames one entry of this directory onto another, replacing the target entry
        /// (not its contents: a link or a hard-linked file at the target is unlinked, never written).
        pub fn rename(&self, from: &OsStr, to: &OsStr) -> R<()> {
            let (f, t) = (cstr(from).map_err(|e| e.to_string())?, cstr(to).map_err(|e| e.to_string())?);
            check(unsafe { libc::renameat(self.file.as_raw_fd(), f.as_ptr(), self.file.as_raw_fd(), t.as_ptr()) })
                .map_err(|e| format!("could not move {} into place: {e}", self.path.join(to).display()))
        }

        /// Retain the old directory entry for rollback without following links.
        pub fn link(&self, from: &OsStr, to: &OsStr) -> std::io::Result<()> {
            let (f, t) = (cstr(from)?, cstr(to)?);
            check(unsafe { libc::linkat(self.file.as_raw_fd(), f.as_ptr(), self.file.as_raw_fd(), t.as_ptr(), 0) })
        }

        pub fn unlink(&self, name: &OsStr) -> R<()> {
            let c = cstr(name).map_err(|e| e.to_string())?;
            check(unsafe { libc::unlinkat(self.file.as_raw_fd(), c.as_ptr(), 0) }).map_err(|e| format!("could not remove {}: {e}", self.path.join(name).display()))
        }

        /// Makes one folder level; an existing folder is fine.
        pub fn mkdir(&self, name: &OsStr) -> R<()> {
            let c = cstr(name).map_err(|e| e.to_string())?;
            match check(unsafe { libc::mkdirat(self.file.as_raw_fd(), c.as_ptr(), 0o755) }) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && self.entry(name) == Entry::Dir => Ok(()),
                Err(e) => Err(format!("could not create {}: {e}", self.path.join(name).display())),
            }
        }

        /// What an entry is right now, without following a link.
        pub fn entry(&self, name: &OsStr) -> Entry {
            let Ok(c) = cstr(name) else { return Entry::Other };
            let mut st: libc::stat = unsafe { std::mem::zeroed() };
            if unsafe { libc::fstatat(self.file.as_raw_fd(), c.as_ptr(), &mut st, libc::AT_SYMLINK_NOFOLLOW) } != 0 { return Entry::Missing; }
            match st.st_mode & libc::S_IFMT {
                libc::S_IFREG => Entry::File,
                libc::S_IFDIR => Entry::Dir,
                _ => Entry::Other,
            }
        }
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;

    fn not_link(path: &Path) -> R<()> {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => Err(format!("{} is a link, and links inside the allowed folders are not followed", path.display())),
            Ok(_) => Ok(()),
            Err(e) => Err(format!("could not inspect {}: {e}", path.display())),
        }
    }

    impl Dir {
        pub fn try_clone(&self) -> R<Dir> { Ok(Dir { path: self.path.clone() }) }

        pub(super) fn open_root(root: &Path) -> R<Dir> {
            if !root.is_dir() { return Err(format!("{} is not a folder", root.display())); }
            Ok(Dir { path: root.to_path_buf() })
        }

        pub(super) fn descend(&self, name: &OsStr) -> R<Dir> {
            let path = self.path.join(name);
            not_link(&path)?;
            if !path.is_dir() { return Err(format!("{} is not a folder", path.display())); }
            Ok(Dir { path })
        }

        pub fn open_read(&self, name: &OsStr) -> R<File> {
            let path = self.path.join(name);
            not_link(&path)?;
            let file = File::open(&path).map_err(|e| describe("could not open", &path, e))?;
            if !file.metadata().map_err(|e| e.to_string())?.is_file() { return Err(format!("{} is not a regular file", path.display())); }
            Ok(file)
        }

        pub fn create_new(&self, name: &OsStr) -> std::io::Result<File> {
            std::fs::OpenOptions::new().write(true).create_new(true).open(self.path.join(name))
        }

        pub fn rename(&self, from: &OsStr, to: &OsStr) -> R<()> {
            std::fs::rename(self.path.join(from), self.path.join(to)).map_err(|e| format!("could not move {} into place: {e}", self.path.join(to).display()))
        }

        pub fn link(&self, from: &OsStr, to: &OsStr) -> std::io::Result<()> {
            std::fs::hard_link(self.path.join(from), self.path.join(to))
        }

        pub fn unlink(&self, name: &OsStr) -> R<()> {
            std::fs::remove_file(self.path.join(name)).map_err(|e| format!("could not remove {}: {e}", self.path.join(name).display()))
        }

        pub fn mkdir(&self, name: &OsStr) -> R<()> {
            match std::fs::create_dir(self.path.join(name)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && self.entry(name) == Entry::Dir => Ok(()),
                Err(e) => Err(format!("could not create {}: {e}", self.path.join(name).display())),
            }
        }

        pub fn entry(&self, name: &OsStr) -> Entry {
            match std::fs::symlink_metadata(self.path.join(name)) {
                Err(_) => Entry::Missing,
                Ok(m) if m.is_file() => Entry::File,
                Ok(m) if m.is_dir() => Entry::Dir,
                Ok(_) => Entry::Other,
            }
        }
    }
}

/// What a directory entry is, without following links.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry { Missing, File, Dir, Other }

/// A temporary name beside `name` that another process cannot predict.
pub fn temp_name(name: &OsStr) -> OsString {
    use std::hash::{BuildHasher, Hasher};
    let salt = std::collections::hash_map::RandomState::new().build_hasher().finish();
    let mut out = OsString::from(".");
    out.push(name);
    out.push(format!(".{salt:016x}.ferrtmp"));
    out
}
