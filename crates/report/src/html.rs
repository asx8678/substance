//! `report.html`: one static, self-contained page (M1-T12).
//!
//! The plan's requirements shape this module:
//!
//! - **Self-contained.** The stylesheet is inline, there is no `<script>` and no absolute URL, so
//!   a report opens on a machine with no network and cannot phone home. The template is embedded
//!   with `include_str!`, so which page a build writes does not depend on the working directory.
//! - **Every value escaped.** minijinja's default autoescape is on for `.html` templates, so a
//!   repository's own text — a commit subject, a README line quoted in a reason — cannot inject
//!   markup. The only HTML in the output is the template's own.
//! - **No number without its coverage.** Every count is shown beside the total it came from, and a
//!   `not measured` row always carries its reason, because a report whose absences are invisible
//!   reads as a clean bill of health (principle 8).
//!
//! The view model is built here rather than handed to the template as the raw `Report`: a template
//! that can reach into arbitrary fields is a template that can print a metric nobody explained,
//! and the shaping — which evidence becomes a link, which verdicts group under which axis — is
//! logic that belongs in Rust where it is readable and reviewable.

use std::collections::BTreeMap;

use minijinja::{Environment, Error};
use substance_core::check::{Axis, CheckId};
use substance_core::model::{CheckVerdict, Evidence, Report, Verdict};
use substance_core::registry;

/// The embedded page template.
pub const TEMPLATE: &str = include_str!("../templates/report.html");

/// The template's name inside the environment.
const TEMPLATE_NAME: &str = "report.html";

/// Why a report could not be rendered.
#[derive(Debug, thiserror::Error)]
pub enum HtmlError {
    /// The embedded template is malformed, which is a build error.
    #[error("report.html template: {0}")]
    Template(String),
}

/// Renders `report` as one HTML page.
///
/// # Errors
///
/// Returns [`HtmlError::Template`] when the embedded template does not compile or a field it
/// reads is missing. Both are build errors: the template and the view model ship together.
pub fn render(report: &Report, calibrated: bool) -> Result<String, HtmlError> {
    let mut env = Environment::new();
    // minijinja's default callback already escapes HTML for a `.html` name; setting it explicitly
    // is what makes the guarantee independent of the file's extension.
    env.set_auto_escape_callback(|name| {
        if name.ends_with(".html") {
            minijinja::AutoEscape::Html
        } else {
            minijinja::AutoEscape::None
        }
    });
    env.add_template_owned(TEMPLATE_NAME, TEMPLATE)
        .map_err(|error| HtmlError::Template(error.to_string()))?;
    let template = env
        .get_template(TEMPLATE_NAME)
        .map_err(|error| HtmlError::Template(error.to_string()))?;
    let view = view(report, calibrated);
    // The view goes through minijinja's `Serde` wrapper: the template reads its context through
    // `serde`, so the field names are the ones the view's own derives produce.
    let context = minijinja::value::Value::from(minijinja::value::Serde(&view));
    template
        .render(context)
        .map_err(|error| HtmlError::Template(describe(&error)))
}

/// The error's message with its source chain, because minijinja's `Display` shows only the top.
fn describe(error: &Error) -> String {
    let mut text = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        text.push_str(": ");
        text.push_str(&inner.to_string());
        source = inner.source();
    }
    text
}

/// One verdict's counts, for the summary grid and the axis table.
#[derive(Debug, Default, Clone, Copy, serde::Serialize)]
pub struct Counts {
    /// Verdicts that passed.
    pub pass: u32,
    /// Verdicts that failed.
    pub fail: u32,
    /// Verdicts that reported without a threshold.
    pub reported: u32,
    /// Verdicts that could not look: `not_measured` and `unverifiable`.
    pub absent: u32,
}

impl Counts {
    /// Adds one verdict.
    fn add(&mut self, verdict: &Verdict) {
        match verdict {
            Verdict::Pass => self.pass = self.pass.saturating_add(1),
            Verdict::Fail => self.fail = self.fail.saturating_add(1),
            Verdict::Flaky | Verdict::Reported => {
                self.reported = self.reported.saturating_add(1);
            }
            Verdict::NotMeasured { .. } | Verdict::Unverifiable { .. } => {
                self.absent = self.absent.saturating_add(1);
            }
        }
    }
}

/// One axis row.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AxisRow {
    /// The axis name, as the catalog spells it.
    pub name: &'static str,
    /// How many of its checks passed.
    pub pass: u32,
    /// How many failed.
    pub fail: u32,
    /// How many reported.
    pub reported: u32,
    /// How many could not look.
    pub absent: u32,
}

/// One hard gate that fired.
#[derive(Debug, Clone, serde::Serialize)]
pub struct GateRow {
    /// The check whose failure gates the run.
    pub check_id: String,
    /// What it judged.
    pub target: String,
    /// The verdict it reached.
    pub verdict: String,
}

/// One finding, flattened for the table.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FindingRow {
    /// The check that produced it.
    pub check_id: String,
    /// Its severity.
    pub severity: String,
    /// Its confidence.
    pub confidence: String,
    /// Its one-line title.
    pub title: String,
    /// The finding's own file in the run directory.
    pub href: String,
    /// Where to look: relative paths into the run directory.
    pub evidence: Vec<EvidenceLink>,
}

/// One evidence item.
///
/// `href` is **only** set when the target is a file this run directory actually holds: a blob,
/// a record, or a finding. An item whose artifact lives in the repository or on a forge has no
/// file here, so it renders as text. A link that jumps nowhere (`#src/a.rs`) reads as a way to
/// see the evidence and is not one, which is worse than plainly saying where to look.
#[derive(Debug, Clone, serde::Serialize)]
pub struct EvidenceLink {
    /// The `href`, relative to the report's directory, when the run directory holds the target.
    pub href: Option<String>,
    /// What the item says.
    pub text: String,
}

/// One check that could not look.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AbsentRow {
    /// The check.
    pub check_id: String,
    /// Why it could not look.
    pub reason: String,
}

/// One named number, for a table whose order matters.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricRow {
    /// The metric's key.
    pub name: String,
    /// Its value, as text so a missing one reads `unknown`.
    pub value: String,
}

/// One tool and its version.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolRow {
    /// The tool's name.
    pub name: String,
    /// The version found, or the reason none was.
    pub version: String,
}

/// One provenance share, in both units the panel shows.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShareRow {
    /// The tool or model.
    pub name: String,
    /// Permille of blamed lines.
    pub permille: u32,
    /// The same share as a whole-number percentage.
    pub percent: u32,
}

/// The whole view model.
#[derive(Debug, serde::Serialize)]
pub struct View {
    /// The report itself, for the fields the template reads directly.
    pub report: ReportView,
    /// Verdict counts over every check.
    pub counts: Counts,
    /// One row per axis that any verdict feeds.
    pub axes: Vec<AxisRow>,
    /// The hard gates that fired.
    pub gates: Vec<GateRow>,
    /// The findings table.
    pub findings: Vec<FindingRow>,
    /// The checks that could not look, with reasons.
    pub absent: Vec<AbsentRow>,
    /// The archetype's metrics, when the archetype is present.
    pub archetype_metrics: Vec<MetricRow>,
    /// The declared tool shares, sorted by name.
    pub provenance_tools: Vec<ShareRow>,
    /// The declared model shares, sorted by name.
    pub provenance_models: Vec<ShareRow>,
    /// The undeclared share as a percentage.
    pub provenance_undeclared_percent: u32,
    /// The manifest's tools, sorted by name.
    pub tools: Vec<ToolRow>,
    /// Whether the thresholds are still the provisional ones (ADR-0016).
    pub uncalibrated: bool,
}

/// The `Report` shape, re-declared so the template cannot reach a field the view did not build.
///
/// Serde flattens it back to the report's own JSON field names, so the template reads
/// `report.manifest.commit_sha` exactly as `report.json` spells it.
#[derive(Debug, serde::Serialize)]
pub struct ReportView {
    /// The report schema version.
    pub schema_version: String,
    /// Every input the run pinned.
    pub manifest: substance_core::RunManifest,
    /// How much was reached.
    pub coverage: substance_core::model::report::Coverage,
    /// The archetype, when history ran.
    pub archetype: Option<String>,
    /// The provenance panel, when provenance ran.
    pub provenance: Option<substance_core::model::report::Provenance>,
    /// The report's notes.
    pub notes: Vec<String>,
}

/// Builds the view model from `report`.
#[must_use]
pub fn view(report: &Report, calibrated: bool) -> View {
    let mut counts = Counts::default();
    let mut per_axis: BTreeMap<usize, Counts> = BTreeMap::new();
    let mut gates: Vec<GateRow> = Vec::new();
    let mut absent: Vec<AbsentRow> = Vec::new();
    for row in &report.verdicts {
        counts.add(&row.verdict);
        let meta = registry::meta(row.check_id);
        for axis in meta.axes {
            per_axis
                .entry(axis_index(*axis))
                .or_default()
                .add(&row.verdict);
        }
        if matches!(row.verdict, Verdict::Fail) && meta.gate.gates(&row.parts) {
            gates.push(GateRow {
                check_id: row.check_id.as_str().to_owned(),
                target: target_text(row),
                verdict: "fail".to_owned(),
            });
        }
        if let Verdict::NotMeasured { reason } | Verdict::Unverifiable { reason } = &row.verdict {
            absent.push(AbsentRow {
                check_id: row.check_id.as_str().to_owned(),
                reason: reason.clone(),
            });
        }
    }
    let axes: Vec<AxisRow> = per_axis
        .into_iter()
        .map(|(index, counts)| AxisRow {
            name: AXES.get(index).copied().unwrap_or("unknown"),
            pass: counts.pass,
            fail: counts.fail,
            reported: counts.reported,
            absent: counts.absent,
        })
        .collect();

    let findings: Vec<FindingRow> = report
        .findings
        .iter()
        .map(|finding| FindingRow {
            check_id: finding.check_id.as_str().to_owned(),
            severity: format!("{:?}", finding.severity).to_lowercase(),
            confidence: format!("{:?}", finding.confidence).to_lowercase(),
            title: finding.title.clone(),
            href: format!("findings/{}.json", finding.id.as_str()),
            evidence: finding.evidence.iter().map(evidence_link).collect(),
        })
        .collect();

    let (provenance_tools, provenance_models, undeclared_percent) = match &report.provenance {
        Some(panel) => (
            shares(&panel.declared_tool_permille),
            shares(&panel.declared_model_permille),
            percent(panel.undeclared_permille),
        ),
        None => (Vec::new(), Vec::new(), 0),
    };

    View {
        report: ReportView {
            schema_version: report.schema_version.clone(),
            manifest: report.manifest.clone(),
            coverage: report.coverage.clone(),
            archetype: report.archetype.map(archetype_text),
            provenance: report.provenance.clone(),
            notes: report.notes.clone(),
        },
        counts,
        axes,
        gates,
        findings,
        absent,
        archetype_metrics: archetype_metrics(report),
        provenance_tools,
        provenance_models,
        provenance_undeclared_percent: undeclared_percent,
        tools: report
            .manifest
            .tools
            .iter()
            .map(|(name, tool)| ToolRow {
                name: name.clone(),
                version: tool.version.clone(),
            })
            .collect(),
        uncalibrated: !calibrated,
    }
}

/// The metrics the archetype was assigned from, as H22 recorded them.
///
/// H22's own doc says the report shows the metrics that triggered the archetype, so the table is
/// read from that verdict rather than recomputed: a second calculation here could disagree with
/// the one that decided the label, and a reader could not tell which to believe. An empty list
/// means H22 did not run, and the template then prints the archetype alone.
fn archetype_metrics(report: &Report) -> Vec<MetricRow> {
    let Some(row) = report
        .verdicts
        .iter()
        .find(|row| row.check_id == CheckId::H22)
    else {
        return Vec::new();
    };
    row.metrics
        .iter()
        .map(|(name, value)| MetricRow {
            name: name.clone(),
            value: value.to_string(),
        })
        .collect()
}

/// The axis names, in the catalog's order, for the summary table.
const AXES: &[&str] = &[
    "Works now",
    "Honest",
    "Robust",
    "Safe",
    "Data integrity",
    "Operable",
    "Maintainable",
    "Vitality",
    "Provenance (not scored)",
    "Generic",
];

/// The position of `axis` in [`AXES`].
fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::WorksNow => 0,
        Axis::Honest => 1,
        Axis::Robust => 2,
        Axis::Safe => 3,
        Axis::DataIntegrity => 4,
        Axis::Operable => 5,
        Axis::Maintainable => 6,
        Axis::Vitality => 7,
        Axis::Provenance => 8,
        Axis::Generic => 9,
    }
}

/// A verdict's target as text.
fn target_text(row: &CheckVerdict) -> String {
    format!("{:?}", row.target)
}

/// One evidence item as a link into the run directory.
fn evidence_link(evidence: &Evidence) -> EvidenceLink {
    match evidence {
        // A file range names a path in the repository under test, which is not in this run
        // directory: the reader has the repository, so the path is the pointer.
        Evidence::FileRange(range) => EvidenceLink {
            href: None,
            text: format!("{}:{}:{}", range.path, range.start_line, range.end_line),
        },
        Evidence::Commit(sha) => EvidenceLink {
            href: None,
            text: format!("commit {}", short(sha)),
        },
        Evidence::Blob(blob) => {
            // A blob reference is `sha256:<hex>`, and the run directory stores blobs by hex.
            let hex = blob
                .as_str()
                .trim_start_matches(substance_core::BlobRef::PREFIX);
            EvidenceLink {
                href: Some(format!("blobs/sha256/{hex}")),
                text: format!("blob {}", short(hex)),
            }
        }
        Evidence::Record(record) => EvidenceLink {
            href: Some(format!("records/{}.json", record.as_str())),
            text: format!("record {}", short(record.as_str())),
        },
        Evidence::ForgeObject(object) => EvidenceLink {
            href: None,
            text: format!("{} {}", object.kind, object.id),
        },
        Evidence::RegistryObject(object) => EvidenceLink {
            href: None,
            text: format!("{} {}@{}", object.registry, object.package, object.version),
        },
        Evidence::ToolOutput(tool) => {
            let hex = tool
                .blob
                .as_str()
                .trim_start_matches(substance_core::BlobRef::PREFIX);
            EvidenceLink {
                href: Some(format!("blobs/sha256/{hex}")),
                text: format!("{} output", tool.tool),
            }
        }
        Evidence::Note(text) => EvidenceLink {
            href: None,
            text: text.clone(),
        },
    }
}

/// The first eight characters of an identifier.
fn short(text: &str) -> String {
    text.chars().take(8).collect()
}

/// A permille map as rows, sorted by name.
fn shares(map: &BTreeMap<String, u32>) -> Vec<ShareRow> {
    map.iter()
        .map(|(name, permille)| ShareRow {
            name: name.clone(),
            permille: *permille,
            percent: percent(*permille),
        })
        .collect()
}

/// Permille as a whole-number percentage, rounding to nearest.
///
/// Integer arithmetic only (ADR-0009): `permille * 100 + 500` then integer division by 1000 is
/// round-half-up without a float.
fn percent(permille: u32) -> u32 {
    permille.saturating_mul(100).saturating_add(500) / 1000
}

/// The archetype as the page spells it.
fn archetype_text(archetype: substance_core::model::report::Archetype) -> String {
    match archetype {
        substance_core::model::report::Archetype::OneShotDump => "one-shot dump".to_owned(),
        substance_core::model::report::Archetype::BurstThenAbandoned => {
            "burst then abandoned".to_owned()
        }
        substance_core::model::report::Archetype::YoungAndIterating => {
            "young and iterating".to_owned()
        }
        substance_core::model::report::Archetype::SoloMaintained => "solo maintained".to_owned(),
        substance_core::model::report::Archetype::TeamMaintained => "team maintained".to_owned(),
        substance_core::model::report::Archetype::UnverifiableHistory => {
            "history unverifiable".to_owned()
        }
    }
}
