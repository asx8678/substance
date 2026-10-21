//! Running a boot plan (M2-T02).
//!
//! A [`BootPlan`] is data; this module is the one place that turns it into processes. The order is
//! the plan's: install, build, migrate, seed, then start, and a failure at any step ends the
//! attempt with a [`Friction`] entry rather than a panic or a silent `false`.
//!
//! ## What a failed step does *not* do
//!
//! It does not try to "fix" the repository. Every fix the plan contemplates is a fact substance
//! changes about how it runs the app — a different command from a better rung — not an edit to the
//! repository. A run that edited its own subject would make the report describe code nobody has.
//! `Friction::fix` is therefore always `None` here, and a rung that fails is simply abandoned; the
//! ladder records which rung replaced it.
//!
//! ## Migration and seed are optional, install and build are not
//!
//! A repository with no migration tool has nothing to migrate, and a plan with no seed command has
//! nothing to seed; skipping those is normal. A plan with no *install* is different: it means the
//! dependency tree is whatever happens to be committed, which is a fact the caller should hear
//! about, so it is passed through as `None` and the boot proceeds. A plan with no start command
//! cannot boot at all, which is [`BootError::NoStart`].

use std::path::Path;
use std::time::Duration;

use substance_core::model::Friction;
use substance_runner::{Limits, OneShot, OneShotError, StepOutput};

use crate::stack::BootPlan;
use substance_core::model::CommandSpec;

/// How long one boot step may take.
///
/// Generous, because `npm ci` on a large tree on a cold cache is slow, and this is a safety cap
/// rather than a target (§4.3). The runner's `run_s` budget still bounds the whole run.
const STEP_CAP: Duration = Duration::from_secs(900);

/// Why a plan could not be run.
#[derive(Debug, thiserror::Error)]
pub enum BootError {
    /// The plan has no start command, so there is nothing to boot.
    ///
    /// The plan's name is quoted rather than put in a sentence: a plan with no name reads "the
    /// plan", which is the truth, and the ladder's "no rung at all" reason reads as itself.
    #[error("the plan needs a start command, and {plan}")]
    NoStart {
        /// Which plan it was.
        plan: String,
    },
    /// A step did not fail cleanly: it could not be run at all.
    #[error("{step}: {source}")]
    Step {
        /// Which step.
        step: String,
        /// Why it could not run.
        source: OneShotError,
    },
}

/// The result of running one plan's one-shot steps.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// The steps that failed, in order, as friction.
    pub friction: Vec<Friction>,
    /// Whether every step that ran succeeded.
    pub ready_to_start: bool,
    /// Which step failed first, when one did.
    pub failed_step: Option<String>,
}

/// Runs a plan's one-shot steps: install, build, migrate, seed.
///
/// Returns the friction entries for the steps that failed, and whether the start command should be
/// attempted. It stops at the first failure: a build after a failed install produces a second,
/// less informative error, and the first one is what a reader needs.
///
/// # Errors
///
/// Returns [`BootError::NoStart`] when there is nothing to boot, and [`BootError::Step`] when a step
/// could not be *run* (as opposed to running and failing).
pub async fn prepare(
    plan: &BootPlan,
    rung: substance_core::model::BootRung,
    env: &BootEnv<'_>,
    permit: &substance_runner::ExecPermit,
) -> Result<Prepared, BootError> {
    if !plan.can_boot() {
        return Err(BootError::NoStart {
            plan: plan.source.clone(),
        });
    }
    let mut friction = Vec::new();
    // A deferred seed is the caller's step, run after the app is up: running it here too would seed
    // the database twice, and a seed that is not idempotent would fail the second time.
    let seed = if env.defer_seed {
        None
    } else {
        plan.seed.as_ref()
    };
    let steps: [(&str, Option<&CommandSpec>); 4] = [
        ("install", plan.install.as_ref()),
        ("build", plan.build.as_ref()),
        ("migrate", plan.migrate.as_ref()),
        ("seed", seed),
    ];
    for (name, command) in steps {
        let Some(command) = command else {
            continue;
        };
        let step = to_step(command, plan, env, name);
        let output = substance_runner::oneshot::run(&step, env.logs, STEP_CAP, permit)
            .await
            .map_err(|source| BootError::Step {
                step: name.to_owned(),
                source,
            })?;
        if !output.success {
            friction.push(friction_of(rung, name, &step, &output));
            return Ok(Prepared {
                friction,
                ready_to_start: false,
                failed_step: Some(name.to_owned()),
            });
        }
    }
    Ok(Prepared {
        friction,
        ready_to_start: true,
        failed_step: None,
    })
}

/// Where a boot runs: the three directories and the limits, grouped.
///
/// One struct rather than four parameters, because every function on the boot path needs all of
/// them and a caller that mixed up `work` and `logs` would get a boot that installed into the log
/// directory.
#[derive(Debug, Clone)]
pub struct BootEnv<'a> {
    /// The work copy the commands run in.
    pub work: &'a Path,
    /// The run's home directory.
    pub home: &'a Path,
    /// The run's log directory.
    pub logs: &'a Path,
    /// How to limit repo-controlled commands on this host.
    pub limits: Limits,
    /// How to make a fresh export of the same commit, for a rung that must not touch `work`.
    ///
    /// The README rung (rung 1) runs arbitrary shell from a document -- `npm ci`, `bun install`,
    /// `cp .env.example .env` -- and any of it can leave the tree half-built. The ladder calls
    /// this to get an independent copy rather than reusing `work`, so a failed README install
    /// cannot make the next rung fail for a reason that has nothing to do with it.
    ///
    /// `None` means the caller cannot export (a probe over hand-written fixture directories, or a
    /// caller that has no source repository), and the README rung then runs in `work` and says so
    /// in its friction, because refusing to boot at all would be worse than a known-risky attempt.
    pub fresh_copy: Option<&'a FreshCopy<'a>>,
    /// Whether the caller seeds after the app is up, instead of during the boot.
    ///
    /// The runtime stage seeds the *live* database and then snapshots it, so a seed that ran inside
    /// the boot would run twice. A caller that seeds itself sets this and leaves `seed` out of the
    /// steps that run here; the plan still records the command, because the adapter's detection is
    /// what found it. Default `false`: a boot on its own seeds, which is what a probe wants.
    pub defer_seed: bool,
    /// Environment the run needs, on top of the plan's own: the database's URL, typically.
    ///
    /// Applied to every step and to the server, and merged *under* the plan's entries, so a plan
    /// that sets a variable deliberately (a fixed `PORT`, a `NODE_ENV=production`) is not overridden
    /// by an ambient default. Sorted by name because it is written into every step's environment,
    /// and a step's environment is part of what makes two runs comparable.
    pub env: &'a std::collections::BTreeMap<String, String>,
}

/// A source commit that can be exported again, as many times as the ladder needs.
///
/// Borrows the source repository's path and the commit rather than owning a `WorkCopy`, because
/// the ladder may need one copy, two, or none, and only it knows which rung runs where.
#[derive(Debug, Clone, Copy)]
pub struct FreshCopy<'a> {
    /// The repository the commit is in.
    pub source: &'a Path,
    /// The commit to export.
    pub commit_sha: &'a str,
    /// The run directory to export under.
    pub run_dir: &'a Path,
}

impl BootEnv<'_> {
    /// The directories a rung's steps run in: `work`, or a fresh export when one was asked for.
    ///
    /// A separate directory rather than a mutated `BootEnv`: the caller's environment stays the
    /// one the preferred rungs use, whichever rung happens to be running.
    #[must_use]
    pub fn in_dir<'a>(&'a self, work: &'a Path) -> BootEnv<'a> {
        BootEnv {
            work,
            home: self.home,
            logs: self.logs,
            limits: self.limits,
            fresh_copy: self.fresh_copy,
            defer_seed: self.defer_seed,
            env: self.env,
        }
    }
}

/// Turns a [`CommandSpec`] into a runner step.
fn to_step(command: &CommandSpec, plan: &BootPlan, env: &BootEnv<'_>, name: &str) -> OneShot {
    let mut step = OneShot::new(&command.program, &command.args, env.work, name)
        .with_home(env.home)
        .with_limits(env.limits);
    // The run's environment first, so a plan entry that sets the same variable deliberately wins.
    for (key, value) in env.env {
        step = step.with_env(key, value);
    }
    for (key, value) in &plan.env {
        step = step.with_env(key, value);
    }
    for (key, value) in &command.env {
        step = step.with_env(key, value);
    }
    step
}

/// One failed step as a friction entry.
fn friction_of(
    rung: substance_core::model::BootRung,
    name: &str,
    step: &OneShot,
    output: &StepOutput,
) -> Friction {
    Friction {
        rung,
        step: name.to_owned(),
        command: step.display(),
        code: output.code,
        log_end: output.log_end,
        fix: None,
        reason: None,
    }
}
