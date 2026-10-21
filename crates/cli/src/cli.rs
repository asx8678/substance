//! The command line surface (M0-T06).
//!
//! Every subcommand in the plan's synopsis parses here, with typed values rather
//! than free-form strings: the format list, the failure threshold, the check status
//! filter, the schema kinds, seeds, run counts and RFC 3339 instants.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use substance_core::Timestamp;

/// A deterministic, evidence-backed evaluator for applications.
///
/// Substance boots an app, drives it, records what every action changed, and judges
/// the records with checks whose verdicts replay without a model. The same commit,
/// seeds, clock and tool versions always produce the same report.
#[derive(Debug, Parser)]
#[command(
    name = "substance",
    version,
    about = "Deterministic, evidence-backed evaluator for applications",
    long_about = "Substance boots an app, drives it, records what every action changed, and \
                  judges those records with checks that replay without a model.\n\n\
                  The same commit, seeds, clock, timezone, locale and tool versions always \
                  produce the same report; a channel that could not be observed reports \
                  not_measured rather than pass.",
    subcommand_required = true,
    arg_required_else_help = true,
    propagate_version = true
)]
pub struct Cli {
    /// The subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// What to do.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Evaluate a repository end to end.
    Run(RunArgs),
    /// Re-run one probe from a stored effect record or finding.
    Replay(ReplayArgs),
    /// Run the defect-injection benchmark over the base apps.
    Bench(BenchArgs),
    /// Re-render or re-judge the stored data of a finished run.
    Report(ReportArgs),
    /// Work with the JSON schemas and the driver's generated types.
    Schema {
        /// The schema subcommand.
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Inspect the check registry.
    Checks {
        /// The checks subcommand.
        #[command(subcommand)]
        command: ChecksCommand,
    },
    /// Report what this machine and this repository can support.
    Doctor(DoctorArgs),
}

/// `substance run <REPO>`.
#[derive(Debug, Args)]
pub struct RunArgs {
    /// The repository to evaluate: a local directory or a git URL (repo access only).
    #[arg(value_name = "REPO", value_hint = clap::ValueHint::DirPath)]
    pub repo: PathBuf,
    /// The revision to evaluate. Defaults to the default branch head, resolved to a SHA.
    #[arg(long, value_name = "sha")]
    pub rev: Option<String>,
    /// Where the run directory is written.
    #[arg(long, value_name = "dir", value_hint = clap::ValueHint::DirPath)]
    pub out: Option<PathBuf>,
    /// The instant the run pretends it is, as RFC 3339. Defaults to now.
    #[arg(long, value_name = "rfc3339", value_parser = parse_timestamp)]
    pub as_of: Option<Timestamp>,
    /// The master seed. Defaults to a value derived from the commit and config.
    #[arg(long, value_name = "u64")]
    pub seed: Option<u64>,
    /// The project config to use instead of `<repo>/substance.toml`.
    #[arg(long, value_name = "path", value_hint = clap::ValueHint::FilePath)]
    pub config: Option<PathBuf>,
    /// Stop after triage: history, static and dependency checks, no sandbox.
    #[arg(long)]
    pub triage_only: bool,
    /// Allow running repo-controlled code (install, build, tests).
    #[arg(long)]
    pub allow_exec: bool,
    /// Probe against the snapshots of an existing run directory instead of booting again.
    #[arg(long, value_name = "run_dir", value_hint = clap::ValueHint::DirPath)]
    pub snapshot_from: Option<PathBuf>,
    /// The outputs to write. Defaults to `json`.
    #[arg(
        long,
        value_name = "json,html,sarif",
        value_delimiter = ',',
        value_enum,
        default_value = "json"
    )]
    pub format: Vec<Format>,
    /// When to exit 1. Defaults to `gate`, the hard gates of ADR-0002.
    #[arg(long, value_name = "gate|critical|high|none", value_enum, default_value_t = FailOn::Gate)]
    pub fail_on: FailOn,
}

/// `substance replay <RECORD_OR_FINDING_ID> --run <dir>`.
#[derive(Debug, Args)]
pub struct ReplayArgs {
    /// The effect record or finding to replay.
    #[arg(value_name = "RECORD_OR_FINDING_ID")]
    pub id: String,
    /// The run directory the record belongs to.
    #[arg(long, value_name = "dir", value_hint = clap::ValueHint::DirPath)]
    pub run: PathBuf,
}

/// `substance bench`.
#[derive(Debug, Args)]
pub struct BenchArgs {
    /// The benchmark suite directory. Defaults to `bench/`.
    #[arg(long, value_name = "dir", value_hint = clap::ValueHint::DirPath)]
    pub suite: Option<PathBuf>,
    /// How many runs per variant. Defaults to 5, the spread metric's sample size.
    #[arg(long, value_name = "n")]
    pub runs: Option<u32>,
    /// The baseline results to compare against.
    #[arg(long, value_name = "file", value_hint = clap::ValueHint::FilePath)]
    pub baseline: Option<PathBuf>,
}

/// `substance report <RUN_DIR>`.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// The run directory to re-render.
    #[arg(value_name = "RUN_DIR", value_hint = clap::ValueHint::DirPath)]
    pub run_dir: PathBuf,
    /// The outputs to write. Defaults to `json`.
    #[arg(
        long,
        value_name = "json,html,sarif",
        value_delimiter = ',',
        value_enum,
        default_value = "json"
    )]
    pub format: Vec<Format>,
}

/// `substance schema <export|validate>`.
#[derive(Debug, Subcommand)]
pub enum SchemaCommand {
    /// Write the JSON schemas and the driver's TypeScript types.
    Export(SchemaExportArgs),
    /// Validate a document against one schema.
    Validate(SchemaValidateArgs),
}

/// `substance schema export --out <dir>`.
#[derive(Debug, Args)]
pub struct SchemaExportArgs {
    /// Where the JSON schemas are written. The driver's types always go to
    /// `driver/src/generated/`.
    #[arg(long, value_name = "dir", value_hint = clap::ValueHint::DirPath)]
    pub out: PathBuf,
}

/// `substance schema validate --kind <kind> <file>`.
#[derive(Debug, Args)]
pub struct SchemaValidateArgs {
    /// Which schema to validate against.
    #[arg(long, value_name = "kind", value_enum)]
    pub kind: SchemaKind,
    /// The document to validate: JSON, or TOML for the config kind.
    #[arg(value_name = "file", value_hint = clap::ValueHint::FilePath)]
    pub file: PathBuf,
}

/// `substance checks <list>`.
#[derive(Debug, Subcommand)]
pub enum ChecksCommand {
    /// List the checks in the registry.
    List(ChecksListArgs),
}

/// `substance checks list [--json] [--status <status>]`.
#[derive(Debug, Args)]
pub struct ChecksListArgs {
    /// Print a JSON array instead of a table.
    #[arg(long)]
    pub json: bool,
    /// Only checks in this state.
    #[arg(long, value_name = "planned|implemented|reserved|support", value_enum)]
    pub status: Option<StatusFilter>,
}

/// `substance doctor [--json]`.
#[derive(Debug, Args)]
pub struct DoctorArgs {
    /// Print JSON instead of text.
    #[arg(long)]
    pub json: bool,
}

/// An output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Format {
    /// `report.json`.
    Json,
    /// `report.html`, static and self-contained.
    Html,
    /// `report.sarif`, for code scanning.
    Sarif,
}

/// What makes a run exit 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FailOn {
    /// Exit 1 when a hard gate fails (ADR-0002, the default).
    Gate,
    /// Exit 1 on any critical finding.
    Critical,
    /// Exit 1 on any high or critical finding.
    High,
    /// Never exit 1; findings still appear in the report.
    None,
}

/// The check states `checks list --status` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StatusFilter {
    /// Registered; its task has not landed.
    Planned,
    /// Registered and delivered.
    Implemented,
    /// The ID is reserved for later; it never runs.
    Reserved,
    /// A stage that supports other checks rather than judging.
    Support,
}

/// The documents `schema validate --kind` understands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SchemaKind {
    /// One effect record.
    #[value(name = "effect-record")]
    EffectRecord,
    /// One finding.
    Finding,
    /// A run manifest.
    Manifest,
    /// A report.
    Report,
    /// A `substance.toml`.
    Config,
    /// One benchmark injection.
    Injection,
}

/// Parses an RFC 3339 timestamp into the core's [`Timestamp`].
fn parse_timestamp(text: &str) -> Result<Timestamp, String> {
    Timestamp::parse_rfc3339(text)
        .map_err(|error| format!("{text:?} is not an RFC 3339 timestamp: {error}"))
}
