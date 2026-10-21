//! Which files count as authored code (M1-T02).
//!
//! "Count authored lines only" is one of the two history rules. A lockfile, a generated
//! client, a vendored tree, a minified bundle, a snapshot or an image says nothing about
//! who wrote the app, and including them lets a code generator look productive.
//!
//! The decision is split in two on purpose:
//!
//! - [`AuthoredFilter::classify`] is pure and takes everything it needs as arguments, so a
//!   caller can reason about one path without touching the disk;
//! - [`GitRepo::authored`] does the IO: it lists the tree, asks `git check-attr` for the
//!   linguist attributes, and reads the first lines of the files that need them.
//!
//! Every list here is sorted where it is compared, so the same tree always yields the same
//! set.

use std::collections::BTreeSet;
use std::ffi::OsString;

use super::{GitCommand, GitError, GitRepo};

/// The longest line an authored text file may have; a longer one means minified output.
pub const MAX_AUTHORED_LINE: usize = 500;

/// How many lines are read to look for `@generated`.
pub const GENERATED_MARKER_LINES: usize = 5;

/// Lockfiles, by exact file name.
///
/// They are machine-written: every dependency bump rewrites thousands of lines, and the
/// counts say nothing about the code. `Cargo.lock` is included even for applications,
/// where the plan's rule about committing it is the opposite, because the question here is
/// authorship, not policy.
///
/// The list is [`substance_core::deps::LOCKFILES`], the same one the lockfile parsers use
/// (M1-T04): one list, so a file cannot be a lockfile for the dependency report and code
/// for the authored-line count.
pub use substance_core::deps::LOCKFILES as LISTED_LOCKFILES;

/// Directory names whose contents are never authored.
const GENERATED_DIRS: &[&str] = &[
    "__snapshots__",
    "fixtures",
    "node_modules",
    "testdata",
    "third_party",
    "vendor",
];

/// Directory names holding translations: only the machine-written tables in them are excluded.
///
/// A translation service rewrites the string table wholesale, but the code that reads it is
/// authored, and a `locales/en.ts` holding that code must still count.
const I18N_DIRS: &[&str] = &["i18n", "locales"];

/// Extensions of images, fonts, media and archives.
const BINARY_EXTENSIONS: &[&str] = &[
    "7z", "avif", "bin", "bmp", "bz2", "dat", "eot", "flac", "gif", "gz", "ico", "jpeg", "jpg",
    "m4a", "mkv", "mov", "mp3", "mp4", "ogg", "otf", "pdf", "png", "svgz", "tar", "tgz", "ttf",
    "wav", "webm", "webp", "woff", "woff2", "xz", "zip",
];

/// The text marker generated code carries in its first lines.
const GENERATED_MARKER: &str = "@generated";

/// Whether a file counts toward authored lines, and why it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthoredFile {
    /// The file counts.
    Authored,
    /// A lockfile from [`LISTED_LOCKFILES`].
    Lockfile,
    /// `git check-attr` says `linguist-generated` or `linguist-vendored`.
    Linguist,
    /// The first [`GENERATED_MARKER_LINES`] lines hold `@generated`.
    GeneratedMarker,
    /// A generated, vendored, fixture, snapshot or i18n directory.
    GeneratedDirectory,
    /// A minified file, by name or by an over-long line.
    Minified,
    /// An image, font, media file or archive.
    Binary,
}

impl AuthoredFile {
    /// Whether the file counts.
    #[must_use]
    pub const fn is_authored(self) -> bool {
        matches!(self, Self::Authored)
    }

    /// Why it was excluded, as the report spells it; `None` for an authored file.
    #[must_use]
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Authored => None,
            Self::Lockfile => Some("lockfile"),
            Self::Linguist => Some("linguist-generated or vendored"),
            Self::GeneratedMarker => Some("@generated"),
            Self::GeneratedDirectory => Some("generated directory"),
            Self::Minified => Some("minified"),
            Self::Binary => Some("binary"),
        }
    }
}

/// The linguistic attributes `git check-attr` reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinguistAttrs {
    /// Whether `linguist-generated` is set to a true value.
    pub generated: bool,
    /// Whether `linguist-vendored` is set to a true value.
    pub vendored: bool,
}

impl LinguistAttrs {
    /// Reads one `check-attr` value: `set`, `true`, `unset`, `unspecified` or a value.
    ///
    /// Git writes `set`, `unset` and `unspecified`; a custom value (`linguist-generated=yes`)
    /// arrives as that text. Anything but `unset` and `unspecified` counts, which matches
    /// how linguist itself treats the attributes.
    #[must_use]
    pub fn parse(value: &str) -> bool {
        !matches!(value.trim(), "unspecified" | "unset" | "false" | "no" | "0")
    }
}

/// The authored-file rule, as a pure function.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuthoredFilter;

impl AuthoredFilter {
    /// Classifies one path.
    ///
    /// `attrs` is what `git check-attr` said about it, and `head` is at most the first
    /// [`GENERATED_MARKER_LINES`] lines of the file — pass an empty slice when the file was
    /// not read, which is the normal case for a lockfile or an image.
    #[must_use]
    pub fn classify(path: &str, attrs: LinguistAttrs, head: &str) -> AuthoredFile {
        let path = path.trim_start_matches("./");
        let file_name = path.rsplit('/').next().unwrap_or(path);
        if LISTED_LOCKFILES.contains(&file_name) {
            return AuthoredFile::Lockfile;
        }
        if attrs.generated || attrs.vendored {
            return AuthoredFile::Linguist;
        }
        if has_generated_marker(head) {
            return AuthoredFile::GeneratedMarker;
        }
        if in_generated_directory(path) {
            return AuthoredFile::GeneratedDirectory;
        }
        if in_i18n_directory(path) && is_translation(file_name) {
            return AuthoredFile::GeneratedDirectory;
        }
        if is_minified(path, file_name, head) {
            return AuthoredFile::Minified;
        }
        if is_binary(file_name) {
            return AuthoredFile::Binary;
        }
        AuthoredFile::Authored
    }
}

/// Whether any of the first [`GENERATED_MARKER_LINES`] lines holds `@generated`.
fn has_generated_marker(head: &str) -> bool {
    head.lines()
        .take(GENERATED_MARKER_LINES)
        .any(|line| line.contains(GENERATED_MARKER))
}

/// Whether any path segment is a generated directory.
fn in_generated_directory(path: &str) -> bool {
    path.split('/')
        .any(|segment| GENERATED_DIRS.contains(&segment))
}

/// Whether any path segment is a translation directory.
fn in_i18n_directory(path: &str) -> bool {
    path.split('/').any(|segment| I18N_DIRS.contains(&segment))
}

/// Whether the file is a translation table rather than the code that reads one.
fn is_translation(file_name: &str) -> bool {
    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    let lower = extension.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "json"
            | "json5"
            | "po"
            | "pot"
            | "properties"
            | "resx"
            | "strings"
            | "xliff"
            | "yaml"
            | "yml"
    )
}

/// Whether the file is minified: a `.min.` name, a `.snap` name, or a long line.
fn is_minified(path: &str, file_name: &str, head: &str) -> bool {
    if path.ends_with(".snap") || file_name.contains(".min.") {
        return true;
    }
    head.lines()
        .any(|line| line.chars().count() > MAX_AUTHORED_LINE)
}

/// Whether the extension names an image, font, media file or archive.
fn is_binary(file_name: &str) -> bool {
    let Some((_, extension)) = file_name.rsplit_once('.') else {
        return false;
    };
    let lower = extension.to_ascii_lowercase();
    BINARY_EXTENSIONS.contains(&lower.as_str())
}

impl GitRepo {
    /// Lists every tracked path at `rev`, sorted.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when `git ls-tree` fails or times out.
    pub async fn tree(&self, rev: &str) -> Result<Vec<String>, GitError> {
        let args = vec![
            OsString::from("ls-tree"),
            OsString::from("-r"),
            OsString::from("--name-only"),
            OsString::from("-z"),
            OsString::from(rev),
        ];
        let stdout = self.run(GitCommand::ListTree, &args).await?;
        let mut paths: Vec<String> = stdout
            .split('\0')
            .filter(|path| !path.is_empty())
            .map(str::to_owned)
            .collect();
        paths.sort_unstable();
        Ok(paths)
    }

    /// Asks `git check-attr` about `paths`, in one call per chunk of arguments.
    ///
    /// The attributes are read from the tree at `rev` (`--source`), so an uncommitted
    /// `.gitattributes` cannot change the answer.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when git fails or times out.
    pub async fn linguist_attrs(
        &self,
        rev: &str,
        paths: &[String],
    ) -> Result<std::collections::BTreeMap<String, LinguistAttrs>, GitError> {
        let mut attrs = std::collections::BTreeMap::new();
        for chunk in paths.chunks(ATTRIBUTE_CHUNK) {
            let mut args = vec![
                OsString::from("check-attr"),
                OsString::from(format!("--source={rev}")),
                OsString::from("linguist-generated"),
                OsString::from("linguist-vendored"),
                OsString::from("--"),
            ];
            args.extend(chunk.iter().map(OsString::from));
            let stdout = self.run(GitCommand::CheckAttr, &args).await?;
            for line in stdout.lines() {
                let mut parts = line.splitn(3, ": ");
                let (Some(path), Some(attribute), Some(value)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    continue;
                };
                let entry = attrs
                    .entry(path.to_owned())
                    .or_insert_with(LinguistAttrs::default);
                match attribute {
                    "linguist-generated" => entry.generated = LinguistAttrs::parse(value),
                    "linguist-vendored" => entry.vendored = LinguistAttrs::parse(value),
                    _ => {}
                }
            }
        }
        Ok(attrs)
    }

    /// The authored paths at `rev`, sorted, with each excluded path's reason.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when listing the tree, reading attributes or reading a file's
    /// first lines fails.
    pub async fn authored(
        &self,
        rev: &str,
    ) -> Result<(Vec<String>, Vec<(String, &'static str)>), GitError> {
        let paths = self.tree(rev).await?;
        let attrs = self.linguist_attrs(rev, &paths).await?;
        let mut authored = Vec::new();
        let mut excluded = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for path in paths {
            if !seen.insert(path.clone()) {
                continue;
            }
            let attrs = attrs.get(&path).copied().unwrap_or_default();
            // Only a file that passes every cheap test is opened: reading the head of a
            // 50 MB bundle to look for `@generated` is the one cost worth avoiding.
            let quick = AuthoredFilter::classify(&path, attrs, "");
            let decision = if quick == AuthoredFile::Authored {
                let head = self.head(&path, rev).await.unwrap_or_default();
                AuthoredFilter::classify(&path, attrs, &head)
            } else {
                quick
            };
            match decision {
                AuthoredFile::Authored => authored.push(path),
                other => {
                    if let Some(reason) = other.reason() {
                        excluded.push((path, reason));
                    }
                }
            }
        }
        authored.sort_unstable();
        excluded.sort_by(|left, right| left.0.cmp(&right.0));
        Ok((authored, excluded))
    }

    /// The first [`GENERATED_MARKER_LINES`] lines of `path` at `rev`.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when git fails or the path is not a readable blob.
    pub async fn head(&self, path: &str, rev: &str) -> Result<String, GitError> {
        let args = vec![
            OsString::from("show"),
            OsString::from(format!("{rev}:{path}")),
        ];
        let stdout = self.run(GitCommand::Show, &args).await?;
        let head: String = stdout
            .lines()
            .take(GENERATED_MARKER_LINES)
            .collect::<Vec<_>>()
            .join("\n");
        Ok(head)
    }
}

/// How many paths one `check-attr` call may name, to stay clear of ARG_MAX.
const ATTRIBUTE_CHUNK: usize = 500;
