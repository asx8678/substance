//! The GitHub REST implementation of [`Forge`] (M1-T03, ADR-0005).
//!
//! REST only. GraphQL would save calls for the review and issue-comment fan-out — one
//! search instead of one request per pull request — but it needs a token even to read a
//! public repository, and the plan makes the token optional. So the client costs more
//! requests and works anonymously; every request spends one unit of `forge_max_requests`
//! and the run reports how many were spent.
//!
//! The endpoints and fields below were confirmed against GitHub's REST reference
//! (<https://docs.github.com/en/rest>) rather than by calling the API:
//!
//! | Call | Endpoint | Fields used |
//! | --- | --- | --- |
//! | repo | `GET /repos/{owner}/{repo}` | `created_at`, `fork`, `parent.full_name`, `template_repository.full_name`, `default_branch`, `license.spdx_id` |
//! | pulls | `GET /repos/{owner}/{repo}/pulls?state=all&sort=created&direction=desc` | `number`, `user`, `created_at`, `merged_at`, `merged_by.login`, `merge_commit_sha`, `state`, `head.sha/ref`, `base.ref` |
//! | reviews | `GET /repos/{owner}/{repo}/pulls/{number}/reviews` | `id`, `user`, `state`, `submitted_at`, `body` |
//! | issues | `GET /repos/{owner}/{repo}/issues?state=all&sort=created&direction=desc` | `number`, `user`, `author_association`, `created_at`, `closed_at`, `pull_request` (presence marks a PR) |
//! | issue comments | `GET /repos/{owner}/{repo}/issues/comments?sort=created&direction=desc` | `id`, `issue_url`, `user`, `author_association`, `created_at` |
//! | workflow runs | `GET /repos/{owner}/{repo}/actions/runs` | `id`, `head_sha`, `conclusion`, `created_at`, `event`, `name` |
//! | releases | `GET /repos/{owner}/{repo}/releases` | `id`, `tag_name`, `created_at`, `draft`, `prerelease` |
//! | tags | `GET /repos/{owner}/{repo}/tags` | `name`, `commit.sha` |
//! | stargazers | `GET /repos/{owner}/{repo}/stargazers` (oldest first, no sort parameter) | `login`, `starred_at` (star media type) |
//! | contributors | `GET /repos/{owner}/{repo}/contributors` | `login`, `contributions`, `type` |
//! | user | `GET /users/{login}` | `login`, `type`, `created_at` |
//! | compare | `GET /repos/{owner}/{repo}/compare/{base}...{head}` | `ahead_by`, `behind_by`, `changed_files`, `status` |
//!
//! # Which end a cap truncates
//!
//! A cap is only sound if it keeps the end the checks read. H02, H12, H13 and H14 all
//! window over "the last 90 days" and "the last 365 days" from `as_of`, so a client that
//! kept the *oldest* 6,000 pull requests and dropped the recent ones would be blind to
//! exactly what it is asked about.
//!
//! - Pull requests, issues and issue comments are requested `direction=desc`: newest
//!   first, so the page limit truncates the oldest end. Paging also stops as soon as a
//!   whole page is older than [`WINDOW_DAYS`] before `as_of`, which is the longest window
//!   any check uses, and reports that in [`Forge::notes`]; items below that horizon are
//!   dropped rather than kept as an unlooked-at tail.
//! - Workflow runs and releases need no `direction`: GitHub's own documentation returns
//!   both newest first (`actions/runs` is ordered by `created_at` descending, releases by
//!   `created_at` descending), so the same page limit and early stop apply unchanged.
//! - Stargazers accept no sort parameter and are oldest first. The client reads page 1,
//!   takes the `Link` header's `rel="last"` page number, and then walks pages backwards
//!   from the last one until it holds [`STARGAZER_CAP`] stars or has fetched
//!   [`STAR_PAGE_LIMIT`] pages, so the set it keeps is the 5,000 *most recent* stars.
//!   [`LAST_PAGE_ACCEPT`] is what makes `Link` present: it is omitted for an empty list.
//!
//! Page numbers are always a function of this code — a counter, or `last` read from the
//! page-1 response — never of an ordering the server chooses per request, so the same
//! snapshot always yields the same sequence of requests and the same set of items.

use async_trait::async_trait;
use serde::Deserialize;
use substance_core::Timestamp;

use super::{
    Compare, Contributor, Forge, ForgeError, GitHubRepoRef, GitHubUser, Issue, IssueComment,
    PullRequest, Release, RepoInfo, RepoSummary, Review, Stargazer, Tag, TreeEntry, WorkflowRun,
};
use crate::snapshot::{SnapshotError, SnapshotHttp, SnapshotKind, SnapshotRequest};
use substance_core::clock::window_start;

/// The REST API root every path is appended to.
pub const API_ROOT: &str = "https://api.github.com";

/// The media type that adds `starred_at` to a stargazer.
pub const STAR_ACCEPT: &str = "application/vnd.github.star+json";

/// The API version pinned by the `X-GitHub-Api-Version` header.
///
/// Pinning it is what makes the field shapes above a promise rather than a guess: without
/// the header GitHub may answer with an older or newer representation.
pub const API_VERSION: &str = "2022-11-28";

/// How many items one page holds; the API's maximum.
pub const PER_PAGE: u32 = 100;

/// How many pages one collection may read.
///
/// 60 pages is 6,000 items at [`PER_PAGE`], which bounds a collection against a
/// repository with a million issues. Combined with the newest-first order and the
/// [`WINDOW_DAYS`] early stop, the cap truncates the oldest end, which no check reads.
pub const MAX_PAGES: u32 = 60;

/// How far back a collection is read before paging stops.
///
/// 365 days is the longest window any history check uses (H02 reads the last 365 days;
/// H12–H14 read 90 and 365). Anything older is fetched by no check, so fetching it would
/// spend the request budget for nothing and crowd out the recent items a cap would
/// otherwise drop.
pub const WINDOW_DAYS: u32 = 365;

/// The most stargazers the plan asks for (M1-T03: the 5,000 most recent).
pub const STARGAZER_CAP: usize = 5_000;

/// How many pages a backwards stargazer walk may read: 50 pages hold exactly
/// [`STARGAZER_CAP`] stars at [`PER_PAGE`], plus the page-1 probe that found `last`.
pub const STAR_PAGE_LIMIT: u32 = 50;

/// A GitHub client for one repository.
///
/// Holds the request budget's state in the [`SnapshotHttp`] it was given, so a caller
/// builds one per run and every check that uses it shares one count.
pub struct GitHub {
    repo: GitHubRepoRef,
    http: SnapshotHttp,
    auth: Option<String>,
    as_of: Timestamp,
    capped: Vec<String>,
    notes: Vec<String>,
    skipped: Vec<(String, String)>,
}

impl GitHub {
    /// A client for `repo`, sending `auth` (an `Authorization` header value) when present.
    ///
    /// The token is kept in memory for the life of the client and is never given to the
    /// snapshot writer: [`SnapshotHttp`] writes response bodies and an allowlisted set of
    /// response headers, and a request's headers are not among them.
    #[must_use]
    pub fn new(
        repo: GitHubRepoRef,
        http: SnapshotHttp,
        auth: Option<String>,
        as_of: Timestamp,
    ) -> Self {
        Self {
            repo,
            http,
            auth,
            as_of,
            capped: Vec::new(),
            notes: Vec::new(),
            skipped: Vec::new(),
        }
    }

    /// The oldest instant a collection is read back to: `as_of` minus [`WINDOW_DAYS`].
    ///
    /// Taken from the manifest's `as_of` through core's [`window_start`], never from the
    /// wall clock, so two runs of one instant page to the same horizon.
    #[must_use]
    pub fn horizon(&self) -> Timestamp {
        window_start(self.as_of, WINDOW_DAYS)
    }

    /// Reads `GITHUB_TOKEN` and turns it into the header value GitHub expects.
    ///
    /// An inherited variable is read once, here, and never written anywhere. It is not
    /// logged, not stored in the run directory and not part of any snapshot key: the key
    /// hashes the method, URL and body only.
    #[must_use]
    pub fn token_from_env() -> Option<String> {
        match std::env::var("GITHUB_TOKEN") {
            Ok(token) if !token.trim().is_empty() => Some(format!("Bearer {}", token.trim())),
            _ => None,
        }
    }

    /// The URL of a path relative to the API root.
    fn url(&self, path: &str) -> String {
        format!("{API_ROOT}{path}")
    }

    /// The URL of a repository-relative path.
    fn repo_url(&self, suffix: &str) -> String {
        self.url(&format!("/repos/{}{suffix}", self.repo.slug()))
    }

    /// Fetches one page and parses it.
    async fn fetch_page<T: for<'de> Deserialize<'de>>(
        &mut self,
        label: &str,
        url: String,
        accept: &str,
        array: bool,
    ) -> Result<Vec<T>, ForgeError> {
        let (items, _headers) = self
            .fetch_page_with_headers::<T>(label, url, accept, array)
            .await?;
        Ok(items)
    }

    /// Fetches one page, parses it, and keeps the saved response headers.
    ///
    /// The headers matter for one thing only: `Link`, which names the last page of a
    /// collection that cannot be sorted. They are read from the snapshot, so a replay sees
    /// the `Link` the live run saw.
    async fn fetch_page_with_headers<T: for<'de> Deserialize<'de>>(
        &mut self,
        label: &str,
        url: String,
        accept: &str,
        array: bool,
    ) -> Result<(Vec<T>, std::collections::BTreeMap<String, String>), ForgeError> {
        let request = SnapshotRequest::get(SnapshotKind::Forge, &url)
            .with_accept(accept)
            .with_header("x-github-api-version", API_VERSION)
            .with_auth(self.auth.clone());
        let response = match self.http.fetch(&request).await {
            Ok(response) => response,
            Err(error) => return self.map_error(label, error),
        };
        if !response.is_success() {
            return Err(self.map_status(label, response.status));
        }
        let headers = response.headers.clone();
        let text = response.body.as_text();
        let items = if array {
            serde_json::from_str::<Vec<T>>(&text)
                .map_err(|error| schema(label, &error.to_string()))?
        } else {
            let one: T =
                serde_json::from_str(&text).map_err(|error| schema(label, &error.to_string()))?;
            vec![one]
        };
        Ok((items, headers))
    }

    /// Fetches one page of a list and reads the last page number from its `Link` header.
    ///
    /// `Link` looks like
    /// `<https://api.github.com/…?page=2>; rel="next", <…?page=57>; rel="last"`.
    /// A one-page list carries no `Link` at all, and a list with no entries carries none
    /// either, so a missing header means "this page is the whole collection".
    async fn fetch_page_with_last<T: for<'de> Deserialize<'de>>(
        &mut self,
        label: &str,
        url: String,
        accept: &str,
    ) -> Result<(Vec<T>, u32), ForgeError> {
        let (items, headers) = self
            .fetch_page_with_headers::<T>(label, url, accept, true)
            .await?;
        let last = headers
            .get("link")
            .and_then(|link| last_page_of(link))
            .unwrap_or(1);
        Ok((items, last))
    }

    /// Fetches every page of a collection, stopping at [`MAX_PAGES`] or at a short page.
    ///
    /// `direction` and `sort` are in `query`, so the order GitHub walks is fixed by this
    /// module rather than by the server's default. A collection that fills its last page
    /// is recorded in [`Forge::capped`].
    async fn fetch_all<T: for<'de> Deserialize<'de>>(
        &mut self,
        label: &str,
        path: &str,
        query: &str,
        accept: &str,
    ) -> Result<Vec<T>, ForgeError> {
        let mut items: Vec<T> = Vec::new();
        let mut page = 1;
        loop {
            let url = self.page_url(path, page, query);
            let page_items = self.fetch_page::<T>(label, url, accept, true).await?;
            let count = page_items.len();
            items.extend(page_items);
            if count < PER_PAGE as usize {
                break;
            }
            if page >= MAX_PAGES {
                self.note_capped(label);
                break;
            }
            page += 1;
        }
        Ok(items)
    }

    /// The URL of one page of `path`: the page counter is the only source of the number.
    fn page_url(&self, path: &str, page: u32, query: &str) -> String {
        format!(
            "{}?per_page={PER_PAGE}&page={page}{query}",
            self.repo_url(path)
        )
    }

    /// Fetches a newest-first collection, stopping once a whole page is older than the
    /// horizon, at [`MAX_PAGES`], or at a short page.
    ///
    /// `when` reads each item's server-side timestamp. The early stop is what keeps the
    /// page cap harmless on a repository with more than 6,000 items: the pages that are
    /// dropped are the ones no check reads. When the stop fires, the horizon is recorded in
    /// [`Forge::notes`] so a check can say how far back the evidence reaches; when the page
    /// cap fires instead, the collection is capped *and* noted, because then the truncation
    /// is not at a window edge.
    async fn fetch_recent<T, F>(
        &mut self,
        label: &str,
        path: &str,
        query: &str,
        accept: &str,
        when: F,
    ) -> Result<Vec<T>, ForgeError>
    where
        T: for<'de> Deserialize<'de>,
        F: Fn(&T) -> Option<Timestamp>,
    {
        let horizon = self.horizon();
        let mut items: Vec<T> = Vec::new();
        let mut page = 1;
        loop {
            let url = self.page_url(path, page, query);
            let page_items = self.fetch_page::<T>(label, url, accept, true).await?;
            let count = page_items.len();
            // Every item on the page is older than the horizon: the rest of the collection
            // is older still, because the order is newest first. An item with no date
            // counts as recent, so an undated record is never dropped.
            let page_is_old = !page_items.is_empty()
                && page_items
                    .iter()
                    .all(|item| when(item).is_some_and(|stamp| stamp < horizon));
            if page_is_old {
                self.note_reach(label, page);
                break;
            }
            items.extend(
                page_items
                    .into_iter()
                    .filter(|item| when(item).is_none_or(|stamp| stamp >= horizon)),
            );
            if count < PER_PAGE as usize {
                break;
            }
            if page >= MAX_PAGES {
                self.note_capped(label);
                self.note_truncated(label, items.len(), page);
                break;
            }
            page += 1;
        }
        Ok(items)
    }

    /// Records that `label` stopped at the page cap with `kept` of its newest items.
    ///
    /// This is the note a reader needs when the cap, not the horizon, decided the sample:
    /// it says the sample is the newest slice and that older records exist unread.
    fn note_truncated(&mut self, label: &str, kept: usize, page: u32) {
        let note = format!(
            "{label}: page cap at {MAX_PAGES} pages ({page} read), kept the newest {kept} items; older records were not read"
        );
        if !self.notes.iter().any(|seen| seen == &note) {
            self.notes.push(note);
            self.notes.sort();
        }
    }

    /// Fetches the most recent [`STARGAZER_CAP`] stars, walking pages backwards.
    ///
    /// The stargazer endpoint sorts oldest first and takes no sort parameter, so the
    /// newest stars are on the last page. Page 1 is read for its `Link: rel="last"`
    /// header; pages are then fetched from that number down until the cap is reached or
    /// [`STAR_PAGE_LIMIT`] pages have been read. The requests are still a function of the
    /// code plus one server-reported number that the snapshot replays identically.
    async fn fetch_stars(&mut self) -> Result<Vec<WireStar>, ForgeError> {
        // Page 1 is the *oldest* page and exists only to report `last`; its items are used
        // at the end, if the walk reaches it, because they are the oldest stars.
        let (first, last) = self
            .fetch_page_with_last::<WireStar>(
                "stargazers",
                self.page_url("/stargazers", 1, ""),
                STAR_ACCEPT,
            )
            .await?;
        if first.is_empty() {
            return Ok(Vec::new());
        }
        if last == 1 {
            return Ok(first);
        }
        let mut items: Vec<WireStar> = Vec::new();
        let mut pages_read = 0u32;
        let mut page = last;
        while page >= 2 && items.len() < STARGAZER_CAP && pages_read < STAR_PAGE_LIMIT {
            let url = self.page_url("/stargazers", page, "");
            let mut page_items = self
                .fetch_page::<WireStar>("stargazers", url, STAR_ACCEPT, true)
                .await?;
            // A page in the middle of a walk cannot legitimately be empty; stopping keeps
            // the loop total instead of looping on a disagreeing snapshot.
            if page_items.is_empty() {
                break;
            }
            page_items.append(&mut items);
            items = page_items;
            pages_read += 1;
            page -= 1;
        }
        if page >= 2 {
            // The walk stopped with pages still unread: the sample is the newest slice,
            // not the whole history.
            self.note_capped("stargazers");
        } else {
            // Every page was read, so the oldest page's stars belong in the sample too.
            let mut complete = first;
            complete.append(&mut items);
            items = complete;
        }
        // Pages are accumulated oldest-first (a page is ascending, and older pages are
        // prepended), so the newest stars are at the end.
        if items.len() > STARGAZER_CAP {
            items.drain(..items.len() - STARGAZER_CAP);
        }
        Ok(items)
    }

    /// Records that `label` was read back to its window horizon on `page` pages.
    fn note_reach(&mut self, label: &str, page: u32) {
        let note = format!("{label}: read back to the {WINDOW_DAYS}-day horizon ({page} pages)");
        if !self.notes.iter().any(|seen| seen == &note) {
            self.notes.push(note);
            self.notes.sort();
        }
    }

    /// Records that `label` stopped at a cap.
    fn note_capped(&mut self, label: &str) {
        if !self.capped.iter().any(|seen| seen == label) {
            self.capped.push(label.to_owned());
            self.capped.sort();
        }
    }

    /// Records that `label` could not be read, so the check can report the reason.
    fn note_skipped(&mut self, label: &str, reason: &str) {
        if !self.skipped.iter().any(|(seen, _)| seen.as_str() == label) {
            self.skipped.push((label.to_owned(), reason.to_owned()));
            self.skipped.sort_by(|left, right| left.0.cmp(&right.0));
        }
    }

    /// Turns a snapshot failure into a [`ForgeError`].
    fn map_error<T>(&mut self, label: &str, error: SnapshotError) -> Result<T, ForgeError> {
        match error {
            SnapshotError::BudgetExhausted { used, .. } => Err(ForgeError::Budget { used }),
            SnapshotError::Missing { url, .. } => {
                self.note_skipped(label, "not in the snapshot");
                Err(ForgeError::Snapshot {
                    detail: format!("{label} is not in the snapshot ({url})"),
                })
            }
            other => {
                self.note_skipped(label, "the snapshot could not be read");
                Err(ForgeError::Snapshot {
                    detail: other.to_string(),
                })
            }
        }
    }

    /// Maps a non-success status, without echoing the request.
    fn map_status(&mut self, label: &str, status: u16) -> ForgeError {
        match status {
            401 => {
                self.note_skipped(label, "the forge rejected the token");
                ForgeError::Unauthorized
            }
            403 | 429 => {
                self.note_skipped(label, "rate limited");
                ForgeError::RateLimited
            }
            404 | 410 => {
                self.note_skipped(label, "not found on the forge");
                ForgeError::NotFound {
                    what: label.to_owned(),
                }
            }
            other => {
                self.note_skipped(label, "the forge answered an error");
                ForgeError::Http {
                    status: other,
                    what: label.to_owned(),
                }
            }
        }
    }

    /// A single-object fetch.
    async fn fetch_one<T: for<'de> Deserialize<'de>>(
        &mut self,
        label: &str,
        url: String,
    ) -> Result<T, ForgeError> {
        let mut items = self
            .fetch_page::<T>(label, url, crate::snapshot::JSON_ACCEPT, false)
            .await?;
        items
            .pop()
            .ok_or_else(|| schema(label, "an empty document where one object was expected"))
    }
}

/// The page number a `Link` header names as `rel="last"`.
///
/// Parsed without a regex crate: the header is a comma-separated list of
/// `<url>; rel="name"` entries, and the answer is the `page` query value of the entry
/// whose relation is `last`. Anything unexpected — no `last` entry, a `page` that is not a
/// number — yields `None`, which the caller reads as "one page".
fn last_page_of(link: &str) -> Option<u32> {
    for entry in link.split(',') {
        let mut parts = entry.split(';');
        let url = parts.next()?.trim();
        let mut is_last = false;
        for parameter in parts {
            let parameter = parameter.trim();
            if let Some(relation) = parameter.strip_prefix("rel=") {
                let relation = relation.trim().trim_matches('"');
                is_last = relation.eq_ignore_ascii_case("last");
            }
        }
        if !is_last {
            continue;
        }
        let query = url.trim_start_matches('<').trim_end_matches('>');
        let query = query.split_once('?')?.1;
        for pair in query.split('&') {
            if let Some(value) = pair.strip_prefix("page=") {
                return value.parse::<u32>().ok();
            }
        }
    }
    None
}

/// A schema mismatch, naming the document and what was wrong with it.
fn schema(label: &str, detail: &str) -> ForgeError {
    ForgeError::Schema {
        detail: format!("{label}: {detail}"),
    }
}

/// Parses GitHub's `created_at`/`submitted_at`, which is RFC 3339 in UTC.
///
/// An unparseable or missing date becomes `None` rather than an error: a null date means
/// "not known", and a check that needs one reports `not_measured` for that record instead
/// of failing the whole collection.
fn timestamp(text: Option<&str>) -> Option<Timestamp> {
    text.and_then(|value| Timestamp::parse_rfc3339(value).ok())
}

/// `owner/repo` from a `full_name`.
fn repo_ref(name: Option<&str>) -> Option<GitHubRepoRef> {
    let (owner, repo) = name?.split_once('/')?;
    GitHubRepoRef::parse(&format!("https://github.com/{owner}/{repo}")).ok()
}

// ---------------------------------------------------------------------------------------
// Wire types. Only the fields this module reads are named; serde ignores the rest, because
// a GitHub object carries dozens of fields the checks never look at.
// ---------------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct WireUser {
    login: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    created_at: Option<String>,
}

impl WireUser {
    fn into_user(self) -> GitHubUser {
        GitHubUser {
            login: self.login.unwrap_or_default(),
            kind: self.kind,
            created_at: timestamp(self.created_at.as_deref()),
        }
    }
}

#[derive(Debug, Deserialize)]
struct WireLicense {
    spdx_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireRepo {
    created_at: Option<String>,
    archived: Option<bool>,
    pushed_at: Option<String>,
    fork: Option<bool>,
    parent: Option<WireRepoName>,
    template_repository: Option<WireRepoName>,
    default_branch: Option<String>,
    license: Option<WireLicense>,
}

/// One entry of a commit listing: P07 reads only its dates.
#[derive(Debug, Deserialize)]
struct WireCommitEntry {
    commit: Option<WireCommitDetail>,
}

#[derive(Debug, Deserialize)]
struct WireCommitDetail {
    author: Option<WireCommitStamp>,
    committer: Option<WireCommitStamp>,
}

#[derive(Debug, Deserialize)]
struct WireCommitStamp {
    date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireRepoName {
    full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WirePull {
    number: u32,
    user: Option<WireUser>,
    created_at: Option<String>,
    merged_at: Option<String>,
    merged_by: Option<WireUser>,
    merge_commit_sha: Option<String>,
    state: Option<String>,
    head: Option<WireBranch>,
    base: Option<WireBranch>,
    #[serde(default)]
    labels: Vec<WireLabel>,
}

#[derive(Debug, Deserialize)]
struct WireLabel {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireBranch {
    sha: Option<String>,
    #[serde(rename = "ref")]
    branch: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireReview {
    id: u64,
    user: Option<WireUser>,
    state: Option<String>,
    submitted_at: Option<String>,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireIssue {
    number: u32,
    user: Option<WireUser>,
    author_association: Option<String>,
    created_at: Option<String>,
    closed_at: Option<String>,
    /// Present only on pull requests: the issues endpoint returns both kinds.
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct WireComment {
    id: u64,
    issue_url: Option<String>,
    user: Option<WireUser>,
    author_association: Option<String>,
    created_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireRuns {
    workflow_runs: Vec<WireRun>,
}

#[derive(Debug, Deserialize)]
struct WireRun {
    id: u64,
    head_sha: Option<String>,
    conclusion: Option<String>,
    created_at: Option<String>,
    event: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireRelease {
    id: u64,
    tag_name: Option<String>,
    created_at: Option<String>,
    draft: Option<bool>,
    prerelease: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WireTag {
    name: Option<String>,
    commit: Option<WireCommit>,
}

#[derive(Debug, Deserialize)]
struct WireCommit {
    sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireStar {
    login: Option<String>,
    starred_at: Option<String>,
    /// Present when the plain media type answered, which has no `starred_at`.
    user: Option<WireUser>,
}

#[derive(Debug, Deserialize)]
struct WireContributor {
    login: Option<String>,
    contributions: Option<u32>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireRepoSummary {
    name: Option<String>,
    created_at: Option<String>,
    fork: Option<bool>,
    is_template: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WireTree {
    #[serde(default)]
    tree: Vec<WireTreeEntry>,
    truncated: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct WireTreeEntry {
    path: Option<String>,
    sha: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WireCompare {
    ahead_by: Option<u32>,
    behind_by: Option<u32>,
    changed_files: Option<u32>,
    status: Option<String>,
}

/// Compares two optional instants, newest first, treating a missing one as oldest.
fn newest_first(left: Option<Timestamp>, right: Option<Timestamp>) -> std::cmp::Ordering {
    match (left, right) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => std::cmp::Ordering::Equal,
    }
}

#[async_trait]
impl Forge for GitHub {
    fn repo_ref(&self) -> &GitHubRepoRef {
        &self.repo
    }

    async fn repo(&mut self) -> Result<RepoInfo, ForgeError> {
        let url = self.repo_url("");
        let wire: WireRepo = self.fetch_one("repo", url).await?;
        Ok(RepoInfo {
            created_at: timestamp(wire.created_at.as_deref()),
            fork: wire.fork.unwrap_or(false),
            parent: wire
                .parent
                .and_then(|parent| repo_ref(parent.full_name.as_deref())),
            template: wire
                .template_repository
                .and_then(|template| repo_ref(template.full_name.as_deref())),
            default_branch: wire.default_branch,
            archived: wire.archived.unwrap_or(false),
            pushed_at: timestamp(wire.pushed_at.as_deref()),
            license: wire
                .license
                .and_then(|license| license.spdx_id)
                .filter(|spdx| !spdx.is_empty() && spdx != "NOASSERTION"),
        })
    }

    async fn pulls(&mut self) -> Result<Vec<PullRequest>, ForgeError> {
        // Newest first, so a page cap or the window stop drops the oldest pull requests;
        // the vector is sorted by the stated key afterwards, so the wire order is only a
        // paging input.
        let wire: Vec<WirePull> = self
            .fetch_recent::<WirePull, _>(
                "pulls",
                "/pulls",
                "&state=all&sort=created&direction=desc",
                crate::snapshot::JSON_ACCEPT,
                |pull| timestamp(pull.created_at.as_deref()),
            )
            .await?;
        let mut pulls: Vec<PullRequest> = wire
            .into_iter()
            .map(|pull| PullRequest {
                number: pull.number,
                author: pull
                    .user
                    .map_or_else(GitHubUser::default, WireUser::into_user),
                created_at: timestamp(pull.created_at.as_deref()),
                merged_at: timestamp(pull.merged_at.as_deref()),
                merged_by: pull.merged_by.and_then(|user| user.login),
                merge_commit_sha: pull.merge_commit_sha,
                state: pull.state.unwrap_or_default(),
                head_sha: pull.head.as_ref().and_then(|head| head.sha.clone()),
                head_ref: pull.head.as_ref().and_then(|head| head.branch.clone()),
                base_ref: pull.base.as_ref().and_then(|base| base.branch.clone()),
                labels: {
                    // Sorted, so two runs of one snapshot list the same order.
                    let mut labels: Vec<String> = pull
                        .labels
                        .into_iter()
                        .filter_map(|label| label.name)
                        .filter(|name| !name.trim().is_empty())
                        .collect();
                    labels.sort();
                    labels.dedup();
                    labels
                },
            })
            .collect();
        pulls.sort_by(|left, right| {
            newest_first(left.created_at, right.created_at)
                .then_with(|| left.number.cmp(&right.number))
        });
        Ok(pulls)
    }

    async fn reviews(&mut self, pull_number: u32) -> Result<Vec<Review>, ForgeError> {
        let path = format!("/pulls/{pull_number}/reviews");
        let label = format!("reviews #{pull_number}");
        let wire: Vec<WireReview> = self
            .fetch_all::<WireReview>(&label, &path, "", crate::snapshot::JSON_ACCEPT)
            .await?;
        let mut reviews: Vec<Review> = wire
            .into_iter()
            .map(|review| Review {
                id: review.id,
                pull_number,
                author: review
                    .user
                    .map_or_else(GitHubUser::default, WireUser::into_user),
                state: review.state.unwrap_or_default(),
                submitted_at: timestamp(review.submitted_at.as_deref()),
                empty_body: review
                    .body
                    .as_deref()
                    .is_none_or(|body| body.trim().is_empty()),
            })
            .collect();
        reviews.sort_by(|left, right| {
            left.submitted_at
                .cmp(&right.submitted_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(reviews)
    }

    async fn issues(&mut self) -> Result<Vec<Issue>, ForgeError> {
        // The issues endpoint returns pull requests too. The filter runs after the page
        // loop, so a page can hold fewer issues than PER_PAGE without being the last one:
        // the loop counts what the wire returned, before this filter.
        let wire: Vec<WireIssue> = self
            .fetch_recent::<WireIssue, _>(
                "issues",
                "/issues",
                "&state=all&sort=created&direction=desc",
                crate::snapshot::JSON_ACCEPT,
                |issue| timestamp(issue.created_at.as_deref()),
            )
            .await?;
        let mut issues: Vec<Issue> = wire
            .into_iter()
            .filter(|issue| issue.pull_request.is_none())
            .map(|issue| Issue {
                number: issue.number,
                author: issue
                    .user
                    .map_or_else(GitHubUser::default, WireUser::into_user),
                author_association: issue.author_association.unwrap_or_default(),
                created_at: timestamp(issue.created_at.as_deref()),
                closed_at: timestamp(issue.closed_at.as_deref()),
            })
            .collect();
        issues.sort_by(|left, right| {
            newest_first(left.created_at, right.created_at)
                .then_with(|| left.number.cmp(&right.number))
        });
        Ok(issues)
    }

    async fn issue_comments(&mut self) -> Result<Vec<IssueComment>, ForgeError> {
        let wire: Vec<WireComment> = self
            .fetch_recent::<WireComment, _>(
                "issue comments",
                "/issues/comments",
                "&sort=created&direction=desc",
                crate::snapshot::JSON_ACCEPT,
                |comment| timestamp(comment.created_at.as_deref()),
            )
            .await?;
        let repo = self.repo.clone();
        let mut comments: Vec<IssueComment> = Vec::new();
        for comment in wire {
            let Some(handle) = comment
                .issue_url
                .as_deref()
                .and_then(|url| IssueComment::issue_handle(&repo, url))
            else {
                // A comment whose issue cannot be identified is dropped rather than given
                // a made-up handle: H13 and H15 key responses by issue.
                self.note_skipped("issue comments", "a comment has no issue URL");
                continue;
            };
            comments.push(IssueComment {
                id: comment.id,
                issue: handle,
                author: comment
                    .user
                    .map_or_else(GitHubUser::default, WireUser::into_user),
                author_association: comment.author_association.unwrap_or_default(),
                created_at: timestamp(comment.created_at.as_deref()),
            });
        }
        comments.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(comments)
    }

    async fn workflow_runs(&mut self) -> Result<Vec<WorkflowRun>, ForgeError> {
        // `actions/runs` is documented newest first (ordered by `created_at` descending)
        // and takes no direction parameter, so the same page cap and window stop apply as
        // for pulls. The endpoint wraps its page in `{ "workflow_runs": [...] }`, so the
        // wrapper is parsed first and the window stop reads the wrapped items.
        let horizon = self.horizon();
        let mut runs: Vec<WorkflowRun> = Vec::new();
        let mut page = 1;
        loop {
            let url = self.page_url("/actions/runs", page, "");
            let pages: Vec<WireRuns> = self
                .fetch_page::<WireRuns>("workflow runs", url, crate::snapshot::JSON_ACCEPT, false)
                .await?;
            let Some(page_runs) = pages.into_iter().next() else {
                break;
            };
            let count = page_runs.workflow_runs.len();
            let page_is_old = count > 0
                && page_runs.workflow_runs.iter().all(|run| {
                    timestamp(run.created_at.as_deref()).is_some_and(|stamp| stamp < horizon)
                });
            if page_is_old {
                self.note_reach("workflow runs", page);
                break;
            }
            runs.extend(
                page_runs
                    .workflow_runs
                    .into_iter()
                    .filter(|run| {
                        timestamp(run.created_at.as_deref()).is_none_or(|stamp| stamp >= horizon)
                    })
                    .map(|run| WorkflowRun {
                        id: run.id,
                        head_sha: run.head_sha.unwrap_or_default(),
                        conclusion: run.conclusion,
                        created_at: timestamp(run.created_at.as_deref()),
                        event: run.event,
                        name: run.name,
                    }),
            );
            if count < PER_PAGE as usize {
                break;
            }
            if page >= MAX_PAGES {
                self.note_capped("workflow runs");
                self.note_reach("workflow runs", page);
                break;
            }
            page += 1;
        }
        runs.sort_by(|left, right| {
            newest_first(left.created_at, right.created_at).then_with(|| right.id.cmp(&left.id))
        });
        Ok(runs)
    }

    async fn releases(&mut self) -> Result<Vec<Release>, ForgeError> {
        // `releases` is documented newest first (by `created_at` descending) and takes no
        // direction parameter, so the page cap and window stop are the same as for pulls.
        let wire: Vec<WireRelease> = self
            .fetch_recent::<WireRelease, _>(
                "releases",
                "/releases",
                "",
                crate::snapshot::JSON_ACCEPT,
                |release| timestamp(release.created_at.as_deref()),
            )
            .await?;
        let mut releases: Vec<Release> = wire
            .into_iter()
            .filter_map(|release| {
                let tag_name = release.tag_name?;
                Some(Release {
                    id: release.id,
                    tag_name,
                    created_at: timestamp(release.created_at.as_deref()),
                    draft: release.draft.unwrap_or(false),
                    prerelease: release.prerelease.unwrap_or(false),
                })
            })
            .collect();
        releases.sort_by(|left, right| {
            newest_first(left.created_at, right.created_at).then_with(|| right.id.cmp(&left.id))
        });
        Ok(releases)
    }

    async fn last_commit(&mut self) -> Result<Option<Timestamp>, ForgeError> {
        // One commit, newest first: the whole question is its date. `per_page=1` is what keeps
        // this from paging through a repository's history for a number the first page answers.
        let url = self.repo_url("/commits?per_page=1");
        match self.fetch_one::<WireCommitEntry>("commits", url).await {
            Ok(wire) => Ok(timestamp(
                wire.commit
                    .as_ref()
                    .and_then(|commit| commit.committer.as_ref())
                    .and_then(|who| who.date.as_deref())
                    .or_else(|| {
                        wire.commit
                            .as_ref()
                            .and_then(|commit| commit.author.as_ref())
                            .and_then(|who| who.date.as_deref())
                    }),
            )),
            // An empty repository answers 409 or 404 with no commits; that is not an error
            // about the request, it is the answer "nothing has ever been committed here".
            Err(ForgeError::NotFound { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }

    async fn tags(&mut self) -> Result<Vec<Tag>, ForgeError> {
        let wire: Vec<WireTag> = self
            .fetch_all::<WireTag>("tags", "/tags", "", crate::snapshot::JSON_ACCEPT)
            .await?;
        let mut tags: Vec<Tag> = wire
            .into_iter()
            .filter_map(|tag| {
                let name = tag.name?;
                let commit_sha = tag.commit?.sha?;
                Some(Tag { name, commit_sha })
            })
            .collect();
        tags.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(tags)
    }

    async fn stargazers(&mut self) -> Result<Vec<Stargazer>, ForgeError> {
        // Stars are oldest first with no sort parameter, so the newest are on the last
        // page and the walk goes backwards from there (see `fetch_stars`).
        let wire: Vec<WireStar> = self.fetch_stars().await?;
        let mut stars: Vec<Stargazer> = wire
            .into_iter()
            .filter_map(|star| {
                let login = star
                    .login
                    .or_else(|| star.user.and_then(|user| user.login))?;
                Some(Stargazer {
                    login,
                    starred_at: timestamp(star.starred_at.as_deref()),
                })
            })
            .collect();
        if stars.len() > STARGAZER_CAP {
            stars.drain(..stars.len() - STARGAZER_CAP);
        }
        stars.sort_by(|left, right| {
            newest_first(left.starred_at, right.starred_at)
                .then_with(|| left.login.cmp(&right.login))
        });
        Ok(stars)
    }

    async fn contributors(&mut self) -> Result<Vec<Contributor>, ForgeError> {
        let wire: Vec<WireContributor> = self
            .fetch_all::<WireContributor>(
                "contributors",
                "/contributors",
                "&anon=0",
                crate::snapshot::JSON_ACCEPT,
            )
            .await?;
        let mut contributors: Vec<Contributor> = wire
            .into_iter()
            .filter_map(|person| {
                let login = person.login?;
                if login.is_empty() {
                    return None;
                }
                Some(Contributor {
                    login,
                    contributions: person.contributions.unwrap_or(0),
                    kind: person.kind,
                })
            })
            .collect();
        contributors.sort_by(|left, right| {
            right
                .contributions
                .cmp(&left.contributions)
                .then_with(|| left.login.cmp(&right.login))
        });
        Ok(contributors)
    }

    async fn user(&mut self, login: &str) -> Result<GitHubUser, ForgeError> {
        let label = format!("user {login}");
        let url = self.url(&format!("/users/{login}"));
        let wire: WireUser = self.fetch_one(&label, url).await?;
        Ok(wire.into_user())
    }

    async fn owner_repos(
        &mut self,
        since: Timestamp,
        until: Timestamp,
    ) -> Result<Vec<RepoSummary>, ForgeError> {
        // One page: H19 is context, and a prolific owner would otherwise cost hundreds of
        // requests to say something the first hundred already say. `affiliation=owner`
        // excludes repositories the account only collaborates on, and `sort=created`
        // descending makes the page hold the newest, which is the end H19 looks at.
        let label = "owner repos";
        let owner = self.repo.owner.clone();
        let url = format!(
            "{API_ROOT}/users/{owner}/repos?per_page={PER_PAGE}&page=1&sort=created&direction=desc&affiliation=owner"
        );
        let wire: Vec<WireRepoSummary> = self
            .fetch_page::<WireRepoSummary>(label, url, crate::snapshot::JSON_ACCEPT, true)
            .await?;
        let mut repos: Vec<RepoSummary> = wire
            .into_iter()
            .filter_map(|entry| {
                let name = entry.name?;
                let created_at = timestamp(entry.created_at.as_deref());
                // The window is applied here rather than by the API: `created:` would make
                // the query text depend on `as_of`, and a replay compares URLs.
                if let Some(created) = created_at
                    && (created < since || created > until)
                {
                    return None;
                }
                Some(RepoSummary {
                    name,
                    created_at,
                    fork: entry.fork.unwrap_or(false),
                    template: entry.is_template.unwrap_or(false),
                })
            })
            .collect();
        repos.sort_by(|left, right| {
            newest_first(left.created_at, right.created_at).then_with(|| left.name.cmp(&right.name))
        });
        Ok(repos)
    }

    async fn tree(&mut self, repo: &GitHubRepoRef) -> Result<Vec<TreeEntry>, ForgeError> {
        // The recursive tree of the default branch: `HEAD` resolves to it, so no second
        // request is needed to learn the branch name. A truncated tree is recorded, because
        // H17's permille over part of a template would read as divergence.
        let label = format!("tree {}", repo.slug());
        let url = format!(
            "{API_ROOT}/repos/{}/git/trees/HEAD?recursive=1",
            repo.slug()
        );
        let wire: WireTree = self.fetch_one(&label, url).await?;
        if wire.truncated.unwrap_or(false) {
            self.note_capped(&label);
        }
        let mut entries: Vec<TreeEntry> = wire
            .tree
            .into_iter()
            .filter(|entry| entry.kind.as_deref() == Some("blob"))
            .filter_map(|entry| {
                Some(TreeEntry {
                    path: entry.path?,
                    sha: entry.sha?,
                })
            })
            .collect();
        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(entries)
    }

    async fn compare(&mut self, base: &str, head: &str) -> Result<Compare, ForgeError> {
        let label = format!("compare {base}...{head}");
        let url = self.repo_url(&format!("/compare/{base}...{head}"));
        let wire: WireCompare = self.fetch_one(&label, url).await?;
        Ok(Compare {
            ahead_by: wire.ahead_by.unwrap_or(0),
            behind_by: wire.behind_by.unwrap_or(0),
            changed_files: wire.changed_files.unwrap_or(0),
            status: wire.status.unwrap_or_default(),
        })
    }

    fn requests_used(&self) -> u32 {
        self.http.used()
    }

    fn capped(&self) -> Vec<String> {
        self.capped.clone()
    }

    fn skipped(&self) -> Vec<(String, String)> {
        self.skipped.clone()
    }

    fn notes(&self) -> Vec<String> {
        self.notes.clone()
    }
}
