import type { NormalizePattern } from "./generated/NormalizePattern.js";

/**
 * The rules that make two runs of the same app produce the same bytes.
 *
 * **There is no built-in list here.** The rules are `rules/normalize.toml`, embedded in Rust, and
 * `open_context` carries them over. Rust is the source of truth (M3-T05) because the recorder
 * normalizes bodies the browser never saw -- a server's response inside a record, a database value --
 * and two lists would eventually disagree. A disagreement is not a visible failure: it is two runs
 * that hash differently for a reason nobody can find.
 *
 * An earlier version kept a copy of the defaults in this file. That copy is gone on purpose: with
 * both lists live, a rule added in Rust would silently not apply in the browser, and a page's DOM
 * hash would keep a value the record's other hashes had already replaced.
 */

/**
 * Applies the rules in order, globally.
 *
 * The `g` flag is required, not optional: `String.replace` with a non-global pattern replaces only
 * the *first* occurrence, so a page with two timestamps would have one normalized and one not, and
 * two runs that differed only in the first timestamp would still differ. (Measured: it is the reason
 * a bare epoch survived in the first version of this function.)
 *
 * A fresh `RegExp` is built per call rather than kept, because a global regex carries `lastIndex`
 * between calls; rebuilding means one page's scan cannot start where the previous ended.
 */
export function normalize(text: string, patterns: readonly NormalizePattern[]): string {
  let out = text;
  for (const pattern of patterns) {
    let regex: RegExp;
    try {
      regex = new RegExp(pattern.regex, "g");
    } catch {
      // Rust already refused anything outside the shared subset, so a pattern that fails here is a
      // protocol bug rather than a project's mistake. Throwing names it instead of skipping a rule
      // and producing records that silently do not match.
      throw new Error(`normalize: invalid pattern ${JSON.stringify(pattern.regex)}`);
    }
    out = out.replace(regex, pattern.replace);
  }
  return out;
}

/**
 * Whether a URL belongs to a pattern from the idle baseline, for the settle in-flight counter.
 *
 * The baseline writes a *path template* (`GET /api/notifications?since=:id`), not a regex, so the
 * match is on the method and the templated path: `:id` stands for whatever that segment was, which
 * is what makes one pattern cover a poll whose query changes every time.
 */
export function matchesIdle(
  patterns: readonly string[],
  method: string,
  path: string,
): boolean {
  const entry = `${method.toUpperCase()} ${path}`;
  return patterns.some((pattern) => templateMatches(pattern, entry));
}

/**
 * Whether one `METHOD /templated/path` pattern matches one `METHOD /path` string.
 *
 * The path and the query are compared separately, which is not a detail: with the query left
 * attached to the last path segment, `GET /api/notifications?since=:id` would "match"
 * `GET /api/notifications` because that segment looked like a placeholder. Splitting first is what
 * keeps a poll's own URL from being ignored only when it has a query, or ignored when it does not.
 *
 * This mirrors `template_matches` in `crates/core/src/normalize.rs`; the two are pinned against the
 * same cases by the task's probes.
 */
export function templateMatches(pattern: string, entry: string): boolean {
  const [patternMethod, ...patternRest] = pattern.split(" ");
  const [entryMethod, ...entryRest] = entry.split(" ");
  if (patternMethod !== entryMethod) {
    return false;
  }
  const [patternPath, patternQuery] = splitQuery(patternRest.join(" "));
  const [entryPath, entryQuery] = splitQuery(entryRest.join(" "));
  return partsMatch(patternPath, entryPath) && partsMatch(patternQuery, entryQuery);
}

/** A path or query split from its `?`. */
function splitQuery(value: string): [string, string] {
  const at = value.indexOf("?");
  return at === -1 ? [value, ""] : [value.slice(0, at), value.slice(at + 1)];
}

/**
 * Whether every `/`- or `&`-separated part of `entry` matches `pattern`, allowing placeholders.
 *
 * Both sides empty is a match; one side empty and the other not is not, which is how a request
 * without a query stops matching a pattern that has one.
 */
function partsMatch(pattern: string, entry: string): boolean {
  if (pattern === entry) {
    return true;
  }
  if (pattern === "" || entry === "") {
    return false;
  }
  const separator = pattern.includes("&") || entry.includes("&") ? "&" : "/";
  const patternParts = pattern.split(separator);
  const entryParts = entry.split(separator);
  if (patternParts.length !== entryParts.length) {
    return false;
  }
  return patternParts.every((part, index) => part === entryParts[index] || isPlaceholder(part));
}

/**
 * Whether one segment of a pattern is a placeholder or a placeholder-bearing query value.
 *
 * `:id` is a whole-segment placeholder; `since=:id` keeps a key and makes its value the
 * placeholder. Both appear in a request map, so both are matched here rather than at each caller.
 */
function isPlaceholder(part: string): boolean {
  if (part === ":id" || part.startsWith(":")) {
    return true;
  }
  const equals = part.indexOf("=");
  if (equals === -1) {
    return false;
  }
  const value = part.slice(equals + 1);
  return value.startsWith(":");
}

/**
 * The templated form of a request path: numeric and UUID-shaped segments become `:id`.
 *
 * This is what the idle baseline records, so a poll whose id changes every second is one pattern
 * rather than one per request. Query values are templated the same way.
 */
export function templateOf(path: string): string {
  const queryAt = path.indexOf("?");
  const [head, query] = queryAt === -1 ? [path, ""] : [path.slice(0, queryAt), path.slice(queryAt)];
  const templated = head
    .split("/")
    .map((segment) => (isIdentifier(segment) ? ":id" : segment))
    .join("/");
  return `${templated}${templateQuery(query)}`;
}

/**
 * A query string with identifier-shaped values replaced.
 *
 * Keys are kept and order is preserved: two requests that differ only in an id must template to the
 * same string, while a request with a different *set* of keys must not.
 */
function templateQuery(query: string): string {
  if (query === "") {
    return "";
  }
  const parts = query.slice(1).split("&");
  const templated = parts.map((part) => {
    const equals = part.indexOf("=");
    if (equals === -1) {
      return isIdentifier(part) ? ":id" : part;
    }
    const key = part.slice(0, equals);
    const value = part.slice(equals + 1);
    return isIdentifier(value) ? `${key}=:id` : `${key}=${value}`;
  });
  return `?${templated.join("&")}`;
}

/**
 * Whether a path segment or query value is an identifier rather than a word.
 *
 * Deliberately narrow: all digits, a UUID, or a long mixed-case run. A slug such as `invoices` stays
 * as it is, because templating it would merge two different screens into one idle pattern.
 */
export function isIdentifier(value: string): boolean {
  if (value.length === 0) {
    return false;
  }
  if (/^\d+$/.test(value)) {
    return true;
  }
  if (/^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/.test(value)) {
    return true;
  }
  // A ULID or a database id: long, alphanumeric, and not a word.
  return value.length >= 20 && /^[A-Za-z0-9_-]+$/.test(value);
}
