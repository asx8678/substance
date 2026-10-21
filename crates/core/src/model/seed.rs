//! Seed fields that travel as decimal strings, because TypeScript's `number` stops
//! being exact above 2^53 and `ts-rs` maps `u64` to the TS big integer type
//! (review issue 34).

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serializer};

/// Serializes a `u64` seed as a decimal string.
pub fn serialize<S: Serializer>(value: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

/// Deserializes a `u64` seed from a decimal string.
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.parse::<u64>()
        .map_err(|_| D::Error::custom("a seed must be a decimal u64 in a string"))
}
