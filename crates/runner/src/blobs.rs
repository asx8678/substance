//! The content-addressed blob store inside a run directory (M1-T01).
//!
//! Path: `blobs/sha256/<first 2 hex>/<64 hex>`. A blob is written once: if the digest
//! is already there, the bytes are known to be identical, so nothing is rewritten. A
//! write goes to a temporary file first and is renamed into place, so a reader never sees
//! half a blob.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use substance_core::BlobRef;
use substance_core::hash::sha256;

/// The directory every blob lives under, relative to the run directory.
const BLOB_DIR: &str = "blobs/sha256";

/// Where a body's normalized form lives, named by its hash.
const NORMALIZED_DIR: &str = "blobs/normalized";

/// Why a blob could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum BlobError {
    /// The file system refused an operation.
    #[error("blob {path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The IO error.
        source: io::Error,
    },
    /// The reference is not a `sha256:<64 hex>` string.
    #[error("{0} is not a blob reference")]
    NotABlobRef(String),
}

/// A run directory's blob store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Uses `run_dir` as the run directory that holds `blobs/`.
    #[must_use]
    pub fn new(run_dir: &Path) -> Self {
        Self {
            root: run_dir.to_path_buf(),
        }
    }

    /// The path a blob with `reference` is stored at.
    ///
    /// # Errors
    ///
    /// Returns [`BlobError::NotABlobRef`] when `reference` is not `sha256:<hex>`.
    pub fn path_of(&self, reference: &BlobRef) -> Result<PathBuf, BlobError> {
        let text = reference.as_str();
        let digest = text
            .strip_prefix(BlobRef::PREFIX)
            .ok_or_else(|| BlobError::NotABlobRef(text.to_owned()))?;
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(BlobError::NotABlobRef(text.to_owned()));
        }
        let (shard, name) = digest.split_at(2);
        Ok(self.root.join(BLOB_DIR).join(shard).join(name))
    }

    /// Writes `bytes` and returns the reference that names them.
    ///
    /// Writing the same bytes twice is a no-op the second time: the digest is the name,
    /// so an existing file already holds them.
    ///
    /// # Errors
    ///
    /// Returns [`BlobError::Io`] when the shard directory or the file cannot be created.
    pub fn put(&self, bytes: &[u8]) -> Result<BlobRef, BlobError> {
        let reference = BlobRef::from_bytes(bytes);
        let path = self.path_of(&reference)?;
        if path.is_file() {
            return Ok(reference);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| BlobError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let temporary = path.with_extension("partial");
        fs::write(&temporary, bytes).map_err(|source| BlobError::Io {
            path: temporary.clone(),
            source,
        })?;
        fs::rename(&temporary, &path).map_err(|source| BlobError::Io { path, source })?;
        Ok(reference)
    }

    /// Writes a body and the hash of its normalized form, returning both.
    ///
    /// The raw bytes go in the blob store as always; the normalized form goes in a parallel tree
    /// (`blobs/normalized/<first 2 hex>/<64 hex>`) whose *name* is the hash. Keeping the normalized
    /// text next to the raw one rather than only its hash is what lets a reader see what a diff was
    /// actually comparing: two records whose hashes match can be checked by reading one file, and a
    /// mismatch can be diagnosed without re-running the normalizer.
    ///
    /// # Errors
    ///
    /// Returns [`BlobError::Io`] when either write fails.
    pub fn put_with_normalized(
        &self,
        bytes: &[u8],
        normalizer: &substance_core::normalize::Normalizer,
    ) -> Result<(BlobRef, String), BlobError> {
        let reference = self.put(bytes)?;
        let text = String::from_utf8_lossy(bytes);
        let normalized = normalizer.apply(&text);
        let hash = normalizer.hash(&text);
        let digest = hash.strip_prefix(BlobRef::PREFIX).unwrap_or(&hash);
        // The digest is written by `hash`, which uses the same hex form and sharding as a `BlobRef`,
        // so the path has the same shape as a blob's.
        let path = self
            .root
            .join(NORMALIZED_DIR)
            .join(digest.get(..2).unwrap_or("00"))
            .join(digest);
        if !path.is_file() {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| BlobError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            let temporary = path.with_extension("partial");
            fs::write(&temporary, normalized.as_bytes()).map_err(|source| BlobError::Io {
                path: temporary.clone(),
                source,
            })?;
            fs::rename(&temporary, &path).map_err(|source| BlobError::Io { path, source })?;
        }
        Ok((reference, hash))
    }

    /// Reads the bytes a reference names.
    ///
    /// # Errors
    ///
    /// Returns [`BlobError::Io`] when the blob is missing or unreadable, and
    /// [`BlobError::NotABlobRef`] for a malformed reference.
    pub fn get(&self, reference: &BlobRef) -> Result<Vec<u8>, BlobError> {
        let path = self.path_of(reference)?;
        fs::read(&path).map_err(|source| BlobError::Io { path, source })
    }

    /// Whether a blob with `reference` is already stored.
    #[must_use]
    pub fn contains(&self, reference: &BlobRef) -> bool {
        self.path_of(reference).is_ok_and(|path| path.is_file())
    }
}

/// The reference for `bytes` without writing anything.
#[must_use]
pub fn reference_of(bytes: &[u8]) -> BlobRef {
    BlobRef::from_digest(&sha256(bytes))
}
