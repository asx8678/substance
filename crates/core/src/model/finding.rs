//! Findings and their evidence.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::check::{CheckId, Confidence, Severity};
use crate::ids::{BlobRef, FindingId, RecordId};
use crate::model::verdict::Target;

/// One reproducible reason the app failed a check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Finding {
    /// Content-derived finding ID.
    pub id: FindingId,
    /// The check that produced the finding.
    pub check_id: CheckId,
    /// The sub-check, when the check has named parts.
    pub sub_check: Option<String>,
    /// How bad the finding is.
    pub severity: Severity,
    /// How much the finding can be trusted.
    pub confidence: Confidence,
    /// One-line summary.
    pub title: String,
    /// What the finding is about.
    pub target: Target,
    /// Reproducible artifacts behind it, sorted by kind and reference.
    pub evidence: Vec<Evidence>,
    /// The CLI command that replays it, for example `substance replay fnd_…`.
    pub replay: String,
    /// The static explanation, when one exists.
    pub cause: Option<Cause>,
    /// Findings that describe the same defect, sorted by ID.
    pub related: Vec<FindingId>,
}

/// A static explanation a finding points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Cause {
    /// The hint's check, for example an S check.
    pub check_id: CheckId,
    /// The hint's finding.
    pub finding_id: FindingId,
}

/// One reproducible artifact (principle 6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum Evidence {
    /// An effect record.
    Record(RecordId),
    /// A content-addressed blob: a trace, a DOM dump, a screenshot.
    Blob(BlobRef),
    /// A commit SHA.
    Commit(String),
    /// A line range in a file.
    FileRange(FileRange),
    /// An object on the forge: an issue, a pull request or a release.
    ForgeObject(ForgeObject),
    /// An object in a package registry.
    RegistryObject(RegistryObject),
    /// A tool's own output, as a blob.
    ToolOutput(ToolOutput),
    /// A sentence of explanation that stands alone.
    Note(String),
}

/// A line range in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct FileRange {
    /// Repo-relative path.
    pub path: String,
    /// First line, 1-based.
    pub start_line: u32,
    /// Last line, 1-based and inclusive.
    pub end_line: u32,
}

/// An issue, pull request or release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ForgeObject {
    /// `issue`, `pull_request` or `release`.
    pub kind: String,
    /// The forge's identifier.
    pub id: String,
    /// The URL.
    pub url: String,
}

/// A package-registry object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RegistryObject {
    /// `npm`, `pypi` or `crates`.
    pub registry: String,
    /// The package name.
    pub package: String,
    /// The version or range.
    pub version: String,
}

/// A tool's output, kept as a blob plus the command that produced it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ToolOutput {
    /// The tool, as named in `tools.toml`.
    pub tool: String,
    /// The exact command line.
    pub command: String,
    /// The captured output.
    pub blob: BlobRef,
}
