//! The provenance rule files: declared patterns and API vintages (M1-T11).
//!
//! Two files, and they are different kinds of data on purpose:
//!
//! - `rules/provenance/declared.toml` holds the strings a coding tool signs its work with —
//!   commit trailers, author names, PR head-ref prefixes and the instruction files. V01 reads
//!   them from git and the forge, so they are patterns over commit text, not over syntax.
//! - `rules/provenance/api-vintage.toml` dates an API's life. The matching pattern for each
//!   vintage lives in `rules/provenance/api-vintage.yml`, which the ast-grep pack loads
//!   (V03 is `Family::V`, and [`substance_statics`](crate) is where parsed trees live).
//!
//! This module therefore holds the date table and the commit-text patterns, and it lives in
//! `core` because **both** `substance-history` (V01, V03's window) and `substance-statics`
//! (V03's rules, which name a vintage id) need them. Putting the parse in either consumer
//! would make the other depend on a sibling it has no other reason to know.
//!
//! Nothing here does IO: the files are embedded with `include_str!`, so a repository under
//! test cannot change what substance believes a tool signs with.

use crate::Timestamp;
use crate::hash::{hex, sha256};

/// The embedded `rules/provenance/declared.toml`.
pub const DECLARED_TOML: &str = include_str!("../../../rules/provenance/declared.toml");

/// The embedded `rules/provenance/api-vintage.toml`.
pub const VINTAGE_TOML: &str = include_str!("../../../rules/provenance/api-vintage.toml");

/// Where a declared marker is looked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MarkerWhere {
    /// A commit trailer with the marker's `key`.
    Trailer,
    /// The commit's author name and email.
    Author,
    /// The whole commit message, body included, for a signature that is not a trailer.
    Message,
}

/// One declared-provenance marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredMarker {
    /// The tool the marker names, lowercase.
    pub tool: String,
    /// Where to look. `Trailer` reads `key`.
    pub where_: MarkerWhere,
    /// The trailer key, when `where_` is `Trailer`. Empty reads every trailer.
    pub key: String,
    /// Case-insensitive substrings; the marker matches when any one appears. Empty matches on
    /// the key's presence alone.
    pub contains: Vec<String>,
    /// Whether to read the model from a parenthesized suffix, as Aider writes it.
    pub model_in_parens: bool,
}

/// One coding-agent branch prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    /// The tool the prefix names.
    pub tool: String,
    /// The prefix a PR's head ref starts with.
    pub prefix: String,
}

/// One instruction file, which names a tool and never a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstructionFile {
    /// The path, or a directory prefix when it names one.
    pub path: String,
    /// The tool the file's presence names.
    pub tool: String,
}

impl InstructionFile {
    /// Whether `path` is this instruction file, or inside it when it names a directory.
    ///
    /// A directory entry has no file name: `.cursor/rules` matches `.cursor/rules` itself and
    /// everything beneath it, which is how the plan lists it. A file entry matches exactly, so
    /// `CLAUDE.md` does not match `docs/CLAUDE.md`: the plan's list is of files at the root
    /// that the tool actually reads.
    #[must_use]
    pub fn matches(&self, path: &str) -> bool {
        if self.path == path {
            return true;
        }
        let prefix = format!("{}/", self.path);
        path.starts_with(&prefix)
    }

    /// Whether the entry names a directory rather than a file.
    #[must_use]
    pub fn is_dir(&self) -> bool {
        !self
            .path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.contains('.'))
    }
}

/// The parsed declared-provenance rules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeclaredRules {
    /// The markers, sorted by `(tool, where, key)`.
    pub markers: Vec<DeclaredMarker>,
    /// The branch prefixes, sorted by prefix.
    pub branches: Vec<Branch>,
    /// The instruction files, sorted by path.
    pub instruction_files: Vec<InstructionFile>,
}

/// Why a declared rule file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{file}: {detail}")]
pub struct ProvenanceRuleError {
    /// The file's path, as a reader would open it.
    pub file: String,
    /// What was wrong.
    pub detail: String,
}

impl DeclaredRules {
    /// Loads the embedded file.
    ///
    /// # Errors
    ///
    /// Returns [`ProvenanceRuleError`] when the file does not parse or a marker names a `where` this
    /// build does not know.
    pub fn load() -> Result<Self, ProvenanceRuleError> {
        Self::parse(DECLARED_TOML)
    }

    /// Parses `text` as `declared.toml`.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn parse(text: &str) -> Result<Self, ProvenanceRuleError> {
        let file: DeclaredFile = toml::from_str(text).map_err(|error| {
            ProvenanceRuleError::new("rules/provenance/declared.toml", error.to_string())
        })?;
        let mut rules = Self::default();
        for wire in file.marker {
            let where_ = match wire.where_.as_deref().unwrap_or("trailer") {
                "trailer" => MarkerWhere::Trailer,
                "author" => MarkerWhere::Author,
                "message" => MarkerWhere::Message,
                other => {
                    return Err(ProvenanceRuleError::new(
                        "rules/provenance/declared.toml",
                        format!(
                            "marker for `{}` looks in `{other}`, which is not trailer, author or \
                             message",
                            wire.tool
                        ),
                    ));
                }
            };
            if wire.tool.trim().is_empty() {
                return Err(ProvenanceRuleError::new(
                    "rules/provenance/declared.toml",
                    "a marker has no `tool`, so a match could not be attributed",
                ));
            }
            let mut contains = wire.contains;
            for entry in &mut contains {
                *entry = entry.to_ascii_lowercase();
            }
            contains.sort();
            contains.dedup();
            rules.markers.push(DeclaredMarker {
                tool: wire.tool.to_ascii_lowercase(),
                where_,
                key: wire.key.unwrap_or_default(),
                contains,
                model_in_parens: wire.model_in_parens,
            });
        }
        for wire in file.branch {
            if wire.prefix.trim().is_empty() {
                return Err(ProvenanceRuleError::new(
                    "rules/provenance/declared.toml",
                    "a branch rule has no `prefix`, so it could never match",
                ));
            }
            rules.branches.push(Branch {
                tool: wire.tool.to_ascii_lowercase(),
                prefix: wire.prefix.to_ascii_lowercase(),
            });
        }
        for wire in file.instruction_file {
            if wire.path.trim().is_empty() {
                return Err(ProvenanceRuleError::new(
                    "rules/provenance/declared.toml",
                    "an instruction file has no `path`",
                ));
            }
            rules.instruction_files.push(InstructionFile {
                path: wire.path,
                tool: wire.tool.to_ascii_lowercase(),
            });
        }
        rules.markers.sort_by(|left, right| {
            left.tool
                .cmp(&right.tool)
                .then_with(|| left.where_.cmp(&right.where_))
                .then_with(|| left.key.cmp(&right.key))
        });
        rules
            .branches
            .sort_by(|left, right| left.prefix.cmp(&right.prefix));
        rules
            .instruction_files
            .sort_by(|left, right| left.path.cmp(&right.path));
        Ok(rules)
    }

    /// The sha256 over the two embedded files, for the manifest.
    ///
    /// Separate from `rule_pack_hash` because the two answer different questions: the pack is
    /// what judges the code, these are what describe how it was made. A report that pins both
    /// can say which version of each produced it.
    #[must_use]
    pub fn hash() -> String {
        hash_files(&[
            ("rules/provenance/api-vintage.toml", VINTAGE_TOML),
            ("rules/provenance/declared.toml", DECLARED_TOML),
        ])
    }
}

impl ProvenanceRuleError {
    /// One file's error.
    fn new(file: &str, detail: impl Into<String>) -> Self {
        Self {
            file: file.to_owned(),
            detail: detail.into(),
        }
    }
}

/// One API vintage: when an API was current.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vintage {
    /// The id the ast-grep rule's `sub_check` names.
    pub id: String,
    /// When the API became available.
    pub current_from: Timestamp,
    /// When the library retired it. A call to it dates the code before this.
    pub current_to: Timestamp,
    /// Where the dates come from.
    pub source: String,
}

/// The parsed vintage table, sorted by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VintageTable {
    /// The vintages, sorted by id.
    pub vintages: Vec<Vintage>,
}

impl VintageTable {
    /// Loads the embedded table.
    ///
    /// # Errors
    ///
    /// Returns [`ProvenanceRuleError`] for a row that does not parse, has no id, or whose window ends
    /// before it starts.
    pub fn load() -> Result<Self, ProvenanceRuleError> {
        Self::parse(VINTAGE_TOML)
    }

    /// Parses `text` as `api-vintage.toml`.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn parse(text: &str) -> Result<Self, ProvenanceRuleError> {
        let file: VintageFile = toml::from_str(text).map_err(|error| {
            ProvenanceRuleError::new("rules/provenance/api-vintage.toml", error.to_string())
        })?;
        let mut table = Self::default();
        for wire in file.vintage {
            if wire.id.trim().is_empty() {
                return Err(ProvenanceRuleError::new(
                    "rules/provenance/api-vintage.toml",
                    "a vintage has no `id`, so no rule could name it",
                ));
            }
            let from = parse_instant(&wire.current_from).ok_or_else(|| {
                ProvenanceRuleError::new(
                    "rules/provenance/api-vintage.toml",
                    format!("vintage `{}` has an unreadable `current_from`", wire.id),
                )
            })?;
            let to = parse_instant(&wire.current_to).ok_or_else(|| {
                ProvenanceRuleError::new(
                    "rules/provenance/api-vintage.toml",
                    format!("vintage `{}` has an unreadable `current_to`", wire.id),
                )
            })?;
            if to < from {
                return Err(ProvenanceRuleError::new(
                    "rules/provenance/api-vintage.toml",
                    format!(
                        "vintage `{}` ends at {} but starts at {}",
                        wire.id,
                        to.to_rfc3339(),
                        from.to_rfc3339()
                    ),
                ));
            }
            table.vintages.push(Vintage {
                id: wire.id,
                current_from: from,
                current_to: to,
                source: wire.source,
            });
        }
        table.vintages.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(table)
    }

    /// The vintage `id` names, when the table has it.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Vintage> {
        self.vintages.iter().find(|vintage| vintage.id == id)
    }

    /// Whether the table holds no vintages.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.vintages.is_empty()
    }
}

/// Reads an RFC 3339 instant, for a rule file's date fields.
fn parse_instant(text: &str) -> Option<Timestamp> {
    Timestamp::parse_rfc3339(text).ok()
}

/// The sha256 over `(path, text)` pairs, in path order.
///
/// The same derivation `substance_statics::rulepack::hash_files` uses, repeated here because
/// `core` cannot depend on the crate that holds the pack: path, a NUL, the length as eight
/// big-endian bytes, then the text. Length-prefixing is what keeps `("a", "bc")` and
/// `("ab", "c")` from sharing a preimage.
#[must_use]
pub fn hash_files(files: &[(&str, &str)]) -> String {
    let mut sorted: Vec<(&str, &str)> = files.to_vec();
    sorted.sort_by(|left, right| left.0.cmp(right.0));
    let mut preimage: Vec<u8> = Vec::new();
    for (path, text) in sorted {
        preimage.extend_from_slice(path.as_bytes());
        preimage.push(0);
        preimage.extend_from_slice(&(text.len() as u64).to_be_bytes());
        preimage.extend_from_slice(text.as_bytes());
    }
    hex(&sha256(&preimage))
}

/// The wire shape of `declared.toml`.
#[derive(Debug, serde::Deserialize)]
struct DeclaredFile {
    #[serde(default)]
    marker: Vec<MarkerWire>,
    #[serde(default)]
    branch: Vec<BranchWire>,
    #[serde(default)]
    instruction_file: Vec<InstructionFileWire>,
}

#[derive(Debug, serde::Deserialize)]
struct MarkerWire {
    tool: String,
    #[serde(default, rename = "where")]
    where_: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    model_in_parens: bool,
}

#[derive(Debug, serde::Deserialize)]
struct BranchWire {
    tool: String,
    prefix: String,
}

#[derive(Debug, serde::Deserialize)]
struct InstructionFileWire {
    path: String,
    tool: String,
}

/// The wire shape of `api-vintage.toml`.
#[derive(Debug, serde::Deserialize)]
struct VintageFile {
    #[serde(default)]
    vintage: Vec<VintageWire>,
}

#[derive(Debug, serde::Deserialize)]
struct VintageWire {
    id: String,
    current_from: String,
    current_to: String,
    #[serde(default)]
    source: String,
}
