//! The git engine, the GitHub forge client and registry lookups; the history,
//! provenance and dependency checks.
//!
//! M1-T01 lands the snapshot layer: every external response is saved under the run
//! directory and can be replayed with `--snapshot-from`.

pub mod forge;
pub mod git;
pub mod snapshot;

pub use forge::{
    Compare, Contributor, Forge, ForgeError, GitHub, GitHubRepoRef, GitHubUser, Issue,
    IssueComment, PullRequest, Release, RepoInfo, Review, Stargazer, Tag, WorkflowRun,
};
pub use git::{
    AuthoredFile, AuthoredFilter, BlameCoverage, BlameLine, BlameMap, BlameReport, Commit,
    CommitIdentity, FileChange, FileHunks, FileStats, GitCommand, GitError, GitRepo, Hunk,
    HunkLine, HunkLineKind, LinguistAttrs, PathsInCommit, Trailers, blame_all, is_bot,
    is_bot_commit,
};
pub use snapshot::{
    SnapshotBody, SnapshotError, SnapshotHttp, SnapshotKind, SnapshotRequest, SnapshotResponse,
};
