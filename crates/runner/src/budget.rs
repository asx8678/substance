//! Stage time budgets: safety caps, never a verdict input (M2-T01, §4.3).
//!
//! §4.3 separates two kinds of limit. A **count** limit (`blame_max_files`,
//! `crawl_max_actions`) decides what the checker sees, so it applies in sorted order and gives
//! the same answer on every machine. A **wall-time** limit does not: it fires when this machine
//! happens to be slow, so a run that hits one is no longer reproducible. The plan allows it
//! anyway, as a safety cap, on two conditions: the affected checks say `budget exhausted`, and
//! the report carries a note naming the stage.
//!
//! This type is that cap. It holds an [`Instant`] and nothing else, deliberately: an `Instant`
//! has no calendar meaning and must never reach a record or a report, which is why nothing here
//! is `Serialize`.

use std::time::{Duration, Instant};

/// A wall-clock deadline for one stage.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// The stage the cap belongs to, as §4.3's note spells it (`triage`, `runtime`, …).
    stage: &'static str,
    /// How long the cap was, as the caller asked for it.
    ///
    /// Kept because an error has to name the budget that ran out: `remaining()` is `None` once a
    /// cap is spent, so deriving a duration from it at that moment reports zero seconds and reads
    /// as "no budget at all" when the truth is "the whole budget was spent".
    limit: Duration,
    /// When the stage must stop starting work.
    deadline: Instant,
}

impl Budget {
    /// A budget that expires `limit` from now.
    #[must_use]
    pub fn after(stage: &'static str, limit: Duration) -> Self {
        Self {
            stage,
            limit,
            deadline: Instant::now() + limit,
        }
    }

    /// A budget of whole seconds, the unit `substance.toml` uses.
    #[must_use]
    pub fn from_secs(stage: &'static str, seconds: u32) -> Self {
        Self::after(stage, Duration::from_secs(u64::from(seconds)))
    }

    /// The stage the cap belongs to.
    #[must_use]
    pub const fn stage(&self) -> &'static str {
        self.stage
    }

    /// How long the cap was, in whole seconds, for an error message.
    #[must_use]
    pub const fn limit_secs(&self) -> u64 {
        self.limit.as_secs()
    }

    /// Whether the cap has fired.
    ///
    /// A budget of zero seconds is spent immediately, which is what a caller asks for when it
    /// wants the cap's behaviour without waiting for it.
    #[must_use]
    pub fn is_spent(&self) -> bool {
        Instant::now() >= self.deadline
    }

    /// How long is left, or `None` once the cap has fired.
    #[must_use]
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline.checked_duration_since(Instant::now())
    }

    /// The note §4.3 requires in the report when a cap fires.
    ///
    /// Spelled here rather than at each call site, because the sentence is the plan's promise to
    /// the reader and two spellings of it would be two claims.
    #[must_use]
    pub fn note(&self) -> String {
        format!(
            "time cap hit: {}; this result depends on machine speed",
            self.stage
        )
    }

    /// Sleeps until the cap fires, or returns at once when it already has.
    ///
    /// A caller waiting on a process uses this rather than a fixed sleep, so a slow machine
    /// spends its whole budget and a fast one does not.
    pub async fn sleep_until_spent(&self) {
        if let Some(left) = self.remaining() {
            tokio::time::sleep(left).await;
        }
    }
}
