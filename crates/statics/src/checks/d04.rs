//! D04 Startup schema traps (M1-T09).
//!
//! Formula (action plan, D04):
//!
//! > Rules for: `prisma db push` in start, dev or postinstall scripts, a Procfile or a
//! > Dockerfile CMD; TypeORM `synchronize: true`; Sequelize `sync({ force: true })`; SQLAlchemy
//! > `drop_all(` at startup; `DROP TABLE` in startup code; a SQLite path under `/tmp`.
//!
//! Five of the six are syntax or manifest text, and the split is deliberate:
//!
//! - **Rules** (`rules/{ts,tsx,js}/schema-traps.yml`, `rules/python/schema-traps.yml`) hold the
//!   four that a parser can see: `synchronize: true`, `sync({ force: true })`, and
//!   `drop_all(`. They arrive here as matches from [`crate::scan`], already carrying the
//!   finding's evidence.
//! - **This module** holds the three that a parser cannot see, because each is a string in a
//!   file whose *meaning* depends on where it sits: a `db push` in a package.json script, a
//!   `CMD` line in a Dockerfile, a `web: …` line in a Procfile, a `DROP TABLE` in a file that
//!   runs at boot, and a SQLite filename under `/tmp`.
//!
//! The question in every case is the same one the catalog asks: does this run when the app
//! starts, and does it destroy data when it does? A migration that runs `DROP TABLE` on
//! purpose is not the trap; a boot hook that does is. The check therefore looks at the files
//! that *are* startup: manifest scripts, a Procfile, a Dockerfile, and the small set of file
//! names a server's entry point has (`main.py`, `server.ts`, `app.js`, `manage.py`, …).
//!
//! A SQLite path under `/tmp` is the quietest of the six: the database is recreated empty on
//! every boot and on many hosts `/tmp` is wiped, so the application "loses its data" in a way
//! no test catches.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::scan::RuleMatch;
use crate::tree::TrackedTree;

/// The scripts that run at install or startup, and so are worth reading for `db push`.
pub const STARTUP_SCRIPTS: &[&str] = &["start", "dev", "postinstall", "prestart", "predev"];

/// File names a server's entry point has, at any depth.
///
/// A `DROP TABLE` anywhere else is a migration, a test fixture or a fixture's teardown, and
/// accusing those would make the check useless. The list is deliberately short and concrete:
/// the file has to be one a framework boots.
pub const ENTRY_POINTS: &[&str] = &[
    "main.py",
    "app.py",
    "server.py",
    "manage.py",
    "wsgi.py",
    "asgi.py",
    "conftest.py",
    "main.ts",
    "server.ts",
    "index.ts",
    "main.js",
    "server.js",
    "index.js",
    "app.js",
    "main.rs",
];

/// Judges D04.
///
/// `matches` is the scan's own list: the rule half arrives already sorted and already turned
/// into evidence, so this only adds the three textual traps and counts.
#[must_use]
pub async fn startup_schema_traps(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    matches: &[RuleMatch],
) -> StaticReport {
    let mut findings: Vec<Finding> = Vec::new();
    let mut by_trap: BTreeMap<String, i64> = BTreeMap::new();

    // The rule half. Every match on a D04 rule is a finding: the rules were written narrowly
    // (see the `stopBy: end` note in rules/ts/schema-traps.yml) so that a match means the
    // trap, not a token that resembles it.
    for found in matches
        .iter()
        .filter(|found| found.check_id == CheckId::D04)
    {
        *by_trap.entry(found.sub_check.clone()).or_insert(0) += 1;
        findings.push(file_finding::at(
            run_id,
            CheckId::D04,
            &found.sub_check,
            &format!("{} in {}", found.message, found.path),
            Location::lines(&found.path, found.start_line, found.end_line),
        ));
    }

    findings.extend(manifest_scripts(run_id, tree, &mut by_trap).await);
    findings.extend(procfile(run_id, tree, &mut by_trap).await);
    findings.extend(dockerfile(run_id, tree, &mut by_trap).await);
    findings.extend(entry_point_drops(run_id, tree, &mut by_trap).await);
    findings.extend(sqlite_in_tmp(run_id, tree, &mut by_trap).await);

    let mut metrics: BTreeMap<String, i64> = by_trap.clone();
    let traps: i64 = by_trap.values().sum();
    metrics.insert("traps".to_owned(), traps);
    let (findings, cut) = cap(findings);
    let mut notes: Vec<String> = Vec::new();
    notes.push(
        "only startup files are read for `DROP TABLE`: elsewhere it is a migration's job, not \
         a trap"
            .to_owned(),
    );
    if tree
        .paths()
        .iter()
        .all(|path| !path.ends_with("package.json"))
    {
        notes.push(
            "no package.json at this commit, so no npm script could run `db push`".to_owned(),
        );
    }
    notes.extend(cut);

    // D04 has no threshold in the plan, so it reports: the traps are listed and a reader
    // decides whether this project's boot path destroys data (ADR-0013).
    StaticReport::reported(CheckId::D04, metrics)
        .with_findings(findings)
        .with_notes(notes)
}

/// `prisma db push` in a package.json script that runs at install or startup.
///
/// Only those scripts: `db push` in a `db:push` helper is a developer running it deliberately,
/// while in `postinstall` or `start` it runs on every deploy. The distinction is the whole
/// point of the catalog's wording.
async fn manifest_scripts(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    by_trap: &mut BTreeMap<String, i64>,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let paths: Vec<String> = tree
        .matching(|path| path.ends_with("package.json"))
        .into_iter()
        .map(str::to_owned)
        .collect();
    for path in paths {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        let parsed: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let Some(scripts) = parsed
            .get("scripts")
            .and_then(|scripts| scripts.as_object())
        else {
            continue;
        };
        for name in STARTUP_SCRIPTS {
            let Some(command) = scripts.get(*name).and_then(|value| value.as_str()) else {
                continue;
            };
            if !command.contains("prisma db push") {
                continue;
            }
            *by_trap.entry("prisma-db-push".to_owned()).or_insert(0) += 1;
            findings.push(file_finding::at(
                run_id,
                CheckId::D04,
                "prisma-db-push",
                &format!("`{name}` runs `prisma db push` before the app starts in {path}"),
                Location::file(&path),
            ));
        }
    }
    findings
}

/// A Procfile line that runs `prisma db push`.
async fn procfile(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    by_trap: &mut BTreeMap<String, i64>,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let paths: Vec<String> = tree
        .matching(|path| path == "Procfile" || path.ends_with("/Procfile"))
        .into_iter()
        .map(str::to_owned)
        .collect();
    for path in paths {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if !line.contains("prisma db push") {
                continue;
            }
            *by_trap.entry("prisma-db-push".to_owned()).or_insert(0) += 1;
            let line_no = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
            findings.push(file_finding::at(
                run_id,
                CheckId::D04,
                "prisma-db-push",
                &format!("the Procfile's process runs `prisma db push` in {path}"),
                Location::lines(&path, line_no, line_no),
            ));
        }
    }
    findings
}

/// A Dockerfile `CMD`/`ENTRYPOINT` that runs `prisma db push`.
async fn dockerfile(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    by_trap: &mut BTreeMap<String, i64>,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let paths: Vec<String> = tree
        .matching(|path| {
            let name = TrackedTree::file_name(path);
            name == "Dockerfile" || name.starts_with("Dockerfile.") || name.ends_with(".Dockerfile")
        })
        .into_iter()
        .map(str::to_owned)
        .collect();
    for path in paths {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim_start();
            if !(trimmed.starts_with("CMD ") || trimmed.starts_with("ENTRYPOINT ")) {
                continue;
            }
            if !trimmed.contains("prisma db push") {
                continue;
            }
            *by_trap.entry("prisma-db-push".to_owned()).or_insert(0) += 1;
            let line_no = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
            findings.push(file_finding::at(
                run_id,
                CheckId::D04,
                "prisma-db-push",
                &format!("the container starts with `prisma db push` in {path}"),
                Location::lines(&path, line_no, line_no),
            ));
        }
    }
    findings
}

/// `DROP TABLE` in a file that runs at boot.
async fn entry_point_drops(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    by_trap: &mut BTreeMap<String, i64>,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let paths: Vec<String> = tree
        .matching(|path| ENTRY_POINTS.contains(&TrackedTree::file_name(path)))
        .into_iter()
        .map(str::to_owned)
        .collect();
    for path in paths {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let upper = line.to_ascii_uppercase();
            if !upper.contains("DROP TABLE") {
                continue;
            }
            *by_trap
                .entry("drop-table-at-startup".to_owned())
                .or_insert(0) += 1;
            let line_no = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
            findings.push(file_finding::at(
                run_id,
                CheckId::D04,
                "drop-table-at-startup",
                &format!("`DROP TABLE` in {path}, which runs at boot"),
                Location::lines(&path, line_no, line_no),
            ));
        }
    }
    findings
}

/// A SQLite database file under `/tmp`.
///
/// The path can be in a connection string, a config file or code, so every text file this
/// build reads is examined: `/tmp/` followed by a `.db`/`.sqlite`/`.sqlite3` name. A URL that
/// merely mentions `/tmp` is not a match; the extension is what makes it a database.
async fn sqlite_in_tmp(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    by_trap: &mut BTreeMap<String, i64>,
) -> Vec<Finding> {
    let mut findings: Vec<Finding> = Vec::new();
    let candidates: Vec<String> = tree
        .paths()
        .into_iter()
        .filter(|path| {
            let ext = path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
            matches!(
                ext,
                "ts" | "tsx"
                    | "js"
                    | "jsx"
                    | "mjs"
                    | "cjs"
                    | "py"
                    | "rs"
                    | "toml"
                    | "yaml"
                    | "yml"
                    | "json"
                    | "env"
                    | "example"
            )
        })
        .map(str::to_owned)
        .collect();
    for path in candidates {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            if !line.contains("/tmp/") {
                continue;
            }
            if !sqlite_in(line) {
                continue;
            }
            *by_trap.entry("sqlite-in-tmp".to_owned()).or_insert(0) += 1;
            let line_no = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
            findings.push(file_finding::at(
                run_id,
                CheckId::D04,
                "sqlite-in-tmp",
                &format!("a SQLite database lives under /tmp in {path}"),
                Location::lines(&path, line_no, line_no),
            ));
            break;
        }
    }
    findings
}

/// Whether a line naming `/tmp/` also names a SQLite database file.
#[must_use]
pub fn sqlite_in(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    let Some(at) = lower.find("/tmp/") else {
        return false;
    };
    let rest = &lower[at..];
    if !(rest.contains(".db") || rest.contains(".sqlite")) {
        return false;
    }
    // `sqlite` must appear too, so that a temp file named `foo.db` in a log line is not a
    // database. The connection string's scheme is the reliable spelling (`sqlite:///tmp/x.db`).
    lower.contains("sqlite") || rest.contains(".sqlite") || rest.contains("database")
}

/// The check's finding count, for a caller that wants it without the findings.
#[must_use]
pub fn trap_count(report: &StaticReport) -> i64 {
    as_i64(report.findings.len())
}
