import type { NormalizePattern } from "./generated/NormalizePattern.js";

/**
 * The rules that make two runs of the same app produce the same bytes.
 *
 * The project supplies patterns in `substance.toml`; these are the ones no project should have to
 * restate, because every app has them and forgetting one turns a green run red. Each is written
 * against a shape that appears in serialized HTML or JSON, not against a word:
 * timestamps, UUIDs, hex hashes, CSRF/nonce-style attributes, cache-busting query values.
 *
 * The list is applied before hashing and before diffing, and in a fixed order: the timestamp
 * patterns run first, because an ISO instant and a bare epoch millisecond would otherwise be
 * partly eaten by the hex rule.
 */
export const BUILTIN_PATTERNS: readonly NormalizePattern[] = [
  // An ISO 8601 instant, with or without milliseconds or a zone.
  { regex: "\\d{4}-\\d{2}-\\d{2}T\\d{2}:\\d{2}:\\d{2}(?:\\.\\d+)?(?:Z|[+-]\\d{2}:\\d{2})", replace: "<time>" },
  // An HTTP date, RFC 1123.
  {
    regex: "[A-Z][a-z]{2}, \\d{2} [A-Z][a-z]{2} \\d{4} \\d{2}:\\d{2}:\\d{2} GMT",
    replace: "<time>",
  },
  // A bare epoch in seconds or milliseconds.
  { regex: "\\b1[6-9]\\d{8}(?:\\d{3})?\\b", replace: "<epoch>" },
  // A UUID of any version.
  {
    regex:
      "\\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\\b",
    replace: "<uuid>",
  },
  // A long hex run: content hashes in asset names, signed values, nonces.
  { regex: "\\b[0-9a-f]{16,}\\b", replace: "<hex>" },
  // CSRF-style values: a meta tag's `content`, an inline `token: "..."`, a form's hidden value.
  // The three shapes are separate because the value's delimiter differs: a quoted attribute, a
  // quoted JSON string, and an unquoted attribute value that runs to whitespace or `>`.
  {
    regex:
      "((?:csrf|xsrf|nonce|authenticity)[-_]?token[\\w-]*[\"']?(?:\\s+(?:content|value))?[\"']?\\s*[:=]\\s*)([\"'])[^\"']*\\2",
    replace: "$1$2<token>$2",
  },
  {
    regex: "((?:csrf|xsrf|nonce|authenticity)[-_]?token[\\w-]*[\"']?\\s*[:=]\\s*)[^\"'\\s>]+",
    replace: "$1<token>",
  },
  // A cache-busting query value: ?v=1729..., ?_=1729...
  { regex: "([?&](?:v|_|t|cachebust)=)[^&\"'\\s]+", replace: "$1<bust>" },
];

/**
 * Applies the built-in rules and then the project's, and rebuilds every regular expression on
 * each call.
 *
 * The `g` flag is required, not optional: `String.replace` with a non-global pattern replaces
 * only the *first* occurrence, so a page with two timestamps would have one normalized and one
 * not, and two runs that differed only in the first timestamp would still differ. (Measured: it
 * is the reason a bare epoch survived in the first version of this function.)
 *
 * A fresh `RegExp` is built per call rather than kept, because a global regex carries `lastIndex`
 * between `exec` calls; rebuilding means one page's scan cannot start where the previous ended.
 */
export function normalize(text: string, projectPatterns: readonly NormalizePattern[]): string {
  let out = text;
  for (const pattern of [...BUILTIN_PATTERNS, ...projectPatterns]) {
    let regex: RegExp;
    try {
      regex = new RegExp(pattern.regex, "g");
    } catch {
      // A project pattern that does not compile is the project's mistake; skipping it is worse
      // than failing, because a skipped rule makes records that do not match, silently.
      throw new Error(`normalize: invalid pattern ${JSON.stringify(pattern.regex)}`);
    }
    out = out.replace(regex, pattern.replace);
  }
  return out;
}
