//! L05 LLM keys in the client (M1-T09).
//!
//! Formula (action plan, L05):
//!
//! > Variable names matching
//! > `^(VITE_|NEXT_PUBLIC_|PUBLIC_|REACT_APP_|EXPO_PUBLIC_).*(OPENAI|ANTHROPIC|GEMINI|MISTRAL|
//! > GROQ|COHERE|REPLICATE).*(KEY|TOKEN|SECRET)` in tracked env files and code; provider key
//! > patterns in client source (`sk-`, `sk-ant-`, `AIza`). Confirmed
//!
//! **The secret never leaves this module.** That is stronger than "is redacted before it is
//! logged": a [`Leak`] has no field a key could be stored in, the finding's title is built from
//! the file, the line and a classification, and [`redact`] is the only function that ever looks
//! at a key's characters — it returns the first few and a length, never more. A caller holding
//! a [`Leak`] therefore cannot write a secret to a blob, a report or a log even by mistake,
//! which is the property the plan's "redact to a short prefix" is aiming at.
//!
//! Why this is `confirmed` and gated rather than a hint: a build-time prefix on a provider key
//! means the key is shipped to every visitor of the site. It is not a style question, it is a
//! bill and a credential leak, and the two halves catch different mistakes:
//!
//! - **the name half** catches a *plan* to expose a key: `VITE_OPENAI_API_KEY` in `.env.example`
//!   or in code is a key the bundle will contain, whether or not the value is committed yet;
//! - **the value half** catches a key already in the tree, in any file, whatever it is called.
//!
//! The name half is a regex on identifier text; the value half is a prefix scan. Neither needs
//! a parser, so both read text rather than syntax.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::{Finding, Verdict};

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// The key-name prefixes the catalog names, in the order it lists them.
pub const CLIENT_PREFIXES: &[&str] = &[
    "VITE_",
    "NEXT_PUBLIC_",
    "PUBLIC_",
    "REACT_APP_",
    "EXPO_PUBLIC_",
];

/// The providers the catalog names.
pub const PROVIDERS: &[&str] = &[
    "OPENAI",
    "ANTHROPIC",
    "GEMINI",
    "MISTRAL",
    "GROQ",
    "COHERE",
    "REPLICATE",
];

/// The suffixes that make a name a credential rather than, say, a model name.
pub const SECRET_SUFFIXES: &[&str] = &["KEY", "TOKEN", "SECRET"];

/// The provider key prefixes the catalog names, longest first so `sk-ant-` is tested before
/// `sk-` and a report says which provider rather than only "a key".
pub const KEY_PREFIXES: &[(&str, &str)] = &[
    ("sk-ant-", "anthropic"),
    ("sk-proj-", "openai"),
    ("sk-", "openai"),
    ("AIza", "google"),
    ("gsk_", "groq"),
    ("r8_", "replicate"),
];

/// The shortest string after a prefix that still counts as a key.
///
/// A prefix on its own is documentation: `sk-` appears in every README that explains the
/// format, and a `sk-your-key-here` placeholder is not a leak. Twenty characters is far below
/// any real key and far above a placeholder.
pub const MIN_KEY_BODY: usize = 20;

/// The `parts` key for the repository scan.
pub const REPO_PART: &str = "repo";

/// The `parts` key for the built-bundle scan (M4-T04).
pub const BUNDLE_PART: &str = "bundle";

/// One leak, described without its secret.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Leak {
    /// The file the leak is in.
    pub path: String,
    /// The 1-based line.
    pub line: u32,
    /// Which half found it: `exposed-name` or `provider-key`.
    pub sub_check: String,
    /// The variable name, when the name half found it; empty otherwise.
    pub name: String,
    /// A redacted rendering of the value, when the value half found it; empty otherwise.
    ///
    /// Produced only by [`redact`], which keeps at most eight characters — enough for a reader
    /// to recognise *which* key it is, and not enough to use it.
    pub redacted: String,
}

impl Leak {
    /// The sentence a finding carries. Contains the path, the name and the redacted prefix and
    /// never a value.
    #[must_use]
    pub fn title(&self) -> String {
        match self.sub_check.as_str() {
            "exposed-name" => format!(
                "{} is a client-exposed name for a provider credential in {}",
                self.name, self.path
            ),
            _ if self.redacted.is_empty() => {
                format!("a provider key is committed in {}", self.path)
            }
            _ => format!(
                "a provider key is committed in {} ({}…)",
                self.path, self.redacted
            ),
        }
    }
}

/// Judges L05's repository half.
///
/// Both halves run over every tracked text file. The env-file half looks for the catalogue's
/// name shape anywhere, because a `VITE_OPENAI_API_KEY` in a README is still a plan to ship
/// one; the value half looks for a provider prefix followed by enough characters to be a key.
#[must_use]
pub async fn llm_keys_in_client(run_id: &str, tree: &mut TrackedTree<'_>) -> StaticReport {
    let candidates: Vec<String> = tree
        .paths()
        .into_iter()
        .filter(|path| readable(path))
        .map(str::to_owned)
        .collect();
    let mut leaks: Vec<Leak> = Vec::new();
    for path in candidates {
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            let line_no = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
            if let Some(name) = exposed_name(line) {
                leaks.push(Leak {
                    path: path.clone(),
                    line: line_no,
                    sub_check: "exposed-name".to_owned(),
                    name,
                    redacted: String::new(),
                });
            }
            if let Some((prefix, provider)) = provider_key(line) {
                leaks.push(Leak {
                    path: path.clone(),
                    line: line_no,
                    sub_check: "provider-key".to_owned(),
                    name: provider.to_owned(),
                    redacted: redact(&prefix),
                });
            }
        }
    }
    leaks.sort();
    leaks.dedup();

    let findings: Vec<Finding> = leaks
        .iter()
        .map(|leak| {
            file_finding::at(
                run_id,
                CheckId::L05,
                &leak.sub_check,
                &leak.title(),
                Location::lines(&leak.path, leak.line, leak.line),
            )
        })
        .collect();

    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    let exposed = leaks
        .iter()
        .filter(|leak| leak.sub_check == "exposed-name")
        .count();
    let keys = leaks
        .iter()
        .filter(|leak| leak.sub_check == "provider-key")
        .count();
    metrics.insert("exposed_names".to_owned(), as_i64(exposed));
    metrics.insert("provider_keys".to_owned(), as_i64(keys));
    metrics.insert("files".to_owned(), as_i64(leaks.len()));
    let (findings, cut) = cap(findings);

    let mut notes: Vec<String> = vec![
        "the secret itself is never stored: a finding names the file, the line and at most \
         eight characters of the value"
            .to_owned(),
    ];
    if exposed == 0 && keys == 0 {
        notes.push(
            "no client-exposed provider name and no provider key prefix in the tree".to_owned(),
        );
    }
    notes.extend(cut);

    // L05 is `confirmed` and gated in the catalog, so a leak fails and its absence passes;
    // either way the bundle part has not been looked at.
    let repo_verdict = if leaks.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    let mut report = StaticReport::reported(CheckId::L05, metrics)
        .with_findings(findings)
        .with_notes(notes)
        .with_part(
            BUNDLE_PART,
            Verdict::NotMeasured {
                reason: "bundle scanning arrives in M4-T04".to_owned(),
            },
        );
    report.verdict.verdict = repo_verdict.clone();
    report.with_part(REPO_PART, repo_verdict)
}

/// Whether a tracked path is the kind of file a key can be in.
///
/// Every text file is read, including env files with no extension, because the catalog's second
/// half is about *code* and the first about *env files*, and a `.env` has neither a suffix nor
/// a language. Binary and vendored paths are skipped: a key inside a `node_modules` bundle is
/// the dependency's problem and not this project's.
#[must_use]
pub fn readable(path: &str) -> bool {
    if path.starts_with("node_modules/")
        || path.starts_with("vendor/")
        || path.contains("/node_modules/")
        || path.starts_with("target/")
        || path.starts_with("dist/")
        || path.starts_with("build/")
    {
        return false;
    }
    let name = TrackedTree::file_name(path);
    if name.starts_with('.') && name != ".env" && !name.starts_with(".env.") {
        // `.gitignore`, `.gitattributes`: a key cannot hide in them, and reading every dotfile
        // in a repository is work for no answer.
        return !matches!(name, ".dockerignore" | ".npmrc" | ".yarnrc" | ".yarnrc.yml");
    }
    let ext = path.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");
    matches!(
        ext,
        "ts" | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "cjs"
            | "py"
            | "rs"
            | "toml"
            | "yaml"
            | "yml"
            | "json"
            | "md"
            | "txt"
            | "env"
            | "example"
            | "sh"
            | "bash"
            | "html"
            | "htm"
            | "vue"
            | "svelte"
            | "astro"
            | "mdx"
            | "properties"
            | "ini"
            | "cfg"
            | "conf"
    ) || name == ".env"
        || name.starts_with(".env.")
}

/// The client-exposed credential name in a line, if there is one.
///
/// The catalog's regex is `^(prefix).*(provider).*(suffix)`. It is applied to every identifier
/// the line contains rather than to the whole line, because a name rarely starts a line:
/// `const key = import.meta.env.VITE_OPENAI_API_KEY` is the normal spelling.
#[must_use]
pub fn exposed_name(line: &str) -> Option<String> {
    for word in identifiers(line) {
        let upper = word.to_ascii_uppercase();
        let Some(prefix) = CLIENT_PREFIXES
            .iter()
            .find(|prefix| upper.starts_with(*prefix))
        else {
            continue;
        };
        let rest = &upper[prefix.len()..];
        if !PROVIDERS.iter().any(|provider| rest.contains(provider)) {
            continue;
        }
        if !SECRET_SUFFIXES.iter().any(|suffix| rest.ends_with(suffix)) {
            continue;
        }
        return Some(word.to_owned());
    }
    None
}

/// A provider key in a line, with the redacted text the report may show.
///
/// Returns `(redacted, provider)`. The value is never returned.
#[must_use]
pub fn provider_key(line: &str) -> Option<(String, &'static str)> {
    for (prefix, provider) in KEY_PREFIXES {
        let Some(at) = line.find(prefix) else {
            continue;
        };
        // A key is a bare token: the character before it must not be part of a word, so that
        // the `sk-` inside `task-` or a hash does not match.
        let before_ok = line[..at]
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric() && ch != '_');
        if !before_ok {
            continue;
        }
        let body: String = line[at..]
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .collect();
        if body.len() < prefix.len() + MIN_KEY_BODY {
            continue;
        }
        if is_placeholder(&body) {
            continue;
        }
        return Some((redact(&body), provider));
    }
    None
}

/// Whether a token with a key's shape is a documented placeholder rather than a key.
///
/// `sk-your-key-here`, `sk-xxxxxxxx`, `sk-0000000000000000000`: a real key is random, and the
/// words a README uses to describe one are not. Without this, every provider's own
/// documentation would fail the check.
///
/// The test is deliberately narrow, because the failure that matters is the opposite one: a
/// heuristic that called a real key a placeholder would silence the check on exactly the case
/// it exists for. So only whole-token shapes count — a token that is one repeated character, or
/// a short run like `0123456789`/`abcdefghij` iterated — and the word list matches whole
/// segments between separators rather than substrings. `sk-abc…` is a *real* key's spelling (a
/// random base62 string contains `abc` about a quarter of the time), while `sk-your-key-here`
/// has `your`, `key` and `here` as segments.
#[must_use]
pub fn is_placeholder(token: &str) -> bool {
    let lower = token.to_ascii_lowercase();
    const WORDS: &[&str] = &[
        "your",
        "xxx",
        "placeholder",
        "example",
        "dummy",
        "sample",
        "test",
        "fake",
        "secret",
        "key",
        "token",
        "here",
        "insert",
        "replace",
        "redacted",
        "changeme",
        "none",
        "null",
    ];
    let segments: Vec<&str> = lower
        .split(['-', '_', '.'])
        .filter(|segment| !segment.is_empty())
        .collect();
    if segments.iter().any(|segment| WORDS.contains(segment)) {
        return true;
    }
    // A token drawn rather than generated: one character repeated, or the whole body a single
    // ascending run (`0123456789abcdef`). Checked on the body *after* the prefix, so a key whose
    // prefix contains a repeated character is not discarded for that reason.
    let body = segments
        .iter()
        .rev()
        .next_back()
        .map(|first| {
            let skip = prefix_len(&lower, first);
            &lower[skip..]
        })
        .unwrap_or(lower.as_str());
    if body.is_empty() {
        return true;
    }
    let mut chars = body.chars();
    let Some(first) = chars.next() else {
        return true;
    };
    if body.chars().all(|ch| ch == first) {
        return true;
    }
    body.len() <= 10 && is_run(body)
}

/// How many bytes of `lower` are the known key prefix.
fn prefix_len(lower: &str, _first_segment: &str) -> usize {
    for (prefix, _provider) in KEY_PREFIXES {
        if lower.starts_with(prefix) {
            return prefix.len();
        }
    }
    0
}

/// Whether a short string is one ascending character run.
fn is_run(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let mut previous = first;
    for ch in chars {
        if (ch as i32) != (previous as i32) + 1 {
            return false;
        }
        previous = ch;
    }
    true
}

/// Redacts a key to at most eight characters and an ellipsis.
///
/// This is the only function that reads a key's characters for output, and it is the reason a
/// [`Leak`] cannot carry one: callers receive this string, not the input.
#[must_use]
pub fn redact(value: &str) -> String {
    let kept: String = value.chars().take(8).collect();
    format!("{kept}…")
}

/// The identifier-shaped words in a line: runs of `A-Z`, `a-z`, `0-9` and `_`.
fn identifiers(line: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    let bytes = line.as_bytes();
    let mut start: Option<usize> = None;
    for (index, byte) in bytes.iter().enumerate() {
        let word = byte.is_ascii_alphanumeric() || *byte == b'_';
        match (word, start) {
            (true, None) => start = Some(index),
            (false, Some(begin)) => {
                out.push(&line[begin..index]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(begin) = start {
        out.push(&line[begin..]);
    }
    out
}
