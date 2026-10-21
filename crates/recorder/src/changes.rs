//! Parsing logical-decoding output into per-table change counts (M3-T03).
//!
//! Two plugins, one shape. `wal2json` says `{"action":"I","schema":"public","table":"notes",...}`;
//! `test_decoding` says `table public.notes: INSERT: id[integer]:1 body[text]:'hi'`. Both are
//! text Postgres gave us and neither is JSON we control, so both parsers are total functions over
//! `&str`: a line they do not understand is reported, never guessed at, and never panics.
//!
//! The counts are what a check reads ("did this action write a row?"), and the row images are what
//! an oracle reads when the count is not the question: B11 judges an after-image, B06/D03/B23
//! compare a stored value with what was typed, B25 looks for one user's email in another user's
//! rows. So a change carries both images, column by column.
//!
//! ## Why the images hold values and the record does not
//!
//! The full changes live only in the rows blob, which is a local, content-addressed run artifact
//! -- the same kind of object as a response body. The record itself keeps counts and a reference
//! to that blob, so a report that is attached to a ticket never carries a row's contents. Nothing
//! in this module logs a value: a password hash is evidence (B12, B13 judge exactly that), and
//! `tracing` is never given one.
//!
//! ## Why numbers are text
//!
//! A cell is a [`CellValue`]: null, bool, or text. Integers and decimals arrive as text, because a
//! float here would make two equal rows compare unequal after a round trip, and a record has to be
//! byte-identical across runs. The engine's own type name is not kept: a check compares what was
//! written with what was typed, and both are text at the boundaries that matter.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A change's kind, as the record reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChangeKind {
    /// A row appeared.
    Insert,
    /// A row changed.
    Update,
    /// A row disappeared.
    Delete,
}

impl ChangeKind {
    /// The word the record and the report use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// One cell of a row image.
///
/// Three variants and no float: see this module's header. Serialized as the JSON scalar it is
/// (`null`, `true`, `"text"`), so the rows blob reads like a row rather than like a tagged union.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CellValue {
    /// SQL `NULL`.
    Null,
    /// A boolean, whatever type the engine called it.
    Bool(bool),
    /// Everything else, including numbers, as text.
    Text(String),
}

impl CellValue {
    /// The text form, for a key or a comparison that does not care about the JSON type.
    #[must_use]
    pub fn as_text(&self) -> String {
        match self {
            Self::Null => "null".to_owned(),
            Self::Bool(value) => value.to_string(),
            Self::Text(text) => text.clone(),
        }
    }
}

/// A row image: the columns the engine reported, by name.
///
/// A `BTreeMap` rather than an insertion-ordered one because the rows blob is canonical JSON:
/// two engines that report the same columns in a different order must produce the same bytes.
pub type RowImage = BTreeMap<String, CellValue>;

/// One change, with the images an oracle judges.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RowChange {
    /// The table, schema-qualified when the plugin gave a schema.
    pub table: String,
    /// The kind.
    pub operation: String,
    /// The row's identity, as `name=value` pairs, when the engine reported one.
    ///
    /// Derived from the image that holds the old row (an update's identity, a delete's row) or,
    /// for an insert, from the columns that look like an identity. A convenience for a reader and
    /// a stable sort key; the images are the evidence.
    pub key: Option<String>,
    /// The row after the change: an insert's row, or an update's new tuple.
    ///
    /// An update reports only the columns that changed, unless the engine was configured to send
    /// the whole tuple; a check that needs an untouched column must not read it from here.
    pub new: Option<RowImage>,
    /// The row before the change: a delete's row, or an update's old key.
    pub old: Option<RowImage>,
}

/// The changes one capture window saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// Every change, in the order the server reported them.
    pub changes: Vec<RowChange>,
    /// Lines the plugin emitted that this parser did not understand.
    ///
    /// Kept rather than dropped: a plugin upgrade that changes its output must be visible, and a
    /// count that silently missed half the changes would be worse than a count that says so.
    pub unparsed: Vec<String>,
}

impl ChangeSet {
    /// Counts by operation and table: `insert` → `notes` → 3.
    #[must_use]
    pub fn counts(&self) -> BTreeMap<String, BTreeMap<String, u32>> {
        let mut counts: BTreeMap<String, BTreeMap<String, u32>> = BTreeMap::new();
        for change in &self.changes {
            let by_table = counts.entry(change.operation.clone()).or_default();
            let counter = by_table.entry(change.table.clone()).or_insert(0);
            *counter = counter.saturating_add(1);
        }
        counts
    }

    /// The changes of one kind, when a caller wants just those.
    ///
    /// Not `#[must_use]`: the iterator already is, and the attribute on a function that returns
    /// one is what clippy's `must_use_unit`-adjacent lint rejects.
    pub fn of_kind(&self, kind: ChangeKind) -> impl Iterator<Item = &RowChange> {
        let wanted = kind.as_str();
        self.changes
            .iter()
            .filter(move |change| change.operation == wanted)
    }
}

/// Why a line could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// The line is not JSON, though the plugin is `wal2json`.
    #[error("wal2json: not JSON: {0}")]
    Json(String),
    /// The JSON is not the shape `wal2json` documents.
    #[error("wal2json: unexpected shape: {0}")]
    Shape(String),
    /// The line is not the shape `test_decoding` emits.
    #[error("test_decoding: unexpected line: {0}")]
    TestDecoding(String),
    /// The line is a transaction boundary: `wal2json`'s begin/commit/message marker.
    ///
    /// A separate variant rather than an `Ok(vec![])` because the two cases differ in what they
    /// mean to a caller: an empty vector is "a row line with no rows", which does not exist, and
    /// this is "not a row line at all". Callers treat it as no change; nothing records it as a
    /// parse failure.
    #[error("wal2json: transaction marker")]
    Marker,
}

/// Parses one line of `wal2json` v2 output.
///
/// A `begin` or `commit` line has no changes and is not an error. A line this cannot read is a
/// [`ParseError`]; the caller records it as unparsed rather than losing it.
///
/// # Errors
///
/// Returns [`ParseError::Json`] when the line is not JSON and [`ParseError::Shape`] when it is
/// JSON of a shape this parser does not know.
pub fn wal2json_line(line: &str) -> Result<Vec<RowChange>, ParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|error| ParseError::Json(error.to_string()))?;

    // v2 emits one JSON object per line; a wrapper object with a "change" array is also accepted
    // because some callers collect a transaction into one line.
    if let Some(changes) = value.get("change").and_then(serde_json::Value::as_array) {
        let mut out = Vec::with_capacity(changes.len());
        for entry in changes {
            out.push(wal2json_change(entry)?);
        }
        return Ok(out);
    }

    match wal2json_change(&value) {
        Ok(change) => Ok(vec![change]),
        Err(error) => Err(error),
    }
}

/// The actions `wal2json` emits that are transaction boundaries rather than rows.
///
/// They carry an `action` field just like a row change does -- `B` for begin, `C` for commit,
/// `M` for a message -- which is why they cannot be recognized by the absence of one. Treating
/// them as unparsed would put two entries in every record's "could not read" list and teach a
/// reader to ignore that list.
const WAL2JSON_MARKERS: &[&str] = &["B", "C", "M", "T"];

/// One `wal2json` entry.
fn wal2json_change(value: &serde_json::Value) -> Result<RowChange, ParseError> {
    let action = value
        .get("action")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ParseError::Shape(format!("no action in {value}")))?;
    let kind = match action {
        "I" => ChangeKind::Insert,
        "U" => ChangeKind::Update,
        "D" => ChangeKind::Delete,
        other if WAL2JSON_MARKERS.contains(&other) => {
            // A transaction boundary: real output, no row. Signalled to the caller by `None`.
            return Err(ParseError::Marker);
        }
        other => {
            return Err(ParseError::Shape(format!(
                "unknown action {other:?} (expected I, U or D)"
            )));
        }
    };
    let table = value
        .get("table")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ParseError::Shape(format!("no table in {value}")))?;
    let schema = value.get("schema").and_then(serde_json::Value::as_str);
    let qualified = match schema {
        Some(schema) if !schema.is_empty() => format!("{schema}.{table}"),
        _ => table.to_owned(),
    };

    // `wal2json` v2 names the tuples: an insert has `columns`, an update has `columns` (the new
    // tuple, either just what changed or the whole row, depending on configuration) and
    // `identity` (the old tuple's key), a delete has `identity` (the row as it was).
    let new = image_of(value, "columns");
    let old = image_of(value, "identity");
    let key = identity_of(old.as_ref().or(new.as_ref()));
    Ok(RowChange {
        table: qualified,
        operation: kind.as_str().to_owned(),
        key,
        new,
        old,
    })
}

/// A `{"name":…,"value":…}` array as a row image.
fn image_of(value: &serde_json::Value, field: &str) -> Option<RowImage> {
    let entries = value.get(field).and_then(serde_json::Value::as_array)?;
    let mut image = RowImage::new();
    for entry in entries {
        let Some(name) = entry.get("name").and_then(serde_json::Value::as_str) else {
            continue;
        };
        let cell = entry.get("value").map(cell_of).unwrap_or(CellValue::Null);
        image.insert(name.to_owned(), cell);
    }
    match image.is_empty() {
        true => None,
        false => Some(image),
    }
}

/// A JSON value as a cell: numbers stay text, so no float ever enters a record.
fn cell_of(value: &serde_json::Value) -> CellValue {
    match value {
        serde_json::Value::Null => CellValue::Null,
        serde_json::Value::Bool(flag) => CellValue::Bool(*flag),
        serde_json::Value::String(text) => CellValue::Text(text.clone()),
        other => CellValue::Text(other.to_string()),
    }
}

/// The identity columns of an image, as `name=value` pairs.
fn identity_of(image: Option<&RowImage>) -> Option<String> {
    let image = image?;
    let parts: Vec<String> = image
        .iter()
        .filter(|(name, _)| is_identity_name(name))
        .map(|(name, value)| format!("{name}={}", value.as_text()))
        .collect();
    match parts.is_empty() {
        true => None,
        false => Some(parts.join(",")),
    }
}

/// Whether a column name looks like an identity rather than content.
fn is_identity_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "id" || lower == "uuid" || lower.ends_with("_id") || lower.ends_with("_uuid")
}

/// Parses one line of `test_decoding` output.
///
/// The shapes are:
/// ```text
/// table public.notes: INSERT: id[integer]:1 body[text]:'hello'
/// table public.notes: UPDATE: old-key: id[integer]:1 new-tuple: id[integer]:1 body[text]:'hi'
/// table public.notes: DELETE: id[integer]:1
/// ```
/// plus `BEGIN`, `COMMIT` and `COMMIT PREPARED` markers, which have no row.
///
/// # Errors
///
/// Returns [`ParseError::TestDecoding`] for a line that is neither a marker nor a `table` line.
pub fn test_decoding_line(line: &str) -> Result<Vec<RowChange>, ParseError> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed == "BEGIN" || trimmed.starts_with("COMMIT") {
        return Ok(Vec::new());
    }
    let Some(rest) = trimmed.strip_prefix("table ") else {
        return Err(ParseError::TestDecoding(trimmed.to_owned()));
    };
    // `table <qualified name>: <OP>: <payload>`
    let Some((table, remainder)) = rest.split_once(':') else {
        return Err(ParseError::TestDecoding(trimmed.to_owned()));
    };
    let remainder = remainder.trim_start();
    let (operation, payload) = match remainder.split_once(':') {
        Some((operation, payload)) => (operation.trim(), payload.trim()),
        None => (remainder.trim(), ""),
    };
    let kind = match operation {
        "INSERT" => ChangeKind::Insert,
        "UPDATE" => ChangeKind::Update,
        "DELETE" => ChangeKind::Delete,
        // TRUNCATE, or a format this parser does not know: reported, not counted.
        other => {
            return Err(ParseError::TestDecoding(format!(
                "unknown operation {other:?} in {trimmed}"
            )));
        }
    };
    // An update prints two tuples, `old-key: … new-tuple: …`; an insert and a delete print one.
    let (old, new) = match kind {
        ChangeKind::Update => match payload.split_once("new-tuple:") {
            Some((old_part, new_part)) => (
                pairs_image(old_part.trim().trim_start_matches("old-key:").trim()),
                pairs_image(new_part.trim()),
            ),
            // The two-tuple form is what this parser knows; a one-tuple update is reported rather
            // than guessed at, because reading it as the new tuple would lose the old values a
            // check may need to compare against.
            None => {
                return Err(ParseError::TestDecoding(format!(
                    "update without new-tuple: {trimmed}"
                )));
            }
        },
        ChangeKind::Insert => (None, pairs_image(payload)),
        ChangeKind::Delete => (pairs_image(payload), None),
    };
    let key = identity_of(old.as_ref().or(new.as_ref()));
    Ok(vec![RowChange {
        table: table.trim().to_owned(),
        operation: kind.as_str().to_owned(),
        key,
        new,
        old,
    }])
}

/// Parses a `test_decoding` column list into a row image.
///
/// `test_decoding` prints `name[type]:value` pairs separated by spaces. Values can hold spaces
/// (`'a b'`), so splitting on spaces is not enough; a value runs to the next `name[type]:`
/// sequence or the end of the text. The type name is dropped: a check compares values, and the
/// engine's spelling of a type is not part of what was written.
fn pairs_image(text: &str) -> Option<RowImage> {
    let mut image = RowImage::new();
    let mut remainder = text;
    while let Some((name, value, consumed)) = split_pair(remainder) {
        image.insert(name.to_owned(), cell_from_test_decoding(&value));
        // `consumed` is an index into `remainder`; the next pair starts after the space that
        // separated them, and skipping it is what keeps `split_pair` from reading the same
        // value again.
        if consumed >= remainder.len() {
            break;
        }
        remainder = &remainder[consumed + 1..];
    }
    match image.is_empty() {
        true => None,
        false => Some(image),
    }
}

/// A `test_decoding` value as a cell.
///
/// `test_decoding` writes `null` for SQL NULL and quotes text with single quotes. Quoted text is
/// always text, even when it reads like a number; an unquoted `null` is NULL.
fn cell_from_test_decoding(raw: &str) -> CellValue {
    if raw == "null" {
        return CellValue::Null;
    }
    CellValue::Text(raw.to_owned())
}

/// Splits one `name[type]:value` pair: the name, the value, and where the pair ended.
fn split_pair(text: &str) -> Option<(&str, String, usize)> {
    let open = text.find('[')?;
    // `]:` is two bytes, so the value starts after both.
    let close = text[open..].find("]:")? + open;
    let name = text[..open].trim();
    let value_start = close + 2;
    if value_start > text.len() {
        return None;
    }
    let value_text = &text[value_start..];
    let next = find_next_pair(value_text);
    let raw = match next {
        Some(index) => &value_text[..index],
        None => value_text,
    };
    let value = raw.trim();
    // The quotes are `test_decoding`'s way of delimiting text; a value that itself contains a
    // quote is doubled, so stripping one surrounding pair is the inverse.
    let value = match value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
        true => value[1..value.len() - 1].replace("''", "'"),
        false => value.to_owned(),
    };
    let consumed = value_start + next.unwrap_or(value_text.len());
    Some((name, value, consumed))
}

/// Where the next `name[type]:` starts, if there is one.
fn find_next_pair(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    for index in 0..bytes.len() {
        // A pair boundary is a space followed by a name and a '[', which is what `test_decoding`
        // separates columns with.
        if bytes.get(index) == Some(&b' ') {
            let rest = &text[index + 1..];
            if let Some(open) = rest.find('[')
                && rest[..open]
                    .chars()
                    .all(|ch| ch.is_alphanumeric() || ch == '_')
                && open > 0
            {
                return Some(index);
            }
        }
    }
    None
}

/// Parses a whole capture, line by line, keeping what it could not read.
#[must_use]
pub fn parse_capture(output: &str, plugin: &str) -> ChangeSet {
    let mut set = ChangeSet::default();
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed = match plugin {
            "wal2json" => wal2json_line(line),
            _ => test_decoding_line(line),
        };
        match parsed {
            Ok(changes) => set.changes.extend(changes),
            Err(ParseError::Marker) => {}
            Err(error) => set.unparsed.push(error.to_string()),
        }
    }
    set
}
