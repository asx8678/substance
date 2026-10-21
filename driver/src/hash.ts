import { createHash } from "node:crypto";

/**
 * The content hash the records use.
 *
 * One function, so the DOM hash, a body reference and a frame reference are the same kind of
 * value and can be compared by eye in two records that should match.
 */
export function digest(text: string): string {
  return `sha256:${createHash("sha256").update(text, "utf8").digest("hex")}`;
}
