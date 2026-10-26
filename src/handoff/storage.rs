//! Trust the canonical state root; refuse symlinks in all descendants.
use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Component, Path};
use std::time::{Duration, Instant};

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
    pub fn open(root: &Path, path: &Path, create: bool) -> io::Result<Option<Self>> {
        if !root.is_absolute() || path.is_absolute() {
            return Err(io::Error::other("Invalid handoff root or relative path"));
        }
        if create {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(root)?;
        }
        let root = match root.canonicalize() {
            Ok(root) => root,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let mut dir = Self(File::open(root)?);
        for component in path.components() {
            match component {
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
        // The variadic mode uses a promoted integer, including on macOS where
        // mode_t is u16 and cannot be passed directly through C varargs.
        let fd = unsafe {
            libc::openat(
                self.0.as_raw_fd(),
                value.as_ptr(),
                flags | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
                0o600 as libc::c_uint,
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
        match self.open_file(value, libc::O_DIRECTORY) {
            Ok(file) => return Ok(Some(Self(file))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        if !create {
            return Ok(None);
        }
        let c = name(value)?;
        let result = status(unsafe { libc::mkdirat(self.0.as_raw_fd(), c.as_ptr(), 0o700) });
        if let Err(e) = result {
            if e.kind() != io::ErrorKind::AlreadyExists {
                return Err(e);
            }
        }
        self.open_file(value, libc::O_DIRECTORY)
            .map(|f| Some(Self(f)))
    }
    pub fn check_file(&self, value: &str) -> io::Result<bool> {
        let file = match self.open_file(OsStr::new(value), libc::O_RDONLY) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        check_regular(&file)?;
        Ok(true)
    }
    pub fn read(&self, value: &str) -> io::Result<Option<Vec<u8>>> {
        let mut file = match self.open_file(OsStr::new(value), libc::O_RDONLY) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        check_regular(&file)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }
    pub fn append(&self, value: &str) -> io::Result<File> {
        let file = self.open_file(
            OsStr::new(value),
            libc::O_RDWR | libc::O_CREAT | libc::O_APPEND,
        )?;
        check_regular(&file)?;
        Ok(file)
    }
    pub fn lock(&self) -> io::Result<File> {
        let file = self.append(".lock")?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if started.elapsed() >= Duration::from_secs(10) {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "Handoff memory is busy: lock wait exceeded 10 seconds",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(std::fs::TryLockError::Error(e)) => return Err(e),
            }
        }
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
            libc::renameat(
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
        let mut file = self.open_file(
            OsStr::new(&temporary),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        )?;
        let old = name(OsStr::new(&temporary))?;
        let new = name(OsStr::new(value))?;
        let result = (|| {
            file.write_all(bytes)?;
            file.sync_all()?;
            let linked = status(unsafe {
                libc::linkat(
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
            Ok(())
        })();
        // Remove the publication link before syncing the directory, minimizing
        // the interval during which a reader observes two links.
        let cleanup = status(unsafe { libc::unlinkat(self.0.as_raw_fd(), old.as_ptr(), 0) });
        result.and(cleanup)?;
        self.0.sync_all()
    }
}

fn check_regular(file: &File) -> io::Result<()> {
    let started = Instant::now();
    loop {
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(io::Error::other("Handoff state must be a regular file"));
        }
        if metadata.nlink() == 1 {
            return Ok(());
        }
        // Atomic first publication briefly has a temporary second name. Never
        // read or write until that name is removed; persistent links fail closed.
        if started.elapsed() >= Duration::from_millis(50) {
            return Err(io::Error::other(
                "Handoff state must not be a hard link (link count must be 1)",
            ));
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
