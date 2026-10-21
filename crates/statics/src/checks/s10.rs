//! S10 Data-access sprawl (M1-T08).
//!
//! Formula (action plan, S10):
//!
//! > Number of files importing a DB client (prisma, drizzle, knex, typeorm, pg, sqlx, diesel,
//! > sea_orm, sqlalchemy, django.db, psycopg).
//!
//! The count is a shape question, not a syntax one: which *files* mention the database client,
//! not which calls they make. A hundred files each building their own query is a different
//! architecture from three modules behind a repository layer, and the number says which one a
//! project has.
//!
//! The client names are the catalog's, spelled as an import would spell them: the Python and
//! Rust ones are module or crate names, so the match is on the import line rather than on a
//! bare word — `use sqlx::PgPool` and `from sqlalchemy import create_engine` both contain the
//! name the catalog lists.
//!
//! A file that mentions two clients counts once: the question is how many files reach for the
//! database, not how many imports exist.

use std::collections::BTreeMap;

use substance_core::check::CheckId;
use substance_core::model::Finding;

use crate::checks::s02::as_i64;
use crate::checks::verdict::{StaticReport, cap};
use crate::file_finding::{self, Location};
use crate::tree::TrackedTree;

/// The database clients the catalog names.
pub const DB_CLIENTS: &[&str] = &[
    "prisma",
    "drizzle-orm",
    "knex",
    "typeorm",
    "pg",
    "sqlx",
    "diesel",
    "sea_orm",
    "sqlalchemy",
    "django.db",
    "psycopg",
];

/// Judges S10.
///
/// The text of every parsed-language file is read, because an import is a line of code. Files
/// this check does not read (a `.md` naming `prisma`) are not data access.
#[must_use]
pub async fn data_access_sprawl(run_id: &str, tree: &mut TrackedTree<'_>) -> StaticReport {
    let paths: Vec<String> = tree.paths().into_iter().map(str::to_owned).collect();
    let mut by_client: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    let mut files: Vec<String> = Vec::new();
    for path in paths {
        if crate::lang::Lang::from_path(std::path::Path::new(&path)).is_none() {
            continue;
        }
        let Some(text) = tree.text(&path).await else {
            continue;
        };
        let Some(client) = importing_client(&text) else {
            continue;
        };
        files.push(path.clone());
        by_client.entry(client).or_default().push(path);
    }
    let mut metrics: BTreeMap<String, i64> = BTreeMap::new();
    metrics.insert("files".to_owned(), as_i64(files.len()));
    for (client, paths) in &by_client {
        metrics.insert(format!("count.{client}"), as_i64(paths.len()));
    }
    let findings: Vec<Finding> = files
        .iter()
        .map(|path| {
            file_finding::at(
                run_id,
                CheckId::S10,
                "db-client-import",
                &format!("imports a database client: {path}"),
                Location::file(path),
            )
        })
        .collect();
    let (findings, cut) = cap(findings);
    StaticReport::reported(CheckId::S10, metrics)
        .with_findings(findings)
        .with_note(format!(
            "counted files, not call sites: a file importing two clients counts once ({})",
            DB_CLIENTS.len()
        ))
        .with_notes(cut.into_iter().collect())
}

/// The first catalog client the text imports, in catalog order.
///
/// An import line is required, not a bare mention: `// prisma is slow` in a comment is not an
/// import, while `import { PrismaClient } from "@prisma/client"` and
/// `use sqlx::PgPool;` both are.
#[must_use]
pub fn importing_client(text: &str) -> Option<&'static str> {
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !is_import_line(trimmed) {
            continue;
        }
        let found = DB_CLIENTS.iter().find(|client| mentions(trimmed, client));
        if let Some(client) = found {
            return Some(client);
        }
    }
    None
}

/// Whether the line is an import of some kind, in any of the five languages.
fn is_import_line(line: &str) -> bool {
    line.starts_with("import ")
        || line.starts_with("import{")
        || line.starts_with("from ")
        || line.starts_with("use ")
        || line.starts_with("const ")
        || line.starts_with("let ")
        || line.contains("require(")
        || line.starts_with("extern crate ")
}

/// Whether `line` mentions `client` as a whole name.
///
/// The trailing boundary matters: `pg` must not match `pgx`, and `diesel` must not match
/// `diesel_derives` (the catalog's client is the crate, and a derive macro is not the client).
fn mentions(line: &str, client: &str) -> bool {
    let mut from = 0;
    while let Some(found) = line[from..].find(client) {
        let start = from + found;
        let end = start + client.len();
        let before_ok = start == 0
            || !line.as_bytes()[start - 1].is_ascii_alphanumeric()
                && line.as_bytes()[start - 1] != b'_';
        let after_ok = end == line.len()
            || !line.as_bytes()[end].is_ascii_alphanumeric() && line.as_bytes()[end] != b'_';
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}
