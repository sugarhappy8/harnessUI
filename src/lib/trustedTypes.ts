// The app's one Trusted Types policy (0.2.1: the blank window of 0.2.0).
//
// The Content-Security-Policy in src-tauri/tauri.conf.json ends with
// `require-trusted-types-for 'script'`. Chromium (WebView2 on Windows) enforces it: writing a
// plain string to an HTML sink such as `innerHTML` throws. The app writes none itself, but Radix
// Select draws its list's viewport with a fixed `<style>` through React's
// `dangerouslySetInnerHTML`, which React writes with `innerHTML`. Refused, that throw unmounted
// the whole app in 0.2.0 as soon as any Studio menu was drawn (a model with more than one type,
// Studio's settings), and the window went blank.
//
// The `default` policy below lets exactly the library's fixed strings through, and refuses
// every other string as before, so the rule keeps guarding the page. It makes no scripts or
// script URLs either: those stay refused. A test (trustedTypes.test.tsx) reads the library's code
// and fails when it writes a string this list does not have, for example after an upgrade.

/** The fixed `<style>` Radix Select's viewport writes (@radix-ui/react-select, SelectViewport). */
export const RADIX_SELECT_VIEWPORT_STYLE =
  "[data-radix-select-viewport]{scrollbar-width:none;-ms-overflow-style:none;-webkit-overflow-scrolling:touch;}" +
  "[data-radix-select-viewport]::-webkit-scrollbar{display:none}";

/** Every string an HTML sink may take in the app's pages. */
export const ALLOWED_HTML: readonly string[] = [RADIX_SELECT_VIEWPORT_STYLE];

/** The part of the browser's `trustedTypes` this file uses. */
type PolicyFactory = {
  createPolicy(name: string, rules: { createHTML(input: string): string }): unknown;
  readonly defaultPolicy: unknown;
};

/** `input` if it is one of the allowed strings; any other string is refused with a TypeError,
 *  as the browser itself would refuse it. */
export function allowedHtml(input: string): string {
  if (ALLOWED_HTML.includes(input)) return input;
  throw new TypeError("Trusted Types: this HTML is not one the app allows.");
}

/** Creates the `default` policy, once, where the browser has Trusted Types (Chromium, recent
 *  WebKit); elsewhere it does nothing. Returns whether a default policy is in place. Call it before
 *  anything is drawn; it is safe to call again. */
export function installTrustedTypes(): boolean {
  const factory = (globalThis as { trustedTypes?: PolicyFactory }).trustedTypes;
  if (!factory) return false;
  if (factory.defaultPolicy) return true;
  try {
    factory.createPolicy("default", { createHTML: allowedHtml });
    return true;
  } catch {
    // Another `default` policy exists already (made since the check above): keep it.
    return Boolean(factory.defaultPolicy);
  }
}
