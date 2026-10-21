//! The run directory: where every artifact of one run lands (M0-T08).
//!
//! The layout is the plan's. A directory that a later milestone fills is created now and
//! stays empty, so a caller can see which artifacts a run would have written.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use substance_core::RunId;
use substance_core::hash::canonical_json_pretty_value;

/// The subdirectories every run directory has.
pub const SUBDIRECTORIES: &[&str] = &[
    "blobs/sha256",
    "records",
    "findings",
    "forge",
    "registry",
    "tools",
    "logs",
    "snapshots",
];

/// Why a run directory could not be written.
#[derive(Debug, thiserror::Error)]
pub enum RunDirError {
    /// A directory or file could not be created.
    #[error("cannot write {path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// The IO error.
        source: io::Error,
    },
    /// A document could not be serialized.
    #[error("cannot serialize {document}: {source}")]
    Serialize {
        /// Which document failed.
        document: String,
        /// The serde error.
        source: serde_json::Error,
    },
}

/// One run's directory, and the documents in it.
///
/// Documents are written as canonical pretty JSON (sorted keys, two-space indent, a
/// trailing newline), so two runs that produce equal data produce equal bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunDir {
    root: PathBuf,
}

impl RunDir {
    /// Creates `out_root/<run_id>/` with every subdirectory of the layout.
    ///
    /// # Errors
    ///
    /// Returns [`RunDirError::Io`] when a directory cannot be created.
    pub fn create(out_root: &Path, run_id: &RunId) -> Result<Self, RunDirError> {
        let root = out_root.join(run_id.as_str());
        let mut directories = Vec::with_capacity(SUBDIRECTORIES.len() + 1);
        directories.push(root.clone());
        for relative in SUBDIRECTORIES {
            directories.push(root.join(relative));
        }
        for directory in directories {
            create_dir(&directory)?;
        }
        Ok(Self { root })
    }

    /// The root path of the run directory.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        self.root.as_path()
    }

    /// The path of a file at the top of the run directory.
    #[must_use]
    pub fn path_of(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Writes `value` to `name` as canonical pretty JSON, and returns the path written.
    ///
    /// # Errors
    ///
    /// Returns [`RunDirError::Serialize`] when the value cannot be turned into JSON, and
    /// [`RunDirError::Io`] when the file cannot be written.
    pub fn write_json<T: Serialize>(&self, name: &str, value: &T) -> Result<PathBuf, RunDirError> {
        let json = serde_json::to_value(value).map_err(|source| RunDirError::Serialize {
            document: name.to_owned(),
            source,
        })?;
        let bytes = canonical_json_pretty_value(&json);
        let path = self.path_of(name);
        fs::write(&path, bytes).map_err(|source| RunDirError::Io { path, source })?;
        Ok(self.path_of(name))
    }
}

/// Creates `directory` and its parents, treating an existing directory as success.
fn create_dir(directory: &Path) -> Result<(), RunDirError> {
    fs::create_dir_all(directory).map_err(|source| RunDirError::Io {
        path: directory.to_path_buf(),
        source,
    })
}
