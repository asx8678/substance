//! Finding and reading `substance.toml` (M0-T05).
//!
//! Core parses a config it is handed; this module decides which file to read, in the
//! plan's order: `--config <path>`, then `<repo>/substance.toml`, then the defaults.

use std::fs;
use std::path::{Path, PathBuf};

use substance_core::config::{ConfigError, ProjectConfig};

/// The file name the loader looks for inside a repository.
pub const CONFIG_FILE_NAME: &str = "substance.toml";

/// Where the config that was used came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigSource {
    /// `--config <path>` named this file.
    Explicit(PathBuf),
    /// The repository's own `substance.toml`.
    Repository(PathBuf),
    /// No file was read; every key has its default.
    Defaults,
}

impl ConfigSource {
    /// The path that was read, when one was.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Explicit(path) | Self::Repository(path) => Some(path.as_path()),
            Self::Defaults => None,
        }
    }

    /// The source as a short word for logs and the report.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Explicit(_) => "explicit",
            Self::Repository(_) => "repository",
            Self::Defaults => "defaults",
        }
    }
}

/// Why a config could not be loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The file named by `--config` does not exist.
    #[error("config file {0} does not exist")]
    Missing(PathBuf),
    /// The file exists but could not be read.
    #[error("cannot read config file {path}: {source}")]
    Read {
        /// The file that failed.
        path: PathBuf,
        /// The IO error.
        source: std::io::Error,
    },
    /// The file was read but is not a valid config.
    #[error("config file {path}: {source}")]
    Parse {
        /// The file that failed.
        path: PathBuf,
        /// The parse error.
        source: ConfigError,
    },
}

/// Loads the project config for `repo_root`.
///
/// An explicit path is an error when it is missing: a caller that names a file wants
/// that file, not the defaults. A missing `<repo>/substance.toml` is not an error.
pub fn load_config(
    explicit: Option<&Path>,
    repo_root: &Path,
) -> Result<(ProjectConfig, ConfigSource), LoadError> {
    if let Some(path) = explicit {
        if !path.is_file() {
            return Err(LoadError::Missing(path.to_path_buf()));
        }
        let config = read_config(path)?;
        return Ok((config, ConfigSource::Explicit(path.to_path_buf())));
    }
    let candidate = repo_root.join(CONFIG_FILE_NAME);
    if candidate.is_file() {
        let config = read_config(&candidate)?;
        return Ok((config, ConfigSource::Repository(candidate)));
    }
    Ok((ProjectConfig::default(), ConfigSource::Defaults))
}

/// Reads and parses one config file.
pub fn read_config(path: &Path) -> Result<ProjectConfig, LoadError> {
    let text = fs::read_to_string(path).map_err(|source| LoadError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    ProjectConfig::from_toml_str(&text).map_err(|source| LoadError::Parse {
        path: path.to_path_buf(),
        source,
    })
}
