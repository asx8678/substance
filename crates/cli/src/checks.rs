//! `substance checks list`: the registry as a table or as JSON (M0-T06).

use serde::Serialize;
use substance_core::check::{CheckStatus, Confidence, Family, GateRule, Level, Milestone, Need};
use substance_core::registry::meta;
use substance_core::{Axis, CheckId, REGISTRY, Severity};

use crate::cli::{ChecksListArgs, StatusFilter};
use crate::error::CliError;
use crate::output::stdout_line;

/// One registry entry as shown to a caller.
///
/// Field order follows the plan's synopsis. `handler` is whether a stage in this build runs the
/// check: it comes from the orchestrator's own dispatch table (`handler_for`), not from the
/// registry's `status`, so a check the registry calls `implemented` with nothing to run it shows
/// `false` here. That difference is exactly what `C-IMPL` tests for (M1-T12, rule 6).
#[derive(Debug, Serialize)]
struct CheckRow {
    /// The check ID.
    id: CheckId,
    /// The catalog title.
    title: &'static str,
    /// The family, equal to the ID prefix.
    family: Family,
    /// The destructiveness level; absent for reserved IDs and I03.
    level: Option<Level>,
    /// The default severity.
    severity: Severity,
    /// The axes this check feeds.
    axes: Vec<Axis>,
    /// When a failure gates the run.
    gate: GateRule,
    /// What the check needs before it runs.
    needs: Vec<Need>,
    /// The milestone that delivers it.
    milestone: Milestone,
    /// How far it has been built.
    status: CheckStatus,
    /// The confidence a finding starts with.
    default_confidence: Confidence,
    /// Whether a stage in this build runs the check.
    handler: bool,
}

impl CheckRow {
    /// Builds the row for `id` from the registry.
    fn for_check(id: CheckId) -> Self {
        let entry = meta(id);
        Self {
            id: entry.id,
            title: entry.title,
            family: entry.family,
            level: entry.level,
            severity: entry.severity,
            axes: entry.axes.to_vec(),
            gate: entry.gate,
            needs: entry.needs.to_vec(),
            milestone: entry.milestone,
            status: entry.status,
            default_confidence: entry.default_confidence,
            handler: substance_orchestrator::has_handler(entry.id),
        }
    }

    /// Whether this row passes the `--status` filter.
    fn matches(&self, filter: Option<StatusFilter>) -> bool {
        match filter {
            None => true,
            Some(StatusFilter::Planned) => self.status == CheckStatus::Planned,
            Some(StatusFilter::Implemented) => self.status == CheckStatus::Implemented,
            Some(StatusFilter::Reserved) => self.status == CheckStatus::Reserved,
            Some(StatusFilter::Support) => self.status == CheckStatus::Support,
        }
    }
}

/// Runs `checks list`.
///
/// # Errors
///
/// Returns [`CliError::Internal`] when the JSON cannot be built or stdout fails for a
/// reason other than a closed pipe.
pub fn list(args: &ChecksListArgs) -> Result<(), CliError> {
    let rows: Vec<CheckRow> = REGISTRY
        .iter()
        .map(|entry| CheckRow::for_check(entry.id))
        .filter(|row| row.matches(args.status))
        .collect();
    if args.json {
        let text = serde_json::to_string_pretty(&rows).map_err(|error| {
            CliError::Internal(format!("cannot serialize the registry: {error}"))
        })?;
        return stdout_line(text.as_str());
    }
    for line in table_lines(&rows) {
        stdout_line(line.as_str())?;
    }
    Ok(())
}

/// Renders the rows as an aligned table: ID, status, severity, milestone, title.
fn table_lines(rows: &[CheckRow]) -> Vec<String> {
    let header = ["ID", "STATUS", "SEVERITY", "MILESTONE", "TITLE"];
    let cells: Vec<[String; 5]> = rows
        .iter()
        .map(|row| {
            [
                row.id.to_string(),
                text_of(&row.status),
                text_of(&row.severity),
                text_of(&row.milestone),
                row.title.to_owned(),
            ]
        })
        .collect();
    let mut widths = header.map(|cell| cell.chars().count());
    for line in &cells {
        for (width, cell) in widths.iter_mut().zip(line.iter()) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut lines = Vec::with_capacity(cells.len() + 1);
    lines.push(render(&header.map(str::to_owned), &widths));
    for line in &cells {
        lines.push(render(line, &widths));
    }
    lines
}

/// Pads one row to the column widths; the last column is not padded.
fn render(cells: &[String; 5], widths: &[usize; 5]) -> String {
    let mut line = String::new();
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

/// The spelling a value has in JSON, so the table and `--json` never disagree.
///
/// Falls back to the debug form when a value has no JSON text, which cannot happen
/// for the registry's enums but keeps this total without a panic.
fn text_of<T: Serialize + std::fmt::Debug>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(text)) => text,
        Ok(other) => other.to_string(),
        Err(_) => format!("{value:?}"),
    }
}
