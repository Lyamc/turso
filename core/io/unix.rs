use super::{Completion, File, OpenFlags, SharedWalLockKind, SharedWalMappedRegion, IO};
use crate::error::{io_error, CompletionError, LimboError};
use crate::io::clock::{Clock, DefaultClock, MonotonicInstant, WallClockInstant};
use crate::io::common;
use crate::io::FileSyncType;
use crate::Result;
use rustix::{
    fd::{AsFd, AsRawFd, BorrowedFd},
    fs::{self, FlockOperation},
    io::{self, IoSlice},
    mm::{self, MapFlags, ProtFlags},
    param,
    process::{FlockOffsetType, FlockType},
};
use std::ffi::c_void;
use std::os::fd::RawFd;
use std::ptr::NonNull;

use std::{io::ErrorKind, sync::Arc};
#[cfg(feature = "fs")]
use tracing::debug;
use tracing::{instrument, trace, Level};

// Darwin fails pwrite() and pwritev() calls with buffer size larger than INT_MAX so let's treat
// that as maximum buffer size.
const MAX_PWRITE_LEN: usize = i32::MAX as usize;

const MAX_IOV: usize = 1024;

#[repr(C)]
struct Iovec {
    iov_base: *mut c_void,
    iov_len: usize,
}

#[repr(C)]
struct RawFlock {
    l_type: i16,
    l_whence: i16,
    l_start: i64,
    l_len: i64,
    l_pid: i32,
    #[cfg(target_os = "freebsd")]
    l_sysid: i32,
}

#[cfg(target_os = "linux")]
mod fcntl_cmd {
    pub const F_SETLK: i32 = 6;
    pub const F_SETLKW: i32 = 7;
    pub const F_OFD_SETLK: i32 = 37;
    pub const F_OFD_SETLKW: i32 = 38;
}

#[cfg(not(target_os = "linux"))]
mod fcntl_cmd {
    pub const F_SETLK: i32 = 8;
    pub const F_SETLKW: i32 = 9;
}

extern "C" {
    fn fcntl(fd: std::os::raw::c_int, cmd: std::os::raw::c_int, lock: *mut RawFlock)
        -> std::os::raw::c_int;
}

fn fcntl_flock(fd: RawFd, cmd: i32, lock: &mut RawFlock) -> io::Result<()> {
    let rc = unsafe { fcntl(fd, cmd, lock) };
    if rc == -1 {
        Err(io::Errno::last_os_error())
    } else {
        Ok(())
    }
}

fn raw_flock(offset: u64, len: i64, typ: FlockType) -> RawFlock {
    RawFlock {
        l_type: typ as i16,
        l_whence: FlockOffsetType::Set as i16,
        l_start: offset as i64,
        l_len: len,
        l_pid: 0,
        #[cfg(target_os = "freebsd")]
        l_sysid: 0,
    }
}

pub struct UnixIO {}

impl UnixIO {
    #[cfg(feature = "fs")]
    pub fn new() -> Result<Self> {
        debug!("Using IO backend 'syscall'");
        Ok(Self {})
    }
}

impl Clock for UnixIO {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        DefaultClock.current_time_monotonic()
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        DefaultClock.current_time_wall_clock()
    }
}

impl IO for UnixIO {
    fn supports_shared_wal_coordination(&self) -> bool {
        true
    }

    fn open_file(&self, path: &str, flags: OpenFlags, _direct: bool) -> Result<Arc<dyn File>> {
        trace!("open_file(path = {})", path);
        let mut file = std::fs::File::options();
        file.read(true);

        if !flags.contains(OpenFlags::ReadOnly) {
            file.write(true);
            file.create(flags.contains(OpenFlags::Create));
        }

        let file = file.open(path).map_err(|e| io_error(e, "open"))?;

        #[allow(clippy::arc_with_non_send_sync)]
        let unix_file = Arc::new(UnixFile {
            file,
            path: path.to_string(),
        });
        if std::env::var(common::ENV_DISABLE_FILE_LOCK).is_err()
            && !flags.intersects(OpenFlags::ReadOnly | OpenFlags::NoLock)
        {
            unix_file.lock_file(true)?;
        }
        Ok(unix_file)
    }

    fn remove_file(&self, path: &str) -> Result<()> {
        std::fs::remove_file(path).map_err(|e| io_error(e, "remove_file"))?;
        Ok(())
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn step(&self) -> Result<()> {
        Ok(())
    }
}

pub struct UnixFile {
    file: std::fs::File,
    path: String,
}

pub(crate) struct UnixSharedWalMapping {
    mapping_ptr: NonNull<u8>,
    mapping_len: usize,
    ptr: NonNull<u8>,
    len: usize,
}

unsafe impl Send for UnixSharedWalMapping {}
unsafe impl Sync for UnixSharedWalMapping {}

impl SharedWalMappedRegion for UnixSharedWalMapping {
    fn ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    fn len(&self) -> usize {
        self.len
    }
}

impl Drop for UnixSharedWalMapping {
    fn drop(&mut self) {
        if let Err(err) = unsafe { mm::munmap(self.mapping_ptr.as_ptr().cast(), self.mapping_len) }
        {
            // Log rather than panic — panicking in Drop aborts if we're already
            // unwinding (double panic).
            tracing::error!(
                "munmap failed for shared WAL coordination region: {}",
                std::io::Error::from(err)
            );
        }
    }
}

pub(crate) fn unix_shared_wal_lock_byte(
    fd: RawFd,
    offset: u64,
    exclusive: bool,
    blocking: bool,
    kind: SharedWalLockKind,
) -> Result<bool> {
    let typ = if exclusive {
        FlockType::WriteLock
    } else {
        FlockType::ReadLock
    };
    let mut flock = raw_flock(offset, 1, typ);
    let cmd = match (kind, blocking) {
        #[cfg(target_os = "linux")]
        (SharedWalLockKind::LinuxOfd, true) => fcntl_cmd::F_OFD_SETLKW,
        #[cfg(target_os = "linux")]
        (SharedWalLockKind::LinuxOfd, false) => fcntl_cmd::F_OFD_SETLK,
        (SharedWalLockKind::ProcessScopedFcntl, true) => fcntl_cmd::F_SETLKW,
        (SharedWalLockKind::ProcessScopedFcntl, false) => fcntl_cmd::F_SETLK,
        #[cfg(not(target_os = "linux"))]
        (SharedWalLockKind::LinuxOfd, _) => {
            return Err(LimboError::InternalError(
                "linux OFD locks are not supported on this platform".into(),
            ))
        }
    };
    loop {
        match fcntl_flock(fd, cmd, &mut flock) {
            Ok(()) => return Ok(true),
            Err(err) if blocking && err == io::Errno::INTR => continue,
            Err(err) if !blocking && (err == io::Errno::AGAIN || err == io::Errno::ACCESS) => {
                return Ok(false);
            }
            Err(err) => {
                let error = std::io::Error::from(err);
                let message = match error.kind() {
                    ErrorKind::WouldBlock => {
                        "Failed locking shared WAL coordination file. File is locked by another process"
                            .to_string()
                    }
                    _ => format!("Failed locking shared WAL coordination file, {error}"),
                };
                return Err(LimboError::LockingError(message));
            }
        }
    }
}

pub(crate) fn unix_shared_wal_unlock_byte(
    fd: RawFd,
    offset: u64,
    kind: SharedWalLockKind,
) -> Result<()> {
    let mut flock = raw_flock(offset, 1, FlockType::Unlocked);
    let cmd = match kind {
        #[cfg(target_os = "linux")]
        SharedWalLockKind::LinuxOfd => fcntl_cmd::F_OFD_SETLK,
        SharedWalLockKind::ProcessScopedFcntl => fcntl_cmd::F_SETLK,
        #[cfg(not(target_os = "linux"))]
        SharedWalLockKind::LinuxOfd => {
            return Err(LimboError::InternalError(
                "linux OFD locks are not supported on this platform".into(),
            ))
        }
    };
    fcntl_flock(fd, cmd, &mut flock).map_err(|err| {
        LimboError::LockingError(format!(
            "Failed to release shared WAL coordination lock: {}",
            std::io::Error::from(err)
        ))
    })
}

pub(crate) fn unix_shared_wal_map(
    offset: u64,
    len: usize,
    fd: RawFd,
) -> Result<Box<dyn SharedWalMappedRegion>> {
    if len == 0 {
        return Err(LimboError::InternalError(
            "cannot mmap shared WAL coordination region with zero length".into(),
        ));
    }
    let page_size = param::page_size().map_err(|err| {
        LimboError::LockingError(format!(
            "failed to determine shared WAL mmap page size: {}",
            std::io::Error::from(err)
        ))
    })? as u64;
    let aligned_offset = offset / page_size * page_size;
    let prefix_len = (offset - aligned_offset) as usize;
    let mapping_len = prefix_len
        .checked_add(len)
        .ok_or_else(|| LimboError::InternalError("shared WAL mmap length overflow".into()))?;
    let mapping_ptr = unsafe {
        mm::mmap(
            std::ptr::null_mut(),
            mapping_len,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            BorrowedFd::borrow_raw(fd),
            aligned_offset,
        )
    };
    let mapping_ptr = match mapping_ptr {
        Ok(ptr) => ptr,
        Err(err) => {
            let error = std::io::Error::from(err);
            let file_size = unsafe { fs::fstat(BorrowedFd::borrow_raw(fd)) }
                .map(|stat| stat.st_size)
                .unwrap_or(-1);
            return Err(LimboError::LockingError(format!(
                "mmap shared WAL coordination file failed: {error} (offset={offset}, aligned_offset={aligned_offset}, len={len}, mapping_len={mapping_len}, fd={fd}, file_size={file_size})"
            )));
        }
    };
    let mapping_ptr =
        NonNull::new(mapping_ptr.cast::<u8>()).expect("mmap returned null for shared WAL map");
    let ptr = NonNull::new(unsafe { mapping_ptr.as_ptr().add(prefix_len) })
        .expect("aligned mmap base plus prefix_len returned null");
    Ok(Box::new(UnixSharedWalMapping {
        mapping_ptr,
        mapping_len,
        ptr,
        len,
    }))
}

impl File for UnixFile {
    fn lock_file(&self, exclusive: bool) -> Result<()> {
        let fd = self.file.as_fd();
        // F_SETLK is a non-blocking lock. The lock will be released when the file is closed
        // or the process exits or after an explicit unlock.
        fs::fcntl_lock(
            fd,
            if exclusive {
                FlockOperation::NonBlockingLockExclusive
            } else {
                FlockOperation::NonBlockingLockShared
            },
        )
        .map_err(|e| {
            let io_error = std::io::Error::from(e);
            let message = match io_error.kind() {
                ErrorKind::WouldBlock => format!(
                    "Failed locking file '{}'. File is locked by another process",
                    self.path
                ),
                _ => format!("Failed locking file '{}', {io_error}", self.path),
            };
            LimboError::LockingError(message)
        })?;

        Ok(())
    }

    fn unlock_file(&self) -> Result<()> {
        let fd = self.file.as_fd();
        fs::fcntl_lock(fd, FlockOperation::NonBlockingUnlock).map_err(|e| {
            LimboError::LockingError(format!(
                "Failed to release file lock: {}",
                std::io::Error::from(e)
            ))
        })?;
        Ok(())
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn pread(&self, pos: u64, c: Completion) -> Result<Completion> {
        let r = c.as_read();
        let buf = r.buf();
        let slice = buf.as_mut_slice();
        match io::pread(&self.file, slice, pos) {
            Ok(n) => {
                trace!("pread n: {}", n);
                c.complete(n as i32);
                Ok(c)
            }
            Err(err) => Err(io_error(std::io::Error::from(err), "pread")),
        }
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn pwrite(&self, pos: u64, buffer: Arc<crate::Buffer>, c: Completion) -> Result<Completion> {
        let buf_slice = buffer.as_slice();
        let total_size = buf_slice.len();

        let mut total_written = 0usize;
        let mut current_pos = pos;

        while total_written < total_size {
            let remaining_slice = &buf_slice[total_written..];
            let write_len = remaining_slice.len().min(MAX_PWRITE_LEN);
            match io::pwrite(&self.file, &remaining_slice[..write_len], current_pos) {
                Ok(0) => {
                    return Err(LimboError::CompletionError(CompletionError::IOError(
                        ErrorKind::UnexpectedEof,
                        "pwrite",
                    )));
                }
                Ok(written) => {
                    total_written += written;
                    current_pos += written as u64;
                    trace!("pwrite iteration: wrote {written}, total {total_written}/{total_size}");
                }
                Err(err) if err == io::Errno::INTR => continue,
                Err(err) => return Err(io_error(std::io::Error::from(err), "pwrite")),
            }
        }
        trace!("pwrite complete: wrote {total_written} bytes");
        c.complete(total_written as i32);
        Ok(c)
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn pwritev(
        &self,
        pos: u64,
        buffers: Vec<Arc<crate::Buffer>>,
        c: Completion,
    ) -> Result<Completion> {
        if buffers.len().eq(&1) {
            // use `pwrite` for single buffer
            return self.pwrite(pos, buffers[0].clone(), c);
        }

        let total_size: usize = buffers.iter().map(|b| b.as_slice().len()).sum();
        let mut iov: Vec<Iovec> = Vec::with_capacity(MAX_IOV);
        let mut buf_idx = 0;
        let mut buf_offset = 0;
        let mut total_written = 0usize;
        let mut current_pos = pos;

        // This loop converts buffers into MAX_IOV iovecs, submits them for I/O, and runs again.
        // If we we run out of iovecs before we convert a buffer in full, we keep track of buffer
        // offset, and resume conversion from there.
        loop {
            while buf_idx < buffers.len() {
                let buf = buffers[buf_idx].as_slice();
                buf_offset += buf_to_iovecs(&buf[buf_offset..], &mut iov, MAX_IOV);
                // If we ran out of iovecs, let's submit them for I/O.
                if buf_offset < buf.len() {
                    break;
                }
                // Buffer was fully conveted to iovec, move to next buffer.
                buf_idx += 1;
                buf_offset = 0;
            }
            if iov.is_empty() {
                break;
            }
            let ioslices = iovec_to_io_slices(&iov);
            match io::pwritev(&self.file, &ioslices, current_pos) {
                Ok(0) => {
                    return Err(LimboError::CompletionError(CompletionError::IOError(
                        ErrorKind::UnexpectedEof,
                        "pwritev",
                    )));
                }
                Ok(written) => {
                    total_written += written;
                    current_pos += written as u64;
                    trim_iovecs(&mut iov, written);
                    trace!("pwritev iteration: wrote {written}, total {total_written}/{total_size}");
                }
                Err(err) if err == io::Errno::INTR => continue,
                Err(err) => return Err(io_error(std::io::Error::from(err), "pwritev")),
            }
        }
        trace!("pwritev complete: wrote {total_written} bytes");
        c.complete(total_written as i32);
        Ok(c)
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn sync(&self, c: Completion, sync_type: FileSyncType) -> Result<Completion> {
        #[cfg(target_vendor = "apple")]
        {
            match sync_type {
                FileSyncType::Fsync => fs::fsync(&self.file).map_err(|err| {
                    io_error(std::io::Error::from(err), "sync")
                })?,
                FileSyncType::FullFsync => fs::fcntl_fullfsync(&self.file).map_err(|err| {
                    io_error(std::io::Error::from(err), "sync")
                })?,
            }
            match sync_type {
                FileSyncType::FullFsync => trace!("fcntl(F_FULLFSYNC)"),
                FileSyncType::Fsync => trace!("fsync"),
            }
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            let _ = sync_type;
            fs::fsync(&self.file).map_err(|err| io_error(std::io::Error::from(err), "sync"))?;
            trace!("fsync");
        }

        c.complete(0);
        Ok(c)
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn size(&self) -> Result<u64> {
        Ok(self
            .file
            .metadata()
            .map_err(|e| io_error(e, "metadata"))?
            .len())
    }

    #[instrument(err, skip_all, level = Level::DEBUG)]
    fn truncate(&self, len: u64, c: Completion) -> Result<Completion> {
        let result = self.file.set_len(len);
        match result {
            Ok(()) => {
                trace!("file truncated to len=({})", len);
                c.complete(0);
                Ok(c)
            }
            Err(e) => Err(io_error(e, "truncate")),
        }
    }

    fn shared_wal_lock_byte(
        &self,
        offset: u64,
        exclusive: bool,
        kind: SharedWalLockKind,
    ) -> Result<()> {
        unix_shared_wal_lock_byte(self.file.as_raw_fd(), offset, exclusive, true, kind).map(|_| ())
    }

    fn shared_wal_try_lock_byte(
        &self,
        offset: u64,
        exclusive: bool,
        kind: SharedWalLockKind,
    ) -> Result<bool> {
        unix_shared_wal_lock_byte(self.file.as_raw_fd(), offset, exclusive, false, kind)
    }

    fn shared_wal_unlock_byte(&self, offset: u64, kind: SharedWalLockKind) -> Result<()> {
        unix_shared_wal_unlock_byte(self.file.as_raw_fd(), offset, kind)
    }

    fn shared_wal_set_len(&self, len: u64) -> Result<()> {
        self.file
            .set_len(len)
            .map_err(|err| io_error(err, "resize shared WAL coordination file"))
    }

    fn shared_wal_map(&self, offset: u64, len: usize) -> Result<Box<dyn SharedWalMappedRegion>> {
        unix_shared_wal_map(offset, len, self.file.as_raw_fd())
    }
}

fn iovec_to_io_slices(iov: &[Iovec]) -> Vec<IoSlice<'_>> {
    iov.iter()
        .map(|entry| {
            IoSlice::new(unsafe {
                std::slice::from_raw_parts(entry.iov_base.cast::<u8>(), entry.iov_len)
            })
        })
        .collect()
}

/// Append iovec entries for `buf` to `iovecs`, splitting `buf` into chunks of
/// at most `MAX_PWRITE_LEN` bytes. Stops once `iovecs.len()` reaches `max_iovecs`.
/// Returns the number of bytes consumed from `buf`.
fn buf_to_iovecs(buf: &[u8], iovecs: &mut Vec<Iovec>, max_iovecs: usize) -> usize {
    let mut slice = buf;
    while !slice.is_empty() && iovecs.len() < max_iovecs {
        let chunk_len = slice.len().min(MAX_PWRITE_LEN);
        iovecs.push(Iovec {
            iov_base: slice.as_ptr().cast_mut().cast(),
            iov_len: chunk_len,
        });
        slice = &slice[chunk_len..];
    }
    buf.len() - slice.len()
}

/// Drop the first `n` bytes from the front of `iov`, advancing the leading
/// entry's pointer if a partial iovec was consumed.
fn trim_iovecs(iov: &mut Vec<Iovec>, mut n: usize) {
    let mut idx = 0;
    while idx < iov.len() {
        if iov[idx].iov_len > n {
            break;
        }
        n -= iov[idx].iov_len;
        idx += 1;
    }
    iov.drain(..idx);
    if n > 0 {
        assert!(!iov.is_empty());
        let front = &mut iov[0];
        front.iov_base = unsafe { front.iov_base.cast::<u8>().add(n).cast() };
        front.iov_len -= n;
    }
}

impl Drop for UnixFile {
    fn drop(&mut self) {
        self.unlock_file().expect("Failed to unlock file");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_multiple_processes_cannot_open_file() {
        common::tests::test_multiple_processes_cannot_open_file(UnixIO::new);
    }

    #[test]
    fn test_shared_wal_map_supports_unaligned_logical_offset() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let backing_len = 128 * 1024;
        let bytes: Vec<u8> = (0..backing_len).map(|i| (i % 251) as u8).collect();
        file.as_file().write_all(&bytes).unwrap();
        file.as_file().sync_all().unwrap();

        let mapped = unix_shared_wal_map(4096, 81920, file.as_file().as_raw_fd()).unwrap();
        assert_eq!(mapped.len(), 81920);
        let slice = unsafe { std::slice::from_raw_parts(mapped.ptr().as_ptr(), mapped.len()) };
        assert_eq!(&slice[..128], &bytes[4096..4096 + 128]);
        assert_eq!(&slice[mapped.len() - 128..], &bytes[4096 + 81920 - 128..4096 + 81920]);
    }
}
