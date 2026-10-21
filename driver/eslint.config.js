// Lint configuration for the driver (M3-T01).
//
// Typed linting, because the rules that matter here need types: a floating promise inside a
// Playwright event handler is invisible to a syntax-only linter, and every channel of the
// recorder is an event handler.
import js from "@eslint/js";
import tseslint from "typescript-eslint";

export default tseslint.config(
  {
    // Generated protocol types are ts-rs output: linting them would invite hand edits, and any
    // finding would have to be fixed in the Rust type anyway.
    ignores: ["dist/**", "node_modules/**", "src/generated/**"],
  },
  js.configs.recommended,
  ...tseslint.configs.strictTypeChecked,
  ...tseslint.configs.stylisticTypeChecked,
  {
    languageOptions: {
      parserOptions: {
        // The generated files are excluded above; including them here would lint them through
        // the type-aware rules anyway.
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    rules: {
      // The three the plan names.
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": "error",
      "@typescript-eslint/no-explicit-any": "error",

      // A promise that must be awaited is a promise whose rejection nobody sees. The driver
      // talks to a browser over a pipe; a lost rejection is a hang.
      "@typescript-eslint/no-unnecessary-condition": "off",
      "@typescript-eslint/require-await": "off",
      // `String(...)` on a number is how a message is built; the template rule is a style
      // preference that fights the numeric formatting used for logs.
      "@typescript-eslint/restrict-template-expressions": [
        "error",
        { allowNumber: true, allowBoolean: true },
      ],
      // Playwright's event handlers are typed as returning void, and every one of them starts an
      // async task deliberately; `track` is where the rejection is handled.
      "@typescript-eslint/no-empty-function": ["error", { allow: ["arrowFunctions"] }],
      // The unicode flag is a matter of the rules in normalize.ts, not of this codebase.
      "no-control-regex": "off",
    },
  },
  {
    // `main.ts` is the process entry point: it is allowed to be the place a top-level promise
    // is awaited, and it is the only file that writes to stdout.
    files: ["src/main.ts"],
    rules: {
      "no-console": "error",
    },
  },
  {
    files: ["**/*.js"],
    ...tseslint.configs.disableTypeChecked,
  },
);
