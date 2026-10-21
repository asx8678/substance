//! The operability checks O02, O03 and O04, judged from what the runtime stage observed (M2-T05).
//!
//! ## Why the judgement is separate from the observation
//!
//! Each of these checks is a sentence about something that happened while the app was running:
//! the README rung's outcome (O02), what the process group did when it was signalled (O03), and
//! whether the migrate command runs twice (O04). The runtime stage is what can observe those; this
//! module is what decides what they mean.
//!
//! The split matters for testability and for honesty. Every function here takes a plain facts struct
//! that a caller can build by hand, so the judgement can be exercised without booting anything, and
//! -- more importantly -- so the runtime stage cannot make a check pass by accident. A stage that
//! both observed and judged could "pass" on the strength of a number it never collected; here a
//! missing fact is a `None`, and a `None` is `not_measured`, never `pass`.
//!
//! ## The rule every function obeys
//!
//! Principle 8: absence of evidence is never a pass. Each judge is total over its struct and every
//! absent input produces `NotMeasured` with a reason that names what was missing, so a report can
//! always explain why a check did not run.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::stack::{BootOutcome, BootRung, Friction};
use substance_core::model::{CheckVerdict, Target, Verdict};
use substance_runner::ExitInfo;

/// The `parts` key for O01's locked install, which the runner performs.
pub use substance_statics::checks::o01::EXEC_PART;

/// What the runtime stage saw happen to one install step.
#[derive(Debug, Clone, PartialEq, Eq)]
/// The facts O01's exec part is judged from.
pub struct InstallFact {
    /// Whether the locked install exited 0.
    pub success: bool,
    /// Its exit code, when it had one.
    pub code: Option<i32>,
    /// The byte offset where its output ended, which is what a reader opens the log at.
    pub log_end: u64,
    /// The command that ran, for the message.
    pub command: String,
}

/// The facts O02 is judged from.
#[derive(Debug, Clone, Default)]
pub struct ReadmeFact {
    /// The boot outcome, when a boot was attempted at all.
    pub boot: Option<BootOutcome>,
    /// What the README rung did when the runtime stage tried it on its own.
    ///
    /// `None` means there was no rung to try. The stage tries it separately because the ladder
    /// stops at the first rung that works, so a repository with a good config never reaches rung 1
    /// -- and O02 would then be unmeasurable in exactly the case it exists for.
    pub trial: Option<Result<(), String>>,
}

/// The facts O03 is judged from: the exit of the app's process group.
#[derive(Debug, Clone)]
pub struct ShutdownFact {
    /// What the stop observed.
    pub exit: ExitInfo,
}

/// The facts O04 is judged from: whether a second migrate run succeeded.
#[derive(Debug, Clone)]
pub struct MigrateFact {
    /// The second run's success, when a migrate command existed and a first run succeeded.
    pub second_success: bool,
    /// The second run's exit code, when it had one.
    pub code: Option<i32>,
    /// The command, for the message.
    pub command: String,
}

/// Judges O01's exec part.
///
/// The static part is judged in `substance-statics`; this is the half that needs a working copy and a
/// network, so the check's overall verdict is the aggregate of the two and is only ever `pass` when
/// the lockfile is complete *and* the locked install succeeds.
#[must_use]
pub fn locked_install_exec(fact: Option<&InstallFact>) -> Verdict {
    let Some(fact) = fact else {
        return Verdict::NotMeasured {
            reason:
                "no lockfile command ran: the plan chose no install step for this repository, or \
                     the boot never reached one"
                    .to_owned(),
        };
    };
    if fact.success {
        return Verdict::Pass;
    }
    Verdict::Fail
}

/// O01's overall verdict from its two parts, plus the metrics both halves produced.
#[must_use]
pub fn locked_install_verdict(parts: &BTreeMap<String, Verdict>) -> Verdict {
    Verdict::aggregate(parts)
}

/// Judges O02: does the README's own setup instruction boot the app?
///
/// The question is deliberately narrow. It asks whether the repository's *documented* path works,
/// not whether some other rung does -- a project whose README is stale and whose adapter boots fine
/// still has a README that would strand a new contributor, which is what O02 is for.
///
/// Three answers:
///
/// - The README rung booted: `pass`, whatever else failed.
/// - The README rung did not boot (or needed fixes) while the run found another way in: `fail`, with
///   the friction entries as the evidence.
/// - No README rung was attempted at all: `not_measured`, because a repository with no setup
///   instructions is not one whose instructions are wrong.
#[must_use]
pub fn clean_room_boot(fact: Option<&ReadmeFact>) -> Verdict {
    let Some(fact) = fact else {
        return Verdict::NotMeasured {
            reason: "the runtime stage did not run, so no boot was attempted".to_owned(),
        };
    };
    // The stage's own trial answers the question outright, and it is asked in every run that
    // booted at all -- so it is consulted first.
    if let Some(trial) = &fact.trial {
        return match trial {
            Ok(()) => Verdict::Pass,
            Err(_) => Verdict::Fail,
        };
    }
    let Some(boot) = &fact.boot else {
        return Verdict::NotMeasured {
            reason: "the runtime stage did not get as far as a boot".to_owned(),
        };
    };
    // No trial means the ladder booted from the README rung itself, which the stage does not
    // re-try: rung 1 working *is* the answer.
    if boot.rung == Some(BootRung::Readme) && boot.booted {
        return Verdict::Pass;
    }
    let readme_failed = boot
        .friction
        .iter()
        .any(|entry| entry.rung == BootRung::Readme);
    if readme_failed {
        return Verdict::Fail;
    }
    Verdict::NotMeasured {
        reason: "no README setup instructions found, so there was no clean-room path to test"
            .to_owned(),
    }
}

/// Judges O03: did every process in the app's group exit within the grace period?
///
/// Both halves are required. A leader that exits 0 while a child keeps running is the `npm start`
/// case the plan names explicitly: the wrapper is gone, the server is not, and a run that called
/// that a pass would leave a process behind on every machine.
#[must_use]
pub fn graceful_shutdown(fact: Option<&ShutdownFact>) -> Verdict {
    let Some(fact) = fact else {
        return Verdict::NotMeasured {
            reason: "the app was not stopped by this run, so nothing was signalled".to_owned(),
        };
    };
    let exit = &fact.exit;
    if !exit.group_empty {
        return Verdict::Fail;
    }
    if !exit.exited_within_grace {
        return Verdict::Fail;
    }
    // 0 is a clean exit. 143 is 128 + SIGTERM, and on Unix a process the kernel ends with SIGTERM
    // never reports 143 through `ExitStatus::code`: it reports no code and a signal. Both spellings
    // mean the same thing -- the process took the signal and left -- so both are a pass. Anything
    // else (137 from SIGKILL after the grace period ran out, or a wrapper's own error code) is a
    // fail, and the earlier checks above have already caught the two ways to fail structurally.
    match (&exit.code, exit.signal.as_deref()) {
        (Some(0), _) | (Some(143), _) | (None, Some("SIGTERM")) => Verdict::Pass,
        _ => Verdict::Fail,
    }
}

/// Judges O04: does the migrate command run a second time without failing?
///
/// A migration that cannot be re-run is a migration a deploy cannot retry, and the failure usually
/// only appears on the second run -- after the first has already changed the database.
#[must_use]
pub fn idempotent_migrations(fact: Option<&MigrateFact>) -> Verdict {
    let Some(fact) = fact else {
        return Verdict::NotMeasured {
            reason: "no migration tool detected, so there is no migrate command to run twice"
                .to_owned(),
        };
    };
    if fact.second_success {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

/// Builds the verdict one of these checks reports, with its metrics and notes.
///
/// One constructor for the three so a reader comparing them sees the same shape, and so a target is
/// never accidentally left as something other than the repository: all three describe the repo.
#[must_use]
pub fn verdict(check: CheckId, verdict: Verdict, metrics: BTreeMap<String, i64>) -> CheckVerdict {
    CheckVerdict {
        check_id: check,
        target: Target::Repo,
        verdict,
        finding_ids: Vec::new(),
        metrics,
        parts: BTreeMap::new(),
    }
}

/// The friction entries that belong to the README rung, sorted as the boot recorded them.
#[must_use]
pub fn readme_friction(boot: &BootOutcome) -> Vec<&Friction> {
    boot.friction
        .iter()
        .filter(|entry| entry.rung == BootRung::Readme)
        .collect()
}

/// The verdicts O01's exec part, O02, O03 and O04 contribute, built from one runtime outcome.
///
/// Returns them in ID order, one per check, so a caller can merge without sorting. O05 is not here:
/// it is static, judged in triage from the commit's Dockerfile (ADR-0020), and has no runtime input
/// at all.
#[must_use]
pub fn runtime_verdicts(outcome: &crate::runtime::RuntimeOutcome) -> Vec<CheckVerdict> {
    let mut out: Vec<CheckVerdict> = Vec::new();

    // ── O01: the exec half, plus the static half already judged in triage. ──────────────────────
    let install = outcome
        .steps
        .iter()
        .find(|step| step.name == "install")
        .map(|step| InstallFact {
            success: step.success,
            code: step.code,
            log_end: step.log_end,
            command: step.display(),
        });
    let exec = locked_install_exec(install.as_ref());
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert("install_ran".to_owned(), i64::from(install.is_some()));
    if let Some(fact) = &install {
        metrics.insert(
            "install_log_bytes".to_owned(),
            i64::try_from(fact.log_end).unwrap_or(i64::MAX),
        );
    }
    let mut parts: BTreeMap<String, Verdict> = BTreeMap::new();
    parts.insert(EXEC_PART.to_owned(), exec);
    out.push(CheckVerdict {
        check_id: CheckId::O01,
        target: Target::Repo,
        verdict: Verdict::aggregate(&parts),
        finding_ids: Vec::new(),
        metrics,
        parts,
    });

    // ── O02: the README rung. ───────────────────────────────────────────────────────────────────
    let readme = ReadmeFact {
        boot: outcome.boot.clone(),
        trial: outcome.readme.clone(),
    };
    let boot_verdict = clean_room_boot(Some(&readme));
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    if let Some(boot) = &outcome.boot {
        metrics.insert(
            "friction".to_owned(),
            i64::try_from(boot.friction.len()).unwrap_or(i64::MAX),
        );
        metrics.insert(
            "readme_friction".to_owned(),
            i64::try_from(readme_friction(boot).len()).unwrap_or(i64::MAX),
        );
        metrics.insert(
            "rungs_tried".to_owned(),
            i64::try_from(boot.friction.len()).unwrap_or(0),
        );
    }
    out.push(verdict(CheckId::O02, boot_verdict, metrics));

    // ── O03: the process group's exit. ──────────────────────────────────────────────────────────
    let shutdown = outcome.shutdown.clone().map(|exit| ShutdownFact { exit });
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    if let Some(fact) = &shutdown {
        metrics.insert("group_empty".to_owned(), i64::from(fact.exit.group_empty));
        metrics.insert(
            "exited_within_grace".to_owned(),
            i64::from(fact.exit.exited_within_grace),
        );
        if let Some(code) = fact.exit.code {
            metrics.insert("leader_code".to_owned(), i64::from(code));
        }
    }
    out.push(verdict(
        CheckId::O03,
        graceful_shutdown(shutdown.as_ref()),
        metrics,
    ));

    // ── O04: the second migrate run. ────────────────────────────────────────────────────────────
    let migrate = outcome.second_migrate.as_ref().map(|run| MigrateFact {
        second_success: run.success,
        code: run.code,
        command: run.command.clone(),
    });
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    if let Some(fact) = &migrate {
        metrics.insert("second_run_ran".to_owned(), 1);
        if let Some(code) = fact.code {
            metrics.insert("second_run_code".to_owned(), i64::from(code));
        }
    } else {
        metrics.insert("second_run_ran".to_owned(), 0);
    }
    out.push(verdict(
        CheckId::O04,
        idempotent_migrations(migrate.as_ref()),
        metrics,
    ));

    out
}

/// One clause describing the README rung, for the report's notes.
#[must_use]
pub fn readme_note(outcome: &crate::runtime::RuntimeOutcome) -> Option<String> {
    let boot = outcome.boot.as_ref()?;
    let entries = readme_friction(boot);
    if entries.is_empty() {
        return Some(format!("O02: {}", readme_summary(boot)));
    }
    let mut lines: Vec<String> = entries
        .iter()
        .map(|entry| {
            format!(
                "O02: README rung, {}, exit {}, log offset {}",
                entry.step,
                entry
                    .code
                    .map_or_else(|| "signal".to_owned(), |code| code.to_string()),
                entry.log_end
            )
        })
        .collect();
    lines.push(format!("O02: {}", readme_summary(boot)));
    Some(lines.join("; "))
}

/// The reason the README trial gave, when it failed, for a note.
///
/// The verdict is a bare `fail`; this is what makes it actionable, and it names the step and the
/// command rather than restating the verdict.
#[must_use]
pub fn readme_failure_note(outcome: &crate::runtime::RuntimeOutcome) -> Option<String> {
    match outcome.readme.as_ref()? {
        Ok(()) => None,
        Err(reason) => Some(format!(
            "O02: the README setup did not boot the app: {reason}"
        )),
    }
}

/// A one-line summary of what the README rung did, for a note.
#[must_use]
pub fn readme_summary(boot: &BootOutcome) -> String {
    match (boot.rung, boot.booted) {
        (Some(BootRung::Readme), true) => "the README setup blocks booted the app".to_owned(),
        (Some(rung), true) => format!(
            "the app booted from the {} rung; the README rung did not",
            rung.as_str()
        ),
        _ => "no rung booted the app".to_owned(),
    }
}
