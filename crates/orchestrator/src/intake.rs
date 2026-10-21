//! Intake: a local work tree or a git URL, pinned to one commit (M0-T08).
//!
//! ADR-0004 keeps v1 in repo mode only, so intake always ends with a source tree on
//! disk. A local directory is used where it is; a URL is cloned in full into a staging
//! directory under the output root, never into the caller's checkout. Shallow
//! repositories are refused, because every history check needs the whole history.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use substance_core::hash::{hex, sha256};
use substance_runner::ExecPermit;
use substance_runner::tool::{ToolError, run_tool_capture, run_tool_capture_any};

/// How long a clone may take before it is killed.
const CLONE_TIMEOUT: Duration = Duration::from_secs(600);

/// How long a plumbing command such as `git rev-parse` may take.
const PLUMBING_TIMEOUT: Duration = Duration::from_secs(30);

/// The staging directory URL clones land in, under the output root.
const STAGING_DIR: &str = ".intake";

/// What intake pinned about the code under test.
///
/// Not `Clone`: the permit is a grant, and duplicating it would duplicate the grant.
#[derive(Debug)]
pub struct Checkout {
    /// The work tree root: the directory every later stage runs against.
    pub root: PathBuf,
    /// The origin URL with any credentials removed.
    pub origin: String,
    /// The commit under test.
    pub commit_sha: String,
    /// The `git --version` line, recorded in the manifest.
    pub git_version: String,
    /// The permission to execute repo-controlled code, when `--allow-exec` was given.
    ///
    /// This is the only place a permit is created (§4.1); every stage that runs the
    /// repository's own code asks the caller for it.
    pub permit: Option<ExecPermit>,
}

/// Why intake could not pin a checkout.
#[derive(Debug, thiserror::Error)]
pub enum IntakeError {
    /// The path does not exist or is not a directory.
    #[error("{path} is not a directory")]
    NotADirectory {
        /// The path the caller named.
        path: PathBuf,
    },
    /// The directory is not inside a git work tree.
    #[error("{path} is not a git work tree; pass a cloned repository")]
    NotARepository {
        /// The path the caller named.
        path: PathBuf,
    },
    /// The repository was cloned shallowly, so its history is unusable.
    #[error(
        "{path} is a shallow clone; run `git fetch --unshallow` first, or let substance \
         clone the repository itself"
    )]
    Shallow {
        /// The work tree root.
        path: PathBuf,
    },
    /// The revision does not name a commit in this repository.
    #[error("{rev} is not a commit in {path}: {detail}")]
    RevNotFound {
        /// The revision the caller named.
        rev: String,
        /// The work tree root.
        path: PathBuf,
        /// git's own explanation.
        detail: String,
    },
    /// The URL could not be reached, so it is not a repository substance may clone.
    #[error(
        "cannot reach {url} ({detail}); v1 evaluates repositories only (ADR-0004), so pass a \
         cloned repository instead of a live URL"
    )]
    Unreachable {
        /// The URL the caller named.
        url: String,
        /// git's own explanation.
        detail: String,
    },
    /// The clone failed.
    #[error("git clone failed: {detail}")]
    CloneFailed {
        /// git's own explanation.
        detail: String,
    },
    /// `git` is not installed.
    #[error("git is not installed, so substance cannot read the repository")]
    GitMissing,
    /// The file system refused an operation.
    #[error("cannot prepare {path}: {source}")]
    Io {
        /// The directory that could not be prepared.
        path: PathBuf,
        /// The IO error.
        source: io::Error,
    },
}

impl IntakeError {
    /// Maps a failed tool run to an intake error, keeping the tool's own words.
    fn from_tool(error: ToolError) -> Self {
        match error {
            ToolError::Missing { .. } => Self::GitMissing,
            other => Self::CloneFailed {
                detail: other.to_string(),
            },
        }
    }
}

/// Resolves the code under test and pins one commit.
///
/// A local directory is resolved to its work tree root and used in place. Anything that
/// looks like a git URL must be reachable over `git ls-remote`, and is then cloned in
/// full into `<out_root>/.intake/<hash of the URL>`, so the caller's checkout stays
/// untouched and two runs of the same URL share one staging path.
///
/// # Errors
///
/// Returns [`IntakeError`] for a missing path, a non-repository, a shallow clone, an
/// unreachable URL or an unknown revision.
pub async fn intake(
    input: &Path,
    rev: Option<&str>,
    out_root: &Path,
    allow_exec: bool,
) -> Result<Checkout, IntakeError> {
    let text = input.to_str().map(str::to_owned);
    let from_url = text.as_deref().filter(|value| looks_like_url(value));
    let root = match from_url {
        Some(url) => clone_full(url, out_root).await?,
        None => open_local(input).await?,
    };
    let commit_sha = resolve_commit(&root, rev.unwrap_or("HEAD")).await?;
    let origin = origin_of(&root).await;
    let git_version = git_version().await?;
    let permit = if allow_exec {
        Some(ExecPermit::grant())
    } else {
        None
    };
    Ok(Checkout {
        root,
        origin,
        commit_sha,
        git_version,
        permit,
    })
}

/// Whether `text` names a remote repository rather than a path.
///
/// The schemes are git's own; a path that happens to contain a colon stays a path,
/// because nothing here starts with a scheme.
fn looks_like_url(text: &str) -> bool {
    ["http://", "https://", "ssh://", "git://", "git@"]
        .iter()
        .any(|prefix| text.starts_with(prefix))
}

/// Resolves a local directory to the root of the work tree holding it.
async fn open_local(input: &Path) -> Result<PathBuf, IntakeError> {
    if !input.is_dir() {
        return Err(IntakeError::NotADirectory {
            path: input.to_path_buf(),
        });
    }
    let args = [
        OsString::from("rev-parse"),
        OsString::from("--show-toplevel"),
    ];
    let borrowed = borrow(&args);
    let captured = run_tool_capture_any("git", &borrowed, input, PLUMBING_TIMEOUT)
        .await
        .map_err(IntakeError::from_tool)?;
    if !captured.success || captured.stdout_trimmed().is_empty() {
        return Err(IntakeError::NotARepository {
            path: input.to_path_buf(),
        });
    }
    let root = PathBuf::from(captured.stdout_trimmed());
    refuse_shallow(&root).await?;
    Ok(root)
}

/// Clones `url` in full into the staging directory and returns the work tree root.
async fn clone_full(url: &str, out_root: &Path) -> Result<PathBuf, IntakeError> {
    let probe = [OsString::from("ls-remote"), OsString::from(url)];
    let probe_args = borrow(&probe);
    let reachable = run_tool_capture_any("git", &probe_args, out_root, PLUMBING_TIMEOUT)
        .await
        .map_err(IntakeError::from_tool)?;
    if !reachable.success {
        return Err(IntakeError::Unreachable {
            url: url.to_owned(),
            detail: reachable.stderr_trimmed().to_owned(),
        });
    }
    let staging = out_root.join(STAGING_DIR).join(staging_name(url));
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(|source| IntakeError::Io {
            path: staging.clone(),
            source,
        })?;
    }
    if let Some(parent) = staging.parent() {
        std::fs::create_dir_all(parent).map_err(|source| IntakeError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let args = [
        OsString::from("clone"),
        OsString::from("--"),
        OsString::from(url),
        staging.clone().into_os_string(),
    ];
    let clone_args = borrow(&args);
    let captured = run_tool_capture_any("git", &clone_args, out_root, CLONE_TIMEOUT)
        .await
        .map_err(IntakeError::from_tool)?;
    if !captured.success {
        return Err(IntakeError::CloneFailed {
            detail: captured.stderr_trimmed().to_owned(),
        });
    }
    refuse_shallow(&staging).await?;
    Ok(staging)
}

/// Borrows owned arguments as the `&[&OsStr]` the tool runner takes.
fn borrow(args: &[OsString]) -> Vec<&OsStr> {
    args.iter().map(OsString::as_os_str).collect()
}

/// The staging directory name for `url`: its hash, so the same URL reuses one path and
/// no URL text (which may hold credentials) becomes a file name.
fn staging_name(url: &str) -> String {
    hex(&sha256(url.as_bytes())).chars().take(16).collect()
}

/// Refuses a shallow clone: the history checks need the full history.
async fn refuse_shallow(root: &Path) -> Result<(), IntakeError> {
    let args = [
        OsString::from("rev-parse"),
        OsString::from("--is-shallow-repository"),
    ];
    let borrowed = borrow(&args);
    let captured = run_tool_capture_any("git", &borrowed, root, PLUMBING_TIMEOUT)
        .await
        .map_err(IntakeError::from_tool)?;
    if captured.stdout_trimmed() == "true" {
        return Err(IntakeError::Shallow {
            path: root.to_path_buf(),
        });
    }
    Ok(())
}

/// Resolves `rev` to a commit SHA.
///
/// The SHA is what the manifest pins; exporting that commit to a working copy is M2's
/// job, so a `--rev` other than the checked-out one is recorded now and exported later.
async fn resolve_commit(root: &Path, rev: &str) -> Result<String, IntakeError> {
    let spec = format!("{rev}^{{commit}}");
    let args = [OsString::from("rev-parse"), OsString::from(spec)];
    let borrowed = borrow(&args);
    let captured = run_tool_capture_any("git", &borrowed, root, PLUMBING_TIMEOUT)
        .await
        .map_err(IntakeError::from_tool)?;
    let sha = captured.stdout_trimmed();
    if !captured.success || sha.is_empty() {
        return Err(IntakeError::RevNotFound {
            rev: rev.to_owned(),
            path: root.to_path_buf(),
            detail: captured.stderr_trimmed().to_owned(),
        });
    }
    Ok(sha.to_owned())
}

/// The origin URL with credentials removed, or `local:<path>` when there is no remote.
async fn origin_of(root: &Path) -> String {
    let args = [
        OsString::from("config"),
        OsString::from("--get"),
        OsString::from("remote.origin.url"),
    ];
    let borrowed = borrow(&args);
    let remote = match run_tool_capture_any("git", &borrowed, root, PLUMBING_TIMEOUT).await {
        Ok(captured) if captured.success => captured.stdout_trimmed().to_owned(),
        _ => String::new(),
    };
    if remote.is_empty() {
        let absolute = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        return format!("local:{}", absolute.display());
    }
    strip_credentials(&remote)
}

/// Removes the user information from a URL, so a token in a remote never reaches a
/// manifest, a report or a blob.
fn strip_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let Some((authority, tail)) = rest.split_once('/') else {
        return url.to_owned();
    };
    match authority.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://{host}/{tail}"),
        None => url.to_owned(),
    }
}

/// The `git --version` line, recorded so two runs can be compared.
async fn git_version() -> Result<String, IntakeError> {
    let args = [OsString::from("--version")];
    let borrowed = borrow(&args);
    let captured = run_tool_capture("git", &borrowed, Path::new("."), PLUMBING_TIMEOUT)
        .await
        .map_err(IntakeError::from_tool)?;
    Ok(captured.stdout_trimmed().to_owned())
}
