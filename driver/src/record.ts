import { readFile } from "node:fs/promises";

import type {
  ConsoleMessage,
  Dialog,
  Download,
  Page,
  Request,
  Response,
  WebSocket,
} from "playwright";

import type { Blobs } from "./blobs.js";
import type { Logger } from "./log.js";
import type { Logs } from "./generated/Logs.js";
import type { NetEntry } from "./generated/NetEntry.js";
import type { NormalizePattern } from "./generated/NormalizePattern.js";
import type { WsFrame } from "./generated/WsFrame.js";
import { normalize } from "./normalize.js";
import { redactBody } from "./redact.js";

/** The websocket channel: frames, or the reason there are none. */
export type WsChannel =
  | { status: "measured"; value: WsFrame[] }
  | { status: "not_measured"; reason: string };

/** What one action's window produced. */
export interface Captured {
  net: NetEntry[];
  ws: WsChannel;
  logs: Logs;
  dialog: string | null;
  download: string | null;
}

/**
 * Everything the browser tells us about one action, collected while it happens.
 *
 * Attached before the action and read after settle, so the record covers exactly the window the
 * settle rule defined. The channels are the plan's: requests, websocket frames, console and page
 * errors, dialogs, downloads.
 */
export class ActionCapture {
  private readonly net: NetEntry[] = [];
  private readonly frames: WsFrame[] = [];
  private consoleErrors = 0;
  private pageErrors = 0;
  private dialog: string | null = null;
  private download: string | null = null;
  private readonly pending = new Set<Promise<void>>();
  private detach: () => void = () => undefined;

  constructor(
    private readonly page: Page,
    private readonly blobs: Blobs,
    private readonly secrets: ReadonlySet<string>,
    private readonly patterns: readonly NormalizePattern[],
    private readonly log: Logger,
  ) {}

  /** Starts listening. Call before the action, for the same reason the settle observer does. */
  attach(): void {
    const onResponse = (response: Response): void => {
      this.track(this.onResponse(response));
    };
    const onRequestFailed = (request: Request): void => {
      this.track(this.onRequestFailed(request));
    };
    const onWebSocket = (socket: WebSocket): void => {
      const url = this.pathOf(socket.url());
      // Playwright hands the frame as `{ payload }`, and the payload may be a string for a text
      // frame. `Buffer.from` on a string is its UTF-8 bytes, which is the same frame.
      socket.on("framesent", (frame) => {
        this.track(this.pushFrame(url, "sent", Buffer.from(frame.payload)));
      });
      socket.on("framereceived", (frame) => {
        this.track(this.pushFrame(url, "received", Buffer.from(frame.payload)));
      });
    };
    const onConsole = (message: ConsoleMessage): void => {
      if (message.type() === "error") {
        this.consoleErrors += 1;
      }
    };
    const onPageError = (): void => {
      this.pageErrors += 1;
    };
    const onDialog = (dialog: Dialog): void => {
      this.dialog = normalize(dialog.message(), this.patterns);
      // Dismiss, always: an undismissed dialog blocks the page, and the next command would
      // report a hang that is really this dialog waiting for a human.
      void dialog.dismiss().catch(() => undefined);
    };
    const onDownload = (download: Download): void => {
      this.track(this.onDownload(download));
    };

    this.page.on("response", onResponse);
    this.page.on("requestfailed", onRequestFailed);
    this.page.on("websocket", onWebSocket);
    this.page.on("console", onConsole);
    this.page.on("pageerror", onPageError);
    this.page.on("dialog", onDialog);
    this.page.on("download", onDownload);

    // A popup is still part of the action; its frames and errors belong in the same record.
    const onPopup = (opened: Page): void => {
      opened.on("websocket", onWebSocket);
      opened.on("pageerror", onPageError);
    };
    this.page.on("popup", onPopup);

    this.detach = () => {
      this.page.off("response", onResponse);
      this.page.off("requestfailed", onRequestFailed);
      this.page.off("websocket", onWebSocket);
      this.page.off("console", onConsole);
      this.page.off("pageerror", onPageError);
      this.page.off("dialog", onDialog);
      this.page.off("download", onDownload);
      this.page.off("popup", onPopup);
    };
  }

  /** Stops listening and returns what was collected. */
  async finish(): Promise<Captured> {
    // Body reads are asynchronous; a response whose body is still being read would otherwise
    // drop out of the record, and a dropped response is a missing effect.
    await Promise.allSettled([...this.pending]);
    this.detach();
    return {
      net: this.net,
      ws: { status: "measured", value: this.frames },
      logs: {
        console_errors: this.consoleErrors,
        page_errors: this.pageErrors,
        // The recorder fills this from the server's log files; the browser cannot know it.
        server_errors: { status: "not_measured", reason: "the recorder reads the server log" },
        server_lines: null,
      },
      dialog: this.dialog,
      download: this.download,
    };
  }

  private track(promise: Promise<void>): void {
    this.pending.add(promise);
    void promise.finally(() => {
      this.pending.delete(promise);
    });
  }

  private async onResponse(response: Response): Promise<void> {
    const request = response.request();
    const entry: NetEntry = {
      method: request.method(),
      path: this.pathOf(request.url()),
      status: response.status(),
      req_body: await this.blobOf(request.postDataBuffer()),
      res_body: await this.safeBody(response),
      failed: null,
    };
    this.net.push(entry);
  }

  private async onRequestFailed(request: Request): Promise<void> {
    const failure = request.failure();
    this.net.push({
      method: request.method(),
      path: this.pathOf(request.url()),
      // No status: the request never completed, which the plan keeps as `null` rather than 0.
      status: null,
      req_body: await this.blobOf(request.postDataBuffer()),
      res_body: null,
      failed: failure === null ? "failed" : normalize(failure.errorText, this.patterns),
    });
  }

  private async onDownload(download: Download): Promise<void> {
    // `path()` resolves once the download finished and names the file the browser wrote.
    const path = await download.path();
    if (path === null) {
      this.log.warn("record: a download did not produce a file");
      return;
    }
    const bytes = await readFile(path);
    const ref = await this.blobs.put(redactBody(bytes, this.secrets));
    // The record's `download` is one blob, so the first download of an action is the one it
    // names; a second one is a fact about the app, and the count lives in the network log.
    this.download ??= ref;
  }

  private pathOf(url: string): string {
    try {
      const parsed = new URL(url);
      return normalize(`${parsed.pathname}${parsed.search}`, this.patterns);
    } catch {
      return normalize(url, this.patterns);
    }
  }

  private async pushFrame(url: string, direction: string, payload: Buffer): Promise<void> {
    const bytes = payload.byteLength;
    // The plan stores the frame payload as a blob when it is large; a small frame's bytes are
    // recoverable from nothing, so the size is what the record keeps for it.
    const ref = bytes >= 1024 ? await this.blobOf(payload) : null;
    this.frames.push({ url, direction, payload: ref, bytes });
  }

  private async safeBody(response: Response): Promise<string | null> {
    const body = await response.body().catch(() => null);
    // A redirect, a download or a streaming response has no readable body. That is not an
    // error: the status and the path are still evidence.
    return body === null ? null : this.blobOf(body);
  }

  /** Stores a body, with every known secret replaced before it reaches the disk. */
  private async blobOf(bytes: Buffer | null): Promise<string | null> {
    if (bytes === null || bytes.byteLength === 0) {
      return null;
    }
    return this.blobs.put(redactBody(bytes, this.secrets));
  }

}
