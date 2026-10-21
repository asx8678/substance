//! A unified diff of two texts, for replay (M3-T07).
//!
//! Replay compares two records' normalized bytes. When they differ, a reader needs to see *where*,
//! and the answer has to be stable: the same two records must produce the same diff on every machine,
//! or a failing replay could not be pasted into an issue and compared.
//!
//! ## Why a hand-written LCS
//!
//! The alternative is a diff crate, and this is forty lines of dynamic programming over two short
//! texts. A dependency would be one more thing whose output could change between versions, and the
//! output here is part of substance's evidence rather than a convenience.
//!
//! The algorithm is the plain longest-common-subsequence table, then a walk back through it. It is
//! `O(n*m)` in time and memory, which is right for two canonical JSON documents (thousands of lines at
//! worst) and wrong for a whole file tree -- and nothing here diffs a file tree.
//!
//! ## Determinism
//!
//! When two lines could be treated as either deleted or inserted first, the walk back prefers the
//! deletion, always. Without that tie-break the same inputs could produce two different (both correct)
//! diffs depending on which branch a hash-map iteration happened to hit, and a diff is only useful if
//! re-running it says the same thing.

/// One run of equal, deleted or inserted lines.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op<'a> {
    /// Lines present in both.
    Same(&'a str),
    /// A line only in the old text.
    Remove(&'a str),
    /// A line only in the new text.
    Add(&'a str),
}

/// How much context each hunk carries, as `diff -u`'s default.
const CONTEXT: usize = 3;

/// A unified diff of `old` and `new`, with `old_name` and `new_name` in the `---`/`+++` headers.
///
/// Returns an empty string when the texts are identical: a caller that prints it unconditionally
/// then prints nothing, and there is no `---`/`+++` pair claiming a change that did not happen.
#[must_use]
pub fn unified(old: &str, new: &str, old_name: &str, new_name: &str) -> String {
    if old == new {
        return String::new();
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    let ops = walk(&old_lines, &new_lines);
    let hunks = hunks(&ops);

    let mut out = String::new();
    out.push_str("--- ");
    out.push_str(old_name);
    out.push('\n');
    out.push_str("+++ ");
    out.push_str(new_name);
    out.push('\n');
    for hunk in hunks {
        // The line numbers diff prints are 1-based and count the lines *before* the hunk's first line,
        // so a hunk that starts at the first line says `-1`.
        let old_start = hunk.old_start;
        let new_start = hunk.new_start;
        out.push_str(&format!(
            "@@ -{old_start},{old_count} +{new_start},{new_count} @@\n",
            old_count = hunk.old_count,
            new_count = hunk.new_count,
        ));
        for op in &ops[hunk.start..hunk.end] {
            match op {
                Op::Same(line) => {
                    out.push(' ');
                    out.push_str(line);
                    out.push('\n');
                }
                Op::Remove(line) => {
                    out.push('-');
                    out.push_str(line);
                    out.push('\n');
                }
                Op::Add(line) => {
                    out.push('+');
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// One hunk: a half-open range of `ops`, with the line numbers it starts at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Hunk {
    /// Where the hunk starts in `ops`.
    start: usize,
    /// Where it ends, exclusive.
    end: usize,
    /// The old file's line number of the first line the hunk covers.
    old_start: usize,
    /// The new file's line number of the first line the hunk covers.
    new_start: usize,
    /// How many old lines the hunk covers.
    old_count: usize,
    /// How many new lines the hunk covers.
    new_count: usize,
}

/// Groups the ops into hunks with [`CONTEXT`] lines around each change, merging ones that overlap.
fn hunks(ops: &[Op<'_>]) -> Vec<Hunk> {
    // The indices of the ops that are not equal: each of these has to appear in some hunk.
    let changes: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter_map(|(at, op)| match op {
            Op::Same(_) => None,
            _ => Some(at),
        })
        .collect();
    if changes.is_empty() {
        return Vec::new();
    }
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut start = changes[0].saturating_sub(CONTEXT);
    let mut end = (changes[0] + CONTEXT + 1).min(ops.len());
    for at in &changes[1..] {
        let candidate_start = at.saturating_sub(CONTEXT);
        if candidate_start <= end {
            // Overlapping or adjacent contexts belong in one hunk, which is what diff does: two
            // hunks a line apart are harder to read than one.
            end = (at + CONTEXT + 1).min(ops.len());
            continue;
        }
        hunks.push(finish(ops, start, end));
        start = candidate_start;
        end = (at + CONTEXT + 1).min(ops.len());
    }
    hunks.push(finish(ops, start, end));
    hunks
}

/// Fills in a hunk's line numbers from the ops it covers.
fn finish(ops: &[Op<'_>], start: usize, end: usize) -> Hunk {
    let mut old_line = 1;
    let mut new_line = 1;
    for op in &ops[..start] {
        match op {
            Op::Same(_) => {
                old_line += 1;
                new_line += 1;
            }
            Op::Remove(_) => old_line += 1,
            Op::Add(_) => new_line += 1,
        }
    }
    let mut old_count = 0;
    let mut new_count = 0;
    for op in &ops[start..end] {
        match op {
            Op::Same(_) => {
                old_count += 1;
                new_count += 1;
            }
            Op::Remove(_) => old_count += 1,
            Op::Add(_) => new_count += 1,
        }
    }
    Hunk {
        start,
        end,
        old_start: old_line,
        new_start: new_line,
        old_count,
        new_count,
    }
}

/// The op sequence for the longest common subsequence of `old` and `new`.
///
/// The table is built backwards (`table[i][j]` is the LCS length of the suffixes), so the walk
/// forwards always sees a decision it can make from the values it has already read.
fn walk<'a>(old: &[&'a str], new: &[&'a str]) -> Vec<Op<'a>> {
    let rows = old.len() + 1;
    let columns = new.len() + 1;
    // One flat `Vec` rather than a `Vec<Vec<_>>`: the table is dense and this keeps it contiguous,
    // which matters at `n*m` entries.
    let mut table = vec![0_u32; rows * columns];
    let at = |row: usize, column: usize| row * columns + column;
    for row in (0..old.len()).rev() {
        for column in (0..new.len()).rev() {
            table[at(row, column)] = if old[row] == new[column] {
                table[at(row + 1, column + 1)] + 1
            } else {
                table[at(row + 1, column)].max(table[at(row, column + 1)])
            };
        }
    }

    let mut ops = Vec::with_capacity(old.len().max(new.len()));
    let (mut row, mut column) = (0, 0);
    while row < old.len() && column < new.len() {
        if old[row] == new[column] {
            ops.push(Op::Same(old[row]));
            row += 1;
            column += 1;
        } else if table[at(row + 1, column)] >= table[at(row, column + 1)] {
            // The tie-break, and it is deliberate: prefer the removal, so equal inputs always give
            // the same diff.
            ops.push(Op::Remove(old[row]));
            row += 1;
        } else {
            ops.push(Op::Add(new[column]));
            column += 1;
        }
    }
    for line in &old[row..] {
        ops.push(Op::Remove(line));
    }
    for line in &new[column..] {
        ops.push(Op::Add(line));
    }
    ops
}
