//! The normalized form of a record: canonical JSON, minus what legitimately varies.

use crate::hash::CanonicalJsonError;

use super::effect::EffectRecord;

/// Serializes `record` as canonical JSON without `id` and `timing.settle_ms`.
///
/// Two runs of the same action on the same snapshot and seed must produce equal
/// bytes here, which is what replay and the determinism check compare (ADR-0009).
pub fn normalized_bytes(record: &EffectRecord) -> Result<Vec<u8>, CanonicalJsonError> {
    let mut value = serde_json::to_value(record)?;
    if let serde_json::Value::Object(map) = &mut value {
        map.remove("id");
        if let Some(serde_json::Value::Object(timing)) = map.get_mut("timing") {
            timing.remove("settle_ms");
        }
    }
    Ok(crate::hash::canonical_json_value(&value))
}
