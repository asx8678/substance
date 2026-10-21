//! SHA-256, hex and base32 helpers, and canonical JSON.

use serde::Serialize;
use sha2::{Digest, Sha256};

/// The length in bytes of a SHA-256 digest.
pub const SHA256_LEN: usize = 32;

/// Returns the SHA-256 digest of `bytes`.
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; SHA256_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = [0_u8; SHA256_LEN];
    out.copy_from_slice(&digest[..]);
    out
}

/// Returns the lowercase hexadecimal form of `bytes`.
#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

/// Returns lowercase, unpadded RFC 4648 base32 as used by content-derived IDs.
#[must_use]
pub fn base32_lower(bytes: &[u8]) -> String {
    data_encoding::BASE32_NOPAD.encode(bytes).to_lowercase()
}

/// Why canonical JSON could not be produced.
#[derive(Debug, thiserror::Error)]
pub enum CanonicalJsonError {
    /// The value's `Serialize` implementation failed.
    #[error("value cannot be serialized: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Serializes `value` as canonical JSON: keys sorted by UTF-8 bytes, no whitespace.
///
/// This deviates from the plan's `canonical_json<T>(&T) -> Vec<u8>`: a
/// serialization failure must be visible, because silently hashing a placeholder
/// would make different inputs share one content-derived ID. The workspace also
/// denies `unwrap` and `expect`, so the error is returned instead.
pub fn canonical_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, CanonicalJsonError> {
    let value = serde_json::to_value(value)?;
    Ok(canonical_json_value(&value))
}

/// Writes an already-parsed JSON value as canonical JSON.
///
/// This never relies on `serde_json`'s map order, which feature unification can
/// switch to insertion order through `preserve_order`.
#[must_use]
pub fn canonical_json_value(value: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_value(&mut out, value);
    out
}

/// Writes a JSON value as canonical JSON for a file: sorted keys, two-space indent,
/// and a trailing newline.
///
/// Committed artifacts such as `schemas/json/*.schema.json` use this, so regenerating
/// them produces the same bytes on every machine and every run.
#[must_use]
pub fn canonical_json_pretty_value(value: &serde_json::Value) -> Vec<u8> {
    let mut out = Vec::new();
    write_pretty_value(&mut out, value, 0);
    out.push(b'\n');
    out
}

/// Writes `value` at `depth` levels of indentation.
fn write_pretty_value(out: &mut Vec<u8>, value: &serde_json::Value, depth: usize) {
    match value {
        serde_json::Value::Array(items) if !items.is_empty() => {
            out.extend_from_slice(b"[\n");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.extend_from_slice(b",\n");
                }
                write_indent(out, depth + 1);
                write_pretty_value(out, item, depth + 1);
            }
            out.push(b'\n');
            write_indent(out, depth);
            out.push(b']');
        }
        serde_json::Value::Object(map) if !map.is_empty() => {
            out.extend_from_slice(b"{\n");
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.extend_from_slice(b",\n");
                }
                write_indent(out, depth + 1);
                write_string(out, key);
                out.extend_from_slice(b": ");
                match map.get(*key) {
                    Some(inner) => write_pretty_value(out, inner, depth + 1),
                    None => out.extend_from_slice(b"null"),
                }
            }
            out.push(b'\n');
            write_indent(out, depth);
            out.push(b'}');
        }
        other => write_value(out, other),
    }
}

/// Writes two spaces per level.
fn write_indent(out: &mut Vec<u8>, depth: usize) {
    for _ in 0..depth * 2 {
        out.push(b' ');
    }
}

fn write_value(out: &mut Vec<u8>, value: &serde_json::Value) {
    match value {
        serde_json::Value::Null => out.extend_from_slice(b"null"),
        serde_json::Value::Bool(true) => out.extend_from_slice(b"true"),
        serde_json::Value::Bool(false) => out.extend_from_slice(b"false"),
        serde_json::Value::Number(number) => out.extend_from_slice(number.to_string().as_bytes()),
        serde_json::Value::String(text) => write_string(out, text),
        serde_json::Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_value(out, item);
            }
            out.push(b']');
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            out.push(b'{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_string(out, key);
                out.push(b':');
                if let Some(inner) = map.get(*key) {
                    write_value(out, inner);
                }
            }
            out.push(b'}');
        }
    }
}

fn write_string(out: &mut Vec<u8>, text: &str) {
    out.push(b'"');
    for character in text.chars() {
        match character {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            control if control < ' ' => {
                out.extend_from_slice(b"\\u00");
                const DIGITS: &[u8; 16] = b"0123456789abcdef";
                let code = control as usize;
                out.push(DIGITS[(code >> 4) & 0x0f]);
                out.push(DIGITS[code & 0x0f]);
            }
            other => {
                let mut buffer = [0_u8; 4];
                out.extend_from_slice(other.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    out.push(b'"');
}
