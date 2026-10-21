//! Seeds and canaries: the values a run generates from its master seed (M2-T04, ADR-0019).
//!
//! ## Why a canary is not a secret
//!
//! A canary is a string written into one user's data so a later check can tell whether *another*
//! user can see it (B13). It is deliberately not derived from a password: it travels into stored
//! rows, into URLs and into server logs by design, and a value that shared an input with a password
//! would put a password-shaped string in exactly the places B13 searches. It is derived from the
//! master seed, the generator name and the owning handle, so two users of one run never share a
//! canary and the same run always produces the same ones.
//!
//! ## Why the seeds are in the manifest and the canaries are not
//!
//! [`crate::model::manifest::RunManifest::seeds`] holds one seed per generator, because a seed is an
//! *input*: the manifest pins every input, and two runs with different seeds for the same generator
//! must get different run ids. The canaries are an *output* of those seeds, written to the run
//! directory beside the report where a check can read them.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::hash::{hex, sha256};

/// The generator name a canary is derived under.
///
/// One constant rather than a computed name, because the string is part of the derivation: renaming
/// it would change every canary in every run, which is a value-breaking change to a file the report
/// refers to.
pub const CANARY_GENERATOR: &str = "canary";

/// How many bytes of hash a canary keeps.
///
/// 8 bytes is 16 hex characters: long enough that two users' canaries never collide by accident,
/// short enough that a canary is still obviously not a security token to anyone who finds one.
const CANARY_BYTES: usize = 8;

/// What a canary is for, so a check that finds one knows what it proves.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS,
)]
#[serde(rename_all = "snake_case")]
pub enum CanaryKind {
    /// A string written into the owning user's own data (B13).
    UserData,
}

impl CanaryKind {
    /// The kind's name, as the file spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserData => "user_data",
        }
    }
}

/// One canary value, and who owns it.
///
/// The owning handle is what makes a hit meaningful: a canary found in another user's response is a
/// leak, and the same canary in its owner's own response is the app working (B13 says so, and the
/// check would be unusable without it).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Canary {
    /// The value to search for. Not a secret: it is written into the app's own data.
    pub value: String,
    /// The handle of the account whose data holds it.
    pub owner: String,
    /// The role the owner has, so a check can compare across roles without a second lookup.
    pub role: String,
    /// What the value is for.
    pub kind: CanaryKind,
}

/// Every canary of one run, sorted.
///
/// Sorted by `(owner, kind, value)` rather than by value, because the file is read by a person
/// asking "what does this user own" far more often than by one searching for a string. The search
/// path builds its own index; the file is the record.
///
/// Written to `canaries.json` in the run directory. Not part of the report: the report describes
/// the code under test, and the canaries describe values substance itself generated.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Canaries {
    /// One entry per generated value, sorted by owner, kind and value.
    pub canaries: Vec<Canary>,
}

impl Canaries {
    /// Sorts the entries into the one order this file is written in.
    pub fn sort(&mut self) {
        self.canaries.sort_by(|left, right| {
            (&left.owner, left.kind, &left.value).cmp(&(&right.owner, right.kind, &right.value))
        });
    }

    /// The canaries one handle owns, in file order.
    #[must_use]
    pub fn owned_by(&self, handle: &str) -> Vec<&Canary> {
        self.canaries
            .iter()
            .filter(|canary| canary.owner == handle)
            .collect()
    }
}

/// The seed one generator uses in one run.
///
/// `sha256(master_seed || generator)` reduced to the first eight bytes as a big-endian `u64`.
///
/// The plan's rule, implemented once so no generator invents its own derivation. Taking the first
/// eight bytes rather than folding the whole hash is a choice, not a necessity: any fixed rule
/// works, and a documented one is what makes a recorded seed reproducible by hand.
#[must_use]
pub fn seed_of(master_seed: u64, generator: &str) -> u64 {
    let mut input = Vec::with_capacity(8 + generator.len());
    input.extend_from_slice(&master_seed.to_be_bytes());
    input.extend_from_slice(generator.as_bytes());
    let digest = sha256(&input);
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(bytes)
}

/// One seed per generator, from the master seed.
///
/// Sorted by generator name, which is the order the manifest writes them. A caller passes the
/// generators it will actually use: an entry for a generator the run never instantiates would be a
/// pinned input that pins nothing.
#[must_use]
pub fn seeds_for(master_seed: u64, generators: &[&str]) -> BTreeMap<String, u64> {
    let mut seeds = BTreeMap::new();
    for generator in generators {
        seeds.insert((*generator).to_owned(), seed_of(master_seed, generator));
    }
    seeds
}

/// The canary value one user owns.
///
/// `cnry-<16 hex>` from `sha256(master_seed || "canary" || handle)`. The prefix is there so a person
/// reading a log or a database row recognises the string as substance's rather than the app's, and
/// the handle is in the derivation so two users never share one.
#[must_use]
pub fn canary_value(master_seed: u64, handle: &str) -> String {
    let mut input = Vec::with_capacity(8 + CANARY_GENERATOR.len() + handle.len());
    input.extend_from_slice(&master_seed.to_be_bytes());
    input.extend_from_slice(CANARY_GENERATOR.as_bytes());
    input.extend_from_slice(handle.as_bytes());
    let digest = sha256(&input);
    format!("cnry-{}", hex(&digest[..CANARY_BYTES]))
}
