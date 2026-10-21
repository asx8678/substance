import type { Page } from "playwright";

import type { Blobs } from "./blobs.js";
import type { LoginCommand } from "./generated/LoginCommand.js";
import type { Storage } from "./generated/Storage.js";
import type { Logger } from "./log.js";
import type { StorageReader } from "./storage.js";
import { changedKeys } from "./storage.js";
import { normalize } from "./normalize.js";
import { redactSecrets } from "./redact.js";
import type { NormalizePattern } from "./generated/NormalizePattern.js";

/** What a login changed, in the shape the `result` event carries. */
export interface LoginOutcome {
  storage: Storage;
  changed: string[];
  request_path: string | null;
  request_body: string | null;
}

/**
 * Logging in, in the two modes the plan allows.
 *
 * The password is a session secret: it is not written to a log, not echoed in an event, and not
 * stored in a blob. The only place it can survive is the app's own storage, which is what a login
 * is for, and a record's `storage` map is where a session token is meant to appear.
 */
export class Login {
  private readonly secrets: Set<string>;

  constructor(
    private readonly page: Page,
    private readonly reader: StorageReader,
    private readonly blobs: Blobs,
    private readonly patterns: readonly NormalizePattern[],
    private readonly log: Logger,
    secrets: Set<string>,
  ) {
    this.secrets = secrets;
  }

  /** Runs the login and reports what changed. Never returns or logs the password. */
  async run(command: LoginCommand): Promise<LoginOutcome> {
    this.secrets.add(command.password_value);
    const before = await this.reader.read();

    if (command.mode === "ui") {
      // The path is a route, not a URL: a form fill that navigates away first is the normal
      // shape, and `goto` would settle before the form existed.
      await this.page.goto(this.absolute(command.path));
      await this.page.fill(command.identifier, command.email);
      await this.page.fill(command.password, command.password_value);
      await this.page.click(command.submit);
      // The click may navigate; `waitForLoadState` returns immediately when it did not.
      await this.page.waitForLoadState("networkidle").catch((error: unknown) => {
        this.log.warn("login: no idle state after submit; reporting the storage as it is", error);
      });
      const after = await this.reader.read();
      return {
        storage: after,
        changed: changedKeys(before, after),
        request_path: null,
        request_body: null,
      };
    }

    const url = this.absolute(command.path);
    const body = command.body
      .split("{email}")
      .join(command.email)
      .split("{password}")
      .join(command.password_value);
    const contentType = body.trimStart().startsWith("{")
      ? "application/json"
      : "application/x-www-form-urlencoded";
    // Sent through the page's request context, so `Set-Cookie` lands in the context's cookie jar
    // exactly as it would have from a browser navigation.
    await this.page.request.fetch(url, {
      method: command.method,
      headers: { "content-type": contentType },
      data: body,
      failOnStatusCode: false,
    });
    const after = await this.reader.read();
    return {
      storage: after,
      changed: changedKeys(before, after),
      request_path: normalize(command.path, this.patterns),
      // The body is stored as a blob with the password replaced, so a record can say which
      // fields were sent without carrying the value that was sent.
      request_body: await this.blobs.putText(
        normalize(redactSecrets(body, this.secrets), this.patterns),
      ),
    };
  }

  /** Resolves a route against the page's current origin. */
  private absolute(path: string): string {
    if (path.startsWith("http://") || path.startsWith("https://")) {
      return path;
    }
    const origin = new URL(this.page.url()).origin;
    return `${origin}${path.startsWith("/") ? "" : "/"}${path}`;
  }
}
