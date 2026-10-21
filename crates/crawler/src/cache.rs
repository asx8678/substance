//! The AppModel cache key (M4-T01, review fix 40).
//!
//! ## Why a key and not a timestamp
//!
//! A cached app model is a claim about a specific app: these screens, these controls, this request
//! map. Reusing one after anything that changes the app or the crawl would evaluate the wrong
//! program, which is the failure review fix 40 names -- the model was once keyed by commit SHA only,
//! so a changed config, a different seed or a new driver reused a model that no longer described the
//! app. The key is therefore every input the model depends on: the commit, the config's hash, the
//! master seed (forms are filled from it), the rule-pack hash (the idle baseline's exclusions), and
//! the two versions that decide what the browser did -- the driver package and Playwright.
//!
//! The key is a hash of those parts, length-prefixed so two different tuples cannot collide by
//! concatenation, and it is never a timestamp: a key that changed with the clock would make every
//! cache lookup a miss and every cache entry useless.

use substance_core::hash::{hex, sha256};

/// The inputs the app model depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheInputs<'a> {
    /// The commit the model describes.
    pub commit_sha: &'a str,
    /// The project config's content hash, as `sha256:<hex>`.
    pub config_hash: &'a str,
    /// The run's master seed: form values are drawn from it.
    pub master_seed: u64,
    /// The static rule pack's hash, which the idle baseline's exclusions come from.
    pub rule_pack_hash: &'a str,
    /// The driver package version.
    pub driver_version: &'a str,
    /// The Playwright version the driver runs.
    pub playwright_version: &'a str,
}

/// The cache key for an app model built from `inputs`, as `crawl:<hex>`.
///
/// The prefix keeps a key from being mistaken for a hash of something else in a log or a file name.
#[must_use]
pub fn cache_key(inputs: &CacheInputs<'_>) -> String {
    let seed = inputs.master_seed.to_be_bytes();
    let parts: [&[u8]; 6] = [
        inputs.commit_sha.as_bytes(),
        inputs.config_hash.as_bytes(),
        &seed,
        inputs.rule_pack_hash.as_bytes(),
        inputs.driver_version.as_bytes(),
        inputs.playwright_version.as_bytes(),
    ];
    let mut input = Vec::new();
    for part in parts {
        // Length-prefixed, so `["ab", "c"]` and `["a", "bc"]` hash differently. Without this a
        // commit and a config that happen to abut could produce one key for two models.
        input.extend_from_slice(&(part.len() as u64).to_be_bytes());
        input.extend_from_slice(part);
    }
    format!("crawl:{}", hex(&sha256(&input)))
}

/// The driver's version, read from its `package.json` text.
///
/// Parsed here rather than at the call site so the two callers that need it -- the key and the note a
/// reader sees -- cannot disagree about which field is the version.
#[must_use]
pub fn driver_version_of(package_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(package_json).ok()?;
    value
        .get("version")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// The Playwright version the driver pins, from its `package.json` text.
///
/// Read from `dependencies` rather than from the installed tree: the pin is what the driver was
/// built against and what the key should describe, and an installed tree could have been modified
/// after the fact.
#[must_use]
pub fn playwright_version_of(package_json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(package_json).ok()?;
    value
        .get("dependencies")
        .and_then(|deps| deps.get("playwright"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}
