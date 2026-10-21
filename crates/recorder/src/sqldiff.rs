//! Parsing `sqldiff` output into per-table change counts (M3-T03).
//!
//! `sqldiff --changes db1 db2` prints one `UPDATE`, `INSERT` or `DELETE` statement per changed row,
//! each preceded by a comment naming the table:
//!
//! ```sql
//! -- UPDATE notes SET body='edited' WHERE id=7;
//! UPDATE notes SET body='edited' WHERE id=7;
//! ```
//!
//! The counts are what a check reads. The statements themselves are not kept: a record does not
//! need the app's row contents, and copying them into an artifact a report attaches is how a
//! database ends up in a bug tracker.

use std::collections::BTreeMap;

use super::changes::{CellValue, RowChange, RowImage};

/// One changed row, as `sqldiff` described it.
///
/// The same type the logical-decoding parsers produce: a row change is a row change whichever
/// engine noticed it, and one type means the recorder's counts and rows blob work for both
/// without a conversion that could quietly drop the row key.
pub type SqliteChange = RowChange;

/// What one diff found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SqliteDiff {
    /// The changes, in the order `sqldiff` printed them.
    pub changes: Vec<SqliteChange>,
    /// Lines that looked like content but could not be read.
    pub unparsed: Vec<String>,
}

impl SqliteDiff {
    /// Counts by operation and table.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<String, BTreeMap<String, u32>> {
        let mut counts: BTreeMap<String, BTreeMap<String, u32>> = BTreeMap::new();
        for change in &self.changes {
            let counter = counts
                .entry(change.operation.clone())
                .or_default()
                .entry(change.table.clone())
                .or_insert(0);
            *counter = counter.saturating_add(1);
        }
        counts
    }
}

/// Parses a whole `sqldiff` run.
///
/// A comment line is what names the table for the statement that follows, so comments are read
/// rather than skipped. Everything else that is not an `INSERT`, `UPDATE` or `DELETE` statement is
/// ignored: `sqldiff` also emits a trailing newline, and a schema-only difference produces nothing
/// at all.
#[must_use]
pub fn parse(output: &str) -> SqliteDiff {
    let mut diff = SqliteDiff::default();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }
        match parse_statement(trimmed) {
            Some(change) => diff.changes.push(change),
            None => {
                if starts_like_change(trimmed) {
                    diff.unparsed.push(trimmed.to_owned());
                }
            }
        }
    }
    diff
}

/// Whether a line starts like one of the three statements `sqldiff` emits.
fn starts_like_change(line: &str) -> bool {
    let head = line.get(..6).unwrap_or(line).to_ascii_uppercase();
    head.starts_with("INSERT") || head.starts_with("UPDATE") || head.starts_with("DELETE")
}

/// Strips a keyword case-insensitively, returning what follows it.
fn strip_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let head = line.get(..keyword.len())?;
    if head.eq_ignore_ascii_case(keyword) {
        return line.get(keyword.len()..);
    }
    None
}

/// A quoted identifier without its quotes.
fn clean_identifier(text: &str) -> String {
    text.trim_matches(['"', '\'', '`', ';']).to_owned()
}

/// One `sqldiff` statement, as a change.
///
/// The three statements map onto the same images the Postgres parsers produce, so an oracle reads
/// one shape whichever engine the app used:
///
/// - `INSERT INTO t(cols) VALUES(...)` → `new` is the inserted row.
/// - `UPDATE t SET a=…, b=… WHERE key` → `new` is the assigned columns; `old` is the `WHERE`
///   equality, which is the key the row had.
/// - `DELETE FROM t WHERE key` → `old` is the `WHERE` equality, the row as it was last known.
///
/// \`sqldiff --changes\` does not print a deleted row's other columns, so \`old\` holds the key alone;
/// a check that needs a deleted row's body must not read it from here, and the counts still say
/// the delete happened.
fn parse_statement(line: &str) -> Option<SqliteChange> {
    let statement = line.trim_end_matches(';');
    if let Some(rest) = strip_keyword(statement, "INSERT INTO ") {
        // `INSERT INTO notes(id,body) VALUES(...);` -- the table runs to `(` or whitespace.
        let table = clean_identifier(rest.split(['(', ' ']).next().unwrap_or(rest));
        let new = insert_image(rest);
        let key = identity_of(new.as_ref());
        return Some(SqliteChange {
            table,
            operation: "insert".to_owned(),
            key,
            new,
            old: None,
        });
    }
    if let Some(rest) = strip_keyword(statement, "DELETE FROM ") {
        let table = clean_identifier(rest.split_whitespace().next().unwrap_or(rest));
        let old = where_image(rest);
        let key = identity_of(old.as_ref()).or_else(|| where_clause(rest));
        return Some(SqliteChange {
            table,
            operation: "delete".to_owned(),
            key,
            new: None,
            old,
        });
    }
    if let Some(rest) = strip_keyword(statement, "UPDATE ") {
        // `UPDATE <table> SET ... WHERE <key>;`
        let table = clean_identifier(rest.split_whitespace().next().unwrap_or(rest));
        let new = update_image(rest);
        let old = where_image(rest);
        let key = identity_of(old.as_ref()).or_else(|| where_clause(rest));
        return Some(SqliteChange {
            table,
            operation: "update".to_owned(),
            key,
            new,
            old,
        });
    }
    None
}

/// The values of an `INSERT`'s column list and `VALUES` tuple, by column name.
fn insert_image(rest: &str) -> Option<RowImage> {
    let open = rest.find('(')?;
    let close = rest[open..].find(')')? + open;
    let columns = split_list(rest.get(open + 1..close)?);
    let values_at = rest[close..].to_ascii_uppercase().find("VALUES")? + close;
    let tuple = rest.get(values_at..)?;
    let tuple_open = tuple.find('(')?;
    let tuple_close = tuple[tuple_open..].rfind(')')? + tuple_open;
    let values = split_list(tuple.get(tuple_open + 1..tuple_close)?);
    let mut image = RowImage::new();
    for (column, value) in columns.iter().zip(values.iter()) {
        image.insert(clean_identifier(column), cell_from_sql(value));
    }
    match image.is_empty() {
        true => None,
        false => Some(image),
    }
}

/// The assigned columns of an `UPDATE`, by name: everything between `SET` and `WHERE`.
fn update_image(rest: &str) -> Option<RowImage> {
    let text = match where_start(rest) {
        Some(index) => rest.get(..index)?,
        None => rest,
    };
    let upper = text.to_ascii_uppercase();
    let set_at = upper.find(" SET ")?;
    let assignments = text.get(set_at + " SET ".len()..)?;
    let mut image = RowImage::new();
    // Assignments are comma-separated; a value may itself contain a comma, so the split is done
    // on `, ` followed by an identifier and `=`, which is the shape `sqldiff` prints.
    for assignment in split_assignments(assignments) {
        let Some((name, value)) = assignment.split_once('=') else {
            continue;
        };
        image.insert(clean_identifier(name), cell_from_sql(value));
    }
    match image.is_empty() {
        true => None,
        false => Some(image),
    }
}

/// The row a `WHERE` clause describes, when the clause is a chain of equalities.
///
/// `sqldiff` prints `WHERE id=7`, and `WHERE id=7 AND tenant=2` for a composite key. A clause that
/// is not a chain of equalities is not a row image, so it yields `None` and only the textual
/// [`where_clause`] remains.
fn where_image(rest: &str) -> Option<RowImage> {
    let clause = where_clause(rest)?;
    let mut image = RowImage::new();
    for part in clause.split(" AND ") {
        let (name, value) = part.split_once('=')?;
        image.insert(clean_identifier(name), cell_from_sql(value));
    }
    match image.is_empty() {
        true => None,
        false => Some(image),
    }
}

/// Splits a comma-separated column list, respecting quotes.
fn split_list(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for ch in text.chars() {
        match ch {
            '\'' => {
                quoted = !quoted;
                current.push(ch);
            }
            ',' if !quoted => {
                parts.push(current.trim().to_owned());
                current = String::new();
            }
            other => current.push(other),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_owned());
    }
    parts
}

/// Splits `a=1, b='x,y'` into its assignments.
fn split_assignments(text: &str) -> Vec<String> {
    split_list(text)
}

/// Where the `WHERE` keyword starts, if the statement has one.
fn where_start(text: &str) -> Option<usize> {
    text.to_ascii_uppercase().find(" WHERE ")
}

/// A SQL literal as a cell: quoted text loses its quotes, `NULL` is null.
fn cell_from_sql(raw: &str) -> CellValue {
    let value = raw.trim();
    if value.eq_ignore_ascii_case("NULL") {
        return CellValue::Null;
    }
    if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        return CellValue::Text(value[1..value.len() - 1].replace("''", "'"));
    }
    CellValue::Text(value.to_owned())
}

/// The identity columns of an image, as `name=value` pairs.
fn identity_of(image: Option<&RowImage>) -> Option<String> {
    let image = image?;
    let parts: Vec<String> = image
        .iter()
        .filter(|(name, _)| {
            let lower = name.to_ascii_lowercase();
            lower == "id" || lower == "uuid" || lower.ends_with("_id") || lower.ends_with("_uuid")
        })
        .map(|(name, value)| format!("{name}={}", value.as_text()))
        .collect();
    match parts.is_empty() {
        true => None,
        false => Some(parts.join(",")),
    }
}

/// The `WHERE` clause of a statement, which is the row's identity.
fn where_clause(text: &str) -> Option<String> {
    let upper = text.to_ascii_uppercase();
    let index = upper.find(" WHERE ")?;
    let clause = text
        .get(index + " WHERE ".len()..)?
        .trim()
        .trim_end_matches(';');
    match clause.is_empty() {
        true => None,
        false => Some(clause.to_owned()),
    }
}
