//! The verdict for every check a build cannot yet run (M0-T08).
//!
//! Principle 8: a channel the run could not observe reports `not_measured` with a
//! reason, never `pass`. Until a check's handler lands, that reason is the milestone
//! that will implement it; a check the project switched off says so instead, and one
//! that needs `--allow-exec` says that. Reserved and support IDs judge nothing, so
//! they get no verdict at all.

use std::collections::BTreeMap;

use substance_core::check::CheckStatus;
use substance_core::config::ProjectConfig;
use substance_core::model::verdict::{CheckVerdict, Target, Verdict};
use substance_core::registry::CheckMeta;
use substance_core::{REGISTRY, parts_of};

/// Why a check that has not run did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    /// The check's milestone has not landed in this build.
    NotImplemented(&'static str),
    /// The project's `substance.toml` disables the check.
    Disabled,
}

impl SkipReason {
    /// The reason as the report spells it.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::NotImplemented(milestone) => format!("not implemented ({milestone})"),
            Self::Disabled => "disabled by config".to_owned(),
        }
    }
}

/// Decides why `meta` cannot run in this configuration.
///
/// A switched-off check is switched off whatever else is missing, so that answer comes
/// first; otherwise the reason is the milestone that will implement the check. A check
/// whose handler exists but whose inputs are missing gets its own reason from the stage
/// that skipped it (M1-T12), not from here: this function only covers checks no build of
/// substance has ever run.
#[must_use]
pub fn skip_reason(meta: &CheckMeta, config: &ProjectConfig) -> SkipReason {
    if config.checks.disable.contains(&meta.id) {
        return SkipReason::Disabled;
    }
    SkipReason::NotImplemented(meta.milestone.as_str())
}

/// The verdicts this build can state: one `not_measured` per check that judges.
///
/// `reserved` IDs are held for a later catalog revision and `support` IDs are stages
/// other checks build on; neither judges an app, so neither appears here (the plan's
/// `CheckVerdict` rule, and ADR-0013's definition of a measured verdict).
#[must_use]
pub fn pending(config: &ProjectConfig) -> Vec<CheckVerdict> {
    let mut verdicts = Vec::new();
    for meta in REGISTRY {
        if !matches!(meta.status, CheckStatus::Planned | CheckStatus::Implemented) {
            continue;
        }
        let reason = skip_reason(meta, config);
        verdicts.push(verdict_for(meta, &reason));
    }
    verdicts
        .sort_by(|left, right| (left.check_id, &left.target).cmp(&(right.check_id, &right.target)));
    verdicts
}

/// Builds the verdict for one check.
///
/// A check built in parts (O01, S11, L05) also reports each part, so a reader sees
/// which half is missing. The overall verdict carries the check's own reason rather
/// than a part's: the check has not started, so its own milestone is the honest
/// answer, and the parts say which half will land when.
fn verdict_for(meta: &CheckMeta, reason: &SkipReason) -> CheckVerdict {
    let mut parts = BTreeMap::new();
    for part in parts_of(meta.id) {
        parts.insert(
            part.name.to_owned(),
            Verdict::NotMeasured {
                reason: format!("not implemented ({})", part.milestone.as_str()),
            },
        );
    }
    CheckVerdict {
        check_id: meta.id,
        target: Target::Repo,
        verdict: Verdict::NotMeasured {
            reason: reason.as_text(),
        },
        finding_ids: Vec::new(),
        metrics: BTreeMap::new(),
        parts,
    }
}

/// The number of checks that judge an app in this build: `planned` plus `implemented`.
#[must_use]
pub fn judging_checks() -> u32 {
    let mut total = 0_u32;
    for meta in REGISTRY {
        if matches!(meta.status, CheckStatus::Planned | CheckStatus::Implemented) {
            total += 1;
        }
    }
    total
}
