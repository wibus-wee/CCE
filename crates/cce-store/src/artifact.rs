use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use cce_core::{CceError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Source,
    VectorIndex,
    Knowledge,
    Trace,
    Benchmark,
    Model,
    Other,
}

impl std::fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Source => "source",
            Self::VectorIndex => "vector_index",
            Self::Knowledge => "knowledge",
            Self::Trace => "trace",
            Self::Benchmark => "benchmark",
            Self::Model => "model",
            Self::Other => "other",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRecord {
    pub digest: String,
    pub kind: ArtifactKind,
    pub size_bytes: u64,
    pub relative_path: String,
}

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    objects: PathBuf,
    temporary: PathBuf,
}

impl ArtifactStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        let objects = root.join("artifacts").join("blake3");
        let temporary = root.join("tmp");
        fs::create_dir_all(&objects).map_err(|error| CceError::io(&objects, error))?;
        fs::create_dir_all(&temporary).map_err(|error| CceError::io(&temporary, error))?;
        Ok(Self {
            root,
            objects,
            temporary,
        })
    }

    pub fn put_bytes(&self, kind: ArtifactKind, bytes: &[u8]) -> Result<ArtifactRecord> {
        let digest = blake3::hash(bytes).to_hex().to_string();
        let destination = self.path_for(&digest)?;
        let relative_path = destination
            .strip_prefix(&self.root)
            .map_err(|_| CceError::PathEscape(destination.clone()))?
            .to_string_lossy()
            .replace('\\', "/");

        if destination.exists() {
            Self::verify_existing(&destination, &digest, bytes.len() as u64)?;
            return Ok(ArtifactRecord {
                digest,
                kind,
                size_bytes: bytes.len() as u64,
                relative_path,
            });
        }

        let parent = destination
            .parent()
            .ok_or_else(|| CceError::PathEscape(destination.clone()))?;
        fs::create_dir_all(parent).map_err(|error| CceError::io(parent, error))?;

        let mut temporary = tempfile::NamedTempFile::new_in(&self.temporary)
            .map_err(|error| CceError::io(&self.temporary, error))?;
        temporary
            .write_all(bytes)
            .map_err(|error| CceError::io(temporary.path(), error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| CceError::io(temporary.path(), error))?;

        match temporary.persist_noclobber(&destination) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                Self::verify_existing(&destination, &digest, bytes.len() as u64)?;
            }
            Err(error) => return Err(CceError::io(&destination, error.error)),
        }

        Ok(ArtifactRecord {
            digest,
            kind,
            size_bytes: bytes.len() as u64,
            relative_path,
        })
    }

    pub fn read(&self, digest: &str) -> Result<Vec<u8>> {
        let path = self.path_for(digest)?;
        let bytes = fs::read(&path).map_err(|error| CceError::io(&path, error))?;
        let actual = blake3::hash(&bytes).to_hex().to_string();
        if actual != digest {
            return Err(CceError::ArtifactCorrupt(digest.to_owned()));
        }
        Ok(bytes)
    }

    pub fn contains(&self, digest: &str) -> Result<bool> {
        Ok(self.path_for(digest)?.is_file())
    }

    /// Remove object files whose digest is not in `keep`, clear the temporary
    /// directory, and drop empty prefix directories. `keep` is the set of
    /// digests still referenced by metadata.
    pub fn retain(&self, keep: &std::collections::HashSet<String>) -> Result<crate::GcReport> {
        let mut report = crate::GcReport {
            pruned_snapshots: 0,
            removed_digests: 0,
            removed_orphan_files: 0,
            reclaimed_bytes: 0,
        };
        if self.temporary.is_dir() {
            for entry in fs::read_dir(&self.temporary)
                .map_err(|error| CceError::io(&self.temporary, error))?
            {
                let entry = entry.map_err(|error| CceError::io(&self.temporary, error))?;
                let path = entry.path();
                if path.is_file() {
                    report.reclaimed_bytes += entry
                        .metadata()
                        .map_err(|error| CceError::io(&path, error))?
                        .len();
                    fs::remove_file(&path).map_err(|error| CceError::io(&path, error))?;
                    report.removed_orphan_files += 1;
                }
            }
        }
        if !self.objects.is_dir() {
            return Ok(report);
        }
        for prefix in
            fs::read_dir(&self.objects).map_err(|error| CceError::io(&self.objects, error))?
        {
            let prefix = prefix.map_err(|error| CceError::io(&self.objects, error))?;
            let prefix_path = prefix.path();
            if !prefix_path.is_dir() {
                continue;
            }
            for entry in
                fs::read_dir(&prefix_path).map_err(|error| CceError::io(&prefix_path, error))?
            {
                let entry = entry.map_err(|error| CceError::io(&prefix_path, error))?;
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }
                let digest = format!(
                    "{}{}",
                    prefix.file_name().to_string_lossy(),
                    entry.file_name().to_string_lossy()
                );
                if keep.contains(&digest) {
                    continue;
                }
                report.reclaimed_bytes += entry
                    .metadata()
                    .map_err(|error| CceError::io(&path, error))?
                    .len();
                fs::remove_file(&path).map_err(|error| CceError::io(&path, error))?;
                report.removed_orphan_files += 1;
            }
            // Drop the prefix directory when it is now empty.
            let _ = fs::remove_dir(&prefix_path);
        }
        Ok(report)
    }

    fn path_for(&self, digest: &str) -> Result<PathBuf> {
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CceError::ArtifactCorrupt(digest.to_owned()));
        }
        Ok(self.objects.join(&digest[..2]).join(&digest[2..]))
    }

    fn verify_existing(path: &Path, digest: &str, expected_size: u64) -> Result<()> {
        let metadata = fs::metadata(path).map_err(|error| CceError::io(path, error))?;
        if metadata.len() != expected_size {
            return Err(CceError::ArtifactCorrupt(digest.to_owned()));
        }
        let bytes = fs::read(path).map_err(|error| CceError::io(path, error))?;
        if blake3::hash(&bytes).to_hex().as_str() != digest {
            return Err(CceError::ArtifactCorrupt(digest.to_owned()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifacts_are_content_addressed_and_deduplicated() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let store = ArtifactStore::open(directory.path()).expect("artifact store");
        let first = store
            .put_bytes(ArtifactKind::Source, b"same payload")
            .expect("first write");
        let second = store
            .put_bytes(ArtifactKind::Source, b"same payload")
            .expect("second write");
        assert_eq!(first.digest, second.digest);
        assert_eq!(store.read(&first.digest).expect("read"), b"same payload");
    }
}
