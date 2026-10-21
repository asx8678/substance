//! The run manifest: every pinned input, so equal manifests give equal reports.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::clock::Timestamp;
use crate::ids::{BlobRef, RunId};
use crate::model::seed;

/// How the run reaches the code under test. Only `repo` is accepted in v1 (ADR-0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    /// A repository, at a commit.
    Repo,
    /// A live URL; kept in the schema, rejected by intake in v1.
    Url,
    /// A repository plus a live URL; kept in the schema, rejected by intake in v1.
    RepoUrl,
}

/// Where the code under test came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RepoRef {
    /// The origin URL, without credentials.
    pub origin: String,
    /// The commit that was exported to the working copy.
    pub commit_sha: String,
}

/// How the server's clock behaves during the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case")]
pub enum ServerClock {
    /// Started at `frozen_clock` and left advancing (Linux only, ADR-0022).
    FixedStart,
    /// The host's real clock; clock-dependent checks report not_measured.
    Real,
}

/// The version of a tool found on `PATH`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ToolVersion {
    /// The tool, as named in `tools.toml`.
    pub tool: String,
    /// The version it reported.
    pub version: String,
    /// The blob holding the output.
    pub output: Option<BlobRef>,
}

/// The host the run happened on, for explanation only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Host {
    /// `macos`, `linux` or `windows`.
    pub os: String,
    /// `aarch64` or `x86_64`.
    pub arch: String,
}

/// Every pinned input, and nothing that depends on where the run was invoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema, TS)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RunManifest {
    /// The report schema version this run writes.
    pub schema_version: String,
    /// The substance version.
    pub substance_version: String,
    /// Content-derived run ID.
    pub run_id: RunId,
    /// The code under test.
    pub repo: RepoRef,
    /// How the code was reached.
    pub access_mode: AccessMode,
    /// The instant every relative date is computed from.
    pub as_of: Timestamp,
    /// The master seed all generator seeds derive from.
    #[serde(with = "seed")]
    #[ts(type = "string")]
    #[schemars(with = "String")]
    pub master_seed: u64,
    /// One seed per random-input generator, sorted by generator name.
    #[serde(with = "seeds")]
    #[ts(type = "Record<string, string>")]
    #[schemars(with = "BTreeMap<String, String>")]
    pub seeds: BTreeMap<String, u64>,
    /// The clock value pinned for the browser and, when supported, the server.
    pub frozen_clock: Timestamp,
    /// Whether the server clock was moved or left real.
    pub server_clock: ServerClock,
    /// The timezone forced on the app, normally `"UTC"`.
    pub timezone: String,
    /// The locale forced on the browser, normally `"en-US"`.
    pub locale: String,
    /// Tool versions found on `PATH`, sorted by tool name.
    pub tools: BTreeMap<String, ToolVersion>,
    /// Model IDs, empty while the agent layer is off (ADR-0006).
    pub models: BTreeMap<String, String>,
    /// The rule pack's content hash.
    pub rule_pack_hash: String,
    /// The thresholds version.
    pub thresholds_version: String,
    /// The scoring weights version.
    pub scoring_weights_version: String,
    /// The provenance rule files hash: `rules/provenance/*.toml` (M1-T11).
    ///
    /// Separate from `rule_pack_hash` because the two answer different questions: the pack is
    /// what judges the code, these describe how it was made.
    pub provenance_rules_hash: String,
    /// The project config's content hash.
    pub config_hash: String,
    /// Whether `--allow-exec` was given.
    pub allow_exec: bool,
    /// Whether egress was actually enforced; false on the host runner (ADR-0003).
    pub egress_enforced: bool,
    /// How many times each action runs; 3, for the 3-of-3 rule.
    pub repetitions: u8,
    /// The host.
    pub host: Host,
}

/// Serializes the seed map with decimal-string values.
mod seeds {
    use std::collections::BTreeMap;

    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    /// Writes every seed as a decimal string.
    pub fn serialize<S: Serializer>(
        value: &BTreeMap<String, u64>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let text: BTreeMap<&String, String> = value
            .iter()
            .map(|(key, seed)| (key, seed.to_string()))
            .collect();
        text.serialize(serializer)
    }

    /// Reads every seed from a decimal string.
    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<BTreeMap<String, u64>, D::Error> {
        let text = BTreeMap::<String, String>::deserialize(deserializer)?;
        let mut parsed_seeds = BTreeMap::new();
        for (key, seed) in text {
            let parsed = seed
                .parse::<u64>()
                .map_err(|_| D::Error::custom("a seed must be a decimal u64 in a string"))?;
            parsed_seeds.insert(key, parsed);
        }
        Ok(parsed_seeds)
    }
}
