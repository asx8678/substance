//! O01 Locked install, the static half (M1-T09).
//!
//! Formula (action plan, O01):
//!
//! > Lockfile missing for a detected ecosystem; direct dependencies absent from the lockfile;
//! > requirements without `==` pins. The exec part is in M2-T05
//!
//! Three questions, each answerable from the tree alone:
//!
//! 1. **Is there a lockfile at all** for an ecosystem whose manifest is present? A
//!    `package.json` with no `package-lock.json`/`pnpm-lock.yaml`/`yarn.lock`/`bun.lockb` means
//!    the install resolves whatever the registry has today, so two runs build different code.
//! 2. **Does the lockfile contain every direct dependency** the manifest names? A lockfile can
//!    exist and be stale: adding a dependency without re-locking leaves it absent, and
//!    `npm ci` then fails — or, worse, a floating install quietly fetches it.
//! 3. **Are the Python requirements pinned** with `==`? A `requirements.txt` of `flask` alone
//!    is a requirements file that describes an intention, not a build.
//!
//! This is the static half only. Whether the locked install *succeeds* needs a working copy and
//! a network, which is M2-T05's job, so `parts["exec"]` reports `not_measured` with the
//! milestone that delivers it. A reader seeing O01 as a pass must see that the exec part did
//! not run, which is why the parts exist at all (M0-T08).
//!
//! Lockfile *parsing* is not repeated here: `substance_adapters` already reads every supported
//! lockfile for M1-T04, and a second parser would be a second set of bugs.

use std::collections::BTreeMap;

use substance_adapters::deps::{LockfileInput, ManifestDeps, parse_lockfile};
use substance_core::check::CheckId;
use substance_core::deps::Dependency;
use substance_core::model::Verdict;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::tree::TrackedTree;

/// The `parts` key for the static analysis.
pub const STATIC_PART: &str = "static";

/// The `parts` key for the locked install the runner performs (M2-T05).
pub const EXEC_PART: &str = "exec";

/// The lockfile names that satisfy each ecosystem, in the order a project is likely to have
/// them. `pnpm-lock.yaml` before `package-lock.json` matters: a repository that switched
/// managers often still has the old file, and the one the project's own `packageManager` field
/// names is the one that counts.
pub const LOCKFILES: &[(&str, &[&str])] = &[
    (
        "node",
        &[
            "pnpm-lock.yaml",
            "package-lock.json",
            "yarn.lock",
            "bun.lockb",
        ],
    ),
    ("rust", &["Cargo.lock"]),
    (
        "python",
        &["uv.lock", "poetry.lock", "Pipfile.lock", "requirements.txt"],
    ),
];

/// The manifests whose presence means the ecosystem is in use.
pub const MANIFESTS: &[(&str, &[&str])] = &[
    ("node", &["package.json"]),
    ("rust", &["Cargo.toml"]),
    (
        "python",
        &["pyproject.toml", "requirements.txt", "setup.py"],
    ),
];

/// Judges O01's static half.
#[must_use]
pub async fn locked_install(run_id: &str, tree: &mut TrackedTree<'_>) -> StaticReport {
    let paths: Vec<String> = tree.paths().into_iter().map(str::to_owned).collect();
    let root_files: Vec<String> = tree.root_files().into_iter().map(str::to_owned).collect();

    let mut findings: Vec<substance_core::model::Finding> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut missing_lockfile = 0usize;
    let mut ecosystems = 0usize;
    let mut unpinned = 0usize;
    let mut absent_deps = 0usize;

    for (ecosystem, manifests) in MANIFESTS {
        let Some(manifest) = manifests
            .iter()
            .find(|name| root_files.iter().any(|file| file == *name))
        else {
            continue;
        };
        ecosystems += 1;
        let Some((_eco, lock_names)) = LOCKFILES.iter().find(|(name, _)| name == ecosystem) else {
            continue;
        };
        let lockfile = lock_names
            .iter()
            .find(|name| root_files.iter().any(|file| file == *name));
        let Some(lockfile) = lockfile else {
            missing_lockfile += 1;
            findings.push(crate::file_finding::at(
                run_id,
                CheckId::O01,
                "missing-lockfile",
                &format!(
                    "{manifest} is present but {ecosystem} has no lockfile, so installs float"
                ),
                crate::file_finding::Location::file(manifest),
            ));
            continue;
        };

        // The dependency comparison, where this build has both parsers.
        if *ecosystem == "node" {
            let Some(manifest_text) = tree.text(manifest).await else {
                continue;
            };
            let direct = match ManifestDeps::from_package_json(manifest, &manifest_text) {
                Ok(direct) => direct,
                Err(error) => {
                    notes.push(format!("{manifest} could not be read: {error}"));
                    continue;
                }
            };
            if direct.is_empty() {
                continue;
            }
            let Some(lock_text) = tree.text(lockfile).await else {
                continue;
            };
            let parsed: Vec<Dependency> = match parse_lockfile(
                LockfileInput {
                    path: lockfile,
                    text: &lock_text,
                },
                &direct,
            ) {
                Ok(parsed) => parsed,
                Err(error) => {
                    notes.push(format!("{lockfile} could not be read: {error}"));
                    continue;
                }
            };
            // A direct dependency the lockfile never mentions is the stale-lockfile case: the
            // manifest asks for it, the lock cannot supply it, and a locked install fails.
            for name in direct_absent(&direct, &parsed) {
                absent_deps += 1;
                findings.push(crate::file_finding::at(
                    run_id,
                    CheckId::O01,
                    "absent-direct-dependency",
                    &format!(
                        "{name} is a direct dependency in {manifest} but absent from {lockfile}"
                    ),
                    crate::file_finding::Location::file(lockfile),
                ));
            }
        }

        if *ecosystem == "python"
            && (*lockfile == "requirements.txt" || *lockfile == "pyproject.toml")
        {
            let Some(text) = tree.text(lockfile).await else {
                continue;
            };
            for (index, line) in text.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
                    continue;
                }
                if trimmed.contains("==") || trimmed.contains(" @ ") {
                    continue;
                }
                unpinned += 1;
                let line_no = u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX);
                findings.push(crate::file_finding::at(
                    run_id,
                    CheckId::O01,
                    "unpinned-requirement",
                    &format!("{lockfile} has an unpinned requirement: {trimmed}"),
                    crate::file_finding::Location::lines(lockfile, line_no, line_no),
                ));
            }
        }
    }

    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert("ecosystems".to_owned(), as_i64(ecosystems));
    metrics.insert("missing_lockfiles".to_owned(), as_i64(missing_lockfile));
    metrics.insert("absent_dependencies".to_owned(), as_i64(absent_deps));
    metrics.insert("unpinned_requirements".to_owned(), as_i64(unpinned));
    metrics.insert("files".to_owned(), as_i64(paths.len()));

    let clean = findings.is_empty();
    let (findings, cut) = cap(findings);
    notes.extend(cut);
    if ecosystems == 0 {
        return StaticReport::not_measured(
            CheckId::O01,
            "no package.json, Cargo.toml or Python manifest at this commit, so there is no \
             install to check",
        )
        .with_part(
            STATIC_PART,
            Verdict::NotMeasured {
                reason: "no manifest for any ecosystem this build installs".to_owned(),
            },
        )
        .with_part(
            EXEC_PART,
            Verdict::NotMeasured {
                reason: "not implemented (M2)".to_owned(),
            },
        );
    }

    // The static half's own verdict is the findings: no stale lockfile is a pass. It is a part,
    // not the whole check, because the exec part has not run — the check's overall verdict says
    // `not_measured` until M2, which is exactly what a reader must see.
    let static_verdict = if clean { Verdict::Pass } else { Verdict::Fail };
    let mut report = StaticReport::reported(CheckId::O01, metrics)
        .with_findings(findings)
        .with_notes(notes)
        .with_part(STATIC_PART, static_verdict)
        .with_part(
            EXEC_PART,
            Verdict::NotMeasured {
                reason: "not implemented (M2)".to_owned(),
            },
        );
    report.verdict.verdict = Verdict::NotMeasured {
        reason: "the static part ran; the locked install is not implemented (M2)".to_owned(),
    };
    report
}

/// The direct dependencies the lockfile never mentions, sorted.
///
/// The comparison is on the normalized name the lockfile's parser produced, so a manifest's
/// `@scope/pkg` and a lockfile's resolved entry agree, and dev dependencies count: they are
/// installed by the same locked command.
#[must_use]
pub fn direct_absent(manifest: &ManifestDeps, parsed: &[Dependency]) -> Vec<String> {
    let mut absent: Vec<String> = manifest
        .direct
        .iter()
        .chain(manifest.dev.iter())
        .filter(|name| !parsed.iter().any(|dep| dep.name == **name))
        .cloned()
        .collect();
    absent.sort_unstable();
    absent.dedup();
    absent
}

/// The manifests a project has at its root, for a caller that wants the ecosystem list.
#[must_use]
pub fn ecosystems_present(root_files: &[String]) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for (ecosystem, manifests) in MANIFESTS {
        if manifests
            .iter()
            .any(|name| root_files.iter().any(|file| file == name))
        {
            out.push(ecosystem);
        }
    }
    out
}

/// The lockfile a project of `ecosystem` has, if any.
#[must_use]
pub fn lockfile_of(ecosystem: &str, root_files: &[String]) -> Option<&'static str> {
    let (_name, lock_names) = LOCKFILES.iter().find(|(name, _)| *name == ecosystem)?;
    lock_names
        .iter()
        .find(|name| root_files.iter().any(|file| file == *name))
        .copied()
}

/// A direct-dependency count as a metric, for a caller that reports one.
#[must_use]
pub fn direct_count(deps: &ManifestDeps) -> i64 {
    as_i64(deps.direct.len())
}
