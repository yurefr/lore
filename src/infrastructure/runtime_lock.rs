use std::{
    fs::{File, OpenOptions},
    io::ErrorKind,
};

use fs2::FileExt;

use crate::{
    application::ports::{RuntimeLockGuard, RuntimeLockProvider},
    error::{LoreError, Result},
    paths::LorePaths,
};

pub struct InstanceLock {
    file: File,
}

#[derive(Debug, Clone)]
pub struct FileLockProvider {
    paths: LorePaths,
}

impl FileLockProvider {
    pub fn new(paths: LorePaths) -> Self {
        Self { paths }
    }
}

impl InstanceLock {
    pub fn acquire(paths: &LorePaths) -> Result<Self> {
        paths.ensure_home()?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&paths.lock_file)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { file }),
            Err(error) if is_lock_contended(&error) => {
                Err(LoreError::AlreadyRunning(paths.lock_file.clone()))
            }
            Err(error) => Err(LoreError::Io(error)),
        }
    }

    pub fn try_acquire(paths: &LorePaths) -> Result<Option<Self>> {
        paths.ensure_home()?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&paths.lock_file)?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { file })),
            Err(error) if is_lock_contended(&error) => Ok(None),
            Err(error) => Err(LoreError::Io(error)),
        }
    }
}

fn is_lock_contended(error: &std::io::Error) -> bool {
    if error.kind() == ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }

    #[cfg(not(windows))]
    {
        false
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

impl RuntimeLockGuard for InstanceLock {}

impl RuntimeLockProvider for FileLockProvider {
    fn acquire(&self) -> Result<Box<dyn RuntimeLockGuard>> {
        Ok(Box::new(InstanceLock::acquire(&self.paths)?))
    }

    fn try_acquire(&self) -> Result<Option<Box<dyn RuntimeLockGuard>>> {
        Ok(InstanceLock::try_acquire(&self.paths)?
            .map(|lock| Box::new(lock) as Box<dyn RuntimeLockGuard>))
    }
}
