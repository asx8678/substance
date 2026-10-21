//! Normalization: the rules that make two runs of the same app produce the same bytes (M3-T05).
//!
//! ## One list, two engines
//!
//! The rules live in `rules/normalize.toml` and are embedded here. Rust applies them to every body it
//! hashes or compares, and the driver applies the *same list* inside the browser. That is only sound
//! if a pattern means the same thing in both engines, so this module refuses to load a rule outside a
//! documented common subset rather than letting a divergence appear as a hash that changes between
//! runs for no visible reason.
//!
//! ## The common subset
//!
//! Accepted, because `regex` and JavaScript's `RegExp` agree on all of them:
//!
//! - character classes `[abc]`, `[^abc]`, ranges, with `\d \w \s \D \W \S` inside them;
//! - the escapes `\d \w \s`, `\.` `\?` `\+` `\*` `\(` `\)` `\[` `\]` `\{` `\}` `\|` `\^`
//!   `\$` `\\` `\/` `\-` `\"'`;
//! - anchors `^ $` and the word boundary `\b`;
//! - groups `(...)`, non-capturing `(?:...)`, and alternation `|`;
//! - the quantifiers `? * + {n} {n,} {n,m}`, plus the lazy forms `?? *? +? {n,m}?`.
//!
//! Rejected, because the engines differ (or because the difference is easy to trip over):
//!
//! - **all lookarounds** `(?=...)` `(?!...)` `(?<=...)` `(?<!...)`: Rust's `regex` crate does not
//!   implement them at all, so a rule using one would compile only in the driver and the two
//!   engines would quietly disagree. A rule that needs one is rewritten (see the generated nanoid
//!   alternation in `rules/normalize.toml`);
//! - `(?P<name>...)` / `(?<name>...)` named groups (JavaScript's spelling differs by version),
//!   backreferences `\1`, and `\p{...}` Unicode classes (Rust needs an opt-in and JavaScript
//!   spells it `\p{...}` with the `u` flag only);
//! - inline flags `(?i)` and friends (JavaScript has none; case is folded with `[aA]` instead);
//! - `\A \z \Z`, `(?#...)` comments, and possessive quantifiers `*+`.
//!
//! The replacement string is likewise restricted to plain text and `$1`…`$9`: JavaScript also
//! accepts `$&`, `` $` `` and `$'`, and Rust's `regex` does not.
//!
//! ## Idempotence
//!
//! Applying the rules twice must give the same bytes as applying them once, or a normalized body that
//! is itself normalized would keep changing and a diff could never be empty. Every token is chosen so
//! no rule matches it: `<ts>` has no digits, `<uuid>` no hex-and-dash run of the right length, and so
//! on. `apply` is therefore idempotent, and the probe in the task's evidence checks it.

use std::sync::OnceLock;

use crate::model::NormalizePattern;

/// The embedded rule file.
pub const RULES_TOML: &str = include_str!("../../../rules/normalize.toml");

/// How a rule's pattern and replacement are written down.
///
/// `id` and `class` travel with the rule so a record can name the rule that changed a body, and so a
/// diff can say which class of value moved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Rule {
    /// A stable name: `iso-8601`, `uuid`, `next-build-path`.
    pub id: String,
    /// The plan's class: `timestamp`, `epoch`, `id`, `csrf`, `nonce`, `asset`, `build`.
    pub class: String,
    /// The pattern, in the common subset.
    pub regex: String,
    /// What a match becomes, with `$1`…`$9` for capture groups.
    pub token: String,
}

/// The rules file, as it is written.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
struct RuleFile {
    /// The rules, in application order.
    rules: Vec<Rule>,
}

/// Why the rules could not be loaded or a pattern does not fit the shared subset.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum NormalizeError {
    /// The embedded file is not valid TOML or does not match [`RuleFile`].
    #[error("rules/normalize.toml: {0}")]
    Load(String),
    /// A rule's pattern uses something the two engines disagree about.
    #[error("normalize rule {id}: {reason}")]
    Subset {
        /// The rule.
        id: String,
        /// What is wrong with it.
        reason: String,
    },
    /// A pattern does not compile at all.
    #[error("normalize rule {id}: {reason}")]
    Compile {
        /// The rule.
        id: String,
        /// The compiler's message.
        reason: String,
    },
    /// A project's own pattern does not compile or does not fit the subset.
    #[error("project normalize pattern {index}: {reason}")]
    Project {
        /// Its position in `[normalize] patterns`.
        index: usize,
        /// What is wrong with it.
        reason: String,
    },
}

/// One compiled rule, ready to apply.
#[derive(Debug, Clone)]
struct Compiled {
    /// The rule it came from.
    rule: Rule,
    /// Its compiled pattern.
    regex: regex::Regex,
}

/// The normalizer: every rule, compiled once.
///
/// Constructed through [`Normalizer::shared`] for the built-in rules or [`Normalizer::with_project`]
/// when a project adds its own. Cheap to clone (the compiled rules are behind an `Arc`), because a
/// caller normalizes many bodies.
#[derive(Debug, Clone)]
pub struct Normalizer {
    rules: std::sync::Arc<Vec<Compiled>>,
}

impl Normalizer {
    /// The built-in rules, compiled once for the process.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError`] if the embedded file cannot be loaded, which is a bug in the
    /// repository rather than a runtime condition: the file is compiled in.
    pub fn shared() -> Result<Self, NormalizeError> {
        static SHARED: OnceLock<Result<std::sync::Arc<Vec<Compiled>>, String>> = OnceLock::new();
        let loaded = SHARED.get_or_init(|| {
            load_rules()
                .map(std::sync::Arc::new)
                .map_err(|error| error.to_string())
        });
        match loaded {
            Ok(rules) => Ok(Self {
                rules: std::sync::Arc::clone(rules),
            }),
            Err(reason) => Err(NormalizeError::Load(reason.clone())),
        }
    }

    /// The built-in rules plus a project's, with the project's last.
    ///
    /// A project rule runs after every built-in one, so a project can normalize something the
    /// defaults missed but cannot undo a default: the built-in list is the floor that makes two runs
    /// comparable at all. A project that needs a default *not* to fire has to change the default.
    ///
    /// # Errors
    ///
    /// Returns [`NormalizeError::Project`] for a project pattern outside the shared subset, and
    /// [`NormalizeError`] if the embedded rules cannot be loaded.
    pub fn with_project(project: &[NormalizePattern]) -> Result<Self, NormalizeError> {
        let mut rules = load_rules()?;
        for (index, pattern) in project.iter().enumerate() {
            check_subset(&pattern.regex, &pattern.replace)
                .map_err(|reason| NormalizeError::Project { index, reason })?;
            let regex =
                regex::Regex::new(&pattern.regex).map_err(|error| NormalizeError::Project {
                    index,
                    reason: error.to_string(),
                })?;
            rules.push(Compiled {
                rule: Rule {
                    id: format!("project-{index}"),
                    class: "project".to_owned(),
                    regex: pattern.regex.clone(),
                    token: pattern.replace.clone(),
                },
                regex,
            });
        }
        Ok(Self {
            rules: std::sync::Arc::new(rules),
        })
    }

    /// The rules as the driver receives them: pattern and replacement, in order.
    ///
    /// The driver needs nothing else, and sending the ids too would be a second thing to keep in
    /// step.
    #[must_use]
    pub fn patterns(&self) -> Vec<NormalizePattern> {
        self.rules
            .iter()
            .map(|compiled| NormalizePattern {
                regex: compiled.rule.regex.clone(),
                replace: compiled.rule.token.clone(),
            })
            .collect()
    }

    /// The rule ids, in application order: what a record's `normalized` field names.
    #[must_use]
    pub fn ids(&self) -> Vec<String> {
        self.rules
            .iter()
            .map(|compiled| compiled.rule.id.clone())
            .collect()
    }

    /// Applies every rule, in order.
    ///
    /// Global replacement per rule, which is the M3-T01 lesson: a page with two timestamps must have
    /// both normalized, or two runs that differ only in the second one still differ.
    #[must_use]
    pub fn apply(&self, text: &str) -> String {
        let mut out = text.to_owned();
        for compiled in self.rules.iter() {
            // `replace_all` allocates only when something matches, and a body with no volatile value
            // is the common case for a small response.
            if compiled.regex.is_match(&out) {
                out = compiled
                    .regex
                    .replace_all(&out, compiled.rule.token.as_str())
                    .into_owned();
            }
        }
        out
    }

    /// The hash of a body's normalized bytes: `sha256:<hex>`.
    ///
    /// Computed over the normalized *text*, so a body that differs only in volatile values hashes the
    /// same, and the raw body keeps its own blob alongside.
    #[must_use]
    pub fn hash(&self, text: &str) -> String {
        format!(
            "sha256:{}",
            crate::hash::hex(&crate::hash::sha256(self.apply(text).as_bytes()))
        )
    }
}

/// Loads and compiles the embedded rules, checking each against the shared subset.
fn load_rules() -> Result<Vec<Compiled>, NormalizeError> {
    let file: RuleFile =
        toml::from_str(RULES_TOML).map_err(|error| NormalizeError::Load(error.to_string()))?;
    let mut compiled = Vec::with_capacity(file.rules.len());
    for rule in file.rules {
        check_subset(&rule.regex, &rule.token).map_err(|reason| NormalizeError::Subset {
            id: rule.id.clone(),
            reason,
        })?;
        let regex = regex::Regex::new(&rule.regex).map_err(|error| NormalizeError::Compile {
            id: rule.id.clone(),
            reason: error.to_string(),
        })?;
        compiled.push(Compiled { rule, regex });
    }
    Ok(compiled)
}

/// Refuses a pattern or replacement outside the subset both engines agree on.
///
/// A refusal says what it saw, not just that something was disallowed: the point is that whoever
/// wrote the rule can fix it without reading this module.
fn check_subset(regex: &str, replacement: &str) -> Result<(), String> {
    let mut chars = regex.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        match ch {
            '\\' => {
                let Some((_, next)) = chars.next() else {
                    return Err("a trailing backslash".to_owned());
                };
                match next {
                    'd' | 'w' | 's' | 'D' | 'W' | 'S' | 'b' | 'B' => {}
                    '.' | '?' | '+' | '*' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$'
                    | '\\' | '/' | '-' | '"' | '\'' | 'n' | 'r' | 't' => {}
                    'A' | 'z' | 'Z' => {
                        return Err(format!("{next:?} is Rust-only; use ^ and $ for anchors"));
                    }
                    'p' | 'P' => {
                        return Err(format!(
                            "{next:?} (a Unicode class) is spelled differently in each engine"
                        ));
                    }
                    '0'..='9' => {
                        return Err(format!(
                            "{next} is a backreference, which JavaScript and Rust count differently"
                        ));
                    }
                    other => return Err(format!("unsupported escape \\{other}")),
                }
            }
            // Every `(?...` form, sorted by what it is.
            '(' => {
                let rest = &regex[at..];
                if rest.starts_with("(?P<") || rest.starts_with("(?P=") {
                    return Err(
                        "named groups and backreferences are not in the shared subset".to_owned(),
                    );
                }
                if rest.starts_with("(?#") {
                    return Err("inline comments are not in the shared subset".to_owned());
                }
                // Rust's `regex` crate has no lookarounds, so a rule using one compiles only in
                // JavaScript. That is the worst case for this module: the pattern would appear to
                // work, and the two engines would normalize differently without saying so.
                if rest.starts_with("(?=")
                    || rest.starts_with("(?!")
                    || rest.starts_with("(?<=")
                    || rest.starts_with("(?<!")
                {
                    return Err(
                        "lookarounds are not in the shared subset; Rust's regex crate has none, \
                         so rewrite the rule (see the nanoid rule for an example)"
                            .to_owned(),
                    );
                }
                if rest.starts_with("(?:") {
                    // A non-capturing group: nothing further to check in this prefix.
                    continue;
                }
                if let Some(flags) = rest.strip_prefix("(?") {
                    let letters: String = flags
                        .chars()
                        .take_while(|ch| ch.is_ascii_alphabetic())
                        .collect();
                    if !letters.is_empty() {
                        return Err(format!(
                            "inline flags (?{letters}) are Rust-only; spell the case out instead"
                        ));
                    }
                }
            }
            // Possessive quantifiers, which JavaScript reads as a syntax error.
            '*' | '+' | '?' | '}' => {
                let possessive = ["*+", "++", "?+"]
                    .iter()
                    .any(|form| regex[at..].starts_with(form));
                if possessive {
                    return Err("possessive quantifiers are not in the shared subset".to_owned());
                }
            }
            _ => {}
        }
    }
    // The replacement side: `$1`…`$9` and literal text only.
    for (at, ch) in replacement.char_indices() {
        if ch != '$' {
            continue;
        }
        match replacement[at..].chars().nth(1) {
            Some('0'..='9') => {}
            Some('$') => {
                return Err("a literal `$` in a replacement must be written `$$`".to_owned());
            }
            Some(other) => {
                return Err(format!(
                    "`${other}` is not a capture reference; only $1-$9 and literal text are shared"
                ));
            }
            None => return Err("a trailing `$` in a replacement".to_owned()),
        }
    }
    Ok(())
}

/// Normalizes a body and returns its hash, for a caller that has the rules already.
///
/// # Errors
///
/// Returns [`NormalizeError`] when the embedded rules cannot be loaded.
pub fn normalized_hash(text: &str) -> Result<String, NormalizeError> {
    Ok(Normalizer::shared()?.hash(text))
}

/// The templated form of a request path: identifier-shaped segments become `:id`.
///
/// The idle baseline records these, so a poll whose id changes every second is one pattern rather
/// than one per request. This mirrors `templateOf` in `driver/src/normalize.ts` exactly; the two are
/// kept in step by the probe in the task's evidence, because a mismatch would make the settle rule
/// ignore a different set of requests than the record excludes.
#[must_use]
pub fn template_of(path: &str) -> String {
    let (head, query) = match path.split_once('?') {
        Some((head, query)) => (head, format!("?{query}")),
        None => (path, String::new()),
    };
    let templated = head
        .split('/')
        .map(|segment| match is_identifier(segment) {
            true => ":id",
            false => segment,
        })
        .collect::<Vec<_>>()
        .join("/");
    format!("{templated}{}", template_query(&query))
}

/// A query string with identifier-shaped values replaced, keys and order kept.
fn template_query(query: &str) -> String {
    let Some(rest) = query.strip_prefix('?') else {
        return String::new();
    };
    let templated = rest
        .split('&')
        .map(|part| match part.split_once('=') {
            Some((key, value)) if is_identifier(value) => format!("{key}=:id"),
            Some(_) => part.to_owned(),
            None if is_identifier(part) => ":id".to_owned(),
            None => part.to_owned(),
        })
        .collect::<Vec<_>>()
        .join("&");
    format!("?{templated}")
}

/// Whether a path segment or query value is an identifier rather than a word.
///
/// Deliberately narrow: all digits, a UUID, or a long mixed-case run. A slug such as `invoices` stays
/// as it is, because templating it would merge two different screens into one idle pattern.
#[must_use]
pub fn is_identifier(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return true;
    }
    let uuid = value.len() == 36
        && value.char_indices().all(|(at, ch)| match at {
            8 | 13 | 18 | 23 => ch == '-',
            _ => ch.is_ascii_hexdigit(),
        });
    if uuid {
        return true;
    }
    // A ULID or a database id: long, alphanumeric, and not a word.
    value.len() >= 20
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// Whether a request belongs to the idle baseline.
///
/// `patterns` are `METHOD /templated/path`; `entry` is the same form for one observed request, so the
/// caller templates the request first. Method plus templated path, never a substring match: an
/// `idle` that matched any URL containing a pattern would swallow the requests a check needs.
#[must_use]
pub fn matches_idle(patterns: &[String], entry: &str) -> bool {
    patterns
        .iter()
        .any(|pattern| template_matches(pattern, entry))
}

/// Whether one `METHOD /templated/path` pattern matches one `METHOD /path` entry.
///
/// The path and the query are compared separately, which is not a detail: with the query left
/// attached to the last path segment, `GET /api/notifications?since=:id` would "match"
/// `GET /api/notifications` because that segment looked like a placeholder. Splitting first is what
/// keeps a poll's own URL from being ignored only when it has a query, or ignored when it does not.
#[must_use]
pub fn template_matches(pattern: &str, entry: &str) -> bool {
    let (Some((pattern_method, pattern_rest)), Some((entry_method, entry_rest))) =
        (pattern.split_once(' '), entry.split_once(' '))
    else {
        return false;
    };
    if pattern_method != entry_method {
        return false;
    }
    let (pattern_path, pattern_query) = split_query(pattern_rest);
    let (entry_path, entry_query) = split_query(entry_rest);
    parts_match(pattern_path, entry_path) && parts_match(pattern_query, entry_query)
}

/// A path or query split from its `?`.
fn split_query(value: &str) -> (&str, &str) {
    match value.split_once('?') {
        Some((path, query)) => (path, query),
        None => (value, ""),
    }
}

/// Whether every `/`- or `&`-separated part of `entry` matches `pattern`, allowing placeholders.
///
/// Both sides empty is a match; one side empty and the other not is not, which is how a request
/// without a query stops matching a pattern that has one.
fn parts_match(pattern: &str, entry: &str) -> bool {
    if pattern == entry {
        return true;
    }
    if pattern.is_empty() || entry.is_empty() {
        return false;
    }
    let separator = if pattern.contains('&') || entry.contains('&') {
        '&'
    } else {
        '/'
    };
    let pattern_parts: Vec<&str> = pattern.split(separator).collect();
    let entry_parts: Vec<&str> = entry.split(separator).collect();
    if pattern_parts.len() != entry_parts.len() {
        return false;
    }
    pattern_parts
        .iter()
        .zip(entry_parts.iter())
        .all(|(pattern_part, entry_part)| {
            pattern_part == entry_part || is_placeholder(pattern_part)
        })
}

/// Whether one segment of a pattern is a placeholder, or a key whose value is one.
fn is_placeholder(part: &str) -> bool {
    if part.starts_with(':') {
        return true;
    }
    part.split_once('=')
        .is_some_and(|(_, value)| value.starts_with(':'))
}

/// A record's normalized bytes, with the rules applied to every string in it.
///
/// [`crate::model::normalize::normalized_bytes`] removes `id` and `timing.settle_ms` (ADR-0009) and
/// leaves the rest alone. This adds the rule pass, walking every string, so a record whose bodies and
/// fields are identical between runs compares equal even when a value nested inside `ui`, `storage`
/// or `logs` moved. Applying the rules to the whole record is what makes "the runs produced the same
/// bytes" true rather than "the runs produced the same shape".
///
/// # Errors
///
/// Returns [`NormalizeError`] when the embedded rules cannot be loaded, which includes a record that
/// cannot be serialized: both mean the comparison could not be made.
pub fn normalized_record(record: &crate::model::EffectRecord) -> Result<Vec<u8>, NormalizeError> {
    let normalizer = Normalizer::shared()?;
    let bytes = crate::model::normalize::normalized_bytes(record)
        .map_err(|error| NormalizeError::Load(error.to_string()))?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| NormalizeError::Load(error.to_string()))?;
    let normalized = normalize_value(&normalizer, &value);
    Ok(crate::hash::canonical_json_value(&normalized))
}

/// Every string in `value` with the rules applied, keys included.
///
/// Keys are normalized too: a JSON body keyed by an id (`{"<uuid>": {...}}`) would otherwise keep
/// the id the rule exists to remove, and two runs would differ in a place no diff explains.
fn normalize_value(normalizer: &Normalizer, value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(normalizer.apply(text)),
        serde_json::Value::Array(items) => serde_json::Value::Array(
            items
                .iter()
                .map(|item| normalize_value(normalizer, item))
                .collect(),
        ),
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            // Sorted, so two objects with the same entries normalize to the same bytes whatever
            // order they arrived in. `canonical_json_value` sorts again; doing it here means the
            // intermediate value is already deterministic.
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by_key(|(left, _)| *left);
            for (key, item) in entries {
                out.insert(normalizer.apply(key), normalize_value(normalizer, item));
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}
