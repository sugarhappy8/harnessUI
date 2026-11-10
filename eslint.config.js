import js from "@eslint/js";
import globals from "globals";
import reactHooks from "eslint-plugin-react-hooks";
import tseslint from "typescript-eslint";

export default tseslint.config(
  { ignores: ["dist", "src-tauri", "crates", "target", "node_modules"] },
  {
    extends: [js.configs.recommended, ...tseslint.configs.recommended],
    files: ["**/*.{ts,tsx}"],
    languageOptions: { ecmaVersion: 2022, globals: globals.browser },
    plugins: { "react-hooks": reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // Model and tool output is rendered as text and React elements, never as an
      // HTML string (ARCHITECTURE.md 13.5). These ban the HTML-string sinks.
      "no-restricted-properties": [
        "error",
        { property: "innerHTML", message: "Never assign HTML strings; render React elements." },
        { property: "outerHTML", message: "Never assign HTML strings; render React elements." },
        { property: "insertAdjacentHTML", message: "Never insert HTML strings." },
      ],
      "no-restricted-syntax": [
        "error",
        {
          selector: "JSXAttribute[name.name='dangerouslySetInnerHTML']",
          message: "dangerouslySetInnerHTML is banned: model output must never become HTML.",
        },
      ],
      "no-eval": "error",
      "no-implied-eval": "error",
      "no-new-func": "error",
    },
  },
);
