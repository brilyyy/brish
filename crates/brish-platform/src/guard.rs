//! RAII guards for file descriptors and terminal state.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};

use crate::{PlatformError, Terminal, TermiosState};

/// Owns a raw file descriptor and closes it on drop.
///
/// `OwnedFd` performs the close; this wrapper only constrains the API.
#[derive(Debug)]
pub struct FdGuard(OwnedFd);

impl FdGuard {
    /// Create a guard wrapping a raw fd.
    ///
    /// # Safety
    /// The caller guarantees `fd` is a valid, owned open file descriptor.
    pub unsafe fn from_raw(fd: RawFd) -> Result<Self, PlatformError> {
        // SAFETY: caller guarantees `fd` is valid and owned.
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }

    /// Release the fd without closing it.
    pub fn into_raw_fd(self) -> RawFd {
        self.0.into_raw_fd()
    }

    /// Borrow the underlying raw fd.
    pub fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl AsFd for FdGuard {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

/// Restores terminal state on drop.
///
/// Drops must not be bounded by `Platform`, only by `Terminal`
/// (Drop impls cannot add trait bounds the struct does not have).
#[derive(Debug)]
pub struct RawModeGuard<'a, P: Terminal> {
    platform: &'a P,
    saved: TermiosState,
    fd: RawFd,
}

impl<'a, P: Terminal> RawModeGuard<'a, P> {
    /// Put `fd` into raw mode; the previous state is restored on drop.
    pub fn new(platform: &'a P, fd: RawFd) -> Result<Self, PlatformError> {
        let saved = platform.make_raw(fd)?;
        Ok(Self {
            platform,
            saved,
            fd,
        })
    }
}

impl<P: Terminal> Drop for RawModeGuard<'_, P> {
    fn drop(&mut self) {
        // Restore terminal state; ignore errors during teardown.
        let _ = self.platform.restore(self.fd, &self.saved);
    }
}
