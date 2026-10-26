//! Descriptor-relative operations: never traverse symlinks, including ancestors.
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path};

unsafe extern "C" {
    fn openat(fd: i32, path: *const std::ffi::c_char, flags: i32, ...) -> i32;
    fn mkdirat(fd: i32, path: *const std::ffi::c_char, mode: u32) -> i32;
    fn renameat(
        oldfd: i32,
        old: *const std::ffi::c_char,
        newfd: i32,
        new: *const std::ffi::c_char,
    ) -> i32;
    fn linkat(
        oldfd: i32,
        old: *const std::ffi::c_char,
        newfd: i32,
        new: *const std::ffi::c_char,
        flags: i32,
    ) -> i32;
    fn unlinkat(fd: i32, path: *const std::ffi::c_char, flags: i32) -> i32;
}
#[cfg(target_os = "linux")]
const DIRECTORY: i32 = 0x10000;
#[cfg(target_os = "linux")]
const NOFOLLOW: i32 = 0x20000;
#[cfg(target_os = "linux")]
const CLOEXEC: i32 = 0x80000;
#[cfg(target_os = "linux")]
const CREATE: i32 = 0x40;
#[cfg(target_os = "linux")]
const EXCL: i32 = 0x80;
#[cfg(target_os = "linux")]
const APPEND: i32 = 0x400;
#[cfg(not(target_os = "linux"))]
const DIRECTORY: i32 = 0x100000;
#[cfg(not(target_os = "linux"))]
const NOFOLLOW: i32 = 0x100;
#[cfg(not(target_os = "linux"))]
const CLOEXEC: i32 = 0x1000000;
#[cfg(not(target_os = "linux"))]
const CREATE: i32 = 0x200;
#[cfg(not(target_os = "linux"))]
const EXCL: i32 = 0x800;
#[cfg(not(target_os = "linux"))]
const APPEND: i32 = 8;
#[cfg(target_os = "linux")]
const NONBLOCK: i32 = 0x800;
#[cfg(not(target_os = "linux"))]
const NONBLOCK: i32 = 4;

pub(super) struct Directory(File);
fn name(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(io::Error::other)
}
fn status(code: i32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}
impl Directory {
    pub fn open(path: &Path, create: bool) -> io::Result<Option<Self>> {
        if !path.is_absolute() {
            return Err(io::Error::other("Handoff path must be absolute"));
        }
        let mut dir = Self(File::open("/")?);
        for component in path.components() {
            match component {
                Component::RootDir | Component::CurDir => (),
                Component::ParentDir => {
                    dir = Self(dir.open_file(OsStr::new(".."), DIRECTORY)?);
                }
                Component::Normal(value) => match dir.child_os(value, create)? {
                    Some(child) => dir = child,
                    None => return Ok(None),
                },
                _ => return Err(io::Error::other("Unsupported handoff path component")),
            }
        }
        Ok(Some(dir))
    }
    fn open_file(&self, value: &OsStr, flags: i32) -> io::Result<File> {
        let value = name(value)?;
        // SAFETY: live directory fd and a NUL-terminated name; returned fd is owned.
        let fd = unsafe {
            openat(
                self.0.as_raw_fd(),
                value.as_ptr(),
                flags | NOFOLLOW | CLOEXEC | NONBLOCK,
                0o600u32,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub fn child(&self, value: &str, create: bool) -> io::Result<Option<Self>> {
        self.child_os(OsStr::new(value), create)
    }
    fn child_os(&self, value: &OsStr, create: bool) -> io::Result<Option<Self>> {
        match self.open_file(value, DIRECTORY) {
            Ok(file) => return Ok(Some(Self(file))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        if !create {
            return Ok(None);
        }
        let c = name(value)?;
        let result = status(unsafe { mkdirat(self.0.as_raw_fd(), c.as_ptr(), 0o700) });
        if let Err(e) = result {
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(e);
            }
        }
        self.open_file(value, DIRECTORY).map(|f| Some(Self(f)))
    }
    pub fn check_file(&self, value: &str) -> io::Result<bool> {
        let file = match self.open_file(OsStr::new(value), 0) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("Handoff state must be a regular file"));
        }
        Ok(true)
    }
    pub fn read(&self, value: &str) -> io::Result<Option<Vec<u8>>> {
        let mut file = match self.open_file(OsStr::new(value), 0) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("Handoff state must be a regular file"));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }
    pub fn append(&self, value: &str) -> io::Result<File> {
        let file = self.open_file(OsStr::new(value), 2 | CREATE | APPEND)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("Handoff state must be a regular file"));
        }
        Ok(file)
    }
    pub fn lock(&self) -> io::Result<File> {
        let file = self.append(".lock")?;
        file.lock()?;
        Ok(file)
    }
    pub fn same_child(&self, value: &str, other: &Self) -> io::Result<bool> {
        let Some(current) = self.child(value, false)? else {
            return Ok(false);
        };
        let a = current.0.metadata()?;
        let b = other.0.metadata()?;
        Ok(a.dev() == b.dev() && a.ino() == b.ino())
    }
    pub fn rename(&self, old: &str, new: &str) -> io::Result<()> {
        let old = name(OsStr::new(old))?;
        let new = name(OsStr::new(new))?;
        status(unsafe {
            renameat(
                self.0.as_raw_fd(),
                old.as_ptr(),
                self.0.as_raw_fd(),
                new.as_ptr(),
            )
        })?;
        self.0.sync_all()
    }
    // Publish a complete file once. Concurrent initializers never expose a
    // partial identity, replace an existing manifest, or require an outer lock.
    pub fn publish(&self, value: &str, bytes: &[u8]) -> io::Result<()> {
        if self.read(value)?.is_some() {
            return Ok(());
        }
        let temporary = format!(".handoff-{}.tmp", uuid::Uuid::new_v4());
        let mut file = self.open_file(OsStr::new(&temporary), 1 | CREATE | EXCL)?;
        let old = name(OsStr::new(&temporary))?;
        let new = name(OsStr::new(value))?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            let linked = status(unsafe {
                linkat(
                    self.0.as_raw_fd(),
                    old.as_ptr(),
                    self.0.as_raw_fd(),
                    new.as_ptr(),
                    0,
                )
            });
            if let Err(e) = linked {
                if e.kind() != io::ErrorKind::AlreadyExists {
                    return Err(e);
                }
            }
            self.0.sync_all()
        })();
        let cleanup = status(unsafe { unlinkat(self.0.as_raw_fd(), old.as_ptr(), 0) });
        result.and(cleanup)
    }
}
