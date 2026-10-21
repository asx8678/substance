//! The only wall-clock access in the workspace (ADR-0009).

use schemars::JsonSchema;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use ts_rs::TS;

/// An instant, truncated to whole seconds and always UTC.
///
/// Serialized as RFC 3339 with a `Z` suffix, which keeps records byte-identical
/// between runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, TS)]
#[ts(type = "string")]
pub struct Timestamp(OffsetDateTime);

impl JsonSchema for Timestamp {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("Timestamp")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "format": "date-time",
        })
    }
}

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

/// The start of a "last `days` days" window that ends at `as_of`.
///
/// Every relative date in the report is computed this way, from the manifest's `as_of`
/// rather than from the wall clock, so two runs of one instant measure one window. A
/// window longer than the calendar can represent starts at the earliest timestamp instead
/// of failing: the checks that use a long window treat the whole history as in scope, and
/// an out-of-range subtraction is a calendar limit, not a finding.
#[must_use]
pub fn window_start(as_of: Timestamp, days: u32) -> Timestamp {
    let span = time::Duration::days(i64::from(days));
    match as_of.as_offset_date_time().checked_sub(span) {
        Some(instant) => Timestamp::new(instant),
        None => Timestamp::new(time::OffsetDateTime::new_utc(
            time::Date::MIN,
            time::Time::MIDNIGHT,
        )),
    }
}
