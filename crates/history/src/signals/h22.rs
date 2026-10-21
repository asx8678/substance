//! H22 Archetype: the one word that summarises how this project was built.
//!
//! Formula (action plan, H22):
//!
//! > Archetype: H01 unverifiable → Unverifiable history. Otherwise the first match in this
//! > order: One-shot dump, Burst then abandoned, Team maintained, Solo maintained, Young and
//! > iterating. No match → `unclassified`. The report shows the metrics that triggered it.
//!
//! The order is the specification, not a detail: a repository can satisfy several rules at
//! once, and the plan's order is what decides. "Burst then abandoned" outranks "solo
//! maintained" because the abandonment is the more important fact about it.
//!
//! Every input is a metric computed here once, in [`ArchetypeMetrics`], from the same numbers
//! the other checks report — H02's weeks, H03's big-bang, H11's bus factor, H12's reviewed
//! share, H13's response time. Computing them again rather than reading another check's output
//! keeps H22 independent of the order the checks run in.
//!
//! **A condition on a not-measured metric is false**, as the plan says. That rule has a
//! consequence the plan also states: when a rule failed to match only because a metric was
//! missing, the result is `unclassified` with a note naming the metric. A repository whose
//! history might be a one-shot dump but whose issue data could not be read is not silently
//! labelled something else.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::model::Archetype;
use substance_core::thresholds::{ArchetypeThresholds, Thresholds};

use super::common::{TestPathRules, is_test_path, own_commits};
use super::shape::Judgment;
use super::{HistoryInput, SHORT_WINDOW_DAYS, is_outsider};
use crate::signals::days_between;

/// The numbers every archetype rule reads, each `None` when it could not be measured.
///
/// A `None` here is what makes a rule's condition false: the rule needs the number, and the
/// absence of a number is not evidence for the rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ArchetypeMetrics {
    /// Days from day one to `as_of`.
    pub age_days: Option<u32>,
    /// H03's permille.
    pub big_bang: Option<u32>,
    /// H02's active weeks in the last 90 days.
    pub active_weeks_90: Option<u32>,
    /// H02's active weeks in the last 365 days.
    pub active_weeks_365: Option<u32>,
    /// Distinct UTC days with a non-bot commit, over the repository's life.
    pub active_days: Option<u32>,
    /// Permille of commits after day one that delete or change an existing authored line.
    pub edit_share: Option<u32>,
    /// Permille of commits that change authored non-test files and also test files.
    pub test_lines_change_with_code: Option<u32>,
    /// Distinct non-bot issue authors who are not maintainers.
    pub outside_issue_authors: Option<u32>,
    /// H11's bus factor.
    pub bus_factor: Option<u32>,
    /// Distinct non-bot humans who merged a pull request in the last year.
    pub maintainers: Option<u32>,
    /// H12's reviewed permille.
    pub reviewed_prs: Option<u32>,
    /// Whether a maintainer answers outsiders' issues within the configured hours.
    pub answers_issues: Option<bool>,
    /// The metrics that could not be measured, sorted, for the note.
    pub missing: Vec<&'static str>,
}

impl ArchetypeMetrics {
    /// Computes every metric from the input.
    #[must_use]
    pub fn compute(input: &HistoryInput, rules: &TestPathRules) -> Self {
        let commits = own_commits(input);
        let day_one = input.day_one();
        let mut missing: Vec<&'static str> = Vec::new();

        let age_days = u32::try_from(days_between(day_one.at, input.as_of).max(0)).ok();
        if age_days.is_none() {
            missing.push("age_days");
        }

        let big_bang = super::shape::permille(
            input
                .blame
                .lines()
                .filter(|(_, line)| days_between(day_one.at, line.at) == 0)
                .count() as u64,
            input.blame.coverage.lines_blamed,
        )
        .and_then(|share| u32::try_from(share).ok());
        if big_bang.is_none() {
            missing.push("big_bang");
        }

        let short_start = input.window_start(SHORT_WINDOW_DAYS);
        let long_start = input.window_start(super::LONGEST_WINDOW_DAYS);
        let mut weeks_short: BTreeSet<(i32, u8)> = BTreeSet::new();
        let mut weeks_long: BTreeSet<(i32, u8)> = BTreeSet::new();
        for commit in &commits {
            let at = commit.author.at;
            if at < long_start {
                continue;
            }
            let date = at.as_offset_date_time().date();
            let week = (date.year(), date.iso_week());
            weeks_long.insert(week);
            if at >= short_start {
                weeks_short.insert(week);
            }
        }
        if let Ok(forge) = input.forge_or_reason() {
            for pull in forge.merged_pulls() {
                let Some(merged_at) = pull.merged_at else {
                    continue;
                };
                if merged_at < long_start {
                    continue;
                }
                let date = merged_at.as_offset_date_time().date();
                weeks_long.insert((date.year(), date.iso_week()));
                if merged_at >= short_start {
                    weeks_short.insert((date.year(), date.iso_week()));
                }
            }
        }
        let active_weeks_90 = u32::try_from(weeks_short.len()).ok();
        let active_weeks_365 = u32::try_from(weeks_long.len()).ok();

        let days = super::common::active_days(&commits);
        let active_days = u32::try_from(days.len()).ok();

        // Edit share and the test-with-code share are over commits after day one: the first
        // commit cannot be an edit of anything earlier.
        let after_day_one: Vec<_> = commits
            .iter()
            .filter(|commit| days_between(day_one.at, commit.author.at) > 0)
            .collect();
        let mut editing = 0u64;
        let mut test_with_code = 0u64;
        let mut changing_authored = 0u64;
        for commit in &after_day_one {
            let mut deleted = 0u64;
            let mut changed_code = false;
            let mut changed_test = false;
            for file in &commit.files {
                let authored = input.is_authored(&file.path) || super::path_is_authored(&file.path);
                let produced = file.added.unwrap_or(0) + file.deleted.unwrap_or(0);
                if !authored || produced == 0 {
                    continue;
                }
                deleted += file.deleted.unwrap_or(0);
                if is_test_path(&file.path, rules) {
                    changed_test = true;
                } else {
                    changed_code = true;
                }
            }
            if deleted > 0 {
                editing += 1;
            }
            if changed_code {
                changing_authored += 1;
                if changed_test {
                    test_with_code += 1;
                }
            }
        }
        let total_after = u64::try_from(after_day_one.len()).unwrap_or(u64::MAX);
        let edit_share = super::shape::permille(editing, total_after)
            .and_then(|share| u32::try_from(share).ok());
        let test_share = super::shape::permille(test_with_code, changing_authored)
            .and_then(|share| u32::try_from(share).ok());
        if edit_share.is_none() {
            missing.push("edit_share");
        }
        if test_share.is_none() {
            missing.push("test_lines_change_with_code");
        }

        // The forge half: outsiders, maintainers, reviews, and whether issues get answered.
        let mut outside_issue_authors: Option<u32> = None;
        let mut maintainers: Option<u32> = None;
        let mut reviewed_prs: Option<u32> = None;
        let mut answers_issues: Option<bool> = None;
        let mut bus_factor: Option<u32> = None;
        // The bus factor comes from the blame, not from the forge, so it is measurable even
        // when the forge is not.
        let mut lines_by_author: BTreeMap<String, u64> = BTreeMap::new();
        for (_, line) in input.blame.lines() {
            if crate::git::is_bot(&line.author, &line.email, None) {
                continue;
            }
            *lines_by_author.entry(line.author.clone()).or_default() += 1;
        }
        let total_lines: u64 = lines_by_author.values().sum();
        if total_lines > 0 {
            let mut ordered: Vec<u64> = lines_by_author.values().copied().collect();
            ordered.sort_unstable_by(|left, right| right.cmp(left));
            let mut covered = 0u64;
            let mut factor = 0u32;
            for lines in ordered {
                factor += 1;
                covered += lines;
                if covered.saturating_mul(2) >= total_lines {
                    break;
                }
            }
            bus_factor = Some(factor);
        } else {
            missing.push("bus_factor");
        }

        if let Ok(forge) = input.forge_or_reason() {
            let outsiders: BTreeSet<String> = forge
                .issues
                .iter()
                .filter(|issue| is_outsider(&issue.author_association))
                .filter(|issue| !issue.author.is_bot())
                .map(|issue| issue.author.login.to_lowercase())
                .collect();
            outside_issue_authors = u32::try_from(outsiders.len()).ok();

            let merging: BTreeSet<String> = forge
                .merged_pulls()
                .iter()
                .filter(|pull| pull.merged_at.is_some_and(|at| at >= long_start))
                .filter_map(|pull| {
                    pull.merged_by
                        .clone()
                        .or_else(|| Some(pull.author.login.clone()))
                })
                .filter(|login| !crate::git::is_bot(login, "", None))
                .map(|login| login.to_lowercase())
                .collect();
            maintainers = u32::try_from(merging.len()).ok();

            let mut read = 0u64;
            let mut approved = 0u64;
            for pull in forge.merged_pulls() {
                let Some(reviews) = forge.reviews_of(pull.number) else {
                    continue;
                };
                read += 1;
                let author = pull.author.login.to_lowercase();
                if reviews.iter().any(|review| {
                    review.state.eq_ignore_ascii_case("APPROVED")
                        && review.author.login.to_lowercase() != author
                        && !review.author.is_bot()
                }) {
                    approved += 1;
                }
            }
            reviewed_prs =
                super::shape::permille(approved, read).and_then(|share| u32::try_from(share).ok());

            // `answers_issues`: a median first response within the allowance, or no outsider
            // issues in the window at all (a project with no questions is not failing to
            // answer them).
            let mut responses: Vec<u64> = Vec::new();
            let mut asked = 0u64;
            for issue in &forge.issues {
                let Some(created_at) = issue.created_at else {
                    continue;
                };
                if created_at < long_start
                    || !is_outsider(&issue.author_association)
                    || issue.author.is_bot()
                {
                    continue;
                }
                asked += 1;
                let handle = format!("#{}", issue.number);
                let first = forge
                    .issue_comments
                    .iter()
                    .filter(|comment| comment.issue.ends_with(&handle))
                    .filter(|comment| {
                        !comment.author.is_bot()
                            && super::common::is_maintainer(&comment.author_association)
                    })
                    .filter_map(|comment| comment.created_at)
                    .filter(|at| *at >= created_at)
                    .min();
                if let Some(at) = first {
                    let seconds = at.as_offset_date_time().unix_timestamp()
                        - created_at.as_offset_date_time().unix_timestamp();
                    responses.push(u64::try_from(seconds.max(0) / 3600).unwrap_or(u64::MAX));
                }
            }
            responses.sort_unstable();
            answers_issues = Some(match super::h04::nearest_rank(&responses, 50) {
                Some(median) => median <= 168,
                None => asked == 0,
            });
        } else {
            missing.push("outside_issue_authors");
            missing.push("maintainers");
            missing.push("reviewed_prs");
            missing.push("answers_issues");
        }

        missing.sort_unstable();
        Self {
            age_days,
            big_bang,
            active_weeks_90,
            active_weeks_365,
            active_days,
            edit_share,
            test_lines_change_with_code: test_share,
            outside_issue_authors,
            bus_factor,
            maintainers,
            reviewed_prs,
            answers_issues,
            missing,
        }
    }

    /// The metrics as the integers H22's verdict carries.
    #[must_use]
    pub fn to_metrics(&self) -> BTreeMap<String, i64> {
        let mut metrics = BTreeMap::new();
        let entries: [(&str, Option<u32>); 9] = [
            ("age_days", self.age_days),
            ("big_bang", self.big_bang),
            ("active_weeks_90", self.active_weeks_90),
            ("active_weeks_365", self.active_weeks_365),
            ("active_days", self.active_days),
            ("edit_share", self.edit_share),
            (
                "test_lines_change_with_code",
                self.test_lines_change_with_code,
            ),
            ("outside_issue_authors", self.outside_issue_authors),
            ("bus_factor", self.bus_factor),
        ];
        for (key, value) in entries {
            if let Some(value) = value {
                metrics.insert(key.to_owned(), i64::from(value));
            }
        }
        if let Some(maintainers) = self.maintainers {
            metrics.insert("maintainers".to_owned(), i64::from(maintainers));
        }
        if let Some(reviewed) = self.reviewed_prs {
            metrics.insert("reviewed_prs".to_owned(), i64::from(reviewed));
        }
        if let Some(answers) = self.answers_issues {
            metrics.insert("answers_issues".to_owned(), i64::from(answers));
        }
        metrics
    }
}

/// The archetype a metric set and a threshold file produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// The archetype, or `None` when no rule matched.
    pub archetype: Option<Archetype>,
    /// The rule that matched, by name.
    pub rule: Option<&'static str>,
    /// Why no rule matched, when none did.
    pub note: Option<String>,
    /// The metrics a rule read, sorted, for the report.
    pub triggering: Vec<&'static str>,
}

/// Applies the archetype rules in the plan's order.
///
/// The order is the specification: one-shot dump, burst then abandoned, team maintained,
/// solo maintained, young and iterating. `unverifiable` is tested before any of them, because
/// a timeline that cannot be trusted cannot support any classification.
#[must_use]
pub fn classify(
    metrics: &ArchetypeMetrics,
    thresholds: &ArchetypeThresholds,
    unverifiable: Option<&str>,
) -> Classification {
    if let Some(reason) = unverifiable {
        return Classification {
            archetype: Some(Archetype::UnverifiableHistory),
            rule: Some("unverifiable"),
            note: Some(reason.to_owned()),
            triggering: Vec::new(),
        };
    }

    // 1. One-shot dump: most code from day one, few active weeks, no outside issues.
    if let Some(triggering) = matched(
        &[
            (
                "big_bang",
                at_least(metrics.big_bang, thresholds.one_shot_big_bang_permille),
            ),
            (
                "active_weeks_365",
                at_most(
                    metrics.active_weeks_365,
                    thresholds.one_shot_max_active_weeks,
                ),
            ),
            (
                "outside_issue_authors",
                equals(metrics.outside_issue_authors, Some(0)),
            ),
        ],
        "one_shot",
        metrics,
    ) {
        return triggering;
    }

    // 2. Burst then abandoned: old enough, and nothing in the last 90 days.
    if let Some(triggering) = matched(
        &[
            (
                "age_days",
                at_least(metrics.age_days, thresholds.burst_min_age_days),
            ),
            ("active_weeks_90", equals(metrics.active_weeks_90, Some(0))),
        ],
        "burst_abandoned",
        metrics,
    ) {
        return triggering;
    }

    // 3. Team maintained: two or more merging, most pull requests reviewed.
    if let Some(triggering) = matched(
        &[
            (
                "maintainers",
                at_least(metrics.maintainers, thresholds.team_min_maintainers),
            ),
            (
                "reviewed_prs",
                at_least(metrics.reviewed_prs, thresholds.team_min_reviewed_permille),
            ),
        ],
        "team",
        metrics,
    ) {
        return triggering;
    }

    // 4. Solo maintained: recently active, bus factor one, answers issues.
    if let Some(triggering) = matched(
        &[
            (
                "active_weeks_90",
                at_least(metrics.active_weeks_90, thresholds.solo_min_active_weeks),
            ),
            ("bus_factor", equals(metrics.bus_factor, Some(1))),
            (
                "answers_issues",
                Some(matches!(metrics.answers_issues, Some(true))),
            ),
        ],
        "solo",
        metrics,
    ) {
        return triggering;
    }

    // 5. Young and iterating.
    if let Some(triggering) = matched(
        &[
            (
                "age_days",
                at_most(metrics.age_days, thresholds.young_max_age_days),
            ),
            (
                "active_days",
                at_least(metrics.active_days, thresholds.young_min_active_days),
            ),
            (
                "edit_share",
                at_least(metrics.edit_share, thresholds.young_min_edit_share_permille),
            ),
            (
                "test_lines_change_with_code",
                at_least(
                    metrics.test_lines_change_with_code,
                    thresholds.young_min_test_lines_change_permille,
                ),
            ),
        ],
        "young_iterating",
        metrics,
    ) {
        return triggering;
    }

    // No rule matched. The plan asks for a note naming a metric that was missing and would
    // have changed the answer, so the note lists what could not be measured at all.
    let note = if metrics.missing.is_empty() {
        "no archetype rule matched the measured metrics".to_owned()
    } else {
        format!(
            "no archetype rule matched; these metrics could not be measured, and a condition on \
             a missing metric is false: {}",
            metrics.missing.join(", ")
        )
    };
    Classification {
        archetype: None,
        rule: None,
        note: Some(note),
        triggering: Vec::new(),
    }
}

/// `Some(value)` when the value is at least the threshold, which must be present.
fn at_least(value: Option<u32>, threshold: Option<u32>) -> Option<bool> {
    match (value, threshold) {
        (Some(value), Some(threshold)) => Some(value >= threshold),
        // A missing number makes the condition false, and a missing metric is reported
        // separately: the rule did not match, and the note says why nothing could.
        _ => Some(false),
    }
}

/// `Some(value)` when the value is at most the threshold.
fn at_most(value: Option<u32>, threshold: Option<u32>) -> Option<bool> {
    match (value, threshold) {
        (Some(value), Some(threshold)) => Some(value <= threshold),
        _ => Some(false),
    }
}

/// `Some(value)` when the value is the given number.
fn equals(value: Option<u32>, expected: Option<u32>) -> Option<bool> {
    match (value, expected) {
        (Some(value), Some(expected)) => Some(value == expected),
        _ => Some(false),
    }
}

/// The classification when every condition in `conditions` holds, else `None`.
fn matched(
    conditions: &[(&'static str, Option<bool>)],
    rule: &'static str,
    _metrics: &ArchetypeMetrics,
) -> Option<Classification> {
    let all = conditions
        .iter()
        .all(|(_, value)| matches!(value, Some(true)));
    if !all {
        return None;
    }
    Some(Classification {
        archetype: None,
        rule: Some(rule),
        note: None,
        triggering: conditions.iter().map(|(name, _)| *name).collect(),
    })
}

/// Judges H22.
#[must_use]
pub fn archetype(input: &HistoryInput, thresholds: &Thresholds) -> Judgment {
    let metrics = ArchetypeMetrics::compute(input, &input.test_paths);
    // H01 decides the unverifiable case. Asking it here rather than reading its verdict keeps
    // H22 independent of the order the checks run in.
    let unverifiable = unverifiable_reason(input);
    let classification = classify(
        &metrics,
        &thresholds.history().archetypes,
        unverifiable.as_deref(),
    );

    let mut values = metrics.to_metrics();
    // The archetype itself, as an index into the sorted list of names, so the report can
    // carry it as an integer metric as well as in the report's own field.
    if let Some(archetype) = classification.archetype {
        let position = ARCHETYPES
            .iter()
            .position(|candidate| candidate == &archetype)
            .unwrap_or(0);
        values.insert("archetype".to_owned(), i64::try_from(position).unwrap_or(0));
    }

    let mut judgment = Judgment::measured(values);
    if let Some(note) = &classification.note {
        judgment = judgment.with_note(note.clone());
    }
    if !thresholds.history().archetypes.is_complete() {
        judgment = judgment.with_note(format!(
            "the archetype thresholds are incomplete, so some rules cannot match: {}",
            thresholds.history().archetypes.missing().join(", ")
        ));
    }
    if !classification.triggering.is_empty() {
        judgment = judgment.with_note(format!(
            "{} matched on: {}",
            classification.rule.unwrap_or("a rule"),
            classification.triggering.join(", ")
        ));
    }
    // H22 has no threshold of its own: the archetype is a classification, not a finding, and
    // the archetype rules are the thresholds. Reporting is the correct verdict.
    judgment
}

/// The archetype and the metrics it was read from, for a report that carries both.
///
/// The judgment H22 returns holds the metrics as integers and the archetype as an index into
/// `ARCHETYPES`; a report needs the enum and the numbers beside it, and re-deriving either from
/// the other would be a second implementation of the same classification.
#[must_use]
pub fn classify_report(
    input: &HistoryInput,
    thresholds: &Thresholds,
) -> (Option<Archetype>, ArchetypeMetrics) {
    let metrics = ArchetypeMetrics::compute(input, &input.test_paths);
    let unverifiable = unverifiable_reason(input);
    let classification = classify(
        &metrics,
        &thresholds.history().archetypes,
        unverifiable.as_deref(),
    );
    (classification.archetype, metrics)
}

/// The archetypes in the order the report lists them, matching [`Archetype`]'s variants.
const ARCHETYPES: &[Archetype] = &[
    Archetype::OneShotDump,
    Archetype::BurstThenAbandoned,
    Archetype::YoungAndIterating,
    Archetype::SoloMaintained,
    Archetype::TeamMaintained,
    Archetype::UnverifiableHistory,
];

/// H01's unverifiable condition, computed from the input.
fn unverifiable_reason(input: &HistoryInput) -> Option<String> {
    let forge = input.forge.as_ref()?;
    let created = forge.repo.as_ref().and_then(|repo| repo.created_at)?;
    let older = input
        .commits
        .iter()
        .filter(|commit| days_between(commit.author.at, created) > 1)
        .count();
    (older > 0)
        .then(|| format!("imported or backdated history: {older} commits predate the repository"))
}
