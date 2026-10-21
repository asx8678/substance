//! The five languages the static checks parse, and how a path names one.
//!
//! ast-grep ships a `SupportLang` covering thirty languages. Substance parses five: the
//! checks in the catalog only ever ask about TypeScript, TSX, JavaScript, Python and Rust,
//! and a smaller set is a promise that a rule file cannot quietly target a language no check
//! reads.
//!
//! The mapping is by extension only. Guessing from content (a shebang, a `package.json`
//! dependency) would make the result depend on what else is in the tree, and two runs of one
//! commit must agree.

use std::path::Path;

use ast_grep_language::SupportLang;

/// The languages a rule file may target, in the order their directories are named.
pub const LANGS: &[Lang] = &[
    Lang::TypeScript,
    Lang::Tsx,
    Lang::JavaScript,
    Lang::Python,
    Lang::Rust,
];

/// One of the languages the static checks parse.
///
/// Each variant knows its `rules/<dir>/` name, the YAML `language:` value that loads it,
/// and the extensions that select it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lang {
    /// `.ts`
    TypeScript,
    /// `.tsx`
    Tsx,
    /// `.js`, `.jsx`, `.mjs`, `.cjs`
    JavaScript,
    /// `.py`, `.pyi`
    Python,
    /// `.rs`
    Rust,
}

impl Lang {
    /// The directory under `rules/` that holds this language's rules.
    ///
    /// These are the plan's paths, spelled out the way a reader would look for them:
    /// `rust` and `python`, not the file extensions. The moniker below is a different
    /// thing — that one is ast-grep's YAML vocabulary, where a rule says `language: rs`.
    #[must_use]
    pub const fn dir(self) -> &'static str {
        match self {
            Self::TypeScript => "ts",
            Self::Tsx => "tsx",
            Self::JavaScript => "js",
            Self::Python => "python",
            Self::Rust => "rust",
        }
    }

    /// The `language:` value a rule file may use for this language.
    ///
    /// These are ast-grep's own monikers, so a rule written for the ast-grep CLI loads here
    /// unchanged. `ts` and `tsx` are distinct languages in ast-grep because TypeScript
    /// without JSX and TSX do not parse the same text.
    #[must_use]
    pub const fn moniker(self) -> &'static str {
        match self {
            Self::TypeScript => "ts",
            Self::Tsx => "tsx",
            Self::JavaScript => "js",
            Self::Python => "py",
            Self::Rust => "rs",
        }
    }

    /// The `SupportLang` a parsed document uses.
    #[must_use]
    pub const fn support(self) -> SupportLang {
        match self {
            Self::TypeScript => SupportLang::TypeScript,
            Self::Tsx => SupportLang::Tsx,
            Self::JavaScript => SupportLang::JavaScript,
            Self::Python => SupportLang::Python,
            Self::Rust => SupportLang::Rust,
        }
    }

    /// The extensions that select this language.
    #[must_use]
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::TypeScript => &["ts"],
            Self::Tsx => &["tsx"],
            Self::JavaScript => &["js", "jsx", "mjs", "cjs"],
            Self::Python => &["py", "pyi"],
            Self::Rust => &["rs"],
        }
    }

    /// The language a path belongs to, by extension.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?;
        LANGS
            .iter()
            .copied()
            .find(|lang| lang.extensions().contains(&extension))
    }

    /// The language a rule file's `language:` value names, or `None`.
    ///
    /// Rule files use the short monikers, so this is the function a reader's YAML is
    /// checked against.
    #[must_use]
    pub fn from_moniker(moniker: &str) -> Option<Self> {
        LANGS
            .iter()
            .copied()
            .find(|lang| lang.moniker().eq_ignore_ascii_case(moniker))
    }

    /// The language a parsed `SupportLang` names, or `None` for the twenty-five this
    /// crate does not read.
    ///
    /// A loaded rule has already been through ast-grep's own parser, so this is the reliable
    /// direction: it says what the rule actually parses, not what its text claims.
    #[must_use]
    pub fn from_support(lang: SupportLang) -> Option<Self> {
        LANGS
            .iter()
            .copied()
            .find(|candidate| candidate.support() == lang)
    }
}
