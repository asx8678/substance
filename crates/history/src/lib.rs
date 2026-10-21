//! The git engine, the GitHub forge client and registry lookups; the history,
//! provenance and dependency checks.
//!
//! M1-T01 lands the snapshot layer: every external response is saved under the run
//! directory and can be replayed with `--snapshot-from`.

pub mod dates;
pub mod deps;
pub mod forge;
pub mod git;
pub mod registry;
pub mod signals;
pub mod snapshot;

pub use forge::{
    Compare, Contributor, Forge, ForgeError, GitHub, GitHubRepoRef, GitHubUser, Issue,
    IssueComment, PullRequest, Release, RepoInfo, RepoSummary, Review, Stargazer, Tag, TreeEntry,
    WorkflowRun,
};
pub use git::{
    AuthoredFile, AuthoredFilter, BlameCoverage, BlameLine, BlameMap, BlameReport, Commit,
    CommitIdentity, FileChange, FileHunks, FileStats, GitCommand, GitError, GitRepo, Hunk,
    HunkLine, HunkLineKind, LinguistAttrs, PathsInCommit, Trailers, blame_all, is_bot,
    is_bot_commit,
};
pub use registry::{
    CRATES_API, Crate, CrateVersion, CratesIo, DEPS_DEV_API, Dependents, DepsDev, NPM_API,
    NPM_DOWNLOADS_API, Npm, NpmPackage, NpmVersion, PYPI_API, PyPI, PyPiPackage, PyPiRelease,
    Registry, RegistryError,
};
pub use snapshot::{
    SnapshotBody, SnapshotError, SnapshotHttp, SnapshotKind, SnapshotRequest, SnapshotResponse,
};
