// The app's Trusted Types policy (src/lib/trustedTypes.ts): it lets through exactly the fixed
// HTML the libraries the app draws with write, and refuses everything else.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { ALLOWED_HTML, allowedHtml, installTrustedTypes, RADIX_SELECT_VIEWPORT_STYLE } from "./trustedTypes";
import { enforceTrustedTypes } from "./testing/trustedTypes";
import { SelectField } from "../components/ui/SelectField";

const ROOT = join(__dirname, "..", "..");

/** Every file under `dir` whose name ends in `.ts` or `.tsx`, tests left out. */
function sources(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const path = join(dir, e.name);
    if (e.isDirectory()) return sources(path);
    return /\.tsx?$/.test(e.name) && !/\.test\.tsx?$/.test(e.name) ? [path] : [];
  });
}

/** The Radix packages the app's code reaches: those it imports from "radix-ui", and theirs. */
function radixPackages(): string[] {
  const parts = new Set<string>();
  for (const file of sources(join(ROOT, "src"))) {
    for (const m of readFileSync(file, "utf8").matchAll(/import \{([^}]+)\} from "radix-ui"/g)) {
      for (const name of m[1]!.split(",")) parts.add(name.trim());
    }
  }
  const kebab = (s: string) => s.replace(/([a-z])([A-Z])/g, "$1-$2").toLowerCase();
  const todo = [...parts].filter(Boolean).map((p) => `@radix-ui/react-${kebab(p)}`);
  const seen = new Set<string>();
  while (todo.length > 0) {
    const pkg = todo.pop()!;
    if (seen.has(pkg)) continue;
    seen.add(pkg);
    const manifest = JSON.parse(readFileSync(join(ROOT, "node_modules", pkg, "package.json"), "utf8")) as {
      dependencies?: Record<string, string>;
    };
    todo.push(...Object.keys(manifest.dependencies ?? {}).filter((d) => d.startsWith("@radix-ui/")));
  }
  return [...seen];
}

afterEach(cleanup);

describe("the app's Trusted Types policy", () => {
  it("allows every fixed HTML string the Radix parts the app uses write, and only those", () => {
    const packages = radixPackages();
    expect(packages).toContain("@radix-ui/react-select");
    const written: string[] = [];
    for (const pkg of packages) {
      let code: string;
      try {
        code = readFileSync(join(ROOT, "node_modules", pkg, "dist", "index.mjs"), "utf8");
      } catch {
        continue; // A package of types or helpers only.
      }
      // A fixed string or a template with no `${}`; anything else (a variable, a concatenation)
      // is a write this test cannot read, and fails below rather than being skipped.
      const literals = [...code.matchAll(/__html:\s*(`[^`$]*`|"[^"]*"|'[^']*')\s*[,}]/g)];
      for (const m of literals) written.push(m[1]!.slice(1, -1));
      const all = code.match(/__html\b/g)?.length ?? 0;
      expect(all, `${pkg} writes HTML this test cannot read`).toBe(literals.length);
    }
    expect(written).toContain(RADIX_SELECT_VIEWPORT_STYLE);
    for (const html of written) expect(ALLOWED_HTML, html).toContain(html);
  });

  it("refuses any other string", () => {
    expect(allowedHtml(RADIX_SELECT_VIEWPORT_STYLE)).toBe(RADIX_SELECT_VIEWPORT_STYLE);
    for (const html of ["<img src=x onerror=alert(1)>", `${RADIX_SELECT_VIEWPORT_STYLE} `, "", "<b>hi</b>"]) {
      expect(() => allowedHtml(html), html).toThrow(TypeError);
    }
  });

  it("is made once where the browser has Trusted Types, and is nothing elsewhere", () => {
    // Every test runs under the rule (the setup file); with no `trustedTypes`, as in WebKitGTK
    // or plain jsdom, nothing is made.
    const g = globalThis as { trustedTypes?: unknown };
    const browser = g.trustedTypes;
    g.trustedTypes = undefined;
    try {
      expect(installTrustedTypes()).toBe(false);
    } finally {
      g.trustedTypes = browser;
    }
    const tt = enforceTrustedTypes();
    try {
      expect(installTrustedTypes()).toBe(true);
      expect(installTrustedTypes()).toBe(true); // Again: the same policy, no second one.
      expect([...tt.policies.keys()]).toEqual(["default"]);
      const div = document.createElement("div");
      // eslint-disable-next-line no-restricted-properties -- the test checks this write is refused.
      expect(() => (div.innerHTML = "<p>not allowed</p>")).toThrow(TypeError);
      expect(div.childNodes.length).toBe(0);
    } finally {
      tt.stop();
    }
  });

  it("the test stand-in refuses a plain string on every HTML sink, the empty one too, until a policy exists", () => {
    const tt = enforceTrustedTypes();
    try {
      const host = document.createElement("div");
      const child = host.appendChild(document.createElement("span"));
      /* eslint-disable no-restricted-properties -- the test checks these writes are refused. */
      expect(() => (host.innerHTML = "")).toThrow(TypeError);
      expect(() => (child.outerHTML = "<b>x</b>")).toThrow(TypeError);
      expect(() => host.insertAdjacentHTML("beforeend", "<b>x</b>")).toThrow(TypeError);
      expect(tt.violations).toEqual([
        '<div>.innerHTML = ""',
        '<span>.outerHTML = "<b>x</b>"',
        '<div>.insertAdjacentHTML = "<b>x</b>"',
      ]);
      tt.violations.length = 0; // Seen: not a failure of this test.
      expect(installTrustedTypes()).toBe(true);
      host.insertAdjacentHTML("beforeend", RADIX_SELECT_VIEWPORT_STYLE);
      expect(() => (host.innerHTML = "<b>x</b>")).toThrow(TypeError);
      /* eslint-enable no-restricted-properties */
      expect(host.textContent).toContain("[data-radix-select-viewport]");
    } finally {
      tt.stop();
    }
  });

  it("lets a SelectField draw where the window enforces Trusted Types", () => {
    const tt = enforceTrustedTypes();
    try {
      render(<SelectField label="Model type" value="a" options={[{ value: "a", label: "A" }]} onChange={() => {}} />);
      expect(tt.violations).toEqual([]);
      expect(tt.policies.has("default")).toBe(true);
    } finally {
      tt.stop();
    }
  });
});
