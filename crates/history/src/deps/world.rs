//! What the repository's manifests and lockfiles say (M1-T10).
//!
//! Every dependency check reads the same three things: which ecosystems the project uses, the
//! direct dependencies it asks for, and the exact versions a locked install would install.
//! They are read once here, through the parsers `substance_adapters` already has for O01 and
//! M1-T04, and the answers are handed to the checks as plain data.
//!
//! Nothing in this module touches the network. What a registry or a forge says about a package
//! arrives separately, in [`crate::deps::known::Known`], so a judge reads only facts that were
//! gathered before it — the same split the history signals use.
//!
//! Two properties matter more than the parsing itself:
//!
//! - **Absence is typed.** A project with no `package.json` is not a project with zero npm
//!   dependencies; it is a project this build has nothing to say about, and [`World`] records
//!   the difference so P05 reports instead of passing.
//! - **One order.** Dependencies are sorted by `(ecosystem, name, version)` before anything
//!   reads them, so a report is byte-identical across runs and filesystem orders.

use std::collections::BTreeMap;

use crate::git::{GitError, GitRepo};
use substance_adapters::deps::{LockfileInput, ManifestDeps, choose, parse_lockfile};
use substance_core::deps::{Dependency, Ecosystem};

/// The manifest file names whose presence means the project uses an ecosystem.
///
/// The set is deliberately wider than the lockfiles O01 looks for: P02 and P06 ask about every
/// dependency a project has, including a Python project that lists its packages only in
/// `pyproject.toml` with no lockfile at all.
pub const MANIFESTS: &[(&str, Ecosystem)] = &[
    ("Cargo.toml", Ecosystem::Crates),
    ("package.json", Ecosystem::Npm),
    ("pyproject.toml", Ecosystem::PyPi),
    ("requirements.txt", Ecosystem::PyPi),
    ("setup.py", Ecosystem::PyPi),
];

/// One direct dependency the project asks for, with the version a locked install resolves it
/// to when the lockfile says so.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Direct {
    /// The registry the package comes from.
    pub ecosystem: Ecosystem,
    /// The package name, as the lockfile spells it.
    pub name: String,
    /// The exact version, from the lockfile, or the manifest's specifier text when the
    /// lockfile does not name the package. Never empty: a manifest requirement is always
    /// something.
    pub version: String,
    /// Whether it is needed only for development, tests or building.
    pub dev: bool,
}

impl Direct {
    /// The key a registry lookup is cached under.
    #[must_use]
    pub fn key(&self) -> (Ecosystem, String) {
        (self.ecosystem, self.name.clone())
    }
}

/// Everything the dependency checks read about the project itself.
#[derive(Debug, Clone, Default)]
pub struct World {
    /// The ecosystems whose manifest is present, sorted.
    pub ecosystems: Vec<Ecosystem>,
    /// The direct dependencies, sorted by `(ecosystem, name, version)`.
    pub direct: Vec<Direct>,
    /// Every dependency the lockfiles name, direct and transitive, sorted.
    pub locked: Vec<Dependency>,
    /// The lockfile parsed per ecosystem, so a finding can name the file it came from.
    pub lockfiles: BTreeMap<Ecosystem, String>,
    /// Every tracked path at the revision, sorted. Kept so a caller that has the world
    /// does not list the tree a second time.
    pub paths: Vec<String>,
    /// The licence the project declares, from its manifest (P06).
    pub license: Option<String>,
    /// The manifests read, sorted.
    pub manifests: Vec<String>,
    /// Files that could not be parsed, sorted, with the parser's reason. A file here is a gap
    /// in the measurement, and the checks say so instead of reporting a clean list.
    pub unreadable: Vec<(String, String)>,
}

impl World {
    /// Reads the manifests and lockfiles at one revision.
    ///
    /// # Errors
    ///
    /// Returns [`GitError`] when the tree cannot be listed: without it there is no dependency
    /// list to judge, so the whole family stops rather than reporting an empty project.
    pub async fn load_tracked(repo: &GitRepo, rev: &str) -> Result<Self, GitError> {
        // `authored` already answers with the tracked paths as strings: the list of files
        // that count toward authored lines, which is the same tree this reads.
        let names: Vec<String> = repo.authored(rev).await?.0;
        let mut texts: BTreeMap<String, Option<String>> = BTreeMap::new();
        // Only the files World::load can read are fetched: a repository with 50,000 tracked
        // paths must not cost 50,000 `cat-file` calls to answer a question about six of them.
        for (name, _) in MANIFESTS {
            if names.iter().any(|path| path == name) {
                texts.insert((*name).to_owned(), repo.head(name, rev).await.ok());
            }
        }
        for path in &names {
            if path.contains('/') {
                continue;
            }
            if substance_core::deps::is_lockfile(path) {
                texts.insert(path.clone(), repo.head(path, rev).await.ok());
            }
        }
        let mut world = Self::load(&names, |path| texts.get(path).cloned().flatten()).await;
        world.paths = names;
        Ok(world)
    }

    /// Reads the manifests and lockfiles out of a memoised tree.
    ///
    /// `paths` is every tracked path; `text` reads one at HEAD. A manifest or lockfile that
    /// cannot be read or parsed goes into [`World::unreadable`] and contributes no
    /// dependencies, so the gap is visible rather than counted as "no dependencies".
    pub async fn load<F>(paths: &[String], mut text: F) -> Self
    where
        F: FnMut(&str) -> Option<String>,
    {
        let mut world = Self::default();
        let mut manifest_deps: BTreeMap<Ecosystem, ManifestDeps> = BTreeMap::new();
        for (name, ecosystem) in MANIFESTS {
            // A manifest at the repository root only: a `package.json` inside `examples/` is a
            // different project, and O01 takes the same view.
            if !paths.iter().any(|path| path == name) {
                continue;
            }
            if !world.ecosystems.contains(ecosystem) {
                world.ecosystems.push(*ecosystem);
            }
            world.manifests.push((*name).to_owned());
            if *name == "package.json" {
                // The manifest's own licence field, which P06 compares dependency licences
                // against. Read here because it is the same parse.
                let license = text(name)
                    .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
                    .and_then(|value| {
                        value
                            .get("license")
                            .and_then(|value| value.as_str())
                            .map(str::to_owned)
                    });
                if let Some(license) = license {
                    world.license = Some(license);
                }
                match text(name) {
                    Some(body) => match ManifestDeps::from_package_json(name, &body) {
                        Ok(deps) => {
                            manifest_deps.entry(*ecosystem).or_default().merge(deps);
                        }
                        Err(error) => world
                            .unreadable
                            .push(((*name).to_owned(), error.to_string())),
                    },
                    None => world
                        .unreadable
                        .push(((*name).to_owned(), "the file could not be read".to_owned())),
                }
            }
        }
        world.ecosystems.sort();
        world.manifests.sort();

        // One lockfile per ecosystem, chosen the way a locked install would choose it.
        let candidates: Vec<String> = paths
            .iter()
            .filter(|path| !path.contains('/'))
            .cloned()
            .collect();
        let chosen = choose(&candidates);
        for (ecosystem, (path, _kind)) in &chosen {
            let Some(body) = text(path) else {
                world
                    .unreadable
                    .push(((*path).clone(), "the file could not be read".to_owned()));
                continue;
            };
            let manifest = manifest_deps.get(ecosystem).cloned().unwrap_or_default();
            match parse_lockfile(LockfileInput { path, text: &body }, &manifest) {
                Ok(deps) => {
                    world.lockfiles.insert(*ecosystem, (*path).clone());
                    world.locked.extend(deps);
                }
                Err(error) => world.unreadable.push(((*path).clone(), error.to_string())),
            }
        }
        world
            .locked
            .sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));

        // A direct dependency is one the lockfile marks direct; when the ecosystem has no
        // lockfile, the manifest's own names are all the project has, and they are direct by
        // definition. Both cases produce a version: the locked one when there is one, the
        // manifest's specifier otherwise, because P03 can read a major out of `^4.17.20`.
        let mut direct: Vec<Direct> = Vec::new();
        for ecosystem in &world.ecosystems {
            let locked: Vec<&Dependency> = world
                .locked
                .iter()
                .filter(|dep| dep.ecosystem == *ecosystem && dep.direct)
                .collect();
            if locked.is_empty() {
                if let Some(manifest) = manifest_deps.get(ecosystem) {
                    for name in manifest.direct.iter().chain(manifest.dev.iter()) {
                        let dev = manifest.is_dev(name) && !manifest.is_direct(name);
                        direct.push(Direct {
                            ecosystem: *ecosystem,
                            name: name.clone(),
                            // No lockfile and no specifier: the name is all the project says.
                            version: String::new(),
                            dev,
                        });
                    }
                }
                continue;
            }
            for dep in locked {
                direct.push(Direct {
                    ecosystem: dep.ecosystem,
                    name: dep.name.clone(),
                    version: dep.version.clone(),
                    dev: dep.dev,
                });
            }
        }
        direct.sort();
        direct.dedup();
        world.direct = direct;
        world.unreadable.sort();
        world.unreadable.dedup();
        world
    }

    /// The direct dependencies of one ecosystem, in order.
    #[must_use]
    pub fn direct_of(&self, ecosystem: Ecosystem) -> Vec<&Direct> {
        self.direct
            .iter()
            .filter(|dep| dep.ecosystem == ecosystem)
            .collect()
    }

    /// Whether the project uses `ecosystem` at all.
    #[must_use]
    pub fn uses(&self, ecosystem: Ecosystem) -> bool {
        self.ecosystems.contains(&ecosystem)
    }

    /// The distinct package names to ask a registry about, sorted by `(ecosystem, name)`.
    ///
    /// One question per package, not per version: the registry document answers for every
    /// version at once, and asking twice for `serde` would spend the request budget on a
    /// duplicate.
    #[must_use]
    pub fn package_keys(&self) -> Vec<(Ecosystem, String)> {
        let mut keys: Vec<(Ecosystem, String)> = self
            .direct
            .iter()
            .map(|dep| (dep.ecosystem, dep.name.clone()))
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// The version of `name` the project uses, for a package already known to be direct.
    #[must_use]
    pub fn version_of(&self, ecosystem: Ecosystem, name: &str) -> Option<&str> {
        self.direct
            .iter()
            .find(|dep| dep.ecosystem == ecosystem && dep.name == name)
            .map(|dep| dep.version.as_str())
    }
}
