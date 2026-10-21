//! H11 Maintainers and bus factor: how many people this project actually has.
//!
//! Formula (action plan, H11):
//!
//! > Distinct non-bot humans who merged PRs or own blamed HEAD lines, deduplicated with
//! > mailmap plus GitHub login. Bus factor = the fewest authors covering at least 50% of
//! > blamed authored lines.
//!
//! Two counts, not one. "Maintainers" is everyone who has shipped something a reader is
//! looking at: they merged a pull request in the last year, or they wrote a line that
//! survives at HEAD. "Bus factor" is the smallest number of those people whose lines add up
//! to half of today's code, which is the number that matters when someone leaves.
//!
//! Deduplication is the hard part and the plan names both halves of it. Git already applies
//! `.mailmap` (`--use-mailmap` in the log), so one person's two addresses are one author.
//! The forge's side needs a login: a contributor's login is matched to an author by the
//! commit counts the API reports, and a blamed author with no forge identity stays a
//! separate human because nothing proves they are the same person.
//!
//! Bus factor 1 is a flag, not a verdict — normal for small, good projects (the plan says so
//! explicitly). No threshold in the file reads it.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::thresholds::Thresholds;

use super::shape::Judgment;
use super::{HistoryInput, LONGEST_WINDOW_DAYS};

/// Judges H11.
#[must_use]
pub fn maintainers(input: &HistoryInput, _thresholds: &Thresholds) -> Judgment {
    // Surviving lines per author, from the blame: the people whose work a reader sees.
    let mut lines_by_author: BTreeMap<String, u64> = BTreeMap::new();
    let mut lines_by_author_at: BTreeMap<String, substance_core::Timestamp> = BTreeMap::new();
    for (_, line) in input.blame.lines() {
        if crate::git::is_bot(&line.author, &line.email, None) {
            continue;
        }
        *lines_by_author.entry(line.author.clone()).or_default() += 1;
        // The same author's newest surviving line, for reporting only.
        lines_by_author_at
            .entry(line.author.clone())
            .and_modify(|at| {
                if line.at > *at {
                    *at = line.at;
                }
            })
            .or_insert(line.at);
    }

    let window_start = input.window_start(LONGEST_WINDOW_DAYS);
    let mut forgeless_note = None;
    let mut merging: BTreeSet<String> = BTreeSet::new();
    let mut forge_people = 0u64;
    match input.forge_or_reason() {
        Ok(forge) => {
            for pull in forge.merged_pulls() {
                let Some(merged_at) = pull.merged_at else {
                    continue;
                };
                if merged_at < window_start {
                    continue;
                }
                let login = match &pull.merged_by {
                    Some(login) if !login.is_empty() => login,
                    // A pull request merged by GitHub itself (a merge queue) has no account;
                    // its author is the person whose change landed.
                    _ => &pull.author.login,
                };
                if crate::git::is_bot(login, "", None) {
                    continue;
                }
                merging.insert(login.clone());
            }
            forge_people = u64::try_from(forge.contributors.len()).unwrap_or(0);
        }
        Err(reason) => forgeless_note = Some(reason),
    }

    // The humans: a blamed author, a forge login that merged in the window, or a
    // contributor the API listed. A login that is also an author name is one human; the two
    // namespaces are compared case-insensitively because git preserves case and GitHub does
    // not.
    let mut humans: BTreeSet<String> = BTreeSet::new();
    for author in lines_by_author.keys() {
        humans.insert(author.to_lowercase());
    }
    for login in &merging {
        humans.insert(login.to_lowercase());
    }
    if let Some(forge) = input.forge.as_ref() {
        for contributor in &forge.contributors {
            if contributor.is_bot() {
                continue;
            }
            humans.insert(contributor.login.to_lowercase());
        }
    }

    // The bus factor: sort authors by surviving lines, largest first, and find the fewest
    // that cover half of them. A tie breaks on the name so the answer is stable.
    let total: u64 = lines_by_author.values().sum();
    let mut ordered: Vec<(String, u64)> = lines_by_author
        .iter()
        .map(|(author, lines)| (author.clone(), *lines))
        .collect();
    ordered.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let mut covered = 0u64;
    let mut bus_factor = 0u64;
    for (_, lines) in &ordered {
        if total == 0 {
            break;
        }
        bus_factor += 1;
        covered += lines;
        if covered.saturating_mul(2) >= total {
            break;
        }
    }

    let mut metrics = BTreeMap::new();
    metrics.insert(
        "humans".to_owned(),
        i64::try_from(humans.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "maintainers".to_owned(),
        i64::try_from(merging.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "blamed_authors".to_owned(),
        i64::try_from(lines_by_author.len()).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "contributors".to_owned(),
        i64::try_from(forge_people).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "bus_factor".to_owned(),
        i64::try_from(bus_factor).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "blamed_lines".to_owned(),
        i64::try_from(total).unwrap_or(i64::MAX),
    );
    metrics.insert(
        "bus_factor_is_one".to_owned(),
        i64::from(bus_factor == 1 && total > 0),
    );

    // Evidence: the authors who own the most surviving lines, which is what the bus factor
    // is computed from. They are people, not commits, so the evidence is a note naming them
    // in order rather than a commit list.
    let top: Vec<String> = ordered
        .iter()
        .take(10)
        .map(|(author, lines)| format!("{author} ({lines})"))
        .collect();
    let mut judgment = Judgment::measured(metrics);
    if total == 0 {
        return Judgment::not_measured(
            "no authored line could be blamed, so no author owns any of today's code",
            judgment.metrics,
        );
    }
    if input.blame.coverage.truncated {
        judgment = judgment.with_note(format!(
            "blame covered {} of {} authored files, so the bus factor rests on part of the tree",
            input.blame.coverage.files_blamed, input.blame.coverage.files_total
        ));
    }
    if let Some(reason) = forgeless_note {
        judgment = judgment.with_note(format!(
            "merged pull requests are not counted, so maintainers may be undercounted: {reason}"
        ));
    }
    if bus_factor == 1 {
        judgment = judgment.with_note(
            "bus factor 1: one person wrote half of today's code. That is a risk flag, not a \
             finding — small projects often look like this"
                .to_owned(),
        );
    }
    judgment.with_note(format!(
        "largest authors by surviving lines: {}",
        top.join(", ")
    ))
}
