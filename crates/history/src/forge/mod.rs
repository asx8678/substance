//! The forge client: repository history and vitality from GitHub (M1-T03, ADR-0005).
//!
//! The history checks need data that only the forge has: when the repository was created,
//! who merged which pull request, whether an issue author is an outsider. The plan names
//! GitHub and GitLab, but v1 pays for one client, so this module defines a [`Forge`] trait
//! and one implementation, [`GitHub`]. A second forge becomes another implementation
//! rather than a rewrite of the checks.
//!
//! Three properties make the data usable as evidence:
//!
//! - **Replayable.** Every request goes through [`SnapshotHttp`], so a run's forge replies
//!   are saved under the run directory and `--snapshot-from` reads them back without a
//!   network call. A reply the snapshot does not hold is not an error: the endpoint is
//!   skipped and named in [`Forge::skipped`], and the check that needed it reports
//!   `not_measured` instead of inventing a number.
//! - **Bounded.** Every request spends one unit of the `forge_max_requests` budget, and
//!   every list stops at a cap — 5,000 stargazers is the plan's number, and the same page
//!   cap applies to the other collections. Which lists stopped at a cap is readable from
//!   [`Forge::capped`], so a truncated sample never looks like a complete one.
//! - **Ordered.** Each list is sorted by a stated key, so two runs of one snapshot produce
//!   identical vectors regardless of the order GitHub happened to answer in.
//!
//! A non-GitHub origin, and a local path with no origin at all, answers
//! [`ForgeError::Unavailable`], whose message is exactly `forge unsupported`: the wording
//! ADR-0005 fixed for the `not_measured` reason.

pub mod github;

pub use github::{
    API_VERSION, GitHub, MAX_PAGES, PER_PAGE, STAR_ACCEPT, STAR_PAGE_LIMIT, STARGAZER_CAP,
    WINDOW_DAYS,
};

/// Builds the client for `origin`, or answers [`ForgeError::Unavailable`].
///
/// This is the one entry point a later stage needs: it turns the manifest's origin string
/// into a client, and a repository that is not on GitHub into the `not_measured` reason.
/// The token, when given, has already been read from the environment by
/// [`GitHub::token_from_env`] and lives only in the returned client. `as_of` is the
/// manifest's instant: collections are read back only as far as `as_of` minus the
/// longest check window, never as far as a wall clock allows.
///
/// # Errors
///
/// Returns [`ForgeError::Unavailable`] when `origin` is not a GitHub remote.
pub fn client_for(
    origin: &str,
    http: SnapshotHttp,
    auth: Option<String>,
    as_of: Timestamp,
) -> Result<GitHub, ForgeError> {
    let repo = GitHubRepoRef::parse(origin)?;
    Ok(GitHub::new(repo, http, auth, as_of))
}

use async_trait::async_trait;
use substance_core::Timestamp;

use crate::snapshot::SnapshotHttp;

/// Where a repository lives on a forge.
///
/// Only GitHub is recognised. [`GitHubRepoRef::parse`] accepts the URL shapes git writes
/// into `remote.origin.url` — `https://`, `ssh://`, `git@host:path` and a plain
/// `host/owner/repo` — and rejects everything whose host is not `github.com`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GitHubRepoRef {
    /// The account or organisation.
    pub owner: String,
    /// The repository name, without `.git`.
    pub repo: String,
}

impl GitHubRepoRef {
    /// Parses an origin URL, or answers [`ForgeError::Unavailable`].
    ///
    /// # Errors
    ///
    /// Returns [`ForgeError::Unavailable`] when the URL does not name
    /// `github.com/<owner>/<repo>`: a GitLab remote, a local path, an empty string, or a
    /// URL with a third path segment (a link into a tree, which is not a repository).
    pub fn parse(origin: &str) -> Result<Self, ForgeError> {
        let text = origin.trim().trim_end_matches('/');
        if text.is_empty() {
            return Err(ForgeError::Unavailable);
        }
        let without_scheme = match text.split_once("://") {
            Some((_scheme, rest)) => rest,
            // `git@github.com:owner/repo.git`, and the bare `github.com/owner/repo` form.
            None => text,
        };
        // Drop `user@`, which for an https remote may be a token.
        let authority_and_path = match without_scheme.rsplit_once('@') {
            Some((_, rest)) => rest,
            None => without_scheme,
        };
        let (host, path) = match authority_and_path.split_once([':', '/']) {
            Some(split) => split,
            None => return Err(ForgeError::Unavailable),
        };
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com")
        {
            return Err(ForgeError::Unavailable);
        }
        let path = path.trim_start_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let mut segments = path.split('/');
        let owner = segments.next().unwrap_or_default();
        let repo = segments.next().unwrap_or_default();
        if segments.next().is_some() {
            return Err(ForgeError::Unavailable);
        }
        if !is_slug(owner) || !is_slug(repo) {
            return Err(ForgeError::Unavailable);
        }
        Ok(Self {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
        })
    }

    /// `owner/repo`, the form GitHub's API paths use.
    #[must_use]
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// Whether `text` is a name GitHub would accept as an owner or repository.
///
/// GitHub allows letters, digits, `-`, `_` and `.`; anything else means the URL pointed at
/// something else, and a name with a space or a percent-escape would change the request
/// path rather than the repository it names.
fn is_slug(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 100
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        && text != "."
        && text != ".."
}

/// Why the forge could not answer.
///
/// Every variant carries a message a person can act on, and none of them can carry a
/// credential: no variant holds a header, a token or a request builder, only a status or a
/// URL that was already stripped of user information.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ForgeError {
    /// The origin is not a GitHub repository (ADR-0005).
    ///
    /// The message is the exact `not_measured` reason the checks report.
    #[error("forge unsupported")]
    Unavailable,
    /// GitHub answered 403 or 429: the anonymous or token budget is spent.
    #[error("forge rate limited")]
    RateLimited,
    /// GitHub rejected the credential with 401.
    #[error("forge rejected the token")]
    Unauthorized,
    /// The object does not exist, or the token may not see it.
    #[error("not found on the forge: {what}")]
    NotFound {
        /// What was asked for.
        what: String,
    },
    /// The `forge_max_requests` budget ran out.
    #[error("forge request budget exhausted after {used} requests")]
    Budget {
        /// How many requests were made before the cap.
        used: u32,
    },
    /// The request reached GitHub and failed with a status none of the variants above cover.
    #[error("the forge answered status {status} for {what}")]
    Http {
        /// The status code.
        status: u16,
        /// What was asked for.
        what: String,
    },
    /// GitHub answered, but not with the document this module parses.
    #[error("the forge answered an unexpected document: {detail}")]
    Schema {
        /// Which document and what was expected.
        detail: String,
    },
    /// The run directory could not be read or written.
    #[error("the forge snapshot cannot be used: {detail}")]
    Snapshot {
        /// The file or the missing entry.
        detail: String,
    },
}

impl ForgeError {
    /// Whether the forge is simply not available for this repository.
    #[must_use]
    pub const fn is_unavailable(&self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// One pull request, as the review and maintenance checks need it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequest {
    /// GitHub's per-repository number.
    pub number: u32,
    /// Who opened it.
    pub author: GitHubUser,
    /// When it was opened, server-side.
    pub created_at: Option<Timestamp>,
    /// When it was merged; `None` while it is open or closed unmerged.
    pub merged_at: Option<Timestamp>,
    /// The login that pressed merge, when GitHub recorded one.
    pub merged_by: Option<String>,
    /// The merge commit, when one exists.
    pub merge_commit_sha: Option<String>,
    /// Why it was closed without merging, when it was.
    pub state: String,
    /// The head commit, for comparing a fork against its upstream.
    pub head_sha: Option<String>,
    /// The branch it was opened from.
    pub head_ref: Option<String>,
    /// The branch it targets.
    pub base_ref: Option<String>,
    /// The labels, as their names, sorted. H13 reads the security label from here.
    pub labels: Vec<String>,
}

/// One review of a pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    /// GitHub's review ID.
    pub id: u64,
    /// The pull request the review belongs to.
    pub pull_number: u32,
    /// Who reviewed.
    pub author: GitHubUser,
    /// `APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, `DISMISSED` or `PENDING`.
    pub state: String,
    /// When it was submitted; `None` for a pending review.
    pub submitted_at: Option<Timestamp>,
    /// Whether the review carried no comment, which a rubber stamp needs.
    pub empty_body: bool,
}

/// One issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    /// GitHub's per-repository number.
    pub number: u32,
    /// Who opened it.
    pub author: GitHubUser,
    /// `OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR`, `FIRST_TIME_CONTRIBUTOR`,
    /// `FIRST_TIMER` or `NONE`.
    pub author_association: String,
    /// When it was opened, server-side.
    pub created_at: Option<Timestamp>,
    /// Whether it was closed by its author.
    pub closed_at: Option<Timestamp>,
}

/// One issue or pull-request comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueComment {
    /// GitHub's comment ID.
    pub id: u64,
    /// `owner/repo#number`, taken from the API's own `issue_url`.
    pub issue: String,
    /// Who wrote it.
    pub author: GitHubUser,
    /// `author_association`, as on the issue.
    pub author_association: String,
    /// When it was written.
    pub created_at: Option<Timestamp>,
}

impl IssueComment {
    /// Builds the `owner/repo#number` handle from the API's `issue_url`.
    ///
    /// The comment object carries the issue URL rather than its number, so the number is
    /// the last path segment of that URL. `None` when the URL does not end in a number.
    #[must_use]
    pub fn issue_handle(repo: &GitHubRepoRef, issue_url: &str) -> Option<String> {
        let number = issue_url.trim_end_matches('/').rsplit('/').next()?;
        if number.is_empty() || !number.chars().all(|ch| ch.is_ascii_digit()) {
            return None;
        }
        Some(format!("{}#{number}", repo.slug()))
    }
}

/// A person on the forge.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GitHubUser {
    /// The login.
    pub login: String,
    /// `User`, `Bot`, `Organization`; `None` when the API omitted it.
    pub kind: Option<String>,
    /// When the account was created: an account a week old is not an outsider customer.
    pub created_at: Option<Timestamp>,
}

impl GitHubUser {
    /// Whether the forge calls this account a bot.
    #[must_use]
    pub fn is_bot(&self) -> bool {
        super::is_bot(&self.login, "", self.kind.as_deref())
    }
}

/// The repository object, for chronology and template divergence.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RepoInfo {
    /// When the repository was created, server-side.
    pub created_at: Option<Timestamp>,
    /// Whether it is a fork.
    pub fork: bool,
    /// The parent, when this is a fork.
    pub parent: Option<GitHubRepoRef>,
    /// The template it was made from, when it was made from one.
    pub template: Option<GitHubRepoRef>,
    /// The default branch.
    pub default_branch: Option<String>,
    /// The licence's SPDX identifier, when GitHub detected one.
    pub license: Option<String>,
    /// Whether the repository is archived, which is the forge's own "abandoned" flag.
    ///
    /// P07 reads it as the strongest vitality signal: an archived repository is one its owner
    /// has declared finished, whatever its commit dates say.
    pub archived: bool,
    /// When anything last landed on any branch, as the forge records it.
    ///
    /// P07 reads this as "has anyone pushed recently": it is one field of the repository
    /// document, so the answer costs no request of its own.
    pub pushed_at: Option<Timestamp>,
}

/// One workflow run, for the CI-status and fix-chain checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    /// GitHub's run ID.
    pub id: u64,
    /// The commit it ran on.
    pub head_sha: String,
    /// `success`, `failure`, `cancelled`, `skipped`, `timed_out`; `None` while it runs.
    pub conclusion: Option<String>,
    /// When it was created.
    pub created_at: Option<Timestamp>,
    /// The event that started it, such as `push` or `pull_request`.
    pub event: Option<String>,
    /// The workflow's name.
    pub name: Option<String>,
}

/// One repository as a listing reports it, for H19's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoSummary {
    /// The repository's name.
    pub name: String,
    /// When it was created, server-side.
    pub created_at: Option<Timestamp>,
    /// Whether it is a fork. A fork is not a repository the owner created from nothing.
    pub fork: bool,
    /// Whether it is a template repository.
    pub template: bool,
}

/// One blob in a tree, for H17's template comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeEntry {
    /// The path, repo-relative, forward slashes.
    pub path: String,
    /// The blob SHA: equal for identical content, whatever the branch.
    pub sha: String,
}

/// One release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// GitHub's release ID.
    pub id: u64,
    /// The tag it publishes.
    pub tag_name: String,
    /// When it was published.
    pub created_at: Option<Timestamp>,
    /// Whether it is a draft.
    pub draft: bool,
    /// Whether it is marked pre-release.
    pub prerelease: bool,
}

/// One tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag {
    /// The tag name, such as `v1.2.0`.
    pub name: String,
    /// The commit it points at, peeled when it is an annotated tag.
    pub commit_sha: String,
}

/// One stargazer and the instant they starred, which is what a star spike needs.
///
/// The account's age is the other half of a spike signal, and it is deliberately not
/// fetched here: one `GET /users` per star would be up to 5,000 requests for a number
/// H18 only needs for the burst itself. A check asks [`Forge::user`] for the handful of
/// logins it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stargazer {
    /// The login.
    pub login: String,
    /// When the star was given. GitHub only reports it for the star media type.
    pub starred_at: Option<Timestamp>,
}

/// One contributor and their commit count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contributor {
    /// The login; empty entries GitHub reports for unattributed commits are dropped.
    pub login: String,
    /// The commit count GitHub attributes to them.
    pub contributions: u32,
    /// `User` or `Bot`.
    pub kind: Option<String>,
}

impl Contributor {
    /// Whether the forge calls this account a bot.
    #[must_use]
    pub fn is_bot(&self) -> bool {
        super::is_bot(&self.login, "", self.kind.as_deref())
    }
}

/// The result of comparing two commits across a fork boundary.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Compare {
    /// Commits the head has and the base does not.
    pub ahead_by: u32,
    /// Commits the base has and the head does not.
    pub behind_by: u32,
    /// The files the comparison touched.
    pub changed_files: u32,
    /// `ahead`, `behind`, `diverged` or `identical`.
    pub status: String,
}

/// What the history checks need from a forge.
///
/// Every method is a fetch of one kind of object, not a metric: the checks decide what a
/// merge, a review or an outsider means. Methods are `&mut self` because the request
/// budget and the snapshot writer are stateful, and they are `async` because the client
/// sits on an HTTP stack in live mode.
#[async_trait]
pub trait Forge: Send + Sync {
    /// The repository this client speaks for.
    fn repo_ref(&self) -> &GitHubRepoRef;

    /// The repository object: creation date, fork parent, template, default branch, licence.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn repo(&mut self) -> Result<RepoInfo, ForgeError>;

    /// Pull requests, newest first, by `(created_at desc, number asc)`.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn pulls(&mut self) -> Result<Vec<PullRequest>, ForgeError>;

    /// The reviews of one pull request, by `(submitted_at, id)`.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn reviews(&mut self, pull_number: u32) -> Result<Vec<Review>, ForgeError>;

    /// Issues, excluding pull requests, newest first by `(created_at desc, number asc)`.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn issues(&mut self) -> Result<Vec<Issue>, ForgeError>;

    /// Issue comments, by `(created_at, id)`.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn issue_comments(&mut self) -> Result<Vec<IssueComment>, ForgeError>;

    /// Workflow runs, newest first by `(created_at desc, id desc)`.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn workflow_runs(&mut self) -> Result<Vec<WorkflowRun>, ForgeError>;

    /// Releases, newest first by `(created_at desc, id desc)`.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn releases(&mut self) -> Result<Vec<Release>, ForgeError>;

    /// Tags, by name.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn tags(&mut self) -> Result<Vec<Tag>, ForgeError>;

    /// Stargazers with `starred_at`, newest first, capped at [`STARGAZER_CAP`].
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn stargazers(&mut self) -> Result<Vec<Stargazer>, ForgeError>;

    /// Contributors, by `(contributions desc, login asc)`.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn contributors(&mut self) -> Result<Vec<Contributor>, ForgeError>;

    /// One account, for the age of an issue author's account.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn user(&mut self, login: &str) -> Result<GitHubUser, ForgeError>;

    /// The repository this client speaks for, so a check can name the owner.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    fn repo_slug(&self) -> String {
        self.repo_ref().slug()
    }

    /// The repositories the owner created in a window, for H19's repo-farm context.
    ///
    /// One page, newest first, because H19 asks a contextual question and must not spend a
    /// repository-sized request budget on it.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn owner_repos(
        &mut self,
        since: Timestamp,
        until: Timestamp,
    ) -> Result<Vec<RepoSummary>, ForgeError>;

    /// Every blob path and SHA in `repo`'s default-branch tree, for H17.
    ///
    /// H17 is the one check that reads another repository (the template), so this takes the
    /// repository explicitly instead of using the client's own.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn tree(&mut self, repo: &GitHubRepoRef) -> Result<Vec<TreeEntry>, ForgeError>;

    /// Compares two commits, for telling a fork's own work from its upstream's.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`].
    async fn compare(&mut self, base: &str, head: &str) -> Result<Compare, ForgeError>;

    /// When the newest commit on the default branch was authored, for P07's vitality.
    ///
    /// The repository document carries `pushed_at`, which moves when any branch is pushed —
    /// including a stale feature branch. This asks the narrower question directly, with one
    /// request for one commit (`GET /repos/{owner}/{repo}/commits?per_page=1`), and answers
    /// `None` for an empty repository rather than inventing a date.
    ///
    /// # Errors
    ///
    /// See [`ForgeError`]. A repository with no commits answers `Ok(None)`.
    async fn last_commit(&mut self) -> Result<Option<Timestamp>, ForgeError>;

    /// How many requests have been spent so far.
    fn requests_used(&self) -> u32;

    /// The lists that stopped at a cap, sorted, so a truncated sample is visible.
    fn capped(&self) -> Vec<String>;

    /// The endpoints a replay could not answer, sorted, with the reason.
    fn skipped(&self) -> Vec<(String, String)>;

    /// What the sample covers, sorted: the horizon a collection was read back to, with
    /// the number of pages it took. A check turns this into its coverage note, so a
    /// window edge is never mistaken for a complete history.
    ///
    /// The default is empty, for an implementation that reads everything.
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
}
