//! Scoring plus the report.json, report.html and SARIF output.
//!
//! M0-T08 lands the run directory and `report.json` v0; scoring, the HTML report and
//! SARIF follow in M13, M1-T12 and M13-T02.

pub mod html;
pub mod run_dir;

pub use html::{HtmlError, render};
pub use run_dir::{RunDir, RunDirError};
