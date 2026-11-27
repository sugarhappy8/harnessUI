// A vitest setup file (vite.config.ts): every test that has a DOM runs under the app's
// Trusted Types rule, as the main window does in WebView2 (src-tauri/tauri.conf.json's
// `require-trusted-types-for 'script'`). A string written to an HTML sink with no policy to let
// it through fails the test that wrote it, so this kind of blank window shows up in CI rather
// than on a user's screen.
import { afterEach, beforeEach, expect } from "vitest";
import { enforceTrustedTypes, type Enforcement } from "./trustedTypes";

let current: Enforcement | null = null;

if (typeof Element !== "undefined") {
  beforeEach(() => {
    current = enforceTrustedTypes();
  });
  afterEach(() => {
    const done = current;
    current = null;
    if (!done) return;
    done.stop();
    expect(done.violations, "HTML written as a plain string (refused by require-trusted-types-for)").toEqual([]);
  });
}
