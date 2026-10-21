//! What the file-backed checks read: the tracked tree at the run's commit (M1-T08).
//!
//! S04-S08, S10 and S12 ask about names, configs and text, not syntax, and they all need the
//! same three things:
//!
//! - which paths the commit tracks, and how big each is ([`TrackedTree::entries`]);
//! - the text of one of those files, read from the commit rather than the working tree, so
//!   uncommitted edits never change what a check sees ([`TrackedTree::text`]);
//! - the files at the repository root, which several of the checks treat differently
//!   ([`TrackedTree::root_files`]).
//!
//! Reading happens once per run, when [`TrackedTree::load`] lists the tree. The text of a file
//! is read on demand and memoised in a sorted map, because the checks ask for the same README
//! and config files over and over and a second `git cat-file` per question would dominate the
//! run.

use std::collections::BTreeMap;
use std::path::Path;

use substance_history::git::{BlobEntry, GitError, GitRepo};

/// The largest file [`TrackedTree::text`] reads, in bytes.
///
/// The same megabyte the rule engine uses, for the same reason: above it a "text file" is a
/// bundle or a data table, and none of the checks in this task want one. A check that needs a
/// size of its own (`S06` looks at tracked paths, not contents) works from [`BlobEntry`].
pub const MAX_READ_BYTES: u64 = 1_000_000;

/// One commit's tracked tree, with a small text cache.
#[derive(Debug)]
pub struct TrackedTree<'a> {
    repo: &'a GitRepo,
    rev: String,
    entries: Vec<BlobEntry>,
    cache: BTreeMap<String, Option<String>>,
}

impl<'a> TrackedTree<'a> {
    /// Lists `rev`'s tree.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when the tree cannot be listed, which is the one failure that
    /// stops every file-backed check: without the listing there is nothing to ask about.
    pub async fn load(repo: &'a GitRepo, rev: &str) -> Result<Self, GitError> {
        Ok(Self {
            repo,
            rev: rev.to_owned(),
            entries: repo.sized_blobs(rev).await?,
            cache: BTreeMap::new(),
        })
    }

    /// Every blob in the tree, sorted by path.
    #[must_use]
    pub fn entries(&self) -> &[BlobEntry] {
        &self.entries
    }

    /// The paths in the tree, sorted.
    #[must_use]
    pub fn paths(&self) -> Vec<&str> {
        self.entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect()
    }

    /// The files directly under the repository root, sorted.
    ///
    /// "Root" means no `/` in the path: the checks about scaffold leftovers and report-style
    /// markdown are about what a reader sees on the front page, not about a directory of the
    /// same name three levels down.
    #[must_use]
    pub fn root_files(&self) -> Vec<&str> {
        self.paths()
            .into_iter()
            .filter(|path| !path.contains('/'))
            .collect()
    }

    /// The path's size, or `None` when the commit does not track it.
    #[must_use]
    pub fn size(&self, path: &str) -> Option<u64> {
        self.entries
            .iter()
            .find(|entry| entry.path == path)
            .map(|entry| entry.size)
    }

    /// Whether the commit tracks `path`.
    #[must_use]
    pub fn has(&self, path: &str) -> bool {
        self.size(path).is_some()
    }

    /// The text of `path` at the commit, or `None` when it is absent, too large or not text.
    ///
    /// A check that cannot read a file treats it as absent, which is what the catalog's
    /// wording assumes ("README commands that don't exist" says nothing about a README git
    /// refused to show). The read is memoised, so the second caller pays nothing.
    pub async fn text(&mut self, path: &str) -> Option<String> {
        if let Some(cached) = self.cache.get(path) {
            return cached.clone();
        }
        let read = match self.size(path) {
            Some(size) if size <= MAX_READ_BYTES => self.repo.blob_text(&self.rev, path).await.ok(),
            _ => None,
        };
        self.cache.insert(path.to_owned(), read.clone());
        read
    }

    /// The first of `paths` that the commit tracks, in the order given.
    #[must_use]
    pub fn first_of(&self, paths: &[&str]) -> Option<String> {
        paths
            .iter()
            .find(|path| self.has(path))
            .map(|path| (*path).to_owned())
    }

    /// The tracked paths whose name matches `predicate`.
    #[must_use]
    pub fn matching(&self, predicate: impl Fn(&str) -> bool) -> Vec<&str> {
        self.paths()
            .into_iter()
            .filter(|path| predicate(path))
            .collect()
    }

    /// The tracked paths under `prefix`, sorted.
    #[must_use]
    pub fn under(&self, prefix: &str) -> Vec<&str> {
        self.paths()
            .into_iter()
            .filter(|path| path.starts_with(prefix))
            .collect()
    }

    /// The file name, for a path.
    #[must_use]
    pub fn file_name(path: &str) -> &str {
        Path::new(path)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(path)
    }
}
