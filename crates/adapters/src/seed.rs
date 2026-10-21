//! Finding the repository's seed command (M2-T04).
//!
//! The plan's order is: the config's `app.seed`, otherwise a detected one -- `prisma db seed`,
//! `npm run seed`, a `supabase/seed.sql`, or `manage.py loaddata`. Everything here is pure over a
//! [`RepoView`], so a probe can ask what a fixture would do without spawning anything.
//!
//! ## Why detection is a short list and not a search
//!
//! A seed command is repo-controlled code that writes to the run's database. Running the wrong one
//! -- a `db:reset` that drops tables, or a script that calls an external API -- would make the run
//! describe an app nobody asked about. Each detector therefore looks for a specific, named
//! convention and says which file it came from, and the caller records that as evidence. A
//! repository with a seed script nobody's convention covers gets `None`, and the stage records
//! "no seed command" rather than guessing.
//!
//! ## Order matters and is fixed
//!
//! Four conventions, checked in this order: prisma, then an npm `seed` script, then
//! `supabase/seed.sql`, then Django fixtures. A Next.js app with a Prisma schema and an npm `seed`
//! script usually has the npm script calling Prisma; running both would seed twice, and the first
//! is the more specific signal.

use substance_core::config::ProjectConfig;
use substance_core::model::CommandSpec;

use crate::stack::RepoView;
use crate::ts::shell_command;

/// A seed command, and the evidence that it is the right one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedSeed {
    /// The command to run.
    pub command: CommandSpec,
    /// The repo-relative file that decided it, for the run's notes.
    pub evidence: String,
}

/// The seed command for this repository, from the config first and detection second.
///
/// `package_manager` is the manager the boot plan will use, so the detected command runs the same
/// way the rest of the build does.
#[must_use]
pub fn seed_command(
    repo: &RepoView,
    cfg: &ProjectConfig,
    package_manager: &str,
) -> Option<DetectedSeed> {
    if let Some(line) = &cfg.app.seed {
        return Some(DetectedSeed {
            command: shell_command(line),
            evidence: "substance.toml [app] seed".to_owned(),
        });
    }
    detect(repo, package_manager)
}

/// The detected seed command, ignoring the config.
#[must_use]
pub fn detect(repo: &RepoView, package_manager: &str) -> Option<DetectedSeed> {
    if let Some(found) = prisma(repo, package_manager) {
        return Some(found);
    }
    if let Some(found) = npm_script(repo, package_manager) {
        return Some(found);
    }
    if let Some(found) = supabase_sql(repo) {
        return Some(found);
    }
    django(repo)
}

/// `prisma db seed`, when the repository has a Prisma schema.
///
/// The `prisma` CLI reads the `seed` key from `package.json` itself, so the command is the same for
/// every Prisma project and the schema is the evidence.
fn prisma(repo: &RepoView, package_manager: &str) -> Option<DetectedSeed> {
    let schema = ["prisma/schema.prisma", "schema.prisma"]
        .into_iter()
        .find(|path| repo.exists(path))?;
    let args = ["exec", "--no", "--", "prisma", "db", "seed"];
    let command = match package_manager {
        "pnpm" => CommandSpec::new("pnpm", &["exec", "prisma", "db", "seed"]),
        "yarn" => CommandSpec::new("yarn", &["prisma", "db", "seed"]),
        "bun" => CommandSpec::new("bun", &["x", "--no-install", "prisma", "db", "seed"]),
        _ => CommandSpec::new("npm", &args),
    };
    Some(DetectedSeed {
        command,
        evidence: schema.to_owned(),
    })
}

/// A `seed` script in `package.json`, run through the package manager.
fn npm_script(repo: &RepoView, package_manager: &str) -> Option<DetectedSeed> {
    let package = repo.package_json()?;
    package.has_script("seed").then_some(())?;
    let command = match package_manager {
        "pnpm" => CommandSpec::new("pnpm", &["run", "seed"]),
        "yarn" => CommandSpec::new("yarn", &["run", "seed"]),
        "bun" => CommandSpec::new("bun", &["run", "seed"]),
        _ => CommandSpec::new("npm", &["run", "seed"]),
    };
    Some(DetectedSeed {
        command,
        evidence: "package.json scripts.seed".to_owned(),
    })
}

/// `supabase/seed.sql`, applied by `supabase db reset`.
///
/// There is no separate seed command: the Supabase CLI runs `seed.sql` as part of a reset, so this
/// reports the file as evidence and the command that applies it. A run that has already started the
/// stack has seeded it, which is why the adapter's `db reset` is the command and not `psql`.
fn supabase_sql(repo: &RepoView) -> Option<DetectedSeed> {
    let seed = ["supabase/seed.sql", "supabase/seeds/seed.sql"]
        .into_iter()
        .find(|path| repo.exists(path))?;
    Some(DetectedSeed {
        command: CommandSpec::new("supabase", &["db", "reset"]),
        evidence: seed.to_owned(),
    })
}

/// `manage.py loaddata`, when the repository has Django fixtures.
///
/// Django names the fixture files, so this reports the command only when there is at least one
/// `.json` under a `fixtures/` directory, and passes the stem of the first one (sorted, so two runs
/// of the same tree ask for the same fixture).
fn django(repo: &RepoView) -> Option<DetectedSeed> {
    if !repo.exists("manage.py") {
        return None;
    }
    // Walk one level of each app directory: `app/fixtures/x.json` is the ordinary layout.
    let mut fixtures: Vec<(String, String)> = Vec::new();
    for entry in repo.list("") {
        let Some(app) = entry.strip_suffix('/') else {
            continue;
        };
        for file in repo.list(&format!("{app}/fixtures")) {
            if let Some(stem) = file.strip_suffix(".json") {
                fixtures.push((app.to_owned(), stem.to_owned()));
            }
        }
    }
    fixtures.sort();
    let (app, stem) = fixtures.first()?;
    Some(DetectedSeed {
        command: CommandSpec::new("python", &["manage.py", "loaddata", stem]),
        evidence: format!("{app}/fixtures/{stem}.json"),
    })
}
