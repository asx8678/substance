import { createHash } from "node:crypto";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";

import type { BlobRef } from "./generated/BlobRef.js";

/**
 * Content-addressed bodies, written once and never rewritten.
 *
 * A record refers to a body by its hash, so two runs that saw the same response refer to the
 * same file, and a diff of two records is a diff of hashes. `writeFile` with the `wx` flag makes
 * the write-once rule the filesystem's job: the second writer of the same bytes loses the race
 * harmlessly instead of rewriting a file another run may be reading.
 */
export class Blobs {
  constructor(private readonly root: string) {}

  /**
   * Hashes `bytes` and writes them under `sha256/<first two hex>/<hex>`.
   *
   * Returns the `sha256:<hex>` reference, or `null` for an absent body: "there was no body" and
   * "there was an empty body" are different observations, so the caller's null stays null.
   */
  async put(bytes: Buffer | null): Promise<BlobRef | null> {
    if (bytes === null) {
      return null;
    }
    const hex = createHash("sha256").update(bytes).digest("hex");
    const path = this.pathOf(hex);
    await mkdir(dirname(path), { recursive: true }).catch((error: unknown) => {
      throw asError(error);
    });
    // EEXIST is the normal case, not a failure: identical bytes from an earlier action.
    await writeFile(path, bytes, { flag: "wx" }).catch((error: unknown) => {
      if (!isAlreadyThere(error)) {
        throw asError(error);
      }
    });
    return `sha256:${hex}`;
  }

  /** Hashes text, for bodies the driver produced itself (a redacted login body). */
  async putText(text: string): Promise<BlobRef> {
    const ref = await this.put(Buffer.from(text, "utf8"));
    if (ref === null) {
      throw new Error("a text body always has bytes");
    }
    return ref;
  }

  /** Where a hash lives on disk. */
  pathOf(hex: string): string {
    return join(this.root, "sha256", hex.slice(0, 2), hex);
  }
}

/** An unknown rejection, as an Error the caller can throw without losing its message. */
export function asError(error: unknown): Error {
  return error instanceof Error ? error : new Error(String(error));
}

function isAlreadyThere(error: unknown): boolean {
  return (
    typeof error === "object" &&
    error !== null &&
    "code" in error &&
    (error as { code?: unknown }).code === "EEXIST"
  );
}
