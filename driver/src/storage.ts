import type { BrowserContext, Page } from "playwright";

import type { Storage } from "./generated/Storage.js";
import type { Logger } from "./log.js";

/**
 * Browser storage, flattened into four sorted maps.
 *
 * A record needs to say what the action persisted, and it needs the same answer twice for the
 * same action. So: keys are sorted (serde's maps are `BTreeMap`s for the same reason), and an
 * IndexedDB store is summarized as its record count, not its records — a store holding an
 * autoincremented id per row would otherwise differ between two runs that did the same thing.
 *
 * Cookie values are included, because the checks that read this want tokens. Records are
 * `records/`, the same place a body blob goes, and a record is evidence the plan expects to hold
 * credentials: `storage` is not one of the places the plan forbids a secret.
 */
export class StorageReader {
  constructor(
    private readonly page: Page,
    private readonly context: BrowserContext,
    private readonly log: Logger,
  ) {}

  /** Reads all four stores as they are now. */
  async read(): Promise<Storage> {
    const pageStores = await this.page
      .evaluate(() => {
        const flatten = (store: globalThis.Storage | undefined): Record<string, string> => {
          const out: Record<string, string> = {};
          if (store === undefined) {
            return out;
          }
          for (let index = 0; index < store.length; index += 1) {
            const key = store.key(index);
            if (key !== null) {
              out[key] = store.getItem(key) ?? "";
            }
          }
          return out;
        };
        return {
          local: flatten(globalThis.localStorage),
          session: flatten(globalThis.sessionStorage),
        };
      })
      .catch((error: unknown) => {
        // about:blank and an error page have no storage; that is an empty store, not a failure.
        this.log.warn("storage: the page has no web storage", error);
        return { local: {}, session: {} };
      });

    const indexeddb = await this.indexedDbSummary();
    const cookies = await this.cookieSummary();

    return {
      local: sortMap(pageStores.local),
      session: sortMap(pageStores.session),
      indexeddb,
      cookies,
    };
  }

  /**
   * Database and store names with their record counts: `app/notes` → `"12"`.
   *
   * Opening a database from the page would race the app's own connection, so the names and
   * counts come from `indexedDB.databases()` where it exists. A browser without it reports an
   * empty map; the record then says nothing was found rather than that nothing was there.
   */
  private async indexedDbSummary(): Promise<Record<string, string>> {
    const names = await this.page
      .evaluate(async () => {
        const factory = globalThis.indexedDB as IDBFactory & {
          databases?: () => Promise<{ name?: string | undefined }[]>;
        };
        if (factory.databases === undefined) {
          return [] as string[];
        }
        const databases = await factory.databases();
        return databases
          .map((database) => database.name)
          .filter((name): name is string => name !== undefined);
      })
      .catch((error: unknown) => {
        this.log.warn("storage: could not list IndexedDB databases", error);
        return [] as string[];
      });

    const summary: Record<string, string> = {};
    for (const name of [...names].sort()) {
      const stores = await this.page
        .evaluate(
          (databaseName: string) =>
            new Promise<Record<string, number>>((resolve) => {
              const request = globalThis.indexedDB.open(databaseName);
              request.onerror = () => {
                resolve({});
              };
              request.onsuccess = () => {
                const database = request.result;
                const storeNames = Array.from(database.objectStoreNames);
                const counts: Record<string, number> = {};
                let remaining = storeNames.length;
                if (remaining === 0) {
                  database.close();
                  resolve(counts);
                  return;
                }
                for (const storeName of storeNames) {
                  const transaction = database.transaction(storeName, "readonly");
                  const countRequest = transaction.objectStore(storeName).count();
                  countRequest.onsuccess = () => {
                    counts[storeName] = countRequest.result;
                    remaining -= 1;
                    if (remaining === 0) {
                      database.close();
                      resolve(counts);
                    }
                  };
                  countRequest.onerror = () => {
                    remaining -= 1;
                    if (remaining === 0) {
                      database.close();
                      resolve(counts);
                    }
                  };
                }
              };
            }),
          name,
        )
        .catch(() => ({}));
      for (const [store, count] of Object.entries(stores)) {
        summary[`${name}/${store}`] = String(count);
      }
    }
    return sortMap(summary);
  }

  /** Cookie name → value, sorted. */
  private async cookieSummary(): Promise<Record<string, string>> {
    const cookies = await this.context.cookies().catch((error: unknown) => {
      this.log.warn("storage: could not read cookies", error);
      return [];
    });
    const summary: Record<string, string> = {};
    for (const cookie of cookies) {
      summary[cookie.name] = cookie.value;
    }
    return sortMap(summary);
  }
}

/** Keys in sorted order, so two reads of the same store serialize identically. */
export function sortMap(input: Record<string, string>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const key of Object.keys(input).sort()) {
    const value = input[key];
    if (value !== undefined) {
      out[key] = value;
    }
  }
  return out;
}

/**
 * The keys that appeared or changed between two readings, sorted.
 *
 * Only keys: the point is "did logging in do anything", and the answer does not need the value
 * that was just obtained with a password.
 */
export function changedKeys(before: Storage, after: Storage): string[] {
  const changed: string[] = [];
  const stores = ["local", "session", "indexeddb", "cookies"] as const;
  for (const store of stores) {
    const previous = before[store];
    const current = after[store];
    for (const key of Object.keys(current)) {
      if (previous[key] !== current[key]) {
        changed.push(`${store}:${key}`);
      }
    }
    for (const key of Object.keys(previous)) {
      if (!(key in current)) {
        changed.push(`${store}:${key}`);
      }
    }
  }
  return changed.sort();
}
