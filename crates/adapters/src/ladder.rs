//! The boot ladder (M2-T02, ADR-0017).
//!
//! Three rungs, tried in order, replacing the container ladder the architecture plan originally
//! had:
//!
//! 1. **README.** The shell blocks under install, setup, getting-started or run headings, run in
//!    order in a fresh working copy. This rung *measures O02 only*: it is what tells a reader
//!    whether the repository's own instructions work.
//! 2. **`substance.toml [app]`.** The repository's owner said how to run it, so this is the most
//!    trustworthy answer available.
//! 3. **Adapter-detected.** What the stack adapter works out from the files.
//!
//! ## Which rung a probe uses
//!
//! The plan's rule, kept as written: probes boot from **rung 2**, or **rung 3** when there is no
//! config. When only the README rung works, probes *use* it, and B28 reports `not_measured`,
//! because that rung may be running a dev server -- and a dev server has exactly the markers B28
//! looks for. [`BootOutcome::may_be_dev_server`] carries that fact to the report.
//!
//! The order rungs are *tried* and the order they are *preferred* are different questions, and
//! conflating them is how a ladder ends up booting a README's dev server while a perfectly good
//! config sits unread. [`order`] answers the first, [`preferred_rung`] the second.
//!
//! ## Why the README rung runs in a *fresh* copy
//!
//! Its commands are arbitrary shell from a document. Running them in the work copy a later rung
//! will use would let a failed README install leave a half-installed `node_modules` behind, and the
//! next rung would then fail for a reason that has nothing to do with it.
//!
//! [`boot`] enforces this rather than documenting it: when the caller supplies a
//! [`BootEnv::fresh_copy`], the README rung exports the same commit into `work-readme/` and every
//! step and the server of that rung run there. The copy is deleted when the rung is done with it,
//! unless it is the rung the probe booted from -- deleting the directory out from under a running
//! app would turn a working boot into a mysterious crash, so a successful README boot keeps its
//! copy and reports the path.
//!
//! Without a source to export from (a fixture directory in a probe), the rung runs in `work` and
//! says so in its friction. That is a degraded mode a caller can see, not a silent one.
//!
//! ## Extract, do not interpret
//!
//! The README rung reads fenced blocks tagged `sh`, `bash`, `shell` or untagged, under a heading
//! that names install, setup, getting-started or run. It does not expand variables, follow links or
//! run `npm` on the maintainer's behalf: the block becomes a script and `sh` runs it, in order. A
//! block that `cd`s somewhere still works, because the shell runs it; a block that needs an
//! interactive terminal fails, which is a fact about the instructions worth reporting.

use std::time::Duration;

use substance_core::config::ProjectConfig;
use substance_core::model::{BootOutcome, BootRung, CommandSpec, Friction};
use substance_runner::{AppProcess, Budget, ExecPermit, ProcessSpec, WorkCopy};

use crate::boot::{BootEnv, BootError, FreshCopy, prepare};
use crate::stack::{BootPlan, RepoView};
use crate::ts::shell_command;

/// The headings whose blocks are instructions for running the app.
///
/// Lowercased and matched by substring, because READMEs write `## Getting Started`, `# Setup` and
/// `## Run locally` and all three mean the same thing.
const HEADINGS: &[&str] = &[
    "install",
    "setup",
    "getting started",
    "getting-started",
    "run",
];

/// The block languages that count as shell.
const SHELL_LANGS: &[&str] = &["", "sh", "bash", "shell", "console", "zsh"];

/// How long a booted app gets to answer its ready path.
///
/// Separate from the step cap because a server that never listens is a different failure from an
/// install that never finishes, and the message should say which.
const READY_CAP: Duration = Duration::from_secs(180);

/// One shell block from a README, with the heading that introduced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadmeBlock {
    /// The heading text, as written.
    pub heading: String,
    /// The lines of the block, in order.
    pub lines: Vec<String>,
}

/// Extracts the runnable blocks from a README's text.
///
/// Returned in document order, which is the order they must run in: a README says `npm install`
/// before `npm run dev`, and reversing them would be a second, invented instruction.
#[must_use]
pub fn readme_blocks(text: &str) -> Vec<ReadmeBlock> {
    let mut blocks = Vec::new();
    let mut heading = String::new();
    let mut in_block = false;
    let mut block_lang = String::new();
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if in_block {
            if trimmed.starts_with("```") {
                in_block = false;
                if SHELL_LANGS.contains(&block_lang.as_str()) && interesting(&heading) {
                    let kept: Vec<String> = lines
                        .iter()
                        .filter(|line| !line.trim().is_empty())
                        .filter(|line| !line.trim_start().starts_with('#'))
                        .cloned()
                        .collect();
                    if !kept.is_empty() {
                        blocks.push(ReadmeBlock {
                            heading: heading.clone(),
                            lines: kept,
                        });
                    }
                }
                lines.clear();
                block_lang.clear();
            } else {
                lines.push(line.to_owned());
            }
            continue;
        }
        if trimmed.starts_with("```") {
            in_block = true;
            block_lang = trimmed.trim_start_matches('`').trim().to_lowercase();
            continue;
        }
        if trimmed.starts_with('#') {
            heading = trimmed.trim_start_matches('#').trim().to_owned();
        }
    }
    blocks
}

/// Whether a heading is one whose blocks are instructions for running the app.
fn interesting(heading: &str) -> bool {
    let lower = heading.to_lowercase();
    HEADINGS.iter().any(|word| lower.contains(word))
}

/// The README's blocks as one script, in order.
///
/// One script rather than one process per block, because a block may rely on the previous one: a
/// README that says `cd app` in one block and `npm install` in the next is describing one session.
/// `set -e` stops at the first failure, which is what a reader of O02 expects.
#[must_use]
pub fn readme_script(blocks: &[ReadmeBlock]) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }
    let mut script = String::from("set -e\n");
    for block in blocks {
        script.push_str("# ");
        script.push_str(&block.heading);
        script.push('\n');
        for line in &block.lines {
            script.push_str(line);
            script.push('\n');
        }
    }
    Some(script)
}

/// The README rung's plan: one script as the start command.
///
/// The script *is* the start command, because a README may end with `npm run dev` and never return.
/// That is also why this rung is the one that may be a dev server.
#[must_use]
pub fn readme_plan(repo: &RepoView) -> Option<BootPlan> {
    let mut text = String::new();
    for name in ["README.md", "readme.md", "README.markdown", "README"] {
        if let Some(found) = repo.read(name) {
            text = found;
            break;
        }
    }
    if text.is_empty() {
        return None;
    }
    let blocks = readme_blocks(&text);
    let script = readme_script(&blocks)?;
    Some(BootPlan {
        source: "readme".to_owned(),
        install: None,
        build: None,
        migrate: None,
        seed: None,
        start: Some(CommandSpec::new("sh", &["-c", &script])),
        env: std::collections::BTreeMap::new(),
        port_env: "PORT".to_owned(),
        ready_path: "/".to_owned(),
    })
}

/// The config rung's plan, when `substance.toml` says how to run the app.
///
/// A config with only some commands is still this rung: what it does not say is left out rather
/// than filled in from an adapter, so an owner who configured `start` alone gets exactly that and
/// no silent install.
#[must_use]
pub fn config_plan(cfg: &ProjectConfig) -> Option<BootPlan> {
    let start = cfg.app.start.as_ref()?;
    Some(BootPlan {
        source: "config".to_owned(),
        install: cfg.app.install.as_ref().map(|line| shell_command(line)),
        build: cfg.app.build.as_ref().map(|line| shell_command(line)),
        migrate: cfg.app.migrate.as_ref().map(|line| shell_command(line)),
        seed: cfg.app.seed.as_ref().map(|line| shell_command(line)),
        start: Some(shell_command(start)),
        env: cfg.app.env.clone(),
        port_env: cfg.app.port_env.clone(),
        ready_path: cfg.app.ready_path.clone(),
    })
}

/// The rungs a repository offers, in ladder order.
#[must_use]
pub fn rungs(repo: &RepoView, cfg: &ProjectConfig) -> Vec<(BootRung, BootPlan)> {
    let mut rungs = Vec::new();
    if let Some(plan) = readme_plan(repo) {
        rungs.push((BootRung::Readme, plan));
    }
    if let Some(plan) = config_plan(cfg) {
        rungs.push((BootRung::Config, plan));
    }
    if let Some((adapter, _)) = crate::ts::detect(repo) {
        let plan = adapter.boot_plan(repo, cfg);
        if plan.can_boot() {
            rungs.push((BootRung::Adapter, plan));
        }
    }
    rungs
}

/// The rung a probe boots from, per the plan's rule: rung 2, else rung 3.
///
/// The README rung is never *chosen*: it is used only when the preferred rungs fail, and the
/// outcome then says it may be a dev server.
#[must_use]
pub fn preferred_rung(repo: &RepoView, cfg: &ProjectConfig) -> Option<(BootRung, BootPlan)> {
    let rungs = rungs(repo, cfg);
    rungs
        .iter()
        .find(|(rung, _)| *rung == BootRung::Config)
        .or_else(|| rungs.iter().find(|(rung, _)| *rung == BootRung::Adapter))
        .cloned()
}

/// The order rungs are *tried* in: the preferred rung first, then the rest by rung number.
///
/// Trying rung 1 first would boot whatever a README says even when the owner configured a start
/// command, and once rung 1 is running there is nothing left to discover. So the preferred rung is
/// tried first and the README rung is the fallback -- which is the plan's rule read the way it is
/// written.
#[must_use]
fn order(repo: &RepoView, cfg: &ProjectConfig) -> Vec<(BootRung, BootPlan)> {
    let mut all = rungs(repo, cfg);
    let preferred = preferred_rung(repo, cfg).map(|(rung, _)| rung);
    all.sort_by_key(|(rung, _)| match preferred {
        Some(wanted) if *rung == wanted => 0,
        // Then the README rung last, because it is the fallback.
        _ if *rung == BootRung::Readme => 2,
        _ => 1,
    });
    all
}

/// The directory name the README rung's export lives under.
///
/// Next to `work/`, so a reader of a run directory sees both copies and which rung owns which.
pub const README_COPY: &str = "work-readme";

/// Boots a repository, walking the ladder until an app answers.
///
/// Every rung that fails contributes its friction entries to the outcome, and the rung that worked
/// is recorded. A rung whose *step* could not run at all (a missing binary, a timeout) is friction
/// too: the plan's "every failed rung is logged" does not distinguish a build that failed from an
/// `npm` that is not installed.
///
/// The README rung runs in its own export when `env.fresh_copy` is set, and that copy is removed
/// afterwards unless the README rung is the one that booted.
///
/// # Errors
///
/// Returns [`BootError`] only when no rung could even be attempted: a repository with no README
/// blocks, no config and no adapter has nothing for the ladder to do, which is a fact the caller
/// reports rather than an error to swallow.
pub async fn boot(
    repo: &RepoView,
    cfg: &ProjectConfig,
    env: &BootEnv<'_>,
    shutdown: Budget,
    permit: &ExecPermit,
) -> Result<BootOutcome, BootError> {
    let attempts = order(repo, cfg);
    if attempts.is_empty() {
        return Err(BootError::NoStart {
            plan: "no rung offers a start command".to_owned(),
        });
    }
    let mut friction: Vec<Friction> = Vec::new();
    let mut last_failure: Option<String> = None;
    for (rung, plan) in attempts {
        // The README rung gets its own export. `None` means this rung runs in the run's own work
        // copy; it is set only when a copy could not be made, and that is reported as friction.
        let export: Option<WorkCopy> = if rung == BootRung::Readme {
            match make_readme_copy(env, &mut friction, rung).await {
                Ok(copy) => copy,
                Err(failure) => {
                    last_failure = Some(failure);
                    continue;
                }
            }
        } else {
            None
        };
        let rung_env = match &export {
            Some(copy) => env.in_dir(copy.root()),
            None => env.in_dir(env.work),
        };
        let prepared = match prepare(&plan, rung, &rung_env, permit).await {
            Ok(prepared) => prepared,
            Err(error) => {
                // A step that could not run is friction, not a reason to give up: the next rung
                // may not need that program at all.
                friction.push(Friction {
                    rung,
                    step: "run".to_owned(),
                    command: plan.source.clone(),
                    code: None,
                    log_end: 0,
                    fix: None,
                    reason: Some(error.to_string()),
                });
                last_failure = Some(format!("{} rung: {error}", rung.as_str()));
                remove_copy(export.as_ref(), &mut friction, rung);
                continue;
            }
        };
        friction.extend(prepared.friction);
        if !prepared.ready_to_start {
            last_failure = Some(format!(
                "{} rung: {} failed",
                rung.as_str(),
                prepared.failed_step.unwrap_or_else(|| "a step".to_owned())
            ));
            remove_copy(export.as_ref(), &mut friction, rung);
            continue;
        }
        // The one-shot steps passed; now the server.
        match start_app(&plan, &rung_env, shutdown, permit).await {
            Ok(port) => {
                // A successful README boot keeps its copy: the app the caller is about to probe
                // is running out of it, and removing the directory under a live process would
                // turn a working boot into a crash with no explanation.
                return Ok(BootOutcome {
                    rung: Some(rung),
                    booted: true,
                    port: Some(port),
                    may_be_dev_server: rung.may_be_dev_server(),
                    friction,
                    failure: None,
                });
            }
            Err(reason) => {
                friction.push(Friction {
                    rung,
                    step: "start".to_owned(),
                    command: plan
                        .start
                        .as_ref()
                        .map(CommandSpec::display)
                        .unwrap_or_default(),
                    code: None,
                    log_end: 0,
                    fix: None,
                    reason: Some(reason.clone()),
                });
                last_failure = Some(format!("{} rung: {reason}", rung.as_str()));
                remove_copy(export.as_ref(), &mut friction, rung);
            }
        }
    }
    Ok(BootOutcome {
        rung: None,
        booted: false,
        port: None,
        may_be_dev_server: false,
        friction,
        failure: last_failure,
    })
}

/// Exports the README rung's own copy, or explains in friction why it could not.
///
/// `Ok(None)` is the degraded mode: no source to export from, so the rung runs in the run's work
/// copy, which is recorded as friction because a README install that dirties that tree can make a
/// later rung fail for an unrelated reason.
async fn make_readme_copy(
    env: &BootEnv<'_>,
    friction: &mut Vec<Friction>,
    rung: BootRung,
) -> Result<Option<WorkCopy>, String> {
    let Some(fresh) = env.fresh_copy else {
        friction.push(Friction {
            rung,
            step: "copy".to_owned(),
            command: README_COPY.to_owned(),
            code: None,
            log_end: 0,
            fix: None,
            reason: Some(
                "no source commit to export from, so the README rung runs in the work copy"
                    .to_owned(),
            ),
        });
        return Ok(None);
    };
    let FreshCopy {
        source,
        commit_sha,
        run_dir,
    } = *fresh;
    // The run id is not carried here: `WorkCopy` uses it only for the manifest name, and the
    // directory name is what distinguishes this copy.
    let run_id = substance_core::RunId::derive(&[commit_sha.as_bytes(), README_COPY.as_bytes()]);
    match WorkCopy::create_named(source, commit_sha, run_dir, &run_id, README_COPY).await {
        Ok(copy) => Ok(Some(copy)),
        Err(error) => Err(format!(
            "{} rung: cannot export a fresh copy: {error}",
            rung.as_str()
        )),
    }
}

/// Removes a rung's private copy, recording a failure to do so as friction.
///
/// A leftover copy is not worth failing a boot over: the run directory is deleted wholesale when
/// the run ends. It is worth *saying*, because a reader looking at disk usage should not have to
/// guess why `work-readme/` is still there.
fn remove_copy(copy: Option<&WorkCopy>, friction: &mut Vec<Friction>, rung: BootRung) {
    let Some(copy) = copy else {
        return;
    };
    if let Err(error) = copy.remove() {
        friction.push(Friction {
            rung,
            step: "cleanup".to_owned(),
            command: README_COPY.to_owned(),
            code: None,
            log_end: 0,
            fix: None,
            reason: Some(format!("cannot remove the rung's copy: {error}")),
        });
    }
}

/// Starts the plan's server and waits for it to answer.
///
/// The `AppProcess` is stopped before returning: this function's job is to prove the app boots and
/// to return its port, not to keep it running. A caller that needs it running (a probe) calls
/// [`start_app`] itself and holds the handle.
async fn start_app(
    plan: &BootPlan,
    env: &BootEnv<'_>,
    shutdown: Budget,
    permit: &ExecPermit,
) -> Result<u16, String> {
    let Some(start) = &plan.start else {
        return Err("the plan has no start command".to_owned());
    };
    let mut spec = ProcessSpec::new(&start.program, &[], env.work, "app")
        .with_port(&plan.port_env)
        .with_ready_path(&plan.ready_path)
        .with_home(env.home)
        .with_limits(env.limits);
    spec.args.clone_from(&start.args);
    for (key, value) in &plan.env {
        spec = spec.with_env(key, value);
    }
    let mut app = AppProcess::spawn(&spec, env.logs, shutdown, permit)
        .await
        .map_err(|error| error.to_string())?;
    let ready = app.wait_ready(Budget::after("runtime", READY_CAP)).await;
    let port = app.port().unwrap_or(0);
    let _ = app.stop(Duration::from_secs(5)).await;
    ready.map_err(|error| error.to_string())?;
    Ok(port)
}
