use std::{
    os::unix::fs::FileExt,
    sync::{Arc, RwLock},
};

use tracing::{instrument, Level};
use turso_core::{
    io::{clock::DefaultClock, FileSyncType},
    io_error, Buffer, Clock, Completion, File, MonotonicInstant, OpenFlags, Result,
    WallClockInstant, IO,
};

pub struct SparseLinuxIo {}

impl SparseLinuxIo {
    pub fn new() -> Result<Self> {
        Ok(Self {})
    }
}

impl IO for SparseLinuxIo {
    #[instrument(skip_all, level = Level::TRACE)]
    fn open_file(&self, path: &str, flags: OpenFlags, _direct: bool) -> Result<Arc<dyn File>> {
        let mut file = std::fs::File::options();
        file.read(true);

        if !flags.contains(OpenFlags::ReadOnly) {
            file.write(true);
            file.create(flags.contains(OpenFlags::Create));
        }

        let file = file.open(path).map_err(|e| io_error(e, "open"))?;
        Ok(Arc::new(SparseLinuxFile {
            file: RwLock::new(file),
        }))
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn remove_file(&self, path: &str) -> Result<()> {
        std::fs::remove_file(path).map_err(|e| io_error(e, "remove_file"))?;
        Ok(())
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn step(&self) -> Result<()> {
        Ok(())
    }
}

impl Clock for SparseLinuxIo {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        DefaultClock.current_time_monotonic()
    }

    fn current_time_wall_clock(&self) -> WallClockInstant {
        DefaultClock.current_time_wall_clock()
    }
}

pub struct SparseLinuxFile {
    file: RwLock<std::fs::File>,
}

#[allow(clippy::readonly_write_lock)]
impl File for SparseLinuxFile {
    #[instrument(err, skip_all, level = Level::TRACE)]
    fn lock_file(&self, _exclusive: bool) -> Result<()> {
        Ok(())
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn unlock_file(&self) -> Result<()> {
        Ok(())
    }

    #[instrument(skip(self, c), level = Level::TRACE)]
    fn pread(&self, pos: u64, c: Completion) -> Result<Completion> {
        let file = self.file.read().unwrap();
        let nr = {
            let r = c.as_read();
            let buf = r.buf();
            let buf = buf.as_mut_slice();
            file.read_exact_at(buf, pos)
                .map_err(|e| io_error(e, "pread"))?;
            buf.len() as i32
        };
        c.complete(nr);
        Ok(c)
    }

    #[instrument(skip(self, c, buffer), level = Level::TRACE)]
    fn pwrite(&self, pos: u64, buffer: Arc<Buffer>, c: Completion) -> Result<Completion> {
        let file = self.file.write().unwrap();
        let buf = buffer.as_slice();
        file.write_all_at(buf, pos)
            .map_err(|e| io_error(e, "pwrite"))?;
        c.complete(buffer.len() as i32);
        Ok(c)
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn sync(&self, c: Completion, _sync_type: FileSyncType) -> Result<Completion> {
        let file = self.file.write().unwrap();
        file.sync_all().map_err(|e| io_error(e, "sync"))?;
        c.complete(0);
        Ok(c)
    }

    #[instrument(err, skip_all, level = Level::TRACE)]
    fn truncate(&self, len: u64, c: Completion) -> Result<Completion> {
        let file = self.file.write().unwrap();
        file.set_len(len).map_err(|e| io_error(e, "truncate"))?;
        c.complete(0);
        Ok(c)
    }

    fn size(&self) -> Result<u64> {
        let file = self.file.read().unwrap();
        Ok(file.metadata().map_err(|e| io_error(e, "metadata"))?.len())
    }

    fn has_hole(&self, pos: usize, len: usize) -> turso_core::Result<bool> {
        use rustix::fs::{lseek, SeekFrom};
        use rustix::io::Errno;

        let file = self.file.read().unwrap();
        match lseek(&*file, SeekFrom::Data(pos as u64)) {
            Ok(next_data) => Ok(next_data as usize >= pos + len),
            Err(Errno::NXIO) => Ok(true),
            Err(err) => Err(turso_core::LimboError::CompletionError(
                turso_core::CompletionError::IOError(std::io::Error::from(err).kind(), "lseek"),
            )),
        }
    }

    fn punch_hole(&self, pos: usize, len: usize) -> turso_core::Result<()> {
        use rustix::fs::{fallocate, FallocateFlags};

        let file = self.file.write().unwrap();
        fallocate(
            &*file,
            FallocateFlags::PUNCH_HOLE | FallocateFlags::KEEP_SIZE,
            pos as u64,
            len as u64,
        )
        .map_err(|err| {
            turso_core::LimboError::CompletionError(turso_core::CompletionError::IOError(
                std::io::Error::from(err).kind(),
                "fallocate",
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use turso_core::{Buffer, Completion, OpenFlags, IO};

    use crate::sparse_io::SparseLinuxIo;

    #[test]
    pub fn sparse_io_test() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmp.into_temp_path();
        let tmp_path = tmp_path.as_os_str().to_str().unwrap();
        let io = SparseLinuxIo::new().unwrap();
        let file = io.open_file(tmp_path, OpenFlags::default(), false).unwrap();
        #[expect(clippy::let_underscore_future)]
        let _ = file
            .truncate(1024 * 1024, Completion::new_trunc(|_| {}))
            .unwrap();
        assert!(file.has_hole(0, 4096).unwrap());

        let buffer = Arc::new(Buffer::new_temporary(4096));
        buffer.as_mut_slice().fill(1);
        #[expect(clippy::let_underscore_future)]
        let _ = file
            .pwrite(0, buffer.clone(), Completion::new_write(|_| {}))
            .unwrap();
        assert!(!file.has_hole(0, 4096).unwrap());

        assert!(file.has_hole(4096, 4096).unwrap());
        assert!(file.has_hole(4096 * 2, 4096).unwrap());

        #[expect(clippy::let_underscore_future)]
        let _ = file
            .pwrite(4096 * 2, buffer.clone(), Completion::new_write(|_| {}))
            .unwrap();
        assert!(file.has_hole(4096, 4096).unwrap());
        assert!(!file.has_hole(4096 * 2, 4096).unwrap());

        assert!(!file.has_hole(4096, 4097).unwrap());

        file.punch_hole(2 * 4096, 4096).unwrap();
        assert!(file.has_hole(4096 * 2, 4096).unwrap());
        assert!(file.has_hole(4096, 4097).unwrap());
    }
}
