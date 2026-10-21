//! `substance doctor`: what this machine can and cannot support (M2-T06).
//!
//! ## What it is for
//!
//! Every check that reports `not_measured` names a reason, and the most common reasons are facts
//! about the host rather than about the repository under test: no `gitleaks` on `PATH`, no Postgres
//! admin URL, no Playwright browser, a Node too old for the lockfile's format. One command that lists
//! those facts turns "why is this not measured" from a debugging session into a lookup.
//!
//! ## It is informational, and it always exits 0
//!
//! Doctor is a report, not a gate: a machine with nothing installed is a machine for which doctor has
//! a great deal to say. Running it must never fail a build, so the only non-zero exit is a usage error
//! clap raises before this code runs. That is the opposite of the run path, where a missing tool is a
//! reason to stop -- and the difference is deliberate.
//!
//! ## Secrets
//!
//! Doctor reads `GITHUB_TOKEN` and `SUBSTANCE_PG_ADMIN_URL`, so its output is the single most likely
//! place for a credential to leak by accident. Two rules follow, and both are enforced by types rather
//! than by care:
//!
//! - A variable is reported as `present` or `absent`. [`EnvPresence`] has no field that could hold a
//!   value, so no code path here can print one.
//! - The Postgres URL is shown as [`RedactedUrl`], which keeps the scheme, host, port and database and
//!   drops the user and password. Errors from the driver are passed through `redact_text` for the same
//!   reason: a connection error often quotes the whole connection string.
//!
//! ## Read-only, and never installs anything
//!
//! Every probe here reads a file or asks a program for its version. Nothing is downloaded, nothing is
//! created on the server, and no Playwright browser is installed: a browser that is missing is a fact
//! to report, not a thing to fix behind the operator's back.

use std::path::{Path, PathBuf};

use serde::Serialize;
use substance_core::tools::ToolsFile;

use crate::cli::DoctorArgs;
use crate::error::CliError;
use crate::output::stdout_line;

/// The variable that decides whether the Postgres section is probed at all.
const ADMIN_URL_ENV: &str = "SUBSTANCE_PG_ADMIN_URL";

/// The variable whose presence -- never whose value -- is reported.
const GITHUB_TOKEN_ENV: &str = "GITHUB_TOKEN";

/// The whole report.
///
/// One struct with one section per question, rather than a flat map, because a reader of the text
/// output and a reader of the JSON should see the same grouping.
#[derive(Debug, Serialize)]
pub struct Report {
    /// The host facts that are not about any one tool.
    host: HostReport,
    /// Whether the run-time variables substance reads are set, and nothing about their values.
    environment: EnvironmentReport,
    /// The Postgres section, when an admin URL is set.
    postgres: PostgresReport,
    /// Every tool in `tools.toml`.
    tools: Vec<ToolReport>,
    /// Things a reader should know that are not a tool and not a verdict, sorted.
    notes: Vec<String>,
}

/// The host itself.
#[derive(Debug, Default, Serialize)]
pub struct HostReport {
    /// The operating system family, as `uname -s` would say it.
    os: String,
    /// The machine, as `uname -m` would say it.
    arch: String,
    /// `git --version`.
    git: Option<String>,
    /// `node --version`.
    node: Option<String>,
    /// `npm --version`.
    npm: Option<String>,
    /// Whether a Playwright browser build is present, and where it was found.
    playwright: PlaywrightReport,
    /// `sqldiff --version`, when the binary exists.
    sqldiff: Option<String>,
    /// `prlimit`, which limits a child on Linux (M2-T01).
    prlimit: bool,
    /// `libfaketime`, which only exists on Linux and only matters there (ADR-0011).
    libfaketime: LibfaketimeReport,
}

/// Whether the Playwright browser cache has a Chromium in it.
///
/// Playwright downloads browsers into a per-user cache whose location differs by OS, and
/// `PLAYWRIGHT_BROWSERS_PATH` overrides it. Doctor reads those directories; it does not run
/// `playwright install`, because a doctor that installs is a doctor that changes the patient.
#[derive(Debug, Default, Serialize)]
pub struct PlaywrightReport {
    /// Whether a `chromium-*` directory was found.
    present: bool,
    /// The directory that was searched, or the environment override, for a reader who has to fix it.
    searched: Vec<String>,
}

/// Whether libfaketime's shared object is installed.
#[derive(Debug, Default, Serialize)]
pub struct LibfaketimeReport {
    /// Whether `libfaketime.so.1` was found in a standard library directory.
    present: bool,
    /// Where it was found, when it was.
    path: Option<String>,
    /// Whether this host could use it at all. False on macOS, where it does not exist, and that is
    /// a fact about the platform rather than a gap in the install.
    applicable: bool,
}

/// One environment variable, as presence only.
///
/// Deliberately a bool and not an `Option<String>`: there is no field here that could hold a secret,
/// so no future edit can print one by accident.
#[derive(Debug, Default, Serialize)]
pub struct EnvironmentReport {
    /// Whether `GITHUB_TOKEN` is set and non-empty. The forge checks need it; without it they report
    /// `not_measured` rather than a lower-confidence answer.
    github_token_present: bool,
}

/// The Postgres section.
#[derive(Debug, Default, Serialize)]
pub struct PostgresReport {
    /// Whether `SUBSTANCE_PG_ADMIN_URL` is set. When it is not, every other field is empty and
    /// `reason` says why -- that is the state a reader most often sees.
    configured: bool,
    /// The server's address, credentials removed.
    server: Option<String>,
    /// Whether the server answered.
    reachable: bool,
    /// Why the section is empty or partial, when it is.
    reason: Option<String>,
    /// `wal_level`, when the server answered.
    wal_level: Option<String>,
    /// Which logical-decoding plugin answered a real slot attempt.
    logical_decoding: Option<String>,
    /// Whether `pg_stat_statements` is preloaded, so query counts are available.
    query_stats: bool,
    /// Whether the admin role may create databases.
    createdb: bool,
    /// Every capability that is missing, in the order the adapter reports them.
    problems: Vec<String>,
    /// Leftover `substance_%` databases on the server, names only, sorted.
    ///
    /// These are the residue a `SIGKILL` leaves: no `Drop`, no handler, no teardown. Each one may
    /// also hold a slot, which pins WAL, so their number is worth a look before a long run.
    leftover_databases: Vec<String>,
    /// Leftover replication slots on the server, names only, sorted.
    leftover_slots: Vec<String>,
}

/// One tool from `tools.toml`.
#[derive(Debug, Serialize)]
pub struct ToolReport {
    /// The name in `tools.toml`.
    name: String,
    /// The binary looked up.
    bin: String,
    /// Whether it was found.
    found: bool,
    /// The version it printed, or `missing`.
    version: String,
    /// The lowest version substance accepts.
    min_version: String,
    /// The version CI installs.
    pin: String,
    /// Whether the version meets `min_version`. False when the tool is missing.
    meets_floor: bool,
}

/// Runs `substance doctor`.
///
/// # Errors
///
/// Returns [`CliError::Internal`] only when the report cannot be written. A probe that fails is a
/// finding, never an error: doctor's whole job is to describe a broken host without stopping.
pub fn report(args: &DoctorArgs) -> Result<(), CliError> {
    let table = substance_orchestrator::tools::table().map_err(|error| {
        CliError::Internal(format!("the embedded tools table cannot be read: {error}"))
    })?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| CliError::Internal(format!("cannot start the async runtime: {error}")))?;
    let built = runtime.block_on(collect(&table));
    if args.json {
        let text = serde_json::to_string_pretty(&built)
            .map_err(|error| CliError::Internal(format!("cannot serialize the report: {error}")))?;
        return stdout_line(text.as_str());
    }
    for line in text_lines(&built) {
        stdout_line(line.as_str())?;
    }
    Ok(())
}

/// Collects every section.
async fn collect(table: &ToolsFile) -> Report {
    let mut notes: Vec<String> = Vec::new();
    let host = host_report(&mut notes).await;
    let environment = EnvironmentReport {
        github_token_present: present(GITHUB_TOKEN_ENV),
    };
    let postgres = postgres_report().await;
    let tools = tools_report(table, &mut notes).await;
    Report {
        host,
        environment,
        postgres,
        tools,
        notes,
    }
}

/// Whether an environment variable is set to something non-empty.
fn present(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| !value.trim().is_empty())
}

/// The host section.
async fn host_report(notes: &mut Vec<String>) -> HostReport {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut host = HostReport {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        git: version_of("git", &["--version"], &cwd).await,
        node: version_of("node", &["--version"], &cwd).await,
        npm: version_of("npm", &["--version"], &cwd).await,
        sqldiff: version_of("sqldiff", &["--version"], &cwd).await,
        prlimit: which("prlimit").is_some(),
        ..HostReport::default()
    };
    host.playwright = playwright_report();
    host.libfaketime = libfaketime_report();
    if !host.prlimit && cfg!(target_os = "linux") {
        notes.push(
            "prlimit is missing: repo-controlled commands run without a resource cap on Linux"
                .to_owned(),
        );
    }
    if host.libfaketime.applicable && !host.libfaketime.present {
        notes.push(
            "libfaketime is missing: time-travel checks (ADR-0011) cannot run on this host"
                .to_owned(),
        );
    }
    host
}

/// Runs a program for its version and keeps the first non-empty line.
async fn version_of(program: &str, args: &[&str], cwd: &Path) -> Option<String> {
    let owned: Vec<std::ffi::OsString> = args.iter().map(std::ffi::OsString::from).collect();
    let borrowed: Vec<&std::ffi::OsStr> = owned.iter().map(std::ffi::OsString::as_os_str).collect();
    let captured =
        substance_runner::tool::run_tool_capture_any(program, &borrowed, cwd, VERSION_TIMEOUT)
            .await
            .ok()?;
    first_line(&captured.stdout)
}

/// How long one version probe may take.
///
/// Short, because doctor talks to programs whose whole job is to print a line: a version command
/// that takes longer than this is one whose answer is not worth waiting for.
const VERSION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The first non-empty line of `text`, trimmed.
fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(str::to_owned)
}

/// Whether `program` is on `PATH`.
///
/// A directory walk rather than a spawn: \`prlimit\` with no arguments exits non-zero, and doctor
/// would have to interpret its exit code to learn whether it exists.
fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Where Playwright keeps its browsers, per OS and per override.
fn playwright_cache_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(override_dir) = std::env::var_os("PLAYWRIGHT_BROWSERS_PATH") {
        dirs.push(PathBuf::from(override_dir));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join("Library/Caches/ms-playwright"));
        dirs.push(home.join(".cache/ms-playwright"));
        dirs.push(home.join("AppData/Local/ms-playwright"));
    }
    dirs
}

/// Whether any cache directory holds a `chromium-*` build.
fn playwright_report() -> PlaywrightReport {
    let dirs = playwright_cache_dirs();
    let mut report = PlaywrightReport {
        present: false,
        searched: dirs.iter().map(|dir| dir.display().to_string()).collect(),
    };
    for dir in &dirs {
        if !dir.is_dir() {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name.to_string_lossy().starts_with("chromium-") {
                report.present = true;
                return report;
            }
        }
    }
    report
}

/// Whether `libfaketime.so.1` is installed.
///
/// Linux only, and the `applicable` flag says so: macOS has no libfaketime, so reporting it missing
/// there would be reporting a platform difference as a defect.
fn libfaketime_report() -> LibfaketimeReport {
    if !cfg!(target_os = "linux") {
        return LibfaketimeReport {
            present: false,
            path: None,
            applicable: false,
        };
    }
    for dir in [
        "/usr/lib/x86_64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/usr/lib64",
        "/usr/lib",
        "/usr/local/lib",
    ] {
        let candidate = Path::new(dir).join("faketime/libfaketime.so.1");
        if candidate.is_file() {
            return LibfaketimeReport {
                present: true,
                path: Some(candidate.display().to_string()),
                applicable: true,
            };
        }
        let plain = Path::new(dir).join("libfaketime.so.1");
        if plain.is_file() {
            return LibfaketimeReport {
                present: true,
                path: Some(plain.display().to_string()),
                applicable: true,
            };
        }
    }
    LibfaketimeReport {
        present: false,
        path: None,
        applicable: true,
    }
}

/// The Postgres section: nothing at all when the URL is unset.
async fn postgres_report() -> PostgresReport {
    if !present(ADMIN_URL_ENV) {
        return PostgresReport {
            configured: false,
            reason: Some(format!(
                "{ADMIN_URL_ENV} not set, so no server was contacted; every Postgres-backed check \
                 reports not_measured"
            )),
            ..PostgresReport::default()
        };
    }
    let run_id = substance_core::ids::RunId::derive(&[b"doctor"]);
    let adapter = match substance_adapters::db::postgres::PostgresAdapter::from_env(
        &run_id,
        "DATABASE_URL".to_owned(),
    ) {
        Ok(adapter) => adapter,
        Err(error) => {
            return PostgresReport {
                configured: true,
                reason: Some(error.to_string()),
                ..PostgresReport::default()
            };
        }
    };
    let server = Some(adapter.server().to_string());
    // The adapter's own preflight: the same code a run uses, so doctor cannot disagree with it. It
    // creates and drops one temporary slot, which is how a plugin is proven rather than assumed.
    match adapter.preflight_public().await {
        Ok(capabilities) => PostgresReport {
            configured: true,
            server,
            reachable: true,
            reason: None,
            wal_level: adapter.wal_level().await.ok().flatten(),
            logical_decoding: capabilities
                .logical_decoding
                .value()
                .map(|plugin| plugin.as_str().to_owned()),
            query_stats: capabilities.query_stats.value().is_some(),
            createdb: capabilities.createdb,
            problems: capabilities.problems.clone(),
            leftover_databases: leftover_databases(&adapter).await,
            leftover_slots: leftover_slots(&adapter).await,
        },
        Err(error) => PostgresReport {
            configured: true,
            server,
            reachable: false,
            reason: Some(error.to_string()),
            ..PostgresReport::default()
        },
    }
}

/// The leftover `substance_%` databases, names only, sorted.
async fn leftover_databases(
    adapter: &substance_adapters::db::postgres::PostgresAdapter,
) -> Vec<String> {
    // An unreadable catalog is not a leak: the probe that failed is the one that reports it, and a
    // list of leftovers that could not be read must not look like an empty one. The caller shows
    // this beside `reachable`, which is where the failure is visible.
    adapter.leftover_databases().await.unwrap_or_default()
}

/// The leftover replication slots, names only, sorted.
async fn leftover_slots(
    adapter: &substance_adapters::db::postgres::PostgresAdapter,
) -> Vec<String> {
    adapter.leftover_slots().await.unwrap_or_default()
}

/// Every tool in `tools.toml`, sorted by name.
async fn tools_report(table: &ToolsFile, notes: &mut Vec<String>) -> Vec<ToolReport> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let probes = substance_orchestrator::tools::probe_versions(table, &cwd).await;
    let mut rows: Vec<ToolReport> = Vec::new();
    for (name, spec) in &table.tools {
        let probe = probes.get(name);
        let found = probe.is_some_and(|probe| probe.found);
        let meets_floor = probe.is_some_and(|probe| probe.meets_floor);
        if found && !meets_floor {
            notes.push(format!(
                "{name} is {}, below the {} this build needs",
                probe.map(|probe| probe.version.clone()).unwrap_or_default(),
                spec.min_version
            ));
        }
        rows.push(ToolReport {
            name: name.clone(),
            bin: spec.bin.clone(),
            found,
            version: probe
                .map(|probe| probe.version.clone())
                .unwrap_or_else(|| substance_core::tools::MISSING_VERSION.to_owned()),
            min_version: spec.min_version.clone(),
            pin: spec.pin.clone(),
            meets_floor,
        });
    }
    rows
}

/// Renders the report as a table.
fn text_lines(report: &Report) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    lines.push("HOST".to_owned());
    let mut host: Vec<(&str, String)> = vec![
        ("os", report.host.os.clone()),
        ("arch", report.host.arch.clone()),
        ("git", or_missing(report.host.git.clone())),
        ("node", or_missing(report.host.node.clone())),
        ("npm", or_missing(report.host.npm.clone())),
        ("sqldiff", or_missing(report.host.sqldiff.clone())),
        ("prlimit", yes_no(report.host.prlimit)),
        (
            "playwright chromium",
            yes_no(report.host.playwright.present),
        ),
        (
            "libfaketime",
            if report.host.libfaketime.applicable {
                yes_no(report.host.libfaketime.present)
            } else {
                "not applicable".to_owned()
            },
        ),
        (
            "GITHUB_TOKEN",
            presence(report.environment.github_token_present),
        ),
    ];
    host.sort_by(|left, right| left.0.cmp(right.0));
    lines.extend(pair_lines(&host));
    if !report.host.playwright.present {
        lines.push(format!(
            "  searched: {}",
            report.host.playwright.searched.join(", ")
        ));
    }

    lines.push(String::new());
    lines.push("POSTGRES".to_owned());
    let postgres = &report.postgres;
    if !postgres.configured {
        lines.push(format!(
            "  {}",
            postgres
                .reason
                .clone()
                .unwrap_or_else(|| format!("{ADMIN_URL_ENV} not set"))
        ));
    } else {
        let mut rows: Vec<(&str, String)> = vec![
            (
                "server",
                postgres
                    .server
                    .clone()
                    .unwrap_or_else(|| "unknown".to_owned()),
            ),
            ("reachable", yes_no(postgres.reachable)),
            (
                "wal_level",
                if postgres.reachable {
                    or_missing(postgres.wal_level.clone())
                } else {
                    "unknown (server not reached)".to_owned()
                },
            ),
            (
                "logical decoding",
                match (&postgres.logical_decoding, postgres.reachable) {
                    (Some(plugin), _) => plugin.clone(),
                    // "none" would be a claim about the server; an unreachable server made no claim.
                    (None, true) => "none".to_owned(),
                    (None, false) => "unknown".to_owned(),
                },
            ),
            ("pg_stat_statements", yes_no(postgres.query_stats)),
            ("CREATEDB", yes_no(postgres.createdb)),
            (
                "leftover databases",
                count_or_names(&postgres.leftover_databases),
            ),
            ("leftover slots", count_or_names(&postgres.leftover_slots)),
        ];
        rows.sort_by(|left, right| left.0.cmp(right.0));
        lines.extend(pair_lines(&rows));
        if let Some(reason) = &postgres.reason {
            lines.push(format!("  error: {reason}"));
        }
        for problem in &postgres.problems {
            lines.push(format!("  problem: {problem}"));
        }
    }

    lines.push(String::new());
    lines.push("TOOLS".to_owned());
    let header = ["NAME", "FOUND", "VERSION", "MIN", "PIN"];
    let cells: Vec<[String; 5]> = report
        .tools
        .iter()
        .map(|tool| {
            [
                tool.name.clone(),
                yes_no(tool.found),
                tool.version.clone(),
                tool.min_version.clone(),
                tool.pin.clone(),
            ]
        })
        .collect();
    lines.extend(table_lines(&header, &cells));

    if !report.notes.is_empty() {
        lines.push(String::new());
        lines.push("NOTES".to_owned());
        for note in &report.notes {
            lines.push(format!("  {note}"));
        }
    }
    lines
}

/// `key: value` lines, with the keys already aligned by the caller's sort.
fn pair_lines(pairs: &[(&str, String)]) -> Vec<String> {
    let width = pairs
        .iter()
        .map(|(key, _)| key.chars().count())
        .max()
        .unwrap_or(0);
    pairs
        .iter()
        .map(|(key, value)| {
            let pad = width.saturating_sub(key.chars().count());
            format!("  {key}{} {value}", " ".repeat(pad))
        })
        .collect()
}

/// An aligned table with a header, the last column unpadded.
fn table_lines<const N: usize>(header: &[&str; N], cells: &[[String; N]]) -> Vec<String> {
    let mut widths: [usize; N] = header.map(|cell| cell.chars().count());
    for line in cells {
        for (width, cell) in widths.iter_mut().zip(line.iter()) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut lines: Vec<String> = Vec::with_capacity(cells.len() + 1);
    lines.push(render_row(&header.map(str::to_owned), &widths));
    for line in cells {
        lines.push(render_row(line, &widths));
    }
    lines
}

/// Pads one row to `widths`, leaving the last cell unpadded.
fn render_row<const N: usize>(cells: &[String; N], widths: &[usize; N]) -> String {
    let mut line = String::from("  ");
    for (index, cell) in cells.iter().enumerate() {
        if index + 1 == cells.len() {
            line.push_str(cell);
            continue;
        }
        line.push_str(cell);
        let padding = widths.get(index).copied().unwrap_or(0) + 2;
        for _ in cell.chars().count()..padding {
            line.push(' ');
        }
    }
    line
}

/// `yes`/`no`.
fn yes_no(value: bool) -> String {
    if value {
        "yes".to_owned()
    } else {
        "no".to_owned()
    }
}

/// `present`/`absent`, for a variable.
fn presence(value: bool) -> String {
    if value {
        "present".to_owned()
    } else {
        "absent".to_owned()
    }
}

/// A version or `missing`.
fn or_missing(value: Option<String>) -> String {
    value.unwrap_or_else(|| "missing".to_owned())
}

/// `none`, or the names when there are some.
fn count_or_names(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(", ")
    }
}
