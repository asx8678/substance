//! Content-derived identifiers (ADR-0009) and blob references.

use serde::{Deserialize, Serialize};

use crate::hash::{base32_lower, hex, sha256};

/// How many base32 characters a derived identifier keeps.
const ID_CHARS: usize = 26;

fn derive(prefix: &str, parts: &[&[u8]]) -> String {
    let mut preimage = Vec::new();
    for part in parts {
        preimage.extend_from_slice(&(part.len() as u64).to_le_bytes());
        preimage.extend_from_slice(part);
    }
    let encoded = base32_lower(&sha256(&preimage));
    let mut id = String::with_capacity(prefix.len() + 1 + ID_CHARS);
    id.push_str(prefix);
    id.push('_');
    id.push_str(&encoded[..ID_CHARS]);
    id
}

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident, $prefix:literal, $doc:literal) => {
        $(#[$meta])*
        ///
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// The identifier's prefix.
            pub const PREFIX: &'static str = $prefix;

            /// Derives the identifier from `parts`.
            ///
            /// Each part is length-prefixed, so `["ab", "c"]` and `["a", "bc"]`
            /// hash differently.
            #[must_use]
            pub fn derive(parts: &[&[u8]]) -> Self {
                Self(derive(Self::PREFIX, parts))
            }

            /// The identifier as a string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

id_type!(
    /// A run identifier: `run_<26 base32 chars>`.
    RunId,
    "run",
    "Hashed from the run manifest with the `run_id` field removed."
);
id_type!(
    /// An effect-record identifier: `rec_<26 base32 chars>`.
    RecordId,
    "rec",
    "Hashed from (run_id, check ID, target, step index, repetition)."
);
id_type!(
    /// A finding identifier: `fnd_<26 base32 chars>`.
    FindingId,
    "fnd",
    "Hashed from (run_id, check ID, target, sub-check)."
);
id_type!(
    /// A snapshot identifier: `snap_<26 base32 chars>`.
    SnapshotId,
    "snap",
    "Hashed from (run_id, snapshot name)."
);

/// A content-addressed blob reference, written as `sha256:<hex>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlobRef(String);

impl BlobRef {
    /// The prefix of every blob reference.
    pub const PREFIX: &'static str = "sha256:";

    /// Builds a reference from the SHA-256 of `bytes`.
    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut text = String::with_capacity(Self::PREFIX.len() + SHA_HEX_CHARS);
        text.push_str(Self::PREFIX);
        text.push_str(&hex(&crate::hash::sha256(bytes)));
        Self(text)
    }

    /// Builds a reference from an already-computed digest.
    #[must_use]
    pub fn from_digest(digest: &[u8; crate::hash::SHA256_LEN]) -> Self {
        let mut text = String::with_capacity(Self::PREFIX.len() + SHA_HEX_CHARS);
        text.push_str(Self::PREFIX);
        text.push_str(&hex(digest));
        Self(text)
    }

    /// The reference as a string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The number of hex characters in a SHA-256 digest.
const SHA_HEX_CHARS: usize = crate::hash::SHA256_LEN * 2;
