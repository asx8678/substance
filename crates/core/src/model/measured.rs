//! A channel value that can say it was not measured (ADR-0014).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The value a channel produced, or why it produced none.
///
/// Every channel of an [`crate::model::EffectRecord`] is wrapped in this, so
/// "nothing happened" stays distinguishable from "nothing was watched".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Measured<T> {
    /// The channel was observed and holds `value`.
    Measured {
        /// What the channel observed.
        value: T,
    },
    /// The channel could not be observed; `reason` says why.
    NotMeasured {
        /// Why the channel is missing, for example `"sqlite" `.
        reason: String,
    },
}

impl<T> Measured<T> {
    /// Wraps an observed value.
    #[must_use]
    pub fn observed(value: T) -> Self {
        Self::Measured { value }
    }

    /// Records that the channel could not be observed.
    #[must_use]
    pub fn not_measured(reason: impl Into<String>) -> Self {
        Self::NotMeasured {
            reason: reason.into(),
        }
    }

    /// The observed value, when there is one.
    #[must_use]
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Measured { value } => Some(value),
            Self::NotMeasured { .. } => None,
        }
    }

    /// Why the channel is missing, when it is.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Measured { .. } => None,
            Self::NotMeasured { reason } => Some(reason),
        }
    }

    /// Whether the channel was observed.
    #[must_use]
    pub fn is_measured(&self) -> bool {
        matches!(self, Self::Measured { .. })
    }
}
