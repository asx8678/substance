//! S09 Over-abstraction (M1-T08).
//!
//! Formula (action plan, S09):
//!
//! > TS interfaces or abstract classes with exactly one `implements`; Rust traits with exactly
//! > one `impl … for`; functions whose body only returns a call with the same arguments.
//!
//! Three questions, two of them about counts across the tree:
//!
//! - **An interface with one implementation.** An interface exists to be implemented more than
//!   once. With exactly one `implements`, the interface and the cast to it buy nothing: the
//!   caller could hold the class and the reader would follow one less name.
//! - **A trait with one `impl … for`.** Same argument in Rust, where a trait with one
//!   implementation is often a refactor that stopped halfway.
//! - **A forwarding function.** `function getUser(id) { return db.getUser(id); }` adds a frame
//!   and a name without a decision. The rule finds the shape; the check verifies the arguments
//!   are the parameters themselves, which is what separates forwarding from translating.
//!
//! A trait or interface with *zero* implementations is not reported: that is dead code (a
//! different problem) or a trait implemented for a type the scan never saw because it is behind
//! a feature flag, and neither is over-abstraction.

use std::collections::{BTreeMap, BTreeSet};

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::{as_i64, findings as rule_findings, matching};
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::lang::Lang;
use crate::scan::ScanReport;
use crate::tree::TrackedTree;

/// Judges S09: the tree-wide counts plus the rule engine's forwarding matches.
#[must_use]
pub async fn over_abstraction(
    run_id: &str,
    tree: &mut TrackedTree<'_>,
    scan: &ScanReport,
) -> StaticReport {
    let mut findings: Vec<Finding> = rule_findings(run_id, &matching(scan, CheckId::S09));
    let mut notes: Vec<String> = Vec::new();

    // 1. Interfaces with exactly one implementation (TypeScript), traits with one impl (Rust).
    let (single_impl_types, type_note) = single_implementation_types(tree).await;
    notes.extend(type_note);
    for (path, name, line) in &single_impl_types {
        findings.push(file_finding::at(
            run_id,
            CheckId::S09,
            "single-implementation",
            &format!("`{name}` has exactly one implementation"),
            Location::lines(path, *line, *line),
        ));
    }

    // 2. Forwarding functions the rules found: verified against the parameter names.
    let forwarding = forwarding_findings(&mut findings, tree, run_id).await;

    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert(
        "single_implementation".to_owned(),
        as_i64(single_impl_types.len()),
    );
    metrics.insert("forwarding_functions".to_owned(), as_i64(forwarding));
    metrics.insert("occurrences".to_owned(), as_i64(findings.len()));
    let (findings, cut) = cap(findings);
    notes.extend(cut);
    StaticReport::reported(CheckId::S09, metrics)
        .with_findings(findings)
        .with_notes(notes)
}

/// Declarations with exactly one implementation: `(path, name, 1-based line)`.
///
/// Counted by name across the whole tree, because "exactly one" is a property of the tree. The
/// scan reads each file once and collects two sets: declared types and their implementation
/// counts. A declaration whose name is implemented once is reported; zero or two or more are
/// not.
async fn single_implementation_types(
    tree: &mut TrackedTree<'_>,
) -> (Vec<(String, String, u32)>, Vec<String>) {
    let paths: Vec<String> = tree.paths().into_iter().map(str::to_owned).collect();
    // name -> (path, line) of the declaration
    let mut declared: BTreeMap<String, (String, u32)> = BTreeMap::new();
    // name -> how many implementations were seen
    let mut implementations: BTreeMap<String, u32> = BTreeMap::new();
    let mut notes: Vec<String> = Vec::new();
    for path in paths {
        let Some(lang) = Lang::from_path(std::path::Path::new(&path)) else {
            continue;
        };
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        match lang {
            Lang::TypeScript | Lang::Tsx => {
                for (name, line) in ts_interface_declarations(&text) {
                    declared.entry(name).or_insert((path.clone(), line));
                }
                for name in ts_implements(&text) {
                    *implementations.entry(name).or_insert(0) += 1;
                }
            }
            Lang::Rust => {
                for (name, line) in rust_trait_declarations(&text) {
                    declared.entry(name).or_insert((path.clone(), line));
                }
                for name in rust_trait_impls(&text) {
                    *implementations.entry(name).or_insert(0) += 1;
                }
            }
            Lang::JavaScript | Lang::Python => {}
        }
    }
    let mut out: Vec<(String, String, u32)> = Vec::new();
    let mut zero = 0_usize;
    for (name, (path, line)) in &declared {
        match implementations.get(name) {
            Some(1) => out.push((path.clone(), name.clone(), *line)),
            Some(_) => {}
            None => zero += 1,
        }
    }
    out.sort();
    if !declared.is_empty() {
        notes.push(format!(
            "{} declarations scanned; {zero} have no implementation in this tree and are not \
             reported (that is dead code, not over-abstraction)",
            declared.len()
        ));
    }
    (out, notes)
}

/// `interface Foo {` / `abstract class Foo {` declarations: `(name, line)`.
fn ts_interface_declarations(text: &str) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        let rest = trimmed
            .strip_prefix("export interface ")
            .or_else(|| trimmed.strip_prefix("interface "))
            .or_else(|| trimmed.strip_prefix("export abstract class "))
            .or_else(|| trimmed.strip_prefix("abstract class "));
        if let Some(rest) = rest {
            let name = identifier(rest);
            if !name.is_empty() {
                out.push((name, as_line(index)));
            }
        }
    }
    out
}

/// The names appearing in `implements X` / `implements X, Y`.
fn ts_implements(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let Some(position) = line.find("implements ") else {
            continue;
        };
        let rest = &line[position + "implements ".len()..];
        for part in rest.split([',', '{']) {
            let name = identifier(part.trim());
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

/// `trait Foo {` declarations.
fn rust_trait_declarations(text: &str) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        let Some(rest) = trimmed
            .strip_prefix("pub trait ")
            .or_else(|| trimmed.strip_prefix("trait "))
        else {
            continue;
        };
        let name = identifier(rest);
        if !name.is_empty() {
            out.push((name, as_line(index)));
        }
    }
    out
}

/// The trait names in `impl X for Y` lines.
fn rust_trait_impls(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(position) = trimmed.find("impl") else {
            continue;
        };
        let rest = &trimmed[position + "impl".len()..];
        let Some(for_at) = rest.find(" for ") else {
            // `impl X` without `for` is an inherent impl, not a trait implementation.
            continue;
        };
        // The trait name is the last identifier before `for`, ignoring generics.
        let trait_part = rest[..for_at].trim();
        let name = trait_part
            .rsplit(['<', ' '])
            .next()
            .map(identifier)
            .unwrap_or_default();
        if !name.is_empty() {
            out.push(name);
        }
    }
    out
}

/// Checks that each rule match really is a forwarding function and returns how many were kept.
///
/// The rule matched a function whose body is one call. This drops the ones whose call does not
/// pass the function's own parameters: `function f(x) { return g(x + 1); }` computes something,
/// and calling it over-abstraction would be wrong.
async fn forwarding_findings(
    findings: &mut Vec<Finding>,
    tree: &mut TrackedTree<'_>,
    _run_id: &str,
) -> usize {
    let mut kept = 0_usize;
    let mut kept_findings: Vec<Finding> = Vec::new();
    for finding in findings.iter() {
        let Some(sub) = &finding.sub_check else {
            kept_findings.push(finding.clone());
            continue;
        };
        if !sub.ends_with("passthrough") {
            kept_findings.push(finding.clone());
            continue;
        }
        let path = match &finding.target {
            substance_core::model::Target::File(path) => path.clone(),
            _ => {
                kept_findings.push(finding.clone());
                continue;
            }
        };
        let line = match finding.evidence.first() {
            Some(substance_core::model::Evidence::FileRange(range)) => range.start_line,
            _ => 0,
        };
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        if function_forwards_at(&text, line) {
            kept += 1;
            kept_findings.push(finding.clone());
        }
    }
    *findings = kept_findings;
    kept
}

/// Whether the function starting at `line` forwards its parameters unchanged.
///
/// A small text check rather than a parse: the rule already established the shape, and what is
/// missing is whether the call's arguments are the parameters' names.
#[must_use]
pub fn function_forwards_at(text: &str, line: u32) -> bool {
    let lines: Vec<&str> = text.lines().collect();
    let start = usize::try_from(line.saturating_sub(1)).unwrap_or(0);
    if start >= lines.len() {
        return false;
    }
    let head = lines[start];
    let Some(open) = head.find('(') else {
        return false;
    };
    let Some(close) = head.find(')') else {
        return false;
    };
    if close < open {
        return false;
    }
    let params: Vec<&str> = head[open + 1..close]
        .split(',')
        .map(|param| {
            param
                .split(':')
                .next()
                .unwrap_or(param)
                .trim()
                .trim_start_matches("...")
        })
        .filter(|name| !name.is_empty())
        .collect();
    if params.is_empty() {
        return false;
    }
    // The body is the next line or two; the call is the one with parentheses.
    for body in lines.iter().skip(start + 1).take(2) {
        let trimmed = body.trim();
        if trimmed.starts_with('}') {
            break;
        }
        let Some(call_open) = trimmed.find('(') else {
            continue;
        };
        let Some(call_close) = trimmed.rfind(')') else {
            continue;
        };
        if call_close < call_open {
            continue;
        }
        let args: Vec<&str> = trimmed[call_open + 1..call_close]
            .split(',')
            .map(str::trim)
            .filter(|arg| !arg.is_empty())
            .collect();
        return args.len() == params.len()
            && args
                .iter()
                .zip(params.iter())
                .all(|(arg, param)| arg == param);
    }
    false
}

/// The leading identifier of a declaration's remainder.
fn identifier(text: &str) -> String {
    text.trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '$')
        .collect()
}

/// A zero-based line index as a 1-based line number.
fn as_line(index: usize) -> u32 {
    u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX)
}

/// The trait names in a tree, for a caller that wants the set rather than the findings.
#[must_use]
pub fn trait_names(text: &str) -> BTreeSet<String> {
    rust_trait_declarations(text)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}
