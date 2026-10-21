/**
 * Replacing session secrets in anything that leaves the driver.
 *
 * Three paths carry text out of the driver: a blob written to disk, an `error` event, and a log
 * line. All three call this, because a redaction that exists in two of them is a leak waiting for
 * the third to be added.
 *
 * Percent-encoded forms are replaced too: a form body holds the password encoded, and a search
 * for the literal value would miss exactly the case the author remembered to worry about. The
 * replacement is a marker rather than a removal, so a reader of the record can see that a value
 * was there.
 */
export const REDACTED = "<redacted>";

/** Replaces every secret in `text`, longest first so a secret containing another is whole. */
export function redactSecrets(text: string, secrets: ReadonlySet<string>): string {
  const ordered = [...secrets]
    .filter((secret) => secret !== "")
    .sort((left, right) => right.length - left.length);
  let out = text;
  for (const secret of ordered) {
    out = out.split(secret).join(REDACTED);
    const encoded = encodeURIComponent(secret);
    if (encoded !== secret) {
      out = out.split(encoded).join(REDACTED);
    }
  }
  return out;
}

/**
 * A UTF-8 body with its secrets replaced, or the bytes untouched when they are not UTF-8.
 *
 * Only UTF-8 bodies are decoded: the password travels in a form body, and a form body is text.
 * A binary body that happens to contain the same bytes is not a leak, and decoding it to search
 * would corrupt a blob a check may want to diff.
 */
export function redactBody(bytes: Buffer, secrets: ReadonlySet<string>): Buffer {
  if (secrets.size === 0) {
    return bytes;
  }
  let text: string;
  try {
    text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return bytes;
  }
  return Buffer.from(redactSecrets(text, secrets), "utf8");
}
