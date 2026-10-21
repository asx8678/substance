//! The triage stage: every check that reads the repository without running it (M1-T12).
//!
//! Four family passes, each gathering its own input once, all writing into one sorted report:
//!
//! ```text
//! intake ─► HistoryInput::gather ───► signals    ──┐
//!        ─► run_static_checks ─────► statics    ──┼─► sort ─► report.json
//!        ─► run_dependency_checks ──► deps       ──┤
//!        ─► run_provenance ─────────► provenance ─┘
//! ```
//!
//! Three properties this module exists to guarantee:
//!
//! - **Dispatch goes through [`crate::handlers::handler_for`] only.** A check with no handler keeps
//!   the `not_measured` verdict [`crate::verdicts::pending`] gave it. A check whose handler exists
//!   but whose family failed becomes `not_measured` naming the failure, so a family that could not
//!   run is never confused with a family that found nothing (principle 8).
//! - **The order is fixed.** Families run one after another — they share the run's request budget
//!   and its run directory, and both are `&mut`-shaped — and every list they produce is sorted
//!   before it is written, so two runs of one commit produce byte-identical JSON (ADR-0009).
//! - **A wall-clock cap only ever adds `not_measured`.** It never turns a verdict into a pass and
//!   never changes a number a family already computed; when it fires, the checks that did not run
//!   say `budget exhausted` and the report carries the §4.3 note, so a result that depended on
//!   machine speed says so out loud.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

use substance_core::config::ProjectConfig;
use substance_core::model::report::{Archetype, Coverage, Provenance};
use substance_core::model::{CheckVerdict, Finding, Target, Verdict};
use substance_core::thresholds::Thresholds;
use substance_core::tools::ToolsFile;
use substance_core::{CheckId, Timestamp};
use substance_history::deps::known::Known;
use substance_history::deps::{DepsInput, DepsOutcome, run_dependency_checks};
use substance_history::forge::{Forge, GitHub, GitHubRepoRef};
use substance_history::git::GitRepo;
use substance_history::provenance::{ProvenanceContext, run_provenance};
use substance_history::signals::{HistoryInput, Tools, classify_report, judge_all};
use substance_history::snapshot::SnapshotHttp;
use substance_runner::permit::ExecPermit;
use substance_runner::tool::ToolRunner;
use substance_statics::run_static_checks;

use crate::handlers::{Family, family_of, handler_for};

/// The note §4.3 requires when a time cap fires.
///
/// Spelled as the plan writes it, because a reader deciding whether to trust a report needs to
/// find the sentence they were promised.
pub const TIME_CAP_NOTE: &str = "time cap hit: triage; this result depends on machine speed";

/// The reason a check the cap stopped carries (§4.3: `not_measured("budget exhausted")`).
pub const BUDGET_EXHAUSTED: &str = "budget exhausted";

/// The embedded history rule files.
///
/// `rules/history/` is substance's own data, embedded the way `tools.toml` and the rule pack are:
/// a repository under test must not be able to change the rules that judge it, and a missing file
/// here is a build error rather than a silently empty rule list.
const TEST_PATHS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../rules/history/test-paths.txt"
));
const GENERIC_MESSAGES: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../rules/history/generic-messages.txt"
));

/// What triage needs from the caller.
#[derive(Debug)]
pub struct TriageInput<'a> {
    /// The run's identifier, which every finding ID derives from.
    pub run_id: &'a str,
    /// The checkout intake pinned.
    pub root: &'a Path,
    /// The revision under test.
    pub rev: &'a str,
    /// The origin URL, which decides whether a forge client is built.
    pub origin: &'a str,
    /// The instant every window ends at.
    pub as_of: Timestamp,
    /// The run directory, where tools save their output.
    pub run_dir: &'a Path,
    /// The project config: which checks are disabled, and the budgets.
    pub config: &'a ProjectConfig,
    /// The embedded tool table.
    pub tools: &'a ToolsFile,
    /// The run's thresholds.
    pub thresholds: &'a Thresholds,
    /// A snapshot client for the registries and the forge, when the run has one.
    pub http: Option<SnapshotHttp>,
    /// An executable permit, when `--allow-exec` was given. Only the scanners that run the
    /// repository's own code need one.
    pub permit: Option<&'a ExecPermit>,
}

/// What one family pass produced.
#[derive(Debug, Default)]
struct Pass {
    /// The verdicts the family judged, sorted by check ID.
    verdicts: Vec<CheckVerdict>,
    /// The findings it produced, sorted by ID.
    findings: Vec<Finding>,
    /// Notes per check.
    notes: Vec<(CheckId, Vec<String>)>,
    /// The archetype, when the history family produced one.
    archetype: Option<Archetype>,
    /// The provenance panel, when the provenance family produced one.
    provenance: Option<Provenance>,
    /// The vintage matches the static pass found, which V03 reads.
    vintage_matches: Vec<(String, u32, String)>,
    /// How many registry requests the family spent.
    registry_requests: u32,
    /// Sentences about the family as a whole rather than about one check.
    notes_all: Vec<String>,
}

/// What triage produced.
#[derive(Debug)]
pub struct TriageOutcome {
    /// Every verdict, sorted by `(check ID, target)`.
    pub verdicts: Vec<CheckVerdict>,
    /// Every finding, sorted by ID.
    pub findings: Vec<Finding>,
    /// Notes per check, sorted by check ID.
    pub notes: Vec<(CheckId, Vec<String>)>,
    /// The archetype, when history ran.
    pub archetype: Option<Archetype>,
    /// The provenance panel, when provenance ran.
    pub provenance: Option<Provenance>,
    /// How much was reached.
    pub coverage: Coverage,
    /// The report's own notes, sorted.
    pub notes_all: Vec<String>,
    /// How many registry requests the passes spent.
    pub registry_requests: u32,
    /// Whether the triage time cap fired.
    pub timed_out: bool,
}

/// Runs triage over `input`, starting from the `pending` verdicts a report has before any check
/// runs.
///
/// # Errors
///
/// Returns the reason only when the report could not be assembled at all: a check marked
/// `implemented` with no handler, which is the misreport `C-IMPL` exists to prevent. Every other
/// failure becomes a `not_measured` verdict, because a missing input must be reported rather than
/// crashing the run (principle 8).
pub async fn run_triage(
    input: &TriageInput<'_>,
    pending: Vec<CheckVerdict>,
) -> Result<TriageOutcome, String> {
    let misreported = crate::handlers::implemented_without_handler();
    if !misreported.is_empty() {
        let names: Vec<&str> = misreported.iter().map(|id| id.as_str()).collect();
        return Err(format!(
            "these checks are marked implemented but have no handler: {}",
            names.join(", ")
        ));
    }

    let started = Instant::now();
    let cap = Duration::from_secs(u64::from(input.config.budgets.triage_s));
    let repo = GitRepo::new(input.root);

    // ── History. ────────────────────────────────────────────────────────────────────────────
    // Gathered first because the provenance panel reads its commits, blame and forge data. The
    // gather is the expensive half of triage (a full `git log`, a blame of every authored file,
    // and the forge pages), so it happens once and is borrowed twice.
    let history_http = input.http.clone();
    let tools_owned = Tools::new(
        input.tools.clone(),
        ToolRunner::live(input.root, input.run_dir),
    );
    // The forge client and the gather live in one block so the client's borrow of the snapshot
    // handle ends with it: `gather` holds `&mut dyn Forge` for the whole call, and the gathered
    // input is owned, so nothing outlives the block.
    let (history_input, history_pass) = if capped(started, cap) {
        (None, Err(BUDGET_EXHAUSTED.to_owned()))
    } else {
        let mut forge = forge_for(input.origin, history_http.as_ref(), input.as_of);
        // The gather's result is bound before the client goes out of scope. The client is the
        // concrete `GitHub`, not a boxed trait object: a `Box<dyn Forge>` carries its own
        // destructor, and the drop check then holds the borrow open past the call.
        let gathered = {
            let client: Option<&mut dyn Forge> = match forge.as_mut() {
                Some(client) => Some(client),
                None => None,
            };
            HistoryInput::gather(
                &repo,
                input.as_of,
                input.config.budgets.blame_max_files,
                GENERIC_MESSAGES
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect(),
                TEST_PATHS.to_owned(),
                client,
                None,
                &tools_owned,
            )
            .await
        };
        drop(forge);
        match gathered {
            Ok(gathered) => {
                let pass = history_pass(&gathered, input.thresholds);
                (Some(gathered), Ok(pass))
            }
            Err(error) => (
                None,
                Err(format!("the repository could not be read: {error}")),
            ),
        }
    };

    // ── Statics. ────────────────────────────────────────────────────────────────────────────
    let statics_pass = if capped(started, cap) {
        Err(BUDGET_EXHAUSTED.to_owned())
    } else {
        let runner = ToolRunner::live(input.root, input.run_dir);
        match run_static_checks(&repo, input.rev, input.run_id, input.tools, &runner).await {
            Ok(outcome) => Ok(Pass {
                verdicts: outcome.verdicts,
                findings: outcome.findings,
                notes: outcome.notes,
                vintage_matches: outcome.vintage_matches,
                ..Pass::default()
            }),
            Err(error) => Err(error.to_string()),
        }
    };

    // ── Deps. ───────────────────────────────────────────────────────────────────────────────
    // The pass that spends requests. Its `known` and `added_at` are kept for provenance's V03,
    // which would otherwise fetch the same documents a second time for one date per dependency.
    let mut deps_http = input.http.clone();
    let deps_pass: Result<DepsOutcome, String> = if capped(started, cap) {
        Err(BUDGET_EXHAUSTED.to_owned())
    } else {
        let runner = ToolRunner::live(input.root, input.run_dir);
        run_dependency_checks(DepsInput {
            run_id: input.run_id,
            repo: &repo,
            rev: input.rev,
            as_of: input.as_of,
            tools: input.tools,
            runner: &runner,
            permit: input.permit,
            http: deps_http.as_mut(),
            dates_max_commits: 0,
            blame_max_files: input.config.budgets.blame_max_files,
            thresholds: input.thresholds,
        })
        .await
        .map_err(|error| error.to_string())
    };

    // ── Provenance. ─────────────────────────────────────────────────────────────────────────
    // Last, because it borrows both what history gathered and what deps fetched.
    let provenance_pass = if capped(started, cap) {
        Err(BUDGET_EXHAUSTED.to_owned())
    } else {
        let vintage = match &statics_pass {
            Ok(pass) => pass.vintage_matches.clone(),
            Err(_) => Vec::new(),
        };
        provenance_pass(history_input.as_ref(), &deps_pass, &vintage, input)
    };

    // ── Assemble. ───────────────────────────────────────────────────────────────────────────
    let mut judged: BTreeMap<CheckId, CheckVerdict> = BTreeMap::new();
    let mut findings: Vec<Finding> = Vec::new();
    let mut notes: Vec<(CheckId, Vec<String>)> = Vec::new();
    let mut notes_all: Vec<String> = Vec::new();
    let mut registry_requests = 0u32;
    // Which passes the wall-clock cap stopped. Tracked because the cap's named reason
    // (`budget exhausted`, §4.3) belongs on each of their checks, not on the passes that ran and
    // simply had no verdict for one.
    let mut capped_families: Vec<Family> = Vec::new();
    let mut archetype: Option<Archetype> = None;
    let mut provenance: Option<Provenance> = None;

    let passes: [(Family, &Result<Pass, String>); 4] = [
        (Family::History, &history_pass),
        (Family::Statics, &statics_pass),
        (Family::Deps, &deps_pass_as_pass(&deps_pass)),
        (Family::Provenance, &provenance_pass),
    ];
    for (family, result) in passes {
        match result {
            Ok(pass) => {
                registry_requests = registry_requests.saturating_add(pass.registry_requests);
                findings.extend(pass.findings.iter().cloned());
                notes.extend(pass.notes.iter().cloned());
                notes_all.extend(pass.notes_all.iter().cloned());
                if let Some(found) = pass.archetype {
                    archetype = Some(found);
                }
                if pass.provenance.is_some() {
                    provenance.clone_from(&pass.provenance);
                }
                for verdict in &pass.verdicts {
                    // A family may only answer for a check it owns: a verdict attributed to the
                    // wrong pass would be a table that disagrees with itself.
                    if family_of(verdict.check_id) == Some(family) {
                        judged.insert(verdict.check_id, verdict.clone());
                    }
                }
            }
            Err(reason) => {
                if reason == BUDGET_EXHAUSTED {
                    capped_families.push(family);
                }
                notes_all.push(format!(
                    "the {} pass did not run: {reason}",
                    family.as_str()
                ));
            }
        }
    }

    // Every check whose handler is a triage check must now have a verdict. One that does not is
    // `not_measured` naming its family: a check that never ran is not a check that found nothing.
    for meta in substance_core::REGISTRY {
        if !matches!(
            handler_for(meta.id),
            Some(crate::handlers::Handler::Triage(_))
        ) {
            continue;
        }
        if judged.contains_key(&meta.id) {
            continue;
        }
        let family = family_of(meta.id);
        let reason = family.map_or_else(
            || "no triage family judges this check".to_owned(),
            |family| {
                // §4.3: a check the time cap stopped says `budget exhausted`, so a reader can tell
                // "machine too slow" apart from "this pass had nothing for this check".
                if capped_families.contains(&family) {
                    BUDGET_EXHAUSTED.to_owned()
                } else {
                    format!(
                        "the {} pass returned no verdict for this check",
                        family.as_str()
                    )
                }
            },
        );
        judged.insert(
            meta.id,
            CheckVerdict {
                check_id: meta.id,
                target: Target::Repo,
                verdict: Verdict::NotMeasured {
                    reason: reason.clone(),
                },
                finding_ids: Vec::new(),
                metrics: BTreeMap::new(),
                parts: BTreeMap::new(),
            },
        );
        notes_all.push(format!("{}: {reason}", meta.id.as_str()));
    }

    // The project's own config wins over any pass.
    let disabled = &input.config.checks.disable;
    for verdict in judged.values_mut() {
        if disabled.contains(&verdict.check_id) {
            verdict.verdict = Verdict::NotMeasured {
                reason: "disabled by config".to_owned(),
            };
            verdict.finding_ids.clear();
        }
    }

    // Start from `pending` so a check that judges nothing (reserved, support, M2+) keeps its
    // absence, then overlay what triage judged.
    let mut verdicts: Vec<CheckVerdict> = pending
        .into_iter()
        .map(|row| judged.remove(&row.check_id).unwrap_or(row))
        .collect();
    for (_, row) in judged {
        verdicts.push(row);
    }
    verdicts
        .sort_by(|left, right| (left.check_id, &left.target).cmp(&(right.check_id, &right.target)));

    // A check the project turned off must not still contribute a finding.
    findings.retain(|finding| !disabled.contains(&finding.check_id));
    findings.sort_by(|left, right| {
        left.check_id
            .cmp(&right.check_id)
            .then_with(|| left.id.cmp(&right.id))
    });
    findings.dedup_by(|left, right| left.id == right.id);

    // Every finding is cited by its check's verdict (M13's scoring reads `finding_ids`), and the
    // list is built here so no pass can leave one uncited.
    let mut ids_by_check: BTreeMap<CheckId, Vec<substance_core::FindingId>> = BTreeMap::new();
    for finding in &findings {
        ids_by_check
            .entry(finding.check_id)
            .or_default()
            .push(finding.id.clone());
    }
    for verdict in &mut verdicts {
        if let Some(ids) = ids_by_check.get(&verdict.check_id) {
            verdict.finding_ids = ids.clone();
        }
    }

    // Notes belong only to checks the report lists, sorted and deduplicated.
    let listed: BTreeSet<CheckId> = verdicts.iter().map(|row| row.check_id).collect();
    notes.retain(|(check, _)| listed.contains(check));
    notes.sort_by_key(|(check, _)| *check);
    notes.dedup_by(|left, right| left.0 == right.0);
    for (_, lines) in &mut notes {
        lines.sort();
        lines.dedup();
    }

    let timed_out = capped(started, cap);
    if timed_out {
        notes_all.push(TIME_CAP_NOTE.to_owned());
    }
    notes_all.sort();
    notes_all.dedup();

    let measured = verdicts
        .iter()
        .filter(|row| row.verdict.is_measured())
        .count();
    // The denominator is every judging check in the catalog, not the rows this pass happened to
    // write: when the time cap truncates triage, `checks_total` must not shrink with it, or a run
    // that measured nothing would report `100%` of a smaller total (principle 8).
    let coverage = Coverage {
        checks_total: crate::verdicts::judging_checks(),
        checks_measured: u32::try_from(measured).unwrap_or(u32::MAX),
        routes: None,
        controls: None,
        server_code: None,
    };

    Ok(TriageOutcome {
        verdicts,
        findings,
        notes,
        archetype,
        provenance,
        coverage,
        notes_all,
        registry_requests,
        timed_out,
    })
}

/// Whether the cap has fired. A cap of zero would stop everything, which `Budgets::validate`
/// already refuses, so no special case is needed here.
fn capped(started: Instant, cap: Duration) -> bool {
    started.elapsed() >= cap
}

/// Runs the history family over an input that was already gathered.
fn history_pass(input: &HistoryInput, thresholds: &Thresholds) -> Pass {
    let mut pass = Pass::default();
    for report in judge_all(input, thresholds) {
        pass.notes.push((report.verdict.check_id, report.notes));
        pass.verdicts.push(report.verdict);
    }
    // H22's own verdict carries the archetype as an index metric; the report needs the enum, and
    // `classify_report` is the one function that produces both from the same rule set.
    let (archetype, _metrics) = classify_report(input, thresholds);
    pass.archetype = archetype;
    pass.verdicts.sort_by_key(|verdict| verdict.check_id);
    pass
}

/// The dependency pass's outcome, shaped as a family pass.
///
/// Separate from [`history_pass`] because `DepsOutcome` carries two things the report does not
/// take from a `Pass` — the registry facts and the adoption dates — and those go to provenance
/// through `deps_pass` directly rather than through this projection.
fn deps_to_pass(outcome: &DepsOutcome) -> Pass {
    Pass {
        verdicts: outcome.verdicts.clone(),
        findings: outcome.findings.clone(),
        notes: outcome.notes.clone(),
        registry_requests: outcome.registry_requests,
        notes_all: outcome.registry_notes.clone(),
        ..Pass::default()
    }
}

/// Runs the provenance family, borrowing what history and deps produced.
fn provenance_pass(
    history: Option<&HistoryInput>,
    deps: &Result<DepsOutcome, String>,
    vintage_matches: &[(String, u32, String)],
    input: &TriageInput<'_>,
) -> Result<Pass, String> {
    let Some(history) = history else {
        return Err(
            "the panel reads the history pass's commits and blame, which were not gathered"
                .to_owned(),
        );
    };
    // A deps failure leaves the registry facts empty, which V03 already reports as a missing
    // lower bound rather than as a pass: the honest answer is that the bound is unknown.
    let empty_known = Known::default();
    let empty_dates: BTreeMap<(substance_core::deps::Ecosystem, String), Timestamp> =
        BTreeMap::new();
    let (known, added_at) = match deps {
        Ok(outcome) => (&outcome.known, &outcome.added_at),
        Err(_) => (&empty_known, &empty_dates),
    };
    let matches = vintage_matches.to_vec();
    let context = ProvenanceContext {
        as_of: input.as_of,
        added_at,
        known,
        vintage_matches: &matches,
    };
    let outcome = run_provenance(history, &context);
    Ok(Pass {
        verdicts: outcome.verdicts,
        notes: outcome.notes,
        provenance: Some(outcome.panel),
        notes_all: vec!["provenance is reported and never scored (ADR-0012)".to_owned()],
        ..Pass::default()
    })
}

/// The forge client for the origin, when the origin is a GitHub repository.
///
/// `None` for anything else — a local path, a GitLab remote — and the history checks that need
/// the forge report the reason `HistoryInput::gather` records. The client is built over the same
/// borrowed [`SnapshotHttp`] as the registries, so one `forge_max_requests` budget covers both
/// and `--snapshot-from` replays both.
fn forge_for(origin: &str, http: Option<&SnapshotHttp>, as_of: Timestamp) -> Option<GitHub> {
    let repo = GitHubRepoRef::parse(origin).ok()?;
    // A clone is a second handle onto the same counter, not a second budget: `SnapshotHttp` keeps
    // its counter behind a shared atomic, so the registries and the forge spend one
    // `forge_max_requests` cap and `--snapshot-from` replays both.
    Some(GitHub::new(repo, http?.clone(), github_token(), as_of))
}

/// The token the forge client authenticates with, when the environment carries one.
///
/// Read from the environment rather than the project config, so no secret is ever written to a
/// file substance produces: a token in a config would end up hashed into the manifest. The value
/// is never logged, stored or reported — the client only uses it as a header.
fn github_token() -> Option<String> {
    match std::env::var("GITHUB_TOKEN") {
        Ok(token) if !token.trim().is_empty() => Some(token),
        _ => None,
    }
}

/// The dependency pass's outcome as a [`Pass`], for the assembly loop.
fn deps_pass_as_pass(outcome: &Result<DepsOutcome, String>) -> Result<Pass, String> {
    outcome.as_ref().map(deps_to_pass).map_err(Clone::clone)
}
