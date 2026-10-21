//! The known-scaffold table: text markers, and the recorded file hashes (M1-T08).
//!
//! Two kinds of evidence, reported separately because they answer different questions:
//!
//! - **Text markers** ([`ScaffoldPack::markers`]) are a string a default scaffold leaves in a
//!   file: `"name": "vite-project"`, "Add your description here", the `Vite + React + TS`
//!   title, an unfilled `[year] [fullname]` LICENSE. They need nothing but the file's text, so
//!   S04 can report them today.
//! - **File hashes** ([`ScaffoldPack::files`]) are sha256 digests of files exactly as an
//!   official template ships them at a recorded version. They must be *recorded*, not
//!   invented, which is what `scripts/scaffold-hashes.sh` does on the owner's machine (§8).
//!   Until that script has been run the list is empty and S04's `hash` part is
//!   `not_measured`; a made-up digest would either match nothing or, worse, match the wrong
//!   file and accuse a project of being a template it is not.
//!
//! This lives in `substance-core` rather than in the static checks or the history signals
//! because **both** read it: S04 compares a tree against the markers and hashes, and H17 adds
//! the same hash matches to its template comparison. Putting it in either consumer would make
//! the other one depend on a sibling it has no other reason to know, and duplicating the parse
//! would let the two drift into disagreeing about what a scaffold looks like.
//!
//! Nothing here does IO: the file is embedded with `include_str!`, so a repository under test
//! cannot change what substance believes a scaffold is.

use crate::hash::{hex, sha256};

/// The embedded `rules/scaffolds.toml`.
pub const SCAFFOLDS_TOML: &str = include_str!("../../../rules/scaffolds.toml");

/// One text marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    /// The scaffold the marker comes from.
    pub tool: String,
    /// The exact path the marker appears in.
    pub path: String,
    /// The substring looked for. Empty means "the file's presence is the marker".
    pub contains: String,
    /// The S04 sub-check this marker feeds.
    pub sub_check: String,
}

/// One recorded scaffold file hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldFile {
    /// The path as the template ships it.
    pub path: String,
    /// The file's sha256, lowercase hex.
    pub sha256: String,
    /// The scaffold the file comes from.
    pub tool: String,
    /// The scaffold version recorded alongside the hash.
    pub version: String,
}

/// The parsed `rules/scaffolds.toml`.
#[derive(Debug, Clone, Default)]
pub struct ScaffoldPack {
    markers: Vec<Marker>,
    files: Vec<ScaffoldFile>,
    hash: String,
}

impl ScaffoldPack {
    /// Parses the embedded file.
    ///
    /// # Errors
    ///
    /// Returns the parser's message when the file is malformed, which is a build error: the
    /// file is part of the binary.
    pub fn load() -> Result<Self, String> {
        Self::parse(SCAFFOLDS_TOML)
    }

    /// Parses a `scaffolds.toml` text.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn parse(text: &str) -> Result<Self, String> {
        let wire: WirePack =
            toml::from_str(text).map_err(|error| format!("scaffolds.toml: {error}"))?;
        let mut markers: Vec<Marker> = wire
            .markers
            .into_iter()
            .map(|marker| Marker {
                tool: marker.tool,
                path: marker.path,
                contains: marker.contains.unwrap_or_default(),
                sub_check: marker.sub_check,
            })
            .collect();
        markers.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.tool.cmp(&right.tool))
                .then_with(|| left.sub_check.cmp(&right.sub_check))
        });
        let mut files: Vec<ScaffoldFile> = wire
            .files
            .into_iter()
            .map(|file| ScaffoldFile {
                path: file.path,
                sha256: file.sha256.to_ascii_lowercase(),
                tool: file.tool,
                version: file.version,
            })
            .collect();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(Self {
            markers,
            files,
            hash: hex(&sha256(text.as_bytes())),
        })
    }

    /// The text markers, sorted by path then tool.
    #[must_use]
    pub fn markers(&self) -> &[Marker] {
        &self.markers
    }

    /// The recorded file hashes, sorted by path.
    #[must_use]
    pub fn files(&self) -> &[ScaffoldFile] {
        &self.files
    }

    /// The file's own sha256, so a run can name the knowledge it used.
    #[must_use]
    pub fn hash(&self) -> &str {
        &self.hash
    }

    /// Whether any hash has been recorded yet.
    ///
    /// False is the normal state until the owner has run `scripts/scaffold-hashes.sh`, and it
    /// is what makes S04's hash part `not_measured` rather than a silent zero.
    #[must_use]
    pub fn has_hashes(&self) -> bool {
        !self.files.is_empty()
    }
}

/// The recorded scaffold hashes, for H17's "changed since the scaffold" comparison.
///
/// # Errors
///
/// Returns the parse error when the embedded file is malformed.
pub fn hashes() -> Result<Vec<ScaffoldFile>, String> {
    Ok(ScaffoldPack::load()?.files().to_vec())
}

/// The on-disk shape of `rules/scaffolds.toml`.
#[derive(Debug, serde::Deserialize)]
struct WirePack {
    #[serde(default)]
    markers: Vec<WireMarker>,
    #[serde(default)]
    files: Vec<WireFile>,
}

#[derive(Debug, serde::Deserialize)]
struct WireMarker {
    tool: String,
    path: String,
    contains: Option<String>,
    sub_check: String,
}

#[derive(Debug, serde::Deserialize)]
struct WireFile {
    path: String,
    sha256: String,
    tool: String,
    version: String,
}
