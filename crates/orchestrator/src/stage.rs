//! The pipeline stages, in the order a run visits them (M0-T08).
//!
//! M0-T08 lands intake and report; the stages between them arrive with their own
//! milestones. The enum is the plan's, so a later stage is added by filling a variant
//! rather than by inventing a new order.

/// One stage of the run pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Pin the code under test to a commit and fill the manifest.
    Intake,
    /// History, static and dependency checks; no app execution (M1).
    Triage,
    /// Install, build, migrate, seed and boot the app (M2).
    Runtime,
    /// Crawl the running app and build the app model (M4, M7).
    Explore,
    /// Infer or read the API spec (M7).
    Spec,
    /// Run the probes against the app (M4, M8).
    Probe,
    /// Turn records and measurements into verdicts and findings (M1, M13).
    Judge,
    /// Score, render and write the report.
    Report,
}

impl Stage {
    /// The stage as a lowercase word, for logs and the invocation record.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Intake => "intake",
            Self::Triage => "triage",
            Self::Runtime => "runtime",
            Self::Explore => "explore",
            Self::Spec => "spec",
            Self::Probe => "probe",
            Self::Judge => "judge",
            Self::Report => "report",
        }
    }
}
