//! `tools.toml`: the external tools substance drives, and their versions (M1-T01).
//!
//! The table is data, not code. Each entry says which binary to run, how to ask for its
//! version, the floor `substance doctor` enforces, the exact version CI installs, the
//! checks the tool serves, and two booleans: whether running it executes repo-controlled
//! code (so it needs an `ExecPermit`), and whether its output depends on online data (so
//! `--snapshot-from` replays it instead of running it).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::check::CheckId;

/// The version text recorded for a tool that is not on `PATH`.
pub const MISSING_VERSION: &str = "missing";

/// The placeholder a pin carries while no released version could be confirmed.
pub const PIN_TBD: &str = "TBD";

/// One external tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ToolSpec {
    /// The binary looked up on `PATH`.
    pub bin: String,
    /// The arguments that print the version, in order.
    pub version_args: Vec<String>,
    /// The lowest version `substance doctor` accepts, for example `"8.19.0"`.
    pub min_version: String,
    /// The exact version CI installs, or `"TBD"` while no release could be confirmed.
    pub pin: String,
    /// The checks this tool serves, sorted by ID.
    #[serde(default)]
    pub checks: Vec<CheckId>,
    /// Whether running the tool executes repo-controlled code or loads its config.
    pub exec_repo_code: bool,
    /// Whether the tool's output depends on online data, so a snapshot has to replay it.
    pub network: bool,
}

impl ToolSpec {
    /// The version arguments as the `&[&str]` callers pass on.
    #[must_use]
    pub fn version_arg_refs(&self) -> Vec<&str> {
        self.version_args.iter().map(String::as_str).collect()
    }

    /// Whether this tool's pin is still unconfirmed.
    #[must_use]
    pub fn pin_is_tbd(&self) -> bool {
        self.pin.eq_ignore_ascii_case(PIN_TBD)
    }
}

/// The whole `tools.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ToolsFile {
    /// One entry per tool, sorted by name.
    pub tools: BTreeMap<String, ToolSpec>,
}

impl ToolsFile {
    /// Parses `tools.toml`. Unknown keys and unknown check IDs are errors.
    ///
    /// Each entry's `checks` list is sorted and must not repeat an ID: a repeated ID
    /// would make "which tool serves this check" ambiguous.
    ///
    /// # Errors
    ///
    /// Returns [`ToolsError::Parse`] for malformed TOML or an unknown key, and
    /// [`ToolsError::DuplicateCheck`] for a repeated ID.
    pub fn from_toml_str(text: &str) -> Result<Self, ToolsError> {
        let mut parsed: Self = toml::from_str(text)?;
        for (name, spec) in &mut parsed.tools {
            spec.checks.sort_unstable();
            if let Some(pair) = spec.checks.windows(2).find(|pair| pair[0] == pair[1]) {
                return Err(ToolsError::DuplicateCheck {
                    tool: name.clone(),
                    check: pair[0],
                });
            }
        }
        Ok(parsed)
    }

    /// The spec for `name`, when the table lists it.
    #[must_use]
    pub fn spec(&self, name: &str) -> Option<&ToolSpec> {
        self.tools.get(name)
    }

    /// The tools that serve `check`, sorted by tool name.
    #[must_use]
    pub fn serving(&self, check: CheckId) -> Vec<&str> {
        self.tools
            .iter()
            .filter(|(_, spec)| spec.checks.contains(&check))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// The tools whose pins are still `TBD`, sorted by name.
    #[must_use]
    pub fn unpinned(&self) -> Vec<&str> {
        self.tools
            .iter()
            .filter(|(_, spec)| spec.pin_is_tbd())
            .map(|(name, _)| name.as_str())
            .collect()
    }
}

/// Why `tools.toml` could not be read.
#[derive(Debug, thiserror::Error)]
pub enum ToolsError {
    /// The document is not TOML, or does not match the shape.
    #[error("tools.toml: {0}")]
    Parse(#[from] toml::de::Error),
    /// One tool lists the same check twice.
    #[error("tools.toml: {tool} lists {check} twice")]
    DuplicateCheck {
        /// The tool entry.
        tool: String,
        /// The repeated check.
        check: CheckId,
    },
}

/// A release version with up to three numeric parts.
///
/// Only the shape a tool prints matters here: `v5.6.0`, `cargo-audit 0.22.2` and
/// `git version 2.54.0 (Apple Git-157)` all parse to the same three numbers, and a
/// pre-release suffix such as `-rc1` is ignored, because a floor is about the numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    /// The leading number.
    pub major: u64,
    /// The second number, 0 when absent.
    pub minor: u64,
    /// The third number, 0 when absent.
    pub patch: u64,
}

impl Version {
    /// Reads the first `N`, `N.N` or `N.N.N` in `text`.
    ///
    /// Returns `None` when the text holds no number, which is how a tool that printed
    /// something unexpected is reported rather than guessed at.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        let mut index = bytes.iter().position(u8::is_ascii_digit)?;
        let major = read_number(bytes, &mut index)?;
        let minor = read_part(bytes, &mut index);
        let patch = read_part(bytes, &mut index);
        Some(Self {
            major,
            minor: minor.unwrap_or(0),
            patch: patch.unwrap_or(0),
        })
    }

    /// Whether this version is at least `floor`.
    #[must_use]
    pub fn at_least(&self, floor: Self) -> bool {
        *self >= floor
    }
}

/// Reads the digits starting at `index`, advancing it past them.
fn read_number(bytes: &[u8], index: &mut usize) -> Option<u64> {
    let start = *index;
    let mut value: u64 = 0;
    while let Some(byte) = bytes.get(*index) {
        if !byte.is_ascii_digit() {
            break;
        }
        value = value
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'));
        *index += 1;
    }
    if *index == start { None } else { Some(value) }
}

/// Reads `.<digits>`, advancing `index` past it when present.
fn read_part(bytes: &[u8], index: &mut usize) -> Option<u64> {
    if bytes.get(*index) != Some(&b'.') {
        return None;
    }
    let mut next = *index + 1;
    match read_number(bytes, &mut next) {
        Some(value) => {
            *index = next;
            Some(value)
        }
        None => None,
    }
}

/// Whether `found` is at or above `floor`, both written as version text.
///
/// A version that cannot be parsed is not comparable, so it fails the floor: a check
/// resting on it then reports `not_measured` instead of assuming the tool is new enough.
#[must_use]
pub fn meets_floor(found: &str, floor: &str) -> bool {
    match (Version::parse(found), Version::parse(floor)) {
        (Some(found), Some(floor)) => found.at_least(floor),
        _ => false,
    }
}
