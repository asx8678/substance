//! Assembling the run manifest, and the ID that follows from it (M0-T08).
//!
//! The manifest pins every input: the commit, the instant, the seed, the tool versions.
//! Nothing that depends on where the run was invoked belongs in it, because `run_id`
//! hashes it and every finding ID in turn hashes `run_id` (ADR-0009).
//!
//! Where the plan is silent, ADR-0027 records the choice.

use std::collections::BTreeMap;

use substance_core::hash::{CanonicalJsonError, canonical_json_value, sha256};
use substance_core::model::manifest::{
    AccessMode, Host, RepoRef, RunManifest, ServerClock, ToolVersion,
};
use substance_core::model::report::REPORT_SCHEMA_VERSION;
use substance_core::{RunId, Timestamp};

use crate::intake::Checkout;

/// The literal hash a manifest carries for an input no task produces yet.
///
/// A placeholder that is spelled out beats a plausible-looking fake hash: a reader can
/// tell that `"none"` means "not computed". `thresholds_version` stopped using it in M1-T06
/// and `rule_pack_hash` in M1-T07; `scoring_weights_version` follows when its task lands.
pub const NO_HASH: &str = "none";

/// The value `repetitions` takes: the plan's 3-of-3 rule.
pub const REPETITIONS: u8 = 3;

/// What the manifest is built from.
#[derive(Debug, Clone)]
pub struct ManifestInputs<'a> {
    /// The checkout intake pinned.
    pub checkout: &'a Checkout,
    /// The instant the run pretends it is.
    pub as_of: Timestamp,
    /// The master seed, already defaulted.
    pub master_seed: u64,
    /// The project config, already hashed.
    pub config_hash: String,
    /// Whether `--allow-exec` was given.
    pub allow_exec: bool,
    /// The tool versions probed from `tools.toml`, sorted by name.
    pub tools: BTreeMap<String, ToolVersion>,
    /// The thresholds file's sha256, from the embedded `config/thresholds.toml`.
    pub thresholds_version: String,
    /// The rule pack's sha256, from the embedded `rules/<lang>/*.yml`.
    pub rule_pack_hash: String,
    /// The provenance rule files' sha256, from the embedded `rules/provenance/*.toml`.
    pub provenance_rules_hash: String,
}

/// Builds the manifest, with `run_id` derived from everything else in it.
///
/// # Errors
///
/// Returns [`CanonicalJsonError`] when the manifest cannot be serialized, which would
/// mean a `Serialize` implementation disagrees with its type.
pub fn build(inputs: &ManifestInputs<'_>) -> Result<RunManifest, CanonicalJsonError> {
    let mut manifest = RunManifest {
        schema_version: REPORT_SCHEMA_VERSION.to_owned(),
        substance_version: env!("CARGO_PKG_VERSION").to_owned(),
        run_id: RunId::derive(&[b"unset"]),
        repo: RepoRef {
            origin: inputs.checkout.origin.clone(),
            commit_sha: inputs.checkout.commit_sha.clone(),
        },
        access_mode: AccessMode::Repo,
        as_of: inputs.as_of,
        master_seed: inputs.master_seed,
        seeds: BTreeMap::new(),
        frozen_clock: inputs.as_of,
        server_clock: ServerClock::Real,
        timezone: "UTC".to_owned(),
        locale: "en-US".to_owned(),
        tools: inputs.tools.clone(),
        models: BTreeMap::new(),
        rule_pack_hash: inputs.rule_pack_hash.clone(),
        thresholds_version: inputs.thresholds_version.clone(),
        scoring_weights_version: NO_HASH.to_owned(),
        provenance_rules_hash: inputs.provenance_rules_hash.clone(),
        config_hash: inputs.config_hash.clone(),
        allow_exec: inputs.allow_exec,
        egress_enforced: false,
        repetitions: REPETITIONS,
        host: host(),
    };
    manifest.run_id = derive_run_id(&manifest)?;
    Ok(manifest)
}

/// Derives `run_id` from the manifest with the `run_id` field removed.
///
/// The value is serialized to JSON, the field is removed, and the canonical bytes of
/// the rest are hashed. Three runs of one commit, seed and instant therefore share one
/// `run_id`, whatever directory they were launched from.
///
/// # Errors
///
/// Returns [`CanonicalJsonError`] when the manifest cannot be serialized.
pub fn derive_run_id(manifest: &RunManifest) -> Result<RunId, CanonicalJsonError> {
    let mut value = serde_json::to_value(manifest)?;
    if let serde_json::Value::Object(fields) = &mut value {
        fields.remove("run_id");
    }
    let bytes = canonical_json_value(&value);
    Ok(RunId::derive(&[bytes.as_slice()]))
}

/// The default master seed for a commit: the first eight bytes of
/// `sha256("substance-seed" || commit_sha)`, read big-endian.
///
/// Two runs of one commit share every generator seed without the caller passing one,
/// and a different commit gets a different seed. `--seed` replaces it.
#[must_use]
pub fn default_master_seed(commit_sha: &str) -> u64 {
    let mut preimage = b"substance-seed".to_vec();
    preimage.extend_from_slice(commit_sha.as_bytes());
    let digest = sha256(&preimage);
    let mut word = [0_u8; 8];
    word.copy_from_slice(&digest[..8]);
    u64::from_be_bytes(word)
}

/// The host, spelled the way the plan spells it (`macos`, `aarch64`).
fn host() -> Host {
    Host {
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
    }
}
