//! The JavaScript and TypeScript lockfiles: `package-lock.json` (v2 and v3),
//! `pnpm-lock.yaml`, `yarn.lock` (v1 and berry) and `bun.lock` (M1-T04).
//!
//! The four formats disagree about almost everything, so each one has its own reader:
//!
//! | File | Shape | Where packages live |
//! | --- | --- | --- |
//! | `package-lock.json` v2 | JSON with both `packages` and `dependencies` | `packages["node_modules/<name>"]` |
//! | `package-lock.json` v3 | JSON with `packages` only | the same |
//! | `pnpm-lock.yaml` v6+ | YAML with `importers` and `packages` | `importers[importer][dep kind]` names, `packages` holds versions |
//! | `pnpm-lock.yaml` v5 | YAML without `importers` | `dependencies`/`devDependencies` at the top level |
//! | `yarn.lock` v1 | a bespoke text format | `<name>@<range>:` blocks with `version` |
//! | `yarn.lock` berry | YAML | `"<name>@<range>":` with `version:` |
//! | `bun.lock` | JSONC (JSON with comments and trailing commas) | `packages` / `workspaces` |
//!
//! The formats were confirmed from their own sources rather than by installing anything:
//! npm's `package-lock.json` documentation (v2 carries both trees, v3 only `packages`),
//! pnpm's `pnpm-lock.yaml` documentation (lockfile v6 moved the importer list under
//! `importers` with `.` for the root), the Yarn classic `yarn.lock` description and berry's
//! YAML form, and Bun's `bun.lock` text format (JSONC with `workspaces`).
//!
//! Only top-level entries are named direct: a package inside `node_modules/a/node_modules/b`
//! is transitive by construction, and pnpm and yarn say which importer asked for a package,
//! which is what `direct` records.

use std::collections::BTreeMap;

use substance_core::deps::{Dependency, Ecosystem, LockKind};

use super::{DepsError, LockfileInput, ManifestDeps, clean_version, strip_protocol};

/// Reads a `package-lock.json` (lockfile v2 or v3) or an `npm-shrinkwrap.json`.
///
/// v2 holds both a `packages` map and the legacy `dependencies` tree; v3 dropped the tree.
/// Both are read through `packages`, which v2 also carries, so one code path serves v2, v3
/// and shrinkwrap. A v1 lockfile has no `packages` member at all; it is read from the
/// legacy tree, which is the same data one level down.
///
/// # Errors
///
/// Returns [`DepsError`] when the file is not JSON, or when `packages` holds something
/// other than an object.
pub fn parse_package_lock(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let value: serde_json::Value = serde_json::from_str(input.text)
        .map_err(|error| DepsError::new(input.path, format!("not JSON: {error}")))?;
    let mut out = Vec::new();
    if let Some(packages) = value.get("packages") {
        let entries = packages
            .as_object()
            .ok_or_else(|| DepsError::new(input.path, "`packages` is not a JSON object"))?;
        for (key, entry) in entries {
            // The repository root itself is the key "", and a workspace is a path; neither
            // names a package.
            let Some(name) = package_name_from_key(key) else {
                continue;
            };
            let Some(version) = entry.get("version").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let dev = entry
                .get("dev")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
                || entry
                    .get("devOptional")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
            let top_level = is_top_level_key(key);
            let (direct, dev) = manifest.classify(&name, dev);
            out.push(Dependency {
                ecosystem: Ecosystem::Npm,
                name,
                version: clean_version(version),
                direct: direct && top_level,
                dev,
                lock_kind: LockKind::Locked,
                manifest_path: input.path.to_owned(),
            });
        }
        return Ok(out);
    }
    // Lockfile v1: a nested `dependencies` tree.
    let Some(tree) = value
        .get("dependencies")
        .and_then(serde_json::Value::as_object)
    else {
        return Err(DepsError::new(
            input.path,
            "neither `packages` nor `dependencies` is present",
        ));
    };
    collect_v1_tree(tree, input.path, manifest, true, &mut out);
    Ok(out)
}

/// Walks a lockfile-v1 `dependencies` tree.
fn collect_v1_tree(
    tree: &serde_json::Map<String, serde_json::Value>,
    path: &str,
    manifest: &ManifestDeps,
    top_level: bool,
    out: &mut Vec<Dependency>,
) {
    for (name, entry) in tree {
        if let Some(version) = entry.get("version").and_then(serde_json::Value::as_str) {
            let dev = entry
                .get("dev")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let (direct, dev) = manifest.classify(name, dev);
            out.push(Dependency {
                ecosystem: Ecosystem::Npm,
                name: name.clone(),
                version: clean_version(version),
                direct: direct && top_level,
                dev,
                lock_kind: LockKind::Locked,
                manifest_path: path.to_owned(),
            });
        }
        if let Some(nested) = entry
            .get("dependencies")
            .and_then(serde_json::Value::as_object)
        {
            collect_v1_tree(nested, path, manifest, false, out);
        }
    }
}

/// The package a `packages` key names, or `None` for the root and for workspaces.
///
/// `node_modules/@scope/name` is the usual key; a nested
/// `node_modules/a/node_modules/b` names `b`. A key without `node_modules/` is a workspace
/// path, not a package.
fn package_name_from_key(key: &str) -> Option<String> {
    let rest = key.rsplit_once("node_modules/").map(|(_, tail)| tail)?;
    if rest.is_empty() {
        return None;
    }
    Some(rest.to_owned())
}

/// Whether a `packages` key names a top-level install.
fn is_top_level_key(key: &str) -> bool {
    key.starts_with("node_modules/") && !key["node_modules/".len()..].contains("node_modules/")
}

/// Reads a `pnpm-lock.yaml`, lockfile v6+ (`importers` plus `packages`) or v5 (top-level
/// `dependencies`).
///
/// # Errors
///
/// Returns [`DepsError`] when the YAML does not parse or is not a mapping.
pub fn parse_pnpm_lock(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(input.text)
        .map_err(|error| DepsError::new(input.path, format!("not YAML: {error}")))?;
    let mapping = value
        .as_mapping()
        .ok_or_else(|| DepsError::new(input.path, "the lockfile is not a mapping"))?;
    let mut out = Vec::new();
    // v6+: `importers` maps an importer path to its dependency groups. The root importer is
    // `.`; workspaces are their own importers.
    if let Some(importers) = mapping_get(mapping, "importers")
        && let Some(importers) = importers.as_mapping()
    {
        for (importer, groups) in importers {
            let importer_path = importer.as_str().unwrap_or_default();
            let Some(groups) = groups.as_mapping() else {
                continue;
            };
            for (group_name, entries) in groups {
                let group = group_name.as_str().unwrap_or_default();
                let dev = group.contains("dev");
                let Some(entries) = entries.as_mapping() else {
                    continue;
                };
                for (name, spec) in entries {
                    let Some(name) = name.as_str() else {
                        continue;
                    };
                    let Some(version) = pnpm_version(spec) else {
                        continue;
                    };
                    let (direct, dev) = manifest.classify(name, dev);
                    out.push(Dependency {
                        ecosystem: Ecosystem::Npm,
                        name: name.to_owned(),
                        version,
                        direct: direct || importer_path == ".",
                        dev,
                        lock_kind: LockKind::Locked,
                        manifest_path: input.path.to_owned(),
                    });
                }
            }
        }
        // The `packages` map holds every resolved package including transitive ones;
        // only those no importer named are added here.
        if let Some(packages) =
            mapping_get(mapping, "packages").and_then(|value| value.as_mapping())
        {
            let named: Vec<String> = out.iter().map(|dep| dep.name.clone()).collect();
            for (key, entry) in packages {
                let Some(key) = key.as_str() else {
                    continue;
                };
                let Some((name, key_version)) = split_pnpm_key(key) else {
                    continue;
                };
                if named.contains(&name) {
                    continue;
                }
                let version = entry
                    .as_mapping()
                    .and_then(|entry| mapping_get(entry, "version"))
                    .and_then(|value| value.as_str())
                    .map_or(key_version.clone(), clean_version);
                out.push(Dependency {
                    ecosystem: Ecosystem::Npm,
                    name,
                    version,
                    direct: false,
                    dev: false,
                    lock_kind: LockKind::Locked,
                    manifest_path: input.path.to_owned(),
                });
            }
        }
        return Ok(out);
    }
    // v5: the top-level groups name the root project's own dependencies.
    for (group, dev) in [
        ("dependencies", false),
        ("devDependencies", true),
        ("optionalDependencies", false),
    ] {
        let Some(entries) = mapping_get(mapping, group).and_then(|value| value.as_mapping()) else {
            continue;
        };
        for (name, spec) in entries {
            let Some(name) = name.as_str() else {
                continue;
            };
            let Some(version) = pnpm_version(spec) else {
                continue;
            };
            let (direct, dev) = manifest.classify(name, dev);
            out.push(Dependency {
                ecosystem: Ecosystem::Npm,
                name: name.to_owned(),
                version,
                direct,
                dev,
                lock_kind: LockKind::Locked,
                manifest_path: input.path.to_owned(),
            });
        }
    }
    if out.is_empty() {
        return Err(DepsError::new(
            input.path,
            "neither `importers` nor `dependencies` is present",
        ));
    }
    Ok(out)
}

/// The version of a pnpm dependency entry: a plain string, or `{ version = "1.2.3" }`.
fn pnpm_version(spec: &serde_yaml_ng::Value) -> Option<String> {
    if let Some(version) = spec.as_str() {
        // `1.2.3(peer@1.0.0)` records a peer resolution in parentheses.
        let version = version.split('(').next().unwrap_or(version);
        return Some(clean_version(version));
    }
    let mapping = spec.as_mapping()?;
    let version = mapping_get(mapping, "version")?.as_str()?;
    Some(clean_version(version.split('(').next().unwrap_or(version)))
}

/// Splits a pnpm `packages` key into name and version.
///
/// v6 keys are `name@1.2.3`, or `@scope/name@1.2.3`; v9 keys are `name@1.2.3(peer@2.0.0)`.
fn split_pnpm_key(key: &str) -> Option<(String, String)> {
    let without_peers = key.split('(').next().unwrap_or(key);
    let key = without_peers.trim_start_matches('/');
    if let Some(at) = key.rfind('@')
        && at > 0
    {
        let (name, version) = key.split_at(at);
        return Some((
            name.to_owned(),
            clean_version(version.trim_start_matches('@')),
        ));
    }
    // v5 wrote `/name/1.2.3`, where the version starts with a digit.
    let (name, version) = key.rsplit_once('/')?;
    if name.is_empty() || !version.starts_with(|ch: char| ch.is_ascii_digit()) {
        return None;
    }
    Some((name.to_owned(), clean_version(version)))
}

/// Reads a `yarn.lock`, v1 or berry.
///
/// Berry writes YAML and v1 writes a bespoke format; both start every entry with a quoted
/// or bare selector list and record `version`. The reader accepts both by reading the file
/// as YAML first and falling back to the v1 line scanner, so a project that switched
/// flavours without renaming the file still parses.
///
/// # Errors
///
/// Returns [`DepsError`] when neither reader finds a single package.
pub fn parse_yarn_lock(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let mut out = parse_yarn_berry(input, manifest);
    if out.is_empty() {
        out = parse_yarn_v1(input, manifest);
    }
    if out.is_empty() {
        return Err(DepsError::new(
            input.path,
            "no package entry was found; the file is neither yarn v1 nor berry",
        ));
    }
    Ok(out)
}

/// Reads berry's YAML form.
fn parse_yarn_berry(input: LockfileInput<'_>, manifest: &ManifestDeps) -> Vec<Dependency> {
    let Ok(serde_yaml_ng::Value::Mapping(mapping)) =
        serde_yaml_ng::from_str::<serde_yaml_ng::Value>(input.text)
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, entry) in &mapping {
        let Some(key) = key.as_str() else {
            continue;
        };
        let Some(version) = entry
            .as_mapping()
            .and_then(|entry| mapping_get(entry, "version"))
            .and_then(|value| value.as_str())
        else {
            continue;
        };
        // A berry key is `<name>@<range>`, possibly several comma-separated selectors.
        let Some(name) = key.split(',').next().and_then(yarn_name_of_selector) else {
            continue;
        };
        let (direct, dev) = manifest.classify(&name, false);
        out.push(Dependency {
            ecosystem: Ecosystem::Npm,
            name,
            version: clean_version(version),
            direct,
            dev,
            lock_kind: LockKind::Locked,
            manifest_path: input.path.to_owned(),
        });
    }
    out
}

/// The package name of a `name@range` selector, handling a scope.
fn yarn_name_of_selector(selector: &str) -> Option<String> {
    let selector = selector.trim().trim_matches('"').trim();
    let at = selector.rfind('@')?;
    if at == 0 {
        return None;
    }
    Some(selector[..at].to_owned())
}

/// Reads the v1 text format: `"name@range":` opens an entry, and a two-space-indented
/// `version "1.2.3"` inside it gives the version.
fn parse_yarn_v1(input: LockfileInput<'_>, manifest: &ManifestDeps) -> Vec<Dependency> {
    let mut out = Vec::new();
    let mut current: Option<String> = None;
    for line in input.text.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') && trimmed.ends_with(':') {
            let selector = trimmed.trim_end_matches(':').trim_matches('"');
            current = selector.split(',').next().and_then(yarn_name_of_selector);
            continue;
        }
        if let Some(name) = &current
            && let Some(rest) = line.trim().strip_prefix("version ")
        {
            let version = rest.trim().trim_matches('"');
            let (direct, dev) = manifest.classify(name, false);
            out.push(Dependency {
                ecosystem: Ecosystem::Npm,
                name: name.clone(),
                version: clean_version(version),
                direct,
                dev,
                lock_kind: LockKind::Locked,
                manifest_path: input.path.to_owned(),
            });
            current = None;
        }
    }
    out
}

/// Reads a `bun.lock`.
///
/// Bun's text lockfile is JSONC: JSON with `//` comments and trailing commas. serde_json
/// rejects both, so comments and trailing commas are stripped before parsing. The
/// `workspaces` member names the root project's own dependencies, and `packages` holds
/// every resolved package; both are read.
///
/// # Errors
///
/// Returns [`DepsError`] when the file does not parse as JSONC or lacks both members.
pub fn parse_bun_lock(
    input: LockfileInput<'_>,
    manifest: &ManifestDeps,
) -> Result<Vec<Dependency>, DepsError> {
    let text = strip_jsonc(input.text);
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| DepsError::new(input.path, format!("not JSONC: {error}")))?;
    // `packages` holds the resolved version of every package, including the direct ones;
    // `workspaces` holds the *ranges* the project asked for. So the version is read from
    // `packages` first, and `workspaces` only decides which packages are direct.
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    if let Some(packages) = value.get("packages").and_then(serde_json::Value::as_object) {
        for (key, entry) in packages {
            // A `packages` key is `name` or `name@version`.
            let (name, key_version) = match key.rsplit_once('@') {
                Some((head, version)) if !head.is_empty() => {
                    (head.to_owned(), Some(clean_version(version)))
                }
                _ => (key.clone(), None),
            };
            // An entry is an array: `[resolution, registry, metadata, integrity]`.
            let Some(version) = key_version.or_else(|| entry.get(0).and_then(bun_version)) else {
                continue;
            };
            resolved.insert(name, version);
        }
    }

    let mut out = Vec::new();
    let mut named: Vec<String> = Vec::new();
    if let Some(workspaces) = value
        .get("workspaces")
        .and_then(serde_json::Value::as_object)
    {
        for groups in workspaces.values() {
            let Some(groups) = groups.as_object() else {
                continue;
            };
            for (group, entries) in groups {
                let dev = group.contains("dev");
                let Some(entries) = entries.as_object() else {
                    continue;
                };
                for name in entries.keys() {
                    // The declared range is not a version; without a resolved one the
                    // package cannot be pinned, so it is left out rather than guessed.
                    let Some(version) = resolved.get(name) else {
                        continue;
                    };
                    let (direct, dev) = manifest.classify(name, dev);
                    out.push(Dependency {
                        ecosystem: Ecosystem::Npm,
                        name: name.clone(),
                        version: version.clone(),
                        direct,
                        dev,
                        lock_kind: LockKind::Locked,
                        manifest_path: input.path.to_owned(),
                    });
                    named.push(name.clone());
                }
            }
        }
    }
    for (name, version) in &resolved {
        if named.contains(name) {
            continue;
        }
        out.push(Dependency {
            ecosystem: Ecosystem::Npm,
            name: name.clone(),
            version: version.clone(),
            direct: false,
            dev: false,
            lock_kind: LockKind::Locked,
            manifest_path: input.path.to_owned(),
        });
    }
    if out.is_empty() {
        return Err(DepsError::new(
            input.path,
            "neither `workspaces` nor `packages` holds a package",
        ));
    }
    Ok(out)
}

/// The version from a bun specifier.
///
/// Three shapes appear. In `workspaces` a specifier is the range the project declares
/// (`"^4.18.0"`), so the resolved version comes from `packages` instead and this returns
/// `None` for it. In `packages` an entry is an array whose first item is
/// `"name@1.2.3"`; a scoped name (`"@scope/pkg@1.2.3"`) puts the version after the last
/// `@`. `[name, version]` marks a renamed dependency.
fn bun_version(spec: &serde_json::Value) -> Option<String> {
    if let Some(text) = spec.as_str() {
        // A bare `name@1.2.3` resolves to its version; a bare range (`^4.18.0`) is not one.
        return Some(match version_after_at(text) {
            Some(version) => version,
            None => clean_version(strip_protocol(text)),
        });
    }
    let first = spec.get(0)?;
    if let Some(entry) = first.as_str() {
        // `name@1.2.3` or `@scope/name@1.2.3`; the version follows the last `@`.
        if let Some(at) = entry.rfind('@').filter(|at| *at > 0) {
            return Some(clean_version(&entry[at + 1..]));
        }
        return Some(clean_version(strip_protocol(entry)));
    }
    // `[name, version]` for a renamed dependency.
    first
        .get(1)
        .and_then(serde_json::Value::as_str)
        .map(clean_version)
}

/// Removes `//` and `/* */` comments and trailing commas from JSONC text.
///
/// The scan is per character and aware of strings, so a `//` inside a URL value survives.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                out.push(ch);
            }
            '/' if chars.peek() == Some(&'/') => {
                for next in chars.by_ref() {
                    if next == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
            }
            ',' => out.push(ch),
            other => out.push(other),
        }
    }
    remove_trailing_commas(&out)
}

/// Removes commas that sit before `}` or `]`, outside strings.
fn remove_trailing_commas(text: &str) -> String {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in bytes.iter().copied().enumerate() {
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            continue;
        }
        if ch == ',' {
            let next = bytes[index + 1..]
                .iter()
                .find(|candidate| !candidate.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        out.push(ch);
    }
    out
}

/// Reads one member of a YAML mapping by its string key.
fn mapping_get<'a>(
    mapping: &'a serde_yaml_ng::Mapping,
    key: &str,
) -> Option<&'a serde_yaml_ng::Value> {
    mapping.get(serde_yaml_ng::Value::String(key.to_owned()))
}

/// The version in a `name@1.2.3` string, when there is one.
///
/// The name may be scoped (`@scope/name@1.2.3`), so the separator is the last `@` that is
/// not at the start, and the text after it must look like a version rather than a range.
fn version_after_at(text: &str) -> Option<String> {
    let at = text.rfind('@').filter(|at| *at > 0)?;
    let version = &text[at + 1..];
    if version.starts_with(|ch: char| ch.is_ascii_digit()) {
        Some(clean_version(version))
    } else {
        None
    }
}
