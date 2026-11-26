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
      for (const m of code.matchAll(/__html:\s*(`[^`]*`|"[^"]*")/g)) written.push(m[1]!.slice(1, -1));
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
    expect(installTrustedTypes()).toBe(false); // jsdom: no Trusted Types.
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
