//! The tool table this build ships, and the versions found on `PATH` (M1-T01).
//!
//! `tools.toml` is embedded at compile time rather than read from the evaluated
//! repository: the table describes substance's own dependencies, and a repo under test
//! must not be able to redirect which binary substance runs. Embedding also means the
//! table a run used is the table its binary was built from, which is what the manifest's
//! tool versions are compared against.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use substance_core::BlobRef;
use substance_core::model::manifest::ToolVersion;
use substance_core::tools::{MISSING_VERSION, ToolSpec, ToolsFile, Version, meets_floor};
use substance_runner::tool::{ToolError, run_tool_capture_any};

/// The `tools.toml` this binary was built from.
pub const TOOLS_TOML: &str = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools.toml"));

/// How long one version probe may take.
const VERSION_TIMEOUT: Duration = Duration::from_secs(20);

/// What a version probe found for one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolProbe {
    /// The tool's name in `tools.toml`.
    pub name: String,
    /// Whether the binary was found.
    pub found: bool,
    /// The version text it printed, or [`MISSING_VERSION`].
    pub version: String,
    /// The version printed as numbers, when it could be parsed.
    pub parsed: Option<Version>,
    /// Whether the version meets the `min_version` floor.
    pub meets_floor: bool,
    /// The raw output, kept so the manifest's blob reference can name it.
    pub raw: String,
}

impl ToolProbe {
    /// The manifest entry: the version and a reference to its raw output.
    ///
    /// The reference is computed here rather than written here, because the run
    /// directory does not exist yet; `run` writes the blob once it does.
    #[must_use]
    pub fn to_manifest(&self) -> ToolVersion {
        ToolVersion {
            tool: self.name.clone(),
            version: self.version.clone(),
            output: Some(BlobRef::from_bytes(self.raw.as_bytes())),
        }
    }
}

/// The tool table this build ships.
///
/// # Errors
///
/// Returns the parse error when the embedded table is malformed, which a release build
/// would have caught in CI.
pub fn table() -> Result<ToolsFile, substance_core::ToolsError> {
    ToolsFile::from_toml_str(TOOLS_TOML)
}

/// Probes every tool in `table`, sorted by name.
///
/// A missing binary is recorded, not returned as an error: a run without `gitleaks`
/// still reports its other checks, and the ones that needed it say why they did not run.
///
/// The probe runs `<bin> <version_args>` and keeps the first line of output. A version
/// flag executes nothing from the evaluated repository, so no `ExecPermit` is needed
/// here, even for tools marked `exec_repo_code`.
pub async fn probe_versions(table: &ToolsFile, cwd: &Path) -> BTreeMap<String, ToolProbe> {
    let mut probes = BTreeMap::new();
    for (name, spec) in &table.tools {
        probes.insert(name.clone(), probe_one(name, spec, cwd).await);
    }
    probes
}

/// Probes one tool.
async fn probe_one(name: &str, spec: &ToolSpec, cwd: &Path) -> ToolProbe {
    let owned: Vec<OsString> = spec.version_args.iter().map(OsString::from).collect();
    let args: Vec<&std::ffi::OsStr> = owned.iter().map(OsString::as_os_str).collect();
    match run_tool_capture_any(&spec.bin, &args, cwd, VERSION_TIMEOUT).await {
        Ok(captured) => {
            let raw = captured.stdout.clone();
            let version = first_line(&raw);
            let parsed = Version::parse(&version);
            ToolProbe {
                name: name.to_owned(),
                found: !version.is_empty(),
                meets_floor: meets_floor(&version, &spec.min_version),
                version: if version.is_empty() {
                    MISSING_VERSION.to_owned()
                } else {
                    version
                },
                parsed,
                raw,
            }
        }
        Err(ToolError::Missing { .. }) => ToolProbe {
            name: name.to_owned(),
            found: false,
            version: MISSING_VERSION.to_owned(),
            parsed: None,
            meets_floor: false,
            raw: String::new(),
        },
        Err(error) => ToolProbe {
            name: name.to_owned(),
            found: false,
            version: format!("unreadable: {error}"),
            parsed: None,
            meets_floor: false,
            raw: error.to_string(),
        },
    }
}

/// The first non-empty line of `text`, trimmed.
fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default()
        .to_owned()
}

/// The manifest's tool map from the probes, sorted by tool name.
#[must_use]
pub fn manifest_tools(probes: &BTreeMap<String, ToolProbe>) -> BTreeMap<String, ToolVersion> {
    probes
        .iter()
        .map(|(name, probe)| (name.clone(), probe.to_manifest()))
        .collect()
}

/// The reason a check cannot run because of a missing or stale tool, if any.
///
/// `checks` is the check's tool list from the table, already filtered to the tools the
/// check cannot do without. The first missing tool wins, then the first stale one, so the
/// reason is stable across runs.
#[must_use]
pub fn tool_reason(probes: &BTreeMap<String, ToolProbe>, tools: &[&str]) -> Option<String> {
    if let Some(name) = tools
        .iter()
        .find(|name| probes.get(**name).is_none_or(|probe| !probe.found))
    {
        return Some(format!("tool missing: {name}"));
    }
    tools
        .iter()
        .find(|name| probes.get(**name).is_some_and(|probe| !probe.meets_floor))
        .map(|name| format!("tool too old: {name}"))
}
