//! The only wall-clock access in the workspace (ADR-0009).

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// An instant, truncated to whole seconds and always UTC.
///
/// Serialized as RFC 3339 with a `Z` suffix, which keeps records byte-identical
/// between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    /// Truncates `instant` to whole seconds and normalizes it to UTC.
    #[must_use]
    pub fn new(instant: OffsetDateTime) -> Self {
        Self(instant.truncate_to_second().to_offset(time::UtcOffset::UTC))
    }

    /// Returns the instant.
    #[must_use]
    pub fn as_offset_date_time(&self) -> OffsetDateTime {
        self.0
    }

    /// Returns the instant as RFC 3339 in UTC.
    #[must_use]
    pub fn to_rfc3339(&self) -> String {
        self.0.format(&Rfc3339).unwrap_or_default()
    }

    /// Parses an RFC 3339 timestamp, normalizing it to UTC and whole seconds.
    pub fn parse_rfc3339(text: &str) -> Result<Self, time::error::Parse> {
        let instant = OffsetDateTime::parse(text, &Rfc3339)?;
        Ok(Self::new(instant))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_rfc3339())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse_rfc3339(&text).map_err(D::Error::custom)
    }
}

/// Reads the wall clock. This is the workspace's only wall-clock access.
#[must_use]
pub fn wall_now_utc() -> Timestamp {
    Timestamp::new(OffsetDateTime::now_utc())
}
