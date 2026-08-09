use std::{
    fs::{File, OpenOptions},
    path::Path,
};

use cce_core::{CceError, Result};
use fs4::{FileExt, TryLockError};

#[derive(Debug)]
pub(crate) struct IndexLease {
    _file: File,
}

impl IndexLease {
    pub(crate) fn acquire(data_root: &Path) -> Result<Self> {
        let path = data_root.join("index.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| CceError::io(&path, error))?;
        match FileExt::try_lock(&file) {
            Ok(()) => Ok(Self { _file: file }),
            Err(TryLockError::WouldBlock) => Err(CceError::IndexBusy(path)),
            Err(TryLockError::Error(error)) => Err(CceError::io(path, error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_a_second_writer() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let _first = IndexLease::acquire(directory.path()).expect("first lease");
        let second = IndexLease::acquire(directory.path());
        assert!(matches!(second, Err(CceError::IndexBusy(_))));
    }
}
