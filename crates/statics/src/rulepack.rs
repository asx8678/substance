//! The rule pack: the embedded ast-grep rules, loaded once and hashed (M1-T07).
//!
//! Rules live in `rules/<lang>/*.yml`, next to the other rule files (`rules/history/`)
//! and for the same reason: a rule is data a reviewer can read and diff, not a match
//! expression buried in Rust.
//!
//! The pack is **embedded at compile time** rather than read from the evaluated
//! repository, exactly like `tools.toml` and `config/thresholds.toml`. A repository under
//! test must not be able to add, remove or weaken the rules that judge it: with the files in
//! the binary, the only way to change a rule is to change substance and rebuild.
//!
//! Two consequences shape the code:
//!
//! - Every rule file must be listed in [`FILES`], which is what makes embedding possible
//!   (`include_str!` needs a literal path). A file added to `rules/` without an entry here
//!   is simply not loaded, and `scripts/guards.sh` is where that gets caught.
//! - [`RulePack::hash`] is the sha256 over the sorted files and their bytes, so the
//!   manifest can say which exact pack produced a run's findings.

use ast_grep_config::{GlobalRules, RuleConfig, SerializableRuleConfig, from_multiple};
use ast_grep_language::SupportLang;
use substance_core::check::CheckId;
use substance_core::hash::{hex, sha256};

use crate::lang::{LANGS, Lang};

/// Every embedded rule file, as `(path relative to the repository root, contents)`.
///
/// Sorted by path. A rule file that exists but is missing here is not loaded; one that is
/// listed but does not exist fails the build.
pub const FILES: &[(&str, &str)] = &[
    (
        "rules/js/_engine.yml",
        include_str!("../../../rules/js/_engine.yml"),
    ),
    (
        "rules/py/_engine.yml",
        include_str!("../../../rules/py/_engine.yml"),
    ),
    (
        "rules/rs/_engine.yml",
        include_str!("../../../rules/rs/_engine.yml"),
    ),
    (
        "rules/ts/_engine.yml",
        include_str!("../../../rules/ts/_engine.yml"),
    ),
    (
        "rules/tsx/_engine.yml",
        include_str!("../../../rules/tsx/_engine.yml"),
    ),
];

/// Why a rule pack could not be loaded.
#[derive(Debug, thiserror::Error)]
#[error("{path}: {detail}")]
pub struct RulePackError {
    /// The rule file, relative to the repository root.
    pub path: String,
    /// What was wrong with it.
    pub detail: String,
}

impl RulePackError {
    /// A rule pack error for one file.
    fn new(path: &str, detail: impl Into<String>) -> Self {
        Self {
            path: path.to_owned(),
            detail: detail.into(),
        }
    }
}

/// The metadata every rule file must carry, read from its own YAML.
///
/// Read with a second, tiny struct rather than from ast-grep's `Metadata`: that type wraps an
/// unordered map and exposes no accessor at all (it exists for documentation tools), so there
/// is no supported way to ask it for `check_id`. Deserializing the same text twice is cheap —
/// rules are parsed once per run — and it keeps the requirement explicit: no
/// `metadata.check_id` means no load.
#[derive(Debug, serde::Deserialize)]
struct RuleHeader {
    metadata: Option<RuleMetadata>,
}

#[derive(Debug, serde::Deserialize)]
struct RuleMetadata {
    /// The catalog check this rule contributes to, for example `S02`.
    check_id: Option<String>,
    /// The named part of that check, for example `ts-any`.
    sub_check: Option<String>,
}

/// One loaded rule, with the catalog fields its metadata carried.
///
/// Not `Clone` or `Debug`: ast-grep's compiled `RuleConfig` is neither, and wrapping a type
/// that cannot be copied in one that claims to be would only move the error. A rule is read,
/// not copied: the pack owns it and the scanner borrows it.
pub struct Rule {
    /// The rule's own `id:`, unique within the pack.
    pub id: String,
    /// The language it parses.
    pub lang: Lang,
    /// The rule file it came from, relative to the repository root.
    pub path: String,
    /// The catalog check the metadata names.
    pub check_id: CheckId,
    /// The sub-check the metadata names.
    pub sub_check: String,
    /// The rule's one-line message, shown in the finding title.
    pub message: String,
    /// The compiled matcher.
    pub config: RuleConfig<SupportLang>,
}

impl Rule {
    /// Whether this rule applies to `path`'s language.
    #[must_use]
    pub fn applies_to(&self, lang: Lang) -> bool {
        self.lang == lang
    }
}

/// The loaded rules and the hash of the files they came from.
pub struct RulePack {
    rules: Vec<Rule>,
    hash: String,
}

impl RulePack {
    /// Loads the embedded pack.
    ///
    /// # Errors
    ///
    /// Returns [`RulePackError`] for a rule file that does not parse, a rule without
    /// `metadata.check_id` or `metadata.sub_check`, a `check_id` that is not in the catalog,
    /// or a rule whose `language:` is not one of the five this crate parses. Each names the
    /// file: a rule that silently does nothing is worse than a run that stops.
    pub fn load() -> Result<Self, RulePackError> {
        Self::from_files(FILES)
    }

    /// Loads a pack from `(path, contents)` pairs, sorted or not.
    ///
    /// Public so the loader can be exercised against a fixture file without touching the
    /// embedded list.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn from_files(files: &[(&str, &str)]) -> Result<Self, RulePackError> {
        let globals = GlobalRules::default();
        let mut rules: Vec<Rule> = Vec::new();
        for (path, text) in files {
            rules.extend(parse_file(path, text, &globals)?);
        }
        rules.sort_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then_with(|| left.path.cmp(&right.path))
                .then_with(|| left.sub_check.cmp(&right.sub_check))
        });
        if let Some(pair) = rules.windows(2).find(|pair| pair[0].id == pair[1].id) {
            return Err(RulePackError::new(
                &pair[1].path,
                format!(
                    "rule id `{}` is already defined in {}",
                    pair[1].id, pair[0].path
                ),
            ));
        }
        Ok(Self {
            rules,
            hash: hash_files(files),
        })
    }

    /// The rules, sorted by id.
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The sha256 of the sorted rule files and their bytes, in lowercase hex.
    ///
    /// This is the manifest's `rule_pack_hash`: it changes when a rule changes, and it does
    /// not change when a run does.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// The rules that parse `lang`, sorted by id.
    #[must_use]
    pub fn for_lang(&self, lang: Lang) -> Vec<&Rule> {
        self.rules
            .iter()
            .filter(|rule| rule.applies_to(lang))
            .collect()
    }

    /// How many rules the pack holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    /// Whether the pack holds no rules.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }
}

/// Parses one rule file.
fn parse_file(path: &str, text: &str, globals: &GlobalRules) -> Result<Vec<Rule>, RulePackError> {
    // The YAML may hold several documents, one rule each — ast-grep's usual shape for a
    // rules directory — so the same text is read twice: once for ast-grep, once for the
    // catalog fields it does not expose.
    let headers: Vec<RuleHeader> = from_multiple(text)
        .map_err(|error| RulePackError::new(path, format!("not valid YAML: {error}")))?;
    // ast-grep's parse is two steps: deserialize the document, then compile the matcher.
    // `from_multiple` does the first (`SerializableRuleConfig`), `RuleConfig::try_from` the
    // second, which is where a pattern that does not parse in its language is caught.
    let serialized: Vec<SerializableRuleConfig<SupportLang>> = from_multiple(text)
        .map_err(|error| RulePackError::new(path, format!("not a rule: {error}")))?;
    let mut configs: Vec<RuleConfig<SupportLang>> = Vec::with_capacity(serialized.len());
    for config in serialized {
        configs.push(
            RuleConfig::try_from(config, globals)
                .map_err(|error| RulePackError::new(path, format!("not a rule: {error}")))?,
        );
    }
    if headers.len() != configs.len() {
        return Err(RulePackError::new(
            path,
            "the file's documents do not pair up with their metadata",
        ));
    }
    let mut rules: Vec<Rule> = Vec::with_capacity(configs.len());
    for (header, config) in headers.into_iter().zip(configs) {
        let id = config.id.clone();
        if id.trim().is_empty() {
            return Err(RulePackError::new(
                path,
                "a rule has no `id:`, so its findings could not be named",
            ));
        }
        let metadata = header.metadata.ok_or_else(|| {
            RulePackError::new(
                path,
                format!("rule `{id}` has no `metadata:`, which must name check_id and sub_check"),
            )
        })?;
        let check_text = require(path, &id, "check_id", metadata.check_id)?;
        let sub_check = require(path, &id, "sub_check", metadata.sub_check)?;
        let check_id = CheckId::parse(&check_text).ok_or_else(|| {
            RulePackError::new(
                path,
                format!("rule `{id}` names check_id `{check_text}`, which is not in the catalog"),
            )
        })?;
        if !crate::finding::is_static_check(check_id) {
            return Err(RulePackError::new(
                path,
                format!(
                    "rule `{id}` names check_id `{check_text}`, but static rules belong to S01-S12: \
                     a parse of one file cannot answer a check from another family"
                ),
            ));
        }
        let lang = Lang::from_support(config.language).ok_or_else(|| {
            RulePackError::new(
                path,
                format!(
                    "rule `{id}` parses `{}`, which is not one of the {} languages substance reads",
                    config.language,
                    LANGS.len()
                ),
            )
        })?;
        // The file's directory names the language, so a rule cannot end up matching a file
        // its check never looks at. The check is here rather than in a reviewer's head.
        let expected = format!("rules/{}/", lang.dir());
        if !path.starts_with(&expected) {
            return Err(RulePackError::new(
                path,
                format!(
                    "rule `{id}` parses `{moniker}`, so it belongs in {expected}",
                    moniker = lang.moniker()
                ),
            ));
        }
        let message = if config.message.trim().is_empty() {
            sub_check.clone()
        } else {
            config.message.clone()
        };
        rules.push(Rule {
            id,
            lang,
            path: path.to_owned(),
            check_id,
            sub_check,
            message,
            config,
        });
    }
    Ok(rules)
}

/// One required metadata field, or a load error naming the rule and the field.
fn require(
    path: &str,
    rule: &str,
    field: &str,
    value: Option<String>,
) -> Result<String, RulePackError> {
    match value.filter(|text| !text.trim().is_empty()) {
        Some(text) => Ok(text),
        None => Err(RulePackError::new(
            path,
            format!("rule `{rule}` has no `metadata.{field}`"),
        )),
    }
}

/// The sha256 over the sorted files and their bytes (M1-T07).
///
/// Every file contributes its path, a NUL, its length as a big-endian `u64`, and its bytes.
/// The length is what stops two different packs from hashing the same: without it,
/// `("a", "bc")` and `("ab", "c")` would be the same preimage.
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
