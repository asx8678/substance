import type { Page } from "playwright";

import type { Logger } from "./log.js";

/**
 * The page-side state the settle rule reads.
 *
 * Declared as an optional property on `Window` rather than cast at each use, so `tsc` checks the
 * read and the write against one shape. `exactOptionalPropertyTypes` then makes clearing it a
 * `delete`, which is what clearing it is.
 */
declare global {
  interface Window {
    __substanceMutation?: { last: number };
  }
}

/**
 * How an action ends: no requests in flight and no DOM mutation for a quiet period.
 *
 * The plan says settle, don't sleep. This is that rule, plus the two things that make it usable:
 *
 * - A request the idle baseline saw before the action began (a poll, an analytics ping, a
 *   websocket keepalive) was never going to end, so counting it means never settling on a page
 *   that polls faster than the quiet period. Matching requests are ignored by pattern.
 * - The observer starts before the action, because a mutation that happens before the first
 *   observation is a change we would otherwise miss and report as quiet.
 *
 * Hitting the cap is recorded as `timed_out` and never retried: a page that does not settle is a
 * fact about the page, and retrying it would turn a finding into a wait.
 */
export class Settler {
  private inFlight = 0;
  private detach: () => void = () => undefined;

  constructor(
    private readonly page: Page,
    private readonly quietMs: number,
    private readonly timeoutMs: number,
    private readonly idlePatterns: readonly RegExp[],
    private readonly log: Logger,
  ) {}

  /** Starts counting and observing; call before the action. */
  start(): void {
    this.inFlight = 0;

    const onRequest = (request: { url: () => string }): void => {
      if (!this.isIdle(request.url())) {
        this.inFlight += 1;
      }
    };
    const onDone = (request: { url: () => string }): void => {
      if (!this.isIdle(request.url())) {
        this.inFlight = Math.max(0, this.inFlight - 1);
      }
    };
    this.page.on("request", onRequest);
    this.page.on("requestfinished", onDone);
    this.page.on("requestfailed", onDone);

    // Installed on the page before the action runs. A page that navigates drops the observer,
    // which the settle loop notices as "no state" and treats as quiet; the request counter is
    // what carries the rule across a navigation.
    void this.page
      .evaluate(() => {
        const previous = window.__substanceMutation;
        if (previous !== undefined) {
          return;
        }
        const state = { last: Date.now() };
        window.__substanceMutation = state;
        new MutationObserver(() => {
          state.last = Date.now();
        }).observe(document.documentElement, {
          subtree: true,
          childList: true,
          attributes: true,
          characterData: true,
        });
      })
      .catch((error: unknown) => {
        this.log.warn("settle: could not install the mutation observer", error);
      });

    this.detach = () => {
      this.page.off("request", onRequest);
      this.page.off("requestfinished", onDone);
      this.page.off("requestfailed", onDone);
      void this.page
        .evaluate(() => {
          delete window.__substanceMutation;
        })
        .catch(() => undefined);
    };
  }

  /** Waits for the rule or the cap, then stops counting. */
  async wait(): Promise<{ timed_out: boolean; settle_ms: number }> {
    const began = Date.now();
    let timedOut = false;
    for (;;) {
      const elapsed = Date.now() - began;
      if (elapsed >= this.timeoutMs) {
        timedOut = true;
        break;
      }
      const quietFor = await this.quietForMs();
      if (this.inFlight === 0 && quietFor >= this.quietMs) {
        break;
      }
      // Sleep no longer than the remaining quiet period needs, and never past the cap.
      const sleep = Math.max(5, Math.min(this.quietMs - quietFor, this.timeoutMs - elapsed, 50));
      await delay(sleep);
    }
    this.detach();
    return { timed_out: timedOut, settle_ms: Date.now() - began };
  }

  /** Milliseconds since the page last mutated, or the quiet period when the page is gone. */
  private async quietForMs(): Promise<number> {
    const since = await this.page
      .evaluate(() => {
        const state = window.__substanceMutation;
        return state === undefined ? null : Date.now() - state.last;
      })
      .catch(() => null);
    // No state means no observer: a page that navigated away is not mutating.
    return since ?? this.quietMs;
  }

  /** Whether a URL belongs to the idle baseline. */
  private isIdle(url: string): boolean {
    return this.idlePatterns.some((pattern) => pattern.test(url));
  }
}

/** Resolves after `ms`; the driver's only sleep, used by the settle loop. */
export function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}
