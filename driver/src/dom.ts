import type { Page } from "playwright";

import type { Control } from "./generated/Control.js";
import type { LayoutBox } from "./generated/LayoutBox.js";

/**
 * The page-side walks: controls, normalized DOM, visible text, boxes.
 *
 * All four run as one `page.evaluate` each, because a walk that crosses the boundary per element
 * is a round trip per element. Nothing here uses Playwright locators: the driver observes, and a
 * locator would wait, retry and act.
 */

/** A UI element that a click or a fill could address. */
const CONTROL_ROLES = [
  "button",
  "link",
  "textbox",
  "searchbox",
  "combobox",
  "checkbox",
  "radio",
  "switch",
  "slider",
  "spinbutton",
  "tab",
  "menuitem",
  "option",
  "submit",
] as const;

/** What `list_controls` found, before it is given a screen. */
export interface ControlWalk {
  controls: Control[];
}

/**
 * Walks the interactive controls, with selector candidates in preference order.
 *
 * The order is the plan's: role plus accessible name first, then a test ID, then CSS. A role and
 * name pair survives a restyle; a CSS path does not. A control with none of the three is still
 * reported, with an empty candidate list, because "the crawl found a button it cannot address" is
 * a finding about the app, not a reason to omit it.
 */
export async function walkControls(page: Page, screen: string): Promise<ControlWalk> {
  const raw = await page.evaluate((roles: readonly string[]) => {
    interface FoundField {
      name: string;
      type: string;
      required: boolean;
      pattern: string | null;
      min: string | null;
      max: string | null;
    }
    interface Found {
      role: string;
      name: string;
      selectors: string[];
      form: { selector: string; fields: FoundField[] } | null;
      purpose: string;
    }

    const accessibleName = (element: Element): string => {
      const labelled = element.getAttribute("aria-label");
      if (labelled !== null && labelled.trim() !== "") {
        return labelled.replace(/\s+/g, " ").trim();
      }
      const labelledBy = element.getAttribute("aria-labelledby");
      if (labelledBy !== null) {
        const text = labelledBy
          .split(/\s+/)
          .map((id) => document.getElementById(id)?.textContent ?? "")
          .join(" ");
        if (text.trim() !== "") {
          return text.replace(/\s+/g, " ").trim();
        }
      }
      const elementId = element.getAttribute("id");
      if (elementId !== null && elementId !== "") {
        const label = document.querySelector(`label[for="${CSS.escape(elementId)}"]`);
        if (label !== null && (label.textContent ?? "").trim() !== "") {
          return (label.textContent ?? "").replace(/\s+/g, " ").trim();
        }
      }
      if (element instanceof HTMLInputElement && element.type === "submit") {
        return element.value;
      }
      const text = element.textContent ?? "";
      const normalized = text.replace(/\s+/g, " ").trim();
      return normalized.slice(0, 120);
    };

    const cssPath = (element: Element): string | null => {
      const parts: string[] = [];
      let current: Element | null = element;
      while (current !== null && current !== document.documentElement) {
        const node: Element = current;
        const tag = node.tagName.toLowerCase();
        const parent: Element | null = node.parentElement;
        if (parent === null) {
          parts.unshift(tag);
          break;
        }
        const siblings = Array.from(parent.children).filter(
          (sibling: Element) => sibling.tagName === node.tagName,
        );
        const index = siblings.indexOf(node) + 1;
        parts.unshift(`${tag}:nth-of-type(${index})`);
        current = parent;
      }
      return parts.length === 0 ? null : parts.join(" > ");
    };

    const fieldType = (element: Element): string => {
      const type = element.getAttribute("type");
      return type === null || type === "" ? "text" : type;
    };

    const formOf = (element: Element): Found["form"] => {
      const form = element.closest("form");
      if (form === null) {
        return null;
      }
      const selector =
        form.getAttribute("id") !== null && form.getAttribute("id") !== ""
          ? `form#${CSS.escape(form.getAttribute("id") ?? "")}`
          : cssPath(form);
      if (selector === null) {
        return null;
      }
      const fields = Array.from(form.querySelectorAll("input, select, textarea")).map((field) => ({
        name: field.getAttribute("name") ?? field.getAttribute("id") ?? fieldType(field),
        type: fieldType(field),
        required: field.hasAttribute("required"),
        pattern: field.getAttribute("pattern"),
        min: field.getAttribute("min"),
        max: field.getAttribute("max"),
      }));
      return { selector, fields };
    };

    const purposeOf = (name: string, type: string): string => {
      const lower = name.toLowerCase();
      if (type === "password") {
        return "login";
      }
      // A form with a password field is a login form whatever its button says.
      if (lower.includes("log out") || lower.includes("logout") || lower.includes("sign out")) {
        return "logout";
      }
      if (type === "submit" || lower.includes("submit") || lower.includes("save")) {
        return "submit";
      }
      return "action";
    };

    const found: Found[] = [];
    for (const role of roles) {
      const selector =
        role === "link"
          ? 'a[href], [role="link"]'
          : role === "textbox"
            ? 'input:not([type="hidden"]), textarea, [role="textbox"]'
            : role === "submit"
              ? 'button[type="submit"], input[type="submit"]'
              : role === "button"
                ? 'button, [role="button"], input[type="button"]'
                : `[role="${role}"]`;
      for (const element of Array.from(document.querySelectorAll(selector))) {
        if (element.getAttribute("role") !== null && element.getAttribute("role") !== role) {
          continue;
        }
        const box = element.getBoundingClientRect();
        // An element with no box is not on screen and cannot be clicked. It is also what a
        // hidden duplicate of every form control looks like, and reporting those would triple
        // every screen.
        if (box.width === 0 || box.height === 0) {
          continue;
        }
        const name = accessibleName(element);
        const candidates: string[] = [];
        if (name !== "") {
          candidates.push(`role=${role}[name="${name.replace(/"/g, '\\"')}"]`);
        }
        const testId =
          element.getAttribute("data-testid") ??
          element.getAttribute("data-test-id") ??
          element.getAttribute("data-test");
        if (testId !== null && testId !== "") {
          candidates.push(`[data-testid="${testId.replace(/"/g, '\\"')}"]`);
        }
        const path = cssPath(element);
        if (path !== null) {
          candidates.push(path);
        }
        const form = formOf(element);
        const hasPassword = form?.fields.some((field) => field.type === "password") === true;
        found.push({
          role,
          name,
          selectors: candidates,
          form,
          purpose: hasPassword && role !== "link" ? "login" : purposeOf(name, fieldType(element)),
        });
      }
    }
    return found;
  }, CONTROL_ROLES);

  // One screen can produce the same control twice: an input that is both a textbox and inside a
  // [role=textbox]. Keep the first, which is the one whose role is the more specific match.
  const seen = new Set<string>();
  const controls: Control[] = [];
  for (const entry of raw) {
    const key = `${entry.role}|${entry.name}|${entry.selectors[0] ?? ""}`;
    if (seen.has(key)) {
      continue;
    }
    seen.add(key);
    controls.push({
      screen,
      role: entry.role,
      name: entry.name,
      selectors: entry.selectors,
      form:
        entry.form === null
          ? null
          : {
              selector: entry.form.selector,
              fields: entry.form.fields.map((field) => ({
                name: field.name,
                // The generated `kind` is the normalized field kind, not the raw input type.
                kind: fieldKind(field.type),
                required: field.required,
                pattern: field.pattern,
                min: field.min,
                max: field.max,
              })),
            },
      purpose: purposeOfType(entry.purpose),
    });
  }
  return { controls };
}

/** Maps an input type to the generated field kind. */
function fieldKind(type: string): string {
  switch (type) {
    case "email":
    case "password":
    case "number":
    case "date":
    case "checkbox":
      return type;
    case "select":
    case "select-one":
    case "select-multiple":
      return "select";
    default:
      return "text";
  }
}

/** Narrows a purpose string to the generated union, defaulting to `action`. */
function purposeOfType(purpose: string): Control["purpose"] {
  switch (purpose) {
    case "login":
      return "login";
    case "logout":
      return "logout";
    case "submit":
      return "submit";
    default:
      return "action";
  }
}

/**
 * The body's HTML with the parts that change on every load removed.
 *
 * The rules are the structural ones, in this order: scripts and styles (which carry build output
 * and inline state), `<input>` values and `value=` attributes (which carry what the user typed;
 * a record of a fill should not be identical to a record of a click), comments, then whitespace
 * between tags. The project's own patterns are applied afterwards by the caller, because they
 * are regexes over text and this function returns text.
 */
export async function normalizedBodyHtml(page: Page): Promise<string> {
  return page.evaluate(() => {
    const clone = document.body.cloneNode(true) as HTMLElement;
    for (const element of Array.from(clone.querySelectorAll("script, style, noscript, link"))) {
      element.remove();
    }
    for (const element of Array.from(clone.querySelectorAll("input"))) {
      element.removeAttribute("value");
      element.removeAttribute("checked");
      if (element instanceof HTMLInputElement && element.type === "password") {
        // A password field's value is not in the DOM, but its presence is a fact about the page.
        element.setAttribute("data-substance-password", "");
      }
    }
    for (const element of Array.from(clone.querySelectorAll("textarea"))) {
      element.textContent = "";
    }
    for (const element of Array.from(clone.querySelectorAll("select"))) {
      element.removeAttribute("value");
    }

    const walker = document.createTreeWalker(clone, NodeFilter.SHOW_COMMENT);
    const comments: Comment[] = [];
    while (walker.nextNode()) {
      const node = walker.currentNode;
      if (node instanceof Comment) {
        comments.push(node);
      }
    }
    for (const comment of comments) {
      comment.remove();
    }

    return clone.outerHTML.replace(/>\s+</g, "><").replace(/\s{2,}/g, " ").trim();
  });
}

/**
 * Visible text by element, for the unused-field check.
 *
 * Keyed by a short selector so a reader can find the element again, and limited to leaf elements
 * so the map is a list of what was said rather than every ancestor saying it too.
 */
export async function visibleTextMap(page: Page): Promise<Record<string, string>> {
  return page.evaluate(() => {
    const out: Record<string, string> = {};
    const body = document.body;
    if (body === null) {
      return out;
    }
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
    let index = 0;
    for (;;) {
      const node = walker.nextNode();
      if (node === null) {
        break;
      }
      const text = (node.textContent ?? "").replace(/\s+/g, " ").trim();
      const parent = node.parentElement;
      if (text === "" || parent === null) {
        continue;
      }
      const tag = parent.tagName.toLowerCase();
      if (tag === "script" || tag === "style" || tag === "noscript") {
        continue;
      }
      const box = parent.getBoundingClientRect();
      if (box.width === 0 || box.height === 0) {
        continue;
      }
      out[`${tag}#${index}`] = text.slice(0, 500);
      index += 1;
    }
    return out;
  });
}

/**
 * Element boxes for the layout checks: overflow, overlap and tap targets.
 *
 * Every element with text or an interactive role is measured, not every element: a wrapper div is
 * the same box as its child, and reporting both would double every overlap finding.
 */
export async function measureBoxes(page: Page): Promise<LayoutBox[]> {
  const raw = await page.evaluate(() => {
    const out: {
      selector: string;
      x: number;
      y: number;
      width: number;
      height: number;
      clickable: boolean;
    }[] = [];
    const candidates = Array.from(
      document.querySelectorAll(
        'a[href], button, input, select, textarea, [role], h1, h2, h3, h4, p, li, td, th, span, strong, label',
      ),
    );
    let index = 0;
    for (const element of candidates) {
      const box = element.getBoundingClientRect();
      if (box.width === 0 && box.height === 0) {
        continue;
      }
      const role = element.getAttribute("role");
      const clickable =
        element.matches("a[href], button, input, select, textarea, summary") ||
        role === "button" ||
        role === "link";
      out.push({
        selector: `${element.tagName.toLowerCase()}#${index}`,
        x: Math.round(box.x),
        y: Math.round(box.y),
        width: Math.round(box.width),
        height: Math.round(box.height),
        clickable,
      });
      index += 1;
    }
    return out;
  });
  // Negative coordinates are real (an element scrolled above the viewport), but the generated
  // type is unsigned because a CSS box in document order is; clamp to zero with a note in the
  // value rather than changing the schema for a case the checks do not read.
  return raw.map((entry) => ({
    selector: entry.selector,
    x: Math.max(0, entry.x),
    y: Math.max(0, entry.y),
    width: entry.width,
    height: entry.height,
    clickable: entry.clickable,
  }));
}
