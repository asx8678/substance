import { chromium } from "playwright";
import type { Browser, BrowserContext, Page } from "playwright";

import { ActionCapture } from "./record.js";
import type { Captured } from "./record.js";
import type { Blobs } from "./blobs.js";
import type { Logger } from "./log.js";
import type { OpenContext } from "./generated/OpenContext.js";
import type { NormalizePattern } from "./generated/NormalizePattern.js";
import type { RecordPart } from "./generated/RecordPart.js";
import { Login } from "./login.js";
import type { LoginOutcome } from "./login.js";
import { Signals } from "./signals.js";
import { Settler } from "./settle.js";
import { StorageReader } from "./storage.js";
import { measureBoxes, normalizedBodyHtml, walkControls, visibleTextMap } from "./dom.js";
import type { LayoutBox } from "./generated/LayoutBox.js";
import type { Control } from "./generated/Control.js";
import { digest } from "./hash.js";
import { normalize, templateOf } from "./normalize.js";

/** A set's entries, sorted: the ordering a record needs to be comparable across runs. */
function sorted(entries: ReadonlySet<string>): string[] {
  return [...entries].sort();
}

/** What an observed action produced. */
export interface ActionRecord {
  part: RecordPart;
  url_before: string;
  url_after: string;
}

/**
 * One browser context, and the observation of what happens inside it.
 *
 * The driver holds at most one of these: a command that needs a page arrives, is executed, and is
 * answered before the next line is read. That is the whole concurrency model, and it is why the
 * capture objects are fields rather than arguments.
 */
export class DriverContext {
  private browser: Browser | null = null;
  private context: BrowserContext | null = null;
  private page: Page | null = null;
  private reader: StorageReader | null = null;
  private signals: Signals | null = null;
  private readonly secrets = new Set<string>();
  private readonly blobs: Blobs;
  private readonly patterns: readonly NormalizePattern[];
  private readonly idlePatterns: string[];
  private readonly quietMs: number;
  private readonly timeoutMs: number;

  constructor(
    private readonly open: OpenContext,
    blobs: Blobs,
    private readonly log: Logger,
  ) {
    this.blobs = blobs;
    this.patterns = open.normalize;
    // Patterns as written by the idle baseline: `METHOD /templated/path`, not regexes (see
    // `Settler`). They stay strings so the match can template this request the same way.
    this.idlePatterns = open.idle_patterns;
    this.quietMs = open.settle_quiet_ms;
    this.timeoutMs = open.settle_timeout_ms;
  }

  /** Opens the browser and the context. */
  async start(): Promise<void> {
    this.browser = await chromium.launch({ headless: true });
    // Spread rather than a key with an undefined value: `exactOptionalPropertyTypes` keeps
    // "absent" and "present but undefined" distinct, and Playwright treats them differently.
    const state = this.open.storage_state;
    this.context = await this.browser.newContext({
      viewport: { width: this.open.width, height: this.open.height },
      locale: this.open.locale,
      timezoneId: this.open.tz,
      ...(state === null ? {} : { storageState: state }),
      // The app may serve a self-signed certificate; that is a fact about the app, not a reason
      // to fail every action on it.
      ignoreHTTPSErrors: true,
    });
    this.page = await this.context.newPage();
    this.reader = new StorageReader(this.page, this.context, this.log);
    this.signals = new Signals(
      this.page,
      this.open.toast_containers,
      this.open.success_selectors,
      this.open.error_selectors,
      this.log,
    );
  }

  /** Whether a context is open and usable. */
  get ready(): boolean {
    return this.page !== null && this.context !== null && this.reader !== null;
  }

  /** Navigates and settles, reporting the DOM hash before and after. */
  async goto(url: string): Promise<ActionRecord> {
    const { page, reader } = this.must();
    const before = await this.snapshot(page);
    const urlBefore = page.url();
    const capture = new ActionCapture(page, this.blobs, this.secrets, this.patterns, this.log);
    capture.attach();
    const settler = new Settler(page, this.quietMs, this.timeoutMs, this.idlePatterns, this.log);
    settler.start();
    await page.goto(url, { waitUntil: "domcontentloaded" });
    const settling = await settler.wait();
    const captured = await capture.finish();
    return this.assemble(before, urlBefore, page, reader, captured, settling);
  }

  /**
   * Runs one action and settles.
   *
   * The DOM is hashed before the action, so `dom_changed` is a comparison of the page with
   * itself rather than a guess from the network log.
   */
  async act(kind: string, target: string, value: string | null): Promise<ActionRecord> {
    const { page, reader } = this.must();
    const before = await this.snapshot(page);
    const urlBefore = page.url();
    const capture = new ActionCapture(page, this.blobs, this.secrets, this.patterns, this.log);
    capture.attach();
    const settler = new Settler(page, this.quietMs, this.timeoutMs, this.idlePatterns, this.log);
    settler.start();
    await this.perform(page, kind, target, value);
    const settling = await settler.wait();
    const captured = await capture.finish();
    return this.assemble(before, urlBefore, page, reader, captured, settling);
  }

  /** Lists the controls on the current page. */
  async listControls(): Promise<{ screen: string; controls: Control[] }> {
    const { page } = this.must();
    const screen = this.screenOf(page.url());
    const walk = await walkControls(page, screen);
    return { screen, controls: walk.controls };
  }

  /** The normalized DOM hash and the visible-text map, as its own command. */
  async snapshotDom(): Promise<{
    screen: string;
    dom_hash: string;
    visible_text: Record<string, string>;
  }> {
    const { page } = this.must();
    return {
      screen: this.screenOf(page.url()),
      dom_hash: await this.snapshot(page),
      visible_text: await visibleTextMap(page),
    };
  }

  /** Measures element boxes. */
  async layout(): Promise<{ screen: string; boxes: LayoutBox[] }> {
    const { page } = this.must();
    return { screen: this.screenOf(page.url()), boxes: await measureBoxes(page) };
  }

  /**
   * Watches an already-settled screen for `ms` without acting: the idle baseline.
   *
   * The plan's rule is settle, then watch with no action. What comes back is what the page does on
   * its own -- a poll, an analytics ping, a heartbeat -- so the settle rule can ignore it later.
   * Without this, a page that polls every two seconds never has "no requests in flight" and every
   * action on it settles by the cap.
   *
   * Patterns are method plus a templated path, sorted and deduplicated: a poll whose id or timestamp
   * changes every time is one pattern, not one per request. The watch itself is a plain wait rather
   * than a new network capture, so it cannot disturb the settle counter of an action in flight -- it
   * is only ever called after settle.
   */
  async observeIdle(ms: number): Promise<{
    screen: string;
    patterns: string[];
    pings: string[];
  }> {
    const { page } = this.must();
    const patterns = new Set<string>();
    const pings = new Set<string>();
    const onRequest = (request: { method: () => string; url: () => string; resourceType: () => string }): void => {
      const method = request.method().toUpperCase();
      const path = this.pathOnly(request.url());
      const kind = request.resourceType();
      // A document or a script is the page loading, which is not idle traffic: it happens because
      // something acted, and ignoring it later would hide a real request.
      if (kind === "document" || kind === "script" || kind === "stylesheet" || kind === "font") {
        return;
      }
      patterns.add(`${method} ${templateOf(path)}`);
    };
    const onWebSocket = (socket: { url: () => string }): void => {
      pings.add(`WS ${templateOf(this.pathOnly(socket.url()))}`);
    };
    page.on("request", onRequest);
    page.on("websocket", onWebSocket);
    try {
      await new Promise<void>((resolve) => {
        setTimeout(resolve, ms);
      });
    } finally {
      page.off("request", onRequest);
      page.off("websocket", onWebSocket);
    }
    return {
      screen: this.screenOf(page.url()),
      patterns: sorted(patterns),
      pings: sorted(pings),
    };
  }

  /** The normalized route of the current page. */
  screen(): string {
    const { page } = this.must();
    return this.screenOf(page.url());
  }

  /** Runs axe-core and returns its violations. */
  async axe(): Promise<RecordPart["axe_violations"]> {
    const { page } = this.must();
    // Imported lazily: axe-core is a megabyte of rules, and a run that never asks for an
    // accessibility check should not pay to load it.
    const { default: AxeBuilder } = await import("@axe-core/playwright");
    const results = await new AxeBuilder({ page }).analyze();
    return results.violations.map((violation) => ({
      id: violation.id,
      impact: violation.impact ?? null,
      nodes: violation.nodes.length,
    }));
  }

  /** Applies a route rule. */
  async route(pattern: string, mode: string, ms: number | null): Promise<void> {
    const { context } = this.must();
    if (mode === "fail_500") {
      await context.route(pattern, (routeHandler) => {
        void routeHandler.fulfill({ status: 500, body: "substance: injected 500" });
      });
      return;
    }
    if (mode === "abort") {
      await context.route(pattern, (routeHandler) => {
        void routeHandler.abort("failed");
      });
      return;
    }
    if (mode === "delay") {
      const wait = ms ?? 1000;
      await context.route(pattern, async (routeHandler) => {
        await new Promise((resolve) => {
          setTimeout(resolve, wait);
        });
        await routeHandler.continue();
      });
      return;
    }
    throw new Error(`route: unknown mode ${JSON.stringify(mode)}`);
  }

  /** Sets or advances the browser clock. */
  async clock(op: string, value: string): Promise<void> {
    const { page } = this.must();
    if (op === "install") {
      const time = Date.parse(value);
      if (Number.isNaN(time)) {
        throw new Error(`clock: install needs an instant, got ${JSON.stringify(value)}`);
      }
      // `install` keeps the clock advancing from the given instant, which is what the plan asks
      // for: a frozen instant would freeze every `setTimeout` the page schedules.
      await page.clock.install({ time });
      return;
    }
    if (op === "fast_forward") {
      const ms = Number(value);
      if (!Number.isFinite(ms)) {
        throw new Error(`clock: fast_forward needs milliseconds, got ${JSON.stringify(value)}`);
      }
      await page.clock.fastForward(ms);
      return;
    }
    throw new Error(`clock: unknown op ${JSON.stringify(op)}`);
  }

  /** Starts or stops a trace. */
  async trace(op: string, path: string): Promise<void> {
    const { context } = this.must();
    if (op === "start") {
      await context.tracing.start({ screenshots: true, snapshots: true, sources: false });
      return;
    }
    if (op === "stop") {
      this.tracePath = path;
      await context.tracing.stop({ path });
      return;
    }
    throw new Error(`trace: unknown op ${JSON.stringify(op)}`);
  }

  /** Logs in and reports what changed. */
  async login(command: Parameters<Login["run"]>[0]): Promise<LoginOutcome> {
    const { page, reader } = this.must();
    const login = new Login(page, reader, this.blobs, this.patterns, this.log, this.secrets);
    return login.run(command);
  }

  /** Where the last trace was written, when one was. */
  tracePath: string | null = null;

  /** Closes the context and the browser. */
  async close(): Promise<void> {
    await this.context?.close().catch(() => undefined);
    await this.browser?.close().catch(() => undefined);
    this.context = null;
    this.page = null;
    this.browser = null;
  }

  private must(): { page: Page; context: BrowserContext; reader: StorageReader } {
    if (this.page === null || this.context === null || this.reader === null) {
      throw new Error("no context is open; send open_context first");
    }
    return { page: this.page, context: this.context, reader: this.reader };
  }

  private async perform(
    page: Page,
    kind: string,
    target: string,
    value: string | null,
  ): Promise<void> {
    switch (kind) {
      case "click":
        await page.click(target);
        return;
      case "fill":
        if (value === null) {
          throw new Error("act: fill needs a value");
        }
        await page.fill(target, value);
        return;
      case "select":
        if (value === null) {
          throw new Error("act: select needs a value");
        }
        await page.selectOption(target, value);
        return;
      case "press":
        if (value === null) {
          throw new Error("act: press needs a key");
        }
        await page.press(target, value);
        return;
      case "submit":
        await page.locator(target).evaluate((form: Element) => {
          if (form instanceof HTMLFormElement) {
            form.requestSubmit();
            return;
          }
          const owner = form.closest("form");
          if (owner instanceof HTMLFormElement) {
            owner.requestSubmit();
          }
        });
        return;
      default:
        throw new Error(`act: unknown kind ${JSON.stringify(kind)}`);
    }
  }

  /** The normalized DOM hash of the current page. */
  private async snapshot(page: Page): Promise<string> {
    return digest(normalize(await normalizedBodyHtml(page), this.patterns));
  }

  /** A URL's path and query, as the browser saw it. */
  private pathOnly(url: string): string {
    try {
      const parsed = new URL(url);
      return `${parsed.pathname}${parsed.search}`;
    } catch {
      return url;
    }
  }

  /** The route of a URL, with ids and query values normalized away by the project's rules. */
  private screenOf(url: string): string {
    try {
      const parsed = new URL(url);
      return normalize(parsed.pathname, this.patterns);
    } catch {
      return normalize(url, this.patterns);
    }
  }

  private async assemble(
    before: string,
    urlBefore: string,
    page: Page,
    reader: StorageReader,
    captured: Captured,
    settling: { timed_out: boolean; settle_ms: number },
  ): Promise<ActionRecord> {
    const after = await this.snapshot(page);
    const storage = await reader.read();
    const signals = this.signals === null ? [] : await this.signals.observe();
    const visibleText = await visibleTextMap(page);
    const part: RecordPart = {
      ui: {
        url_before: urlBefore,
        url_after: page.url(),
        dom_changed: before !== after,
        dom_hash_before: before,
        dom_hash_after: after,
        download: captured.download,
        dialog: captured.dialog,
      },
      net: captured.net,
      ws: captured.ws,
      storage,
      logs: captured.logs,
      // The record's instant comes from the driver's own clock, truncated to seconds: the caller
      // (Rust) owns the frozen server clock and stamps the record; this is when the browser saw
      // the settle, which is a duration, not an identity.
      settled_at: new Date(Math.floor(Date.now() / 1000) * 1000).toISOString(),
      // The recorder fills the channels it captures itself.
      db: null,
      egress: null,
      email: null,
      queries: null,
      visible_text: visibleText,
      axe_violations: [],
      signals,
    };
    this.log.info(
      `settled in ${String(settling.settle_ms)}ms${settling.timed_out ? " (timed out)" : ""}`,
    );
    return { part, url_before: urlBefore, url_after: page.url() };
  }
}
