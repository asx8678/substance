//! History signals H01–H10 (M1-T05).
//!
//! Every signal here answers a question about how a repository was built, using only what
//! git and the forge already recorded. They are deliberately not scores: each one writes
//! named integer metrics, and a threshold in `config/thresholds.toml` — or the absence of
//! one — decides whether those metrics pass, fail, or merely report (ADR-0013).
//!
//! Three shapes keep the checks uniform and inspectable:
//!
//! - [`HistoryInput`] is everything the checks read: the commits, the HEAD tree's authored
//!   paths, the blame, the hunks of the most recent commits, the forge objects when a budget
//!   allowed fetching them, and `as_of`. Gathering it is IO and happens once, in
//!   [`HistoryInput::gather`]; judging it is a pure function.
//! - [`Signal`] is `fn(&HistoryInput, &Thresholds) -> Judgment`, one per check. A
//!   [`Judgment`] holds the verdict, the metrics, the evidence and any notes.
//! - A check that needs something the input does not carry answers `not_measured` with a
//!   reason, never `pass` (principle 8).
//!
//! All arithmetic is integer. Shares are permille, medians are nearest-rank, and a rate is
//! lines per `max(1, minutes)`. There is no float and no wall clock anywhere: every window
//! starts at [`window_start`] from `as_of`.

pub mod common;
pub mod h01;
pub mod h02;
pub mod h03;
pub mod h04;
pub mod h05;
pub mod h06;
pub mod h07;
pub mod h08;
pub mod h09;
pub mod h10;
pub mod h11;
pub mod h12;
pub mod h13;
pub mod h14;
pub mod h15;
pub mod h16;
pub mod h17;
pub mod h18;
pub mod h19;
pub mod h20;
pub mod h21;
pub mod h22;
pub mod shape;

use std::collections::BTreeMap;

use substance_core::Timestamp;
use substance_core::check::CheckId;
use substance_core::clock::window_start;
use substance_core::model::{CheckVerdict, Target};
use substance_core::thresholds::Thresholds;
use substance_core::tools::{ToolSpec, ToolsFile};
use substance_runner::tool::ToolRunner;

use crate::forge::{
    Compare, Contributor, Forge, ForgeError, GitHubUser, Issue, IssueComment, PullRequest, Release,
    RepoInfo, RepoSummary, Review, Stargazer, Tag, TreeEntry, WorkflowRun,
};
use crate::git::{
    AuthoredFile, AuthoredFilter, BlameReport, Commit, FileHunks, GitError, GitRepo, LinguistAttrs,
    blame_all,
};

pub use h01::chronology;
pub use h02::active_weeks;
pub use h03::big_bang;
pub use h04::commit_size;
pub use h05::machine_speed;
pub use h06::upload_dumps;
pub use h07::fix_chains;
pub use h08::refactoring;
pub use h09::short_lived;
pub use h10::message_vs_diff;
pub use h11::maintainers;
pub use h12::review;
pub use h13::maintenance;
pub use h14::real_usage;
pub use h15::silent_tracker;
pub use h16::claims_vs_history;
pub use h17::template_divergence;
pub use h18::star_spikes;
pub use h19::repo_farms;
pub use h20::velocity;
pub use h21::{ScorecardRun, scorecard};
pub use h22::{ArchetypeMetrics, Classification, archetype, classify};
pub use shape::{Judgment, Signal, top_commits};

/// The longest window the history checks look back over, in days.
///
/// H02 asks for 90 and 365; H09, H12, H13 and H14 for 365. Older commits are still counted
/// where a formula counts the whole history (H04, H06, H10), but they are outside every
/// "recently" question.
pub const LONGEST_WINDOW_DAYS: u32 = 365;

/// The shorter window H02 reports on, in days.
pub const SHORT_WINDOW_DAYS: u32 = 90;

/// How many commits a judgment keeps as evidence: the plan's "top 20".
pub const EVIDENCE_COMMITS: usize = 20;

/// How many of the most recent commits H08 and H09 read hunks for.
///
/// H09's formula says "the last 200 commits"; H08 reads the same span so the two
/// refactoring numbers describe one interval. Reading hunks costs one git process per
/// commit, so the bound is a count, not a clock (ADR-0009).
pub const HUNK_COMMITS: usize = 200;

/// How many merged pull requests the forge fetch reads reviews for (H12).
///
/// A review costs one request per pull request, so the sample is bounded by a count and the
/// truncation is recorded: the note says how many merges the permille describes.
pub const REVIEW_PULLS: usize = 50;

/// How many issue authors' accounts H14 looks up.
///
/// The plan fixes this cap: 50 lookups is enough to see whether a project's issue authors
/// are established accounts, and small enough to fit in one run's budget.
pub const USER_LOOKUPS_H14: usize = 50;

/// How many stargazers' accounts H18 looks up.
///
/// The plan's cap. A star spike is a burst, so the accounts worth checking are the ones that
/// starred inside it; the client returns the 5,000 most recent stars and this takes the
/// alphabetically first 200 of them, which is stable for a given snapshot.
pub const USER_LOOKUPS_H18: usize = 200;

/// How far either side of a repository's creation H19 looks for the owner's other repos.
pub const OWNER_WINDOW_DAYS: u32 = 30;

/// The `author_association` values GitHub gives people who do not work on the repository.
///
/// `NONE` is an outsider who has never contributed, `FIRST_TIME_CONTRIBUTOR` and
/// `FIRST_TIMER` are outsiders making their first contribution, and `CONTRIBUTOR` has
/// contributed before but is still not a maintainer. The three that are not outsiders are
/// `OWNER`, `MEMBER` and `COLLABORATOR`, which is the list H15 and H22 use in reverse.
pub const MAINTAINER_ASSOCIATIONS: &[&str] = &["OWNER", "MEMBER", "COLLABORATOR"];

/// The logins that starred on a burst day, sorted, for H18's lookups.
///
/// A burst is the plan's 50 stars in one UTC day. Sorting here and again at the call site is
/// deliberate: the set is small, and a sorted set is what makes the sample stable.
fn burst_day_logins(stargazers: &[Stargazer]) -> Vec<String> {
    let mut per_day: std::collections::BTreeMap<i32, Vec<&Stargazer>> =
        std::collections::BTreeMap::new();
    for star in stargazers {
        if let Some(at) = star.starred_at {
            per_day
                .entry(at.as_offset_date_time().date().to_julian_day())
                .or_default()
                .push(star);
        }
    }
    let mut logins: Vec<String> = Vec::new();
    for stars in per_day.values() {
        if stars.len() >= 50 {
            logins.extend(stars.iter().map(|star| star.login.clone()));
        }
    }
    logins
}

/// Whether an `author_association` names someone outside the project.
#[must_use]
pub fn is_outsider(association: &str) -> bool {
    !MAINTAINER_ASSOCIATIONS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(association))
}

/// How many (commit, file) blames H08 spends dating deleted lines.
///
/// The median age of deleted lines needs the history of each line that a commit removed,
/// which is one `git blame` per changed file. The budget bounds that work; when it is
/// reached the sample is smaller and [`Hunks::ages_truncated`] says so.
pub const MAX_AGE_BLAMES: u32 = 100;

/// The tool table this build ships, and the runner a tool call goes through (M1-T01).
///
/// H21 is the one check that runs an external program. It runs during
/// [`HistoryInput::gather`], through a [`ToolRunner`] rather than
/// `run_tool_capture_any`, for two reasons the runner exists:
///
/// - The tool's stdout and stderr are saved as blobs with an entry under
///   `tools/<name>/<sha256(args)>.json`, so a verdict can be traced to the bytes printed.
/// - Under `--snapshot-from` a tool with `network = true` is replayed from the earlier run
///   instead of executed, which is what makes an offline run reproducible (§3 issue 29).
#[derive(Debug, Clone)]
pub struct Tools<'a> {
    /// The parsed `tools.toml`, embedded in the binary rather than read from the evaluated
    /// repository: a repo under test must not choose which binary substance runs.
    pub table: ToolsFile,
    /// The run's tool entry point.
    pub runner: ToolRunner<'a>,
}

impl<'a> Tools<'a> {
    /// A table and the runner that saves its output.
    #[must_use]
    pub fn new(table: ToolsFile, runner: ToolRunner<'a>) -> Self {
        Self { table, runner }
    }

    /// One tool's entry, or `None` when the table does not list it.
    #[must_use]
    pub fn spec(&self, name: &str) -> Option<&ToolSpec> {
        self.table.spec(name)
    }
}

/// Everything the history signals read, gathered once.
///
/// Forge data is [`Option`]al as a whole: a run without a token, a rate-limited client or a
/// non-GitHub origin leaves [`ForgeData`] out and records why in
/// [`HistoryInput::forge_missing`]. The checks that need it report that reason.
#[derive(Debug)]
pub struct HistoryInput {
    /// Every commit reachable from HEAD, newest first by committer date (`--date-order`).
    pub commits: Vec<Commit>,
    /// The authored paths at HEAD, sorted. Lockfiles, generated code and vendored trees are
    /// excluded: every history metric counts authored lines only.
    pub authored: Vec<String>,
    /// The blame of those paths at HEAD, bounded by the caller's budget.
    pub blame: BlameReport,
    /// The hunks of the most recent human commits, newest first.
    pub hunks: Hunks,
    /// The forge objects, when the run could fetch them.
    pub forge: Option<ForgeData>,
    /// Why the forge data is absent, when it is.
    pub forge_missing: Option<String>,
    /// The instant the report describes; every window ends here.
    pub as_of: Timestamp,
    /// The generic commit subjects H10 matches, lowercased and sorted.
    pub generic_subjects: Vec<String>,
    /// The README's text at HEAD, when the repository has one (H16).
    pub readme: Option<String>,
    /// The CHANGELOG's text at HEAD, when the repository has one (H16).
    pub changelog: Option<String>,
    /// The workflow files tracked under `.github/workflows` (H16).
    pub workflows: Vec<String>,
    /// Every blob at HEAD with its SHA, for H17's template comparison.
    pub head_blobs: Vec<(String, String)>,
    /// The test-path rules from `rules/history/test-paths.txt` (H22).
    pub test_paths: common::TestPathRules,
    /// What OpenSSF Scorecard said, or why it said nothing (H21).
    pub scorecard: h21::ScorecardRun,
}

/// The file names a README may have, in the order H16 tries them.
const README_NAMES: &[&str] = &[
    "README.md",
    "README.markdown",
    "README.rst",
    "README.txt",
    "README",
    "readme.md",
];

/// The file names a CHANGELOG may have, in the order H16 tries them.
const CHANGELOG_NAMES: &[&str] = &[
    "CHANGELOG.md",
    "CHANGELOG.markdown",
    "CHANGELOG.rst",
    "CHANGELOG.txt",
    "CHANGELOG",
    "CHANGES.md",
    "HISTORY.md",
    "changelog.md",
];

/// Every blob at HEAD with its SHA, sorted by path.
///
/// `git ls-tree` already prints the SHA, but the engine's tree listing keeps only names, so
/// this asks git once for the pairs. A failure yields an empty list: H17 then reports that it
/// had nothing to compare, which is the honest answer.
async fn head_blobs(repo: &GitRepo) -> Vec<(String, String)> {
    repo.blobs("HEAD").await.unwrap_or_default()
}

/// Reads the first of `names` that exists at HEAD, or `None`.
///
/// A file that cannot be read is treated as absent: H16 says "no README" for a repository
/// whose README git refuses to show, and its note does not claim to tell the two apart.
async fn read_optional(repo: &GitRepo, names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(text) = repo.head(name, "HEAD").await {
            return Some(text);
        }
    }
    None
}

/// The hunks of the recent commits, plus what dating them cost.
#[derive(Debug, Clone, Default)]
pub struct Hunks {
    /// The commits, newest first, with each file's hunks.
    pub commits: Vec<CommitHunks>,
    /// Ages in days of the lines those commits deleted, in commit order; each entry is one
    /// deleted line whose origin could be blamed.
    pub deleted_line_ages: Vec<u32>,
    /// Whether more commits existed in the window than [`HUNK_COMMITS`].
    pub truncated: bool,
    /// Whether dating deleted lines ran out of its blame budget.
    pub ages_truncated: bool,
}

/// One commit's hunks, with the fields a formula needs beside them.
#[derive(Debug, Clone)]
pub struct CommitHunks {
    /// The commit SHA.
    pub sha: String,
    /// The author's instant.
    pub at: Timestamp,
    /// The author's name, as git reports it after mailmap.
    pub author: String,
    /// The first parent, when there is one; the revision a blame starts from.
    pub parent: Option<String>,
    /// The file diff.
    pub files: Vec<FileHunks>,
}

impl HistoryInput {
    /// Gathers everything the signals read from one repository.
    ///
    /// `forge` is the client when the run has one; `forge_missing` says why it does not and
    /// is used verbatim in a `not_measured` reason. `blame_max_files` bounds the blame, and
    /// `generic_subjects` are the lines of `rules/history/generic-messages.txt`. `tools` is
    /// the embedded tool table and the run's `ToolRunner`, through which H21 runs Scorecard:
    /// the tool run is IO like everything else here, so the judge stays pure.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when the git engine cannot read the repository at all (no
    /// commits, no tree, no hunks). A forge failure is not an error: it is recorded in
    /// [`HistoryInput::forge_missing`], because a missing input must report, not abort.
    #[allow(clippy::too_many_arguments)]
    pub async fn gather(
        repo: &GitRepo,
        as_of: Timestamp,
        blame_max_files: u32,
        generic_subjects: Vec<String>,
        test_paths_text: String,
        forge: Option<&mut dyn Forge>,
        forge_missing: Option<String>,
        tools: &Tools<'_>,
    ) -> Result<Self, GitError> {
        let commits = repo.commits("HEAD").await?;
        let (authored, _excluded) = repo.authored("HEAD").await?;
        let blame = blame_all(repo, "HEAD", &authored, blame_max_files).await?;
        let hunks = gather_hunks(repo, &commits, as_of).await?;
        let head_blobs = head_blobs(repo).await;
        // The workflow files H16 reads: names only, from the tree, so a badge can be checked
        // against a file that exists at HEAD.
        let workflows: Vec<String> = authored
            .iter()
            .filter(|path| path.starts_with(".github/workflows/"))
            .cloned()
            .collect();
        let (forge, forge_missing) = match forge {
            Some(client) => match ForgeData::fetch(client).await {
                Ok(data) => (Some(data), None),
                Err(reason) => (None, Some(reason)),
            },
            None => (
                None,
                Some(forge_missing.unwrap_or_else(|| "no forge client for this origin".to_owned())),
            ),
        };
        let generic_subjects = parse_subject_list(&generic_subjects.join("\n"));
        // H21's tool run is IO, so it happens here with everything else: the judge reads the
        // result and never spawns anything. The forge data gathered above names the
        // repository; without it the tool has nothing to be asked about.
        let scorecard = match forge.as_ref().and_then(|data| data.slug.clone()) {
            Some(slug) => {
                match tools
                    .spec(h21::SCORECARD)
                    .map(|spec| (spec, h21::SCORECARD))
                {
                    Some((spec, name)) => {
                        h21::gather(&tools.runner, spec, name, repo.root(), &slug).await
                    }
                    None => h21::ScorecardRun::unavailable(
                        "tools.toml lists no scorecard entry, so this build cannot run it",
                    ),
                }
            }
            None => h21::ScorecardRun::no_slug(),
        };
        Ok(Self {
            commits,
            authored,
            blame,
            hunks,
            forge,
            forge_missing,
            as_of,
            generic_subjects,
            readme: read_optional(repo, README_NAMES).await,
            changelog: read_optional(repo, CHANGELOG_NAMES).await,
            workflows,
            head_blobs,
            test_paths: common::TestPathRules::parse(&test_paths_text),
            scorecard,
        })
    }

    /// The start of the `days`-day window that ends at `as_of`.
    #[must_use]
    pub fn window_start(&self, days: u32) -> Timestamp {
        window_start(self.as_of, days)
    }

    /// The commits a formula counts: non-bot and non-merge.
    ///
    /// A merge commit's first-parent diff repeats work its branch already recorded, so
    /// merges are excluded from commit-level statistics. A commit by a bot (`dependabot`,
    /// `renovate`, a `[bot]` login) is excluded from "who worked on this" questions.
    #[must_use]
    pub fn human_commits(&self) -> Vec<&Commit> {
        self.commits
            .iter()
            .filter(|commit| !commit.is_merge() && !crate::git::is_bot_commit(&commit.author))
            .collect()
    }

    /// Whether `path` is one of HEAD's authored paths.
    ///
    /// The set is sorted, so the test is a binary search.
    #[must_use]
    pub fn is_authored(&self, path: &str) -> bool {
        self.authored
            .binary_search_by(|candidate| candidate.as_str().cmp(path))
            .is_ok()
    }

    /// The authored added lines a commit contributed.
    ///
    /// `Commit::added` sums every file's numstat, which includes lockfiles and generated
    /// files. The history signals count only paths in [`Self::authored`], so a commit that
    /// only bumped `Cargo.lock` contributes zero.
    #[must_use]
    pub fn authored_added(&self, commit: &Commit) -> u64 {
        commit
            .files
            .iter()
            .filter(|file| self.is_authored(&file.path))
            .filter_map(|file| file.added)
            .sum()
    }

    /// The forge data, or the reason it is missing.
    ///
    /// # Errors
    ///
    /// Returns the recorded reason as a `String` when there is no forge data.
    pub fn forge_or_reason(&self) -> Result<&ForgeData, String> {
        match (&self.forge, &self.forge_missing) {
            (Some(data), _) => Ok(data),
            (None, Some(reason)) => Err(reason.clone()),
            (None, None) => Err("the forge was not read".to_owned()),
        }
    }

    /// The instant of the earliest non-bot commit, if there is one.
    #[must_use]
    pub fn first_commit_at(&self) -> Option<Timestamp> {
        self.human_commits()
            .iter()
            .map(|commit| commit.author.at)
            .min()
    }

    /// The instant the project's "day one" begins.
    ///
    /// Normally the first non-bot commit. Under the private-then-public rule, a repository
    /// whose server-side creation date is more than 30 days after its first commit was
    /// developed privately and published later, so its history is not evidence of a
    /// big-bang dump: day one becomes the creation date, and the note says so. H03 and H04
    /// rely on this, which is why the result carries the reason.
    #[must_use]
    pub fn day_one(&self) -> DayOne {
        let first = self.first_commit_at();
        let created = self
            .forge
            .as_ref()
            .and_then(|data| data.repo.as_ref())
            .and_then(|repo| repo.created_at);
        match (first, created) {
            (Some(first), Some(created))
                if created.as_offset_date_time() - first.as_offset_date_time()
                    > time::Duration::days(30) =>
            {
                DayOne {
                    at: created,
                    private_then_public: true,
                }
            }
            (Some(first), _) => DayOne {
                at: first,
                private_then_public: false,
            },
            (None, Some(created)) => DayOne {
                at: created,
                private_then_public: true,
            },
            (None, None) => DayOne {
                at: self.as_of,
                private_then_public: false,
            },
        }
    }
}

/// When the project's "day one" was, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DayOne {
    /// The instant day one starts.
    pub at: Timestamp,
    /// Whether the first-public-push rule moved it.
    pub private_then_public: bool,
}

impl DayOne {
    /// The UTC calendar day day one falls on.
    #[must_use]
    pub fn date(&self) -> time::Date {
        self.at.as_offset_date_time().date()
    }
}

/// The forge objects the history checks read, fetched once.
#[derive(Debug, Clone, Default)]
pub struct ForgeData {
    /// The repository object: creation date, fork parent, template.
    pub repo: Option<RepoInfo>,
    /// Every pull request the client could page through, newest first.
    pub pulls: Vec<PullRequest>,
    /// Issues, excluding pull requests.
    pub issues: Vec<Issue>,
    /// Issue comments.
    pub issue_comments: Vec<IssueComment>,
    /// Workflow runs, newest first.
    pub workflow_runs: Vec<WorkflowRun>,
    /// Releases.
    pub releases: Vec<Release>,
    /// Tags, for H16's "CHANGELOG version with no tag or release".
    pub tags: Vec<Tag>,
    /// Contributors, for the forge's own view of who worked here (H11).
    pub contributors: Vec<Contributor>,
    /// Stargazers with their star instants, for H18's spike detection.
    pub stargazers: Vec<Stargazer>,
    /// The reviews of each merged pull request, keyed by number, for H12.
    pub reviews: Vec<(u32, Vec<Review>)>,
    /// The account ages H14 and H18 asked about, keyed by login, sorted.
    pub users: Vec<(String, GitHubUser)>,
    /// The template's tree, when the repository was made from a template (H17).
    pub template_tree: Option<Vec<TreeEntry>>,
    /// The owner's other repositories around this one's creation (H19).
    pub owner_repos: Vec<RepoSummary>,
    /// The comparison against the fork parent, when the repository is a fork.
    pub upstream_compare: Option<Compare>,
    /// The account that owns the repository, from the client's own reference.
    pub owner: Option<String>,
    /// The repository's `owner/repo`, which is how a forge API names it (H21).
    pub slug: Option<String>,
    /// What the forge client could not answer, sorted, with the reason.
    pub unread: Vec<(String, String)>,
}

impl ForgeData {
    /// Fetches every object the history checks read, in one fixed order.
    ///
    /// The order is the point: requests are spent from one budget, so a run that stops at
    /// the cap stops in the same place twice (ADR-0009). Each object is fetched once, and a
    /// check that needs two of them spends no second request.
    async fn fetch(client: &mut dyn Forge) -> Result<Self, String> {
        let mut data = Self {
            repo: Some(client.repo().await.map_err(describe)?),
            owner: Some(client.repo_ref().owner.clone()),
            slug: Some(client.repo_slug()),
            pulls: client.pulls().await.map_err(describe)?,
            workflow_runs: client.workflow_runs().await.map_err(describe)?,
            issues: client.issues().await.map_err(describe)?,
            issue_comments: client.issue_comments().await.map_err(describe)?,
            releases: client.releases().await.map_err(describe)?,
            ..Self::default()
        };

        // The rest of the collections are optional in a stronger sense: a repository with no
        // tags is an answer, but a call that *fails* must not lose the objects already
        // fetched. Each failure is recorded in `unread`, which is what a check reports.
        match client.tags().await {
            Ok(tags) => data.tags = tags,
            Err(error) => data.unread.push(("tags".to_owned(), describe(error))),
        }
        match client.contributors().await {
            Ok(contributors) => data.contributors = contributors,
            Err(error) => data
                .unread
                .push(("contributors".to_owned(), describe(error))),
        }
        match client.stargazers().await {
            Ok(stargazers) => data.stargazers = stargazers,
            Err(error) => data.unread.push(("stargazers".to_owned(), describe(error))),
        }

        // Reviews, one request per merged pull request, newest first and capped: H12 asks a
        // question about the recent shape of a project, and a repository with 4,000 merged
        // pull requests must not spend its whole budget on them.
        // Numbers, not references: the loop below writes to `data`, and holding a borrow of
        // its own field while doing so is what the borrow checker (correctly) refuses.
        let merged_numbers: Vec<u32> = data
            .merged_pulls()
            .iter()
            .rev()
            .map(|pull| pull.number)
            .collect();
        let reviewed: Vec<u32> = merged_numbers.iter().take(REVIEW_PULLS).copied().collect();
        if merged_numbers.len() > reviewed.len() {
            data.unread.push((
                "reviews".to_owned(),
                format!(
                    "only the {REVIEW_PULLS} most recently merged pull requests were read, of {}",
                    merged_numbers.len()
                ),
            ));
        }
        for number in reviewed {
            match client.reviews(number).await {
                Ok(reviews) => data.reviews.push((number, reviews)),
                Err(error) => data
                    .unread
                    .push((format!("reviews {number}"), describe(error))),
            }
        }
        data.reviews.sort_by_key(|(number, _)| *number);

        // Account ages, deduplicated and capped: H14 needs the outsiders' and H18 the
        // stargazers'. Sorting before the cap makes the sample the same on every run
        // (ADR-0009), and a login is never looked up twice.
        let mut logins: Vec<String> = Vec::new();
        for issue in &data.issues {
            if is_outsider(&issue.author_association) && !issue.author.is_bot() {
                logins.push(issue.author.login.clone());
            }
        }
        logins.sort();
        logins.dedup();
        logins.truncate(USER_LOOKUPS_H14);
        // H18's accounts: the stargazers of burst days, then the rest, so the lookups go
        // where the question is and a repository with thousands of quiet stars still spends
        // its budget on the day that matters.
        let mut burst_logins: Vec<String> = burst_day_logins(&data.stargazers);
        burst_logins.sort();
        burst_logins.dedup();
        burst_logins.truncate(USER_LOOKUPS_H18);
        let mut watchers: Vec<String> = data
            .stargazers
            .iter()
            .map(|star| star.login.clone())
            .collect();
        watchers.sort();
        watchers.dedup();
        let remaining = USER_LOOKUPS_H18.saturating_sub(burst_logins.len());
        watchers.retain(|login| !burst_logins.contains(login));
        watchers.truncate(remaining);
        let watchers: Vec<String> = burst_logins.into_iter().chain(watchers).collect();
        logins.extend(watchers);
        logins.sort();
        logins.dedup();
        for login in logins {
            match client.user(&login).await {
                Ok(user) => data.users.push((login, user)),
                Err(error) => data.unread.push((format!("user {login}"), describe(error))),
            }
        }

        // The template's tree, for H17's blob-SHA comparison.
        if let Some(template) = data.repo.as_ref().and_then(|repo| repo.template.clone()) {
            match client.tree(&template).await {
                Ok(tree) => data.template_tree = Some(tree),
                Err(error) => data
                    .unread
                    .push(("template tree".to_owned(), describe(error))),
            }
        }
        // The owner's other repositories around this one's creation, for H19.
        if let Some(created) = data.repo.as_ref().and_then(|repo| repo.created_at) {
            // Both ends of the window come from `created`, so the query is a function of the
            // forge's own dates and never of the wall clock or of `as_of`.
            let since = substance_core::clock::window_start(created, OWNER_WINDOW_DAYS);
            let until = Timestamp::new(
                created.as_offset_date_time() + time::Duration::days(i64::from(OWNER_WINDOW_DAYS)),
            );
            match client.owner_repos(since, until).await {
                Ok(repos) => data.owner_repos = repos,
                Err(error) => data
                    .unread
                    .push(("owner repos".to_owned(), describe(error))),
            }
        }
        // A fork's upstream comparison is the only way to tell which commits came from
        // upstream, so it is fetched last, after everything a check can use without it.
        if let Some(parent) = data.repo.as_ref().and_then(|repo| repo.parent.clone()) {
            // `compare(base, head)` reads "what does head have that base does not": the
            // upstream parent is the base and this repository is the head.
            let head = client.repo_ref().clone();
            data.upstream_compare = client
                .compare(&parent.slug(), &head.slug())
                .await
                .map_err(describe)
                .ok();
        }
        data.unread.sort_by(|left, right| left.0.cmp(&right.0));
        Ok(data)
    }

    /// The reviews of one pull request, when they were fetched.
    #[must_use]
    pub fn reviews_of(&self, number: u32) -> Option<&[Review]> {
        self.reviews
            .iter()
            .find(|(candidate, _)| *candidate == number)
            .map(|(_, reviews)| reviews.as_slice())
    }

    /// The account age of `login` at `at`, in days, when the account was looked up.
    #[must_use]
    pub fn account_age_days(&self, login: &str, at: substance_core::Timestamp) -> Option<i64> {
        let user = self
            .users
            .iter()
            .find(|(candidate, _)| candidate == login)
            .map(|(_, user)| user)?;
        let created = user.created_at?;
        Some(days_between(created, at))
    }

    /// Whether the forge gave a reason for not answering `what`.
    #[must_use]
    pub fn unread_reason(&self, what: &str) -> Option<&str> {
        self.unread
            .iter()
            .find(|(candidate, _)| candidate == what)
            .map(|(_, reason)| reason.as_str())
    }

    /// The merged pull requests, sorted by merge instant.
    #[must_use]
    pub fn merged_pulls(&self) -> Vec<&PullRequest> {
        let mut merged: Vec<&PullRequest> = self
            .pulls
            .iter()
            .filter(|pull| pull.merged_at.is_some())
            .collect();
        merged.sort_by(|left, right| {
            left.merged_at
                .cmp(&right.merged_at)
                .then_with(|| left.number.cmp(&right.number))
        });
        merged
    }
}

/// Turns a forge failure into the sentence a `not_measured` verdict carries.
///
/// ADR-0005 fixes two of these: an unsupported origin is `"forge unsupported"` and a spent
/// rate limit is `"forge rate limited"`. The rest pass their own message through, so a
/// reader learns which call failed without a second lookup.
#[must_use]
pub fn describe(error: ForgeError) -> String {
    match error {
        ForgeError::Unavailable => "forge unsupported".to_owned(),
        ForgeError::RateLimited => "forge rate limited".to_owned(),
        other => other.to_string(),
    }
}

/// One signal's result: the verdict a report carries, plus what M1-T12 needs to turn it
/// into findings.
///
/// [`CheckVerdict`] holds metric numbers and finding IDs but no evidence of its own — evidence
/// belongs to a [`Finding`](substance_core::model::Finding). Keeping the judgment beside the
/// verdict means a check's evidence and notes are not computed and then discarded: the
/// handler that builds findings reads them here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalReport {
    /// The verdict, for the report's check list.
    pub verdict: CheckVerdict,
    /// The commits (and later, other artifacts) the verdict rests on, at most
    /// [`EVIDENCE_COMMITS`] of them.
    pub evidence: Vec<substance_core::model::Evidence>,
    /// Sentences a reader needs to interpret the numbers.
    pub notes: Vec<String>,
}

/// Turns a judgment into a [`SignalReport`] for `check`.
///
/// This is the one place that knows a history judgment is about `Target::Repo`, and the one
/// place that copies a judgment's metrics into a verdict — so the two can never disagree.
#[must_use]
pub fn report_of(check: CheckId, judgment: Judgment) -> SignalReport {
    SignalReport {
        verdict: CheckVerdict {
            check_id: check,
            target: Target::Repo,
            verdict: judgment.verdict,
            finding_ids: Vec::new(),
            metrics: judgment.metrics,
            parts: BTreeMap::new(),
        },
        evidence: judgment.evidence,
        notes: judgment.notes,
    }
}

/// Every history signal, in ID order.
///
/// H17 runs its template comparison here; its scaffold-hash half arrives in M1-T08 and is
/// named in the check's note until then.
#[must_use]
pub fn signals() -> Vec<Signal> {
    vec![
        Signal::new(CheckId::H01, chronology),
        Signal::new(CheckId::H02, active_weeks),
        Signal::new(CheckId::H03, big_bang),
        Signal::new(CheckId::H04, commit_size),
        Signal::new(CheckId::H05, machine_speed),
        Signal::new(CheckId::H06, upload_dumps),
        Signal::new(CheckId::H07, fix_chains),
        Signal::new(CheckId::H08, refactoring),
        Signal::new(CheckId::H09, short_lived),
        Signal::new(CheckId::H10, message_vs_diff),
        Signal::new(CheckId::H11, maintainers),
        Signal::new(CheckId::H12, review),
        Signal::new(CheckId::H13, maintenance),
        Signal::new(CheckId::H14, real_usage),
        Signal::new(CheckId::H15, silent_tracker),
        Signal::new(CheckId::H16, claims_vs_history),
        Signal::new(CheckId::H17, template_divergence),
        Signal::new(CheckId::H18, star_spikes),
        Signal::new(CheckId::H19, repo_farms),
        Signal::new(CheckId::H20, velocity),
        Signal::new(CheckId::H21, scorecard),
        Signal::new(CheckId::H22, archetype),
    ]
}

/// Runs every history signal over one input, in ID order.
///
/// The result is sorted by check ID, so a report lists H01 … H10 in one order twice.
#[must_use]
pub fn judge_all(input: &HistoryInput, thresholds: &Thresholds) -> Vec<SignalReport> {
    let mut reports: Vec<SignalReport> = signals()
        .into_iter()
        .map(|signal| report_of(signal.check, (signal.judge)(input, thresholds)))
        .collect();
    reports.sort_by_key(|report| report.verdict.check_id.as_str());
    reports
}

/// The verdicts alone, for a caller that only needs those.
#[must_use]
pub fn verdicts_of(reports: Vec<SignalReport>) -> Vec<CheckVerdict> {
    reports.into_iter().map(|report| report.verdict).collect()
}

/// Reads the generic-subject file into sorted, lowercased lines.
#[must_use]
pub fn parse_subject_list(text: &str) -> Vec<String> {
    let mut subjects: Vec<String> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_lowercase)
        .collect();
    subjects.sort();
    subjects.dedup();
    subjects
}

/// Whether `path` is authored, judged from its name alone.
///
/// The signals that only hold a path (H08's hunks, H09's deleted lines) cannot run the full
/// [`AuthoredFilter`], which needs the file's head and `.gitattributes`. The name-only test
/// rejects the lockfiles, vendored directories and generated suffixes it can recognise and
/// accepts everything else; a generated file with no telling name is counted, which is the
/// conservative direction for a "was this written by a person" question.
#[must_use]
pub fn path_is_authored(path: &str) -> bool {
    AuthoredFilter::classify(path, LinguistAttrs::default(), "") == AuthoredFile::Authored
}

/// Splits an instant into the epoch seconds of its UTC day, for day arithmetic.
#[must_use]
pub fn day_index(at: Timestamp) -> i64 {
    let date = at.as_offset_date_time().date();
    // `to_julian_day` is a pure calendar conversion; no clock is read.
    i64::from(date.to_julian_day())
}

/// How many whole days separate two instants, signed.
#[must_use]
pub fn days_between(earlier: Timestamp, later: Timestamp) -> i64 {
    day_index(later) - day_index(earlier)
}

/// The hunks of the recent human commits, newest first, and the ages of the lines they
/// deleted.
///
/// One `git show` per commit, bounded by [`HUNK_COMMITS`]; one bounded `git blame` per
/// changed file for H08's line ages. Both bounds are counts (ADR-0009).
async fn gather_hunks(
    repo: &GitRepo,
    commits: &[Commit],
    _as_of: Timestamp,
) -> Result<Hunks, GitError> {
    let recent: Vec<&Commit> = commits
        .iter()
        .filter(|commit| !commit.is_merge() && !crate::git::is_bot_commit(&commit.author))
        .collect();
    let truncated = recent.len() > HUNK_COMMITS;
    let mut hunks = Hunks {
        commits: Vec::new(),
        deleted_line_ages: Vec::new(),
        truncated,
        ages_truncated: false,
    };
    let mut blames_left = MAX_AGE_BLAMES;
    for commit in recent.iter().take(HUNK_COMMITS) {
        let files = repo.hunks(&commit.sha).await?;
        let parent = commit.parents.first().cloned();
        if let Some(parent_sha) = parent.as_deref() {
            for file in &files {
                if blames_left == 0 {
                    hunks.ages_truncated = true;
                    break;
                }
                if file.binary || file.removed().is_empty() || !path_is_authored(&file.path) {
                    continue;
                }
                blames_left -= 1;
                if let Ok(map) = repo.blame(parent_sha, &file.path).await {
                    for hunk in &file.hunks {
                        for line in &hunk.lines {
                            if line.kind != crate::git::HunkLineKind::Removed {
                                continue;
                            }
                            // A removed line was at `old_start + offset` in the parent.
                            if let Some(age) =
                                removed_line_age(&map, hunk.old_start, hunk, line, commit.author.at)
                            {
                                hunks.deleted_line_ages.push(age);
                            }
                        }
                    }
                }
            }
        }
        hunks.commits.push(CommitHunks {
            sha: commit.sha.clone(),
            at: commit.author.at,
            author: commit.author.name.clone(),
            parent,
            files,
        });
    }
    Ok(hunks)
}

/// The age in days of one removed line, from the blame of the parent commit.
///
/// The walking offset is the position of `line` among the hunk's removed lines so far, so
/// the first removed line sits at `old_start`. A line the blame does not cover (a shrunken
/// file, a rename) contributes nothing rather than a guess.
fn removed_line_age(
    map: &crate::git::BlameMap,
    old_start: u64,
    hunk: &crate::git::Hunk,
    target: &crate::git::HunkLine,
    deleted_at: Timestamp,
) -> Option<u32> {
    let mut offset = 0u64;
    let target_index = hunk
        .lines
        .iter()
        .position(|line| std::ptr::eq(line, target))?;
    for line in hunk.lines.iter().take(target_index) {
        if line.kind == crate::git::HunkLineKind::Removed {
            offset += 1;
        }
    }
    let number = u32::try_from(old_start + offset).ok()?;
    let blamed = map
        .lines
        .iter()
        .find(|candidate| candidate.line == number)?;
    // The line's age is how long it survived: from when it was written to when this commit
    // deleted it. A zero is real (written and deleted the same day) and a negative span is
    // a clock skew in the commit data, which clamps to zero rather than going unsigned.
    let span = days_between(blamed.at, deleted_at);
    Some(u32::try_from(span.max(0)).unwrap_or(u32::MAX))
}
