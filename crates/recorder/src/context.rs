//! `open_context`, built from the project's config (M3-T05).
//!
//! ## Why this lives here and not in the caller
//!
//! `open_context` carries the normalization rules, and Rust is their source of truth: the recorder
//! normalizes bodies the browser never saw, so a list assembled anywhere else would drift. Every
//! caller that opened a context by hand would be a place to forget a rule, and forgetting one is
//! invisible -- two runs simply hash differently.
//!
//! ## The idle baseline travels with it
//!
//! `idle_patterns` is what the settle counter ignores. It comes from a previous `observe_idle` on the
//! same screen, so a context opened before the baseline has taken one passes an empty list and
//! excludes nothing; that is the honest state, not an error.

use substance_core::Timestamp;
use substance_core::config::ProjectConfig;
use substance_core::model::{NormalizePattern, OpenContext, ToastContainer};
use substance_core::normalize::Normalizer;

use crate::driver::DriverError;

/// The signals the browser is told about, from `rules/signals.toml`.
const SIGNALS_TOML: &str = include_str!("../../../rules/signals.toml");

/// The viewport and locale every run uses (M3-T06 fixes the clock separately).
const WIDTH: u32 = 1440;
const HEIGHT: u32 = 900;

/// What a caller knows about the context it wants.
#[derive(Debug, Clone)]
pub struct ContextPlan {
    /// Where bodies are written, as the driver's `blob_dir` (`<run>/blobs`).
    pub blob_dir: String,
    /// A saved storage state, when a session already exists.
    pub storage_state: Option<String>,
    /// The idle baseline's patterns for the screen the context starts on.
    pub idle_patterns: Vec<String>,
    /// The instant the browser clock starts at, from the manifest's `frozen_clock`.
    ///
    /// A field rather than a default: the browser and the server must start at the same instant, and
    /// a context opened without saying when would install a clock at whatever the driver's own
    /// `Date.now()` returned, which is the wall clock this task exists to stop using.
    pub as_of: Timestamp,
}

/// Builds the `open_context` command for `config`.
///
/// # Errors
///
/// Returns [`DriverError::Protocol`] when the project's own `[normalize]` patterns are outside the
/// subset Rust and JavaScript share. Refusing here is deliberate: a pattern that only one engine
/// applies would make records that differ for a reason no diff shows.
pub fn open_context(
    config: &ProjectConfig,
    plan: &ContextPlan,
) -> Result<OpenContext, DriverError> {
    let normalizer = Normalizer::with_project(&config.normalize.patterns).map_err(|error| {
        DriverError::Protocol {
            detail: error.to_string(),
        }
    })?;
    let signals = signals()?;
    Ok(OpenContext {
        width: WIDTH,
        height: HEIGHT,
        // M3-T06: the same constants the manifest and the app's environment use, so a record's
        // `locale`/`timezone` and the browser's are one value rather than three that agree by luck.
        locale: substance_core::clock_controls::BROWSER_LOCALE.to_owned(),
        tz: substance_core::clock_controls::BROWSER_TIMEZONE.to_owned(),
        as_of: plan.as_of,
        storage_state: plan.storage_state.clone(),
        normalize: normalizer.patterns(),
        blob_dir: plan.blob_dir.clone(),
        success_selectors: config.signals.success.clone(),
        error_selectors: config.signals.error.clone(),
        idle_patterns: plan.idle_patterns.clone(),
        toast_containers: signals,
        settle_quiet_ms: config.budgets.settle_quiet_ms,
        settle_timeout_ms: config.budgets.settle_timeout_ms,
    })
}

/// The rule list alone, for a caller that has a `Normalizer` already.
///
/// Kept separate so a replay or a probe can compare the rules the driver was given with the ones Rust
/// applies, without building a whole context.
#[must_use]
pub fn patterns(normalizer: &Normalizer) -> Vec<NormalizePattern> {
    normalizer.patterns()
}

/// The toast containers from `rules/signals.toml`.
///
/// # Errors
///
/// Returns [`DriverError::Protocol`] when the embedded file does not parse, which is a bug in the
/// repository rather than a runtime condition.
fn signals() -> Result<Vec<ToastContainer>, DriverError> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        toast: Vec<ToastContainer>,
    }
    let file: File = toml::from_str(SIGNALS_TOML).map_err(|error| DriverError::Protocol {
        detail: format!("rules/signals.toml: {error}"),
    })?;
    Ok(file.toast)
}
