import type { Page } from "playwright";

import type { Logger } from "./log.js";
import type { SignalObservation } from "./generated/SignalObservation.js";
import type { ToastContainer } from "./generated/ToastContainer.js";

/**
 * Success and error signals: what the page told the user, in a form a machine can read.
 *
 * Three sources, in this order: ARIA (`role=status`, `role=alert`, `aria-live`), toast
 * containers from `rules/signals.toml`, and the project's own `[signals]` selectors. Never a
 * word list: "success" is English, and "erfolgreich" is the same signal in a locale the run was
 * asked to use.
 *
 * A signal is a claim, not a verdict. The recorder pairs it with the DOM change, the network log
 * and the database delta; the verdict for "the app said it saved" is the check's business.
 */
export class Signals {
  constructor(
    private readonly page: Page,
    private readonly toasts: readonly ToastContainer[],
    private readonly successSelectors: readonly string[],
    private readonly errorSelectors: readonly string[],
    private readonly log: Logger,
  ) {}

  /** Every signal currently visible on the page, in discovery order. */
  async observe(): Promise<SignalObservation[]> {
    const raw = await this.page
      .evaluate(
        (input: {
          toasts: {
            selector: string;
            kindAttribute: string;
            success: string[];
            error: string[];
          }[];
          successSelectors: string[];
          errorSelectors: string[];
        }) => {
          const found: { kind: string; source: string; text: string }[] = [];
          const textOf = (element: Element): string => {
            const text = element.textContent ?? "";
            return text.replace(/\s+/g, " ").trim().slice(0, 500);
          };
          const visible = (element: Element): boolean => {
            const box = element.getBoundingClientRect();
            return box.width > 0 && box.height > 0;
          };

          // 1. ARIA. An explicit role beats the implicit one aria-live gives a region.
          for (const element of Array.from(document.querySelectorAll('[role="status"]'))) {
            if (visible(element)) {
              found.push({ kind: "success", source: "role=status", text: textOf(element) });
            }
          }
          for (const element of Array.from(document.querySelectorAll('[role="alert"]'))) {
            if (visible(element)) {
              found.push({ kind: "error", source: "role=alert", text: textOf(element) });
            }
          }
          for (const element of Array.from(document.querySelectorAll("[aria-live]"))) {
            if (element.getAttribute("role") !== null || !visible(element)) {
              continue;
            }
            const politeness = element.getAttribute("aria-live");
            const kind = politeness === "assertive" ? "error" : politeness === "polite" ? "success" : null;
            if (kind !== null) {
              found.push({ kind, source: `aria-live=${politeness}`, text: textOf(element) });
            }
          }

          // 2. Toast containers. The kind comes from the attribute the rule names; a container
          //    whose attribute is missing is not guessed at, because a guess here becomes a
          //    pass or a fail in a check.
          for (const toast of input.toasts) {
            for (const element of Array.from(document.querySelectorAll(toast.selector))) {
              if (!visible(element)) {
                continue;
              }
              const value = element.getAttribute(toast.kindAttribute) ?? "";
              const tokens = value.split(/\s+/);
              const kind = tokens.some((token) => toast.success.includes(token))
                ? "success"
                : tokens.some((token) => toast.error.includes(token))
                  ? "error"
                  : null;
              if (kind !== null) {
                found.push({
                  kind,
                  source: `toast:${toast.selector}`,
                  text: textOf(element),
                });
              }
            }
          }

          // 3. The project's own selectors, last, because they are the least specific.
          for (const [kind, selectors] of [
            ["success", input.successSelectors],
            ["error", input.errorSelectors],
          ] as const) {
            for (const selector of selectors) {
              for (const element of Array.from(document.querySelectorAll(selector))) {
                if (visible(element)) {
                  found.push({ kind, source: `config:${selector}`, text: textOf(element) });
                }
              }
            }
          }

          return found;
        },
        {
          toasts: this.toasts.map((toast) => ({
            selector: toast.selector,
            kindAttribute: toast.kind_attribute,
            success: toast.success_values,
            error: toast.error_values,
          })),
          successSelectors: [...this.successSelectors],
          errorSelectors: [...this.errorSelectors],
        },
      )
      .catch((error: unknown) => {
        // A selector the page cannot parse (an invalid `[signals]` entry) fails the whole walk.
        // Signals are evidence, not the action: report none and say so rather than fail an act.
        this.log.warn("signals: could not read the page", error);
        return [];
      });

    return raw.map((entry) => ({ kind: entry.kind, source: entry.source, text: entry.text }));
  }
}
