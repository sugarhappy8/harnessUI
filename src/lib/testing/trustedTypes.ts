// Trusted Types for tests, enforced as `require-trusted-types-for 'script'` makes Chromium
// (WebView2) enforce them: jsdom has none, so without this a string written to `innerHTML` that
// the app's real window would refuse goes through unseen.
//
// While enforced, a string written to `Element.innerHTML` throws the browser's TypeError unless a
// `default` policy exists (window.trustedTypes.createPolicy("default", ...)); with one, the string
// goes through the policy first, and a policy that throws refuses it.

type Policy = { createHTML?: (s: string) => string };

/** Starts enforcing; the returned `stop` puts jsdom back as it was. `violations` lists each
 *  string refused for want of a policy. */
export function enforceTrustedTypes() {
  const policies = new Map<string, Policy>();
  const violations: string[] = [];
  const inner = Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML")!;
  const g = globalThis as { trustedTypes?: unknown };
  const before = g.trustedTypes;
  g.trustedTypes = {
    createPolicy(name: string, rules: Policy) {
      if (policies.has(name)) throw new TypeError(`Policy with name "${name}" already exists.`);
      policies.set(name, rules);
      return { createHTML: (s: string) => rules.createHTML?.(s) ?? s };
    },
    get defaultPolicy() {
      return policies.get("default") ?? null;
    },
  };
  Object.defineProperty(Element.prototype, "innerHTML", {
    configurable: true,
    get: inner.get,
    set(this: Element, value: unknown) {
      const policy = policies.get("default")?.createHTML;
      if (typeof value === "string" && value !== "" && !policy) {
        violations.push(`<${this.localName}>.innerHTML = ${JSON.stringify(value.slice(0, 60))}`);
        throw new TypeError(
          "Failed to set the 'innerHTML' property on 'Element': This document requires 'TrustedHTML' assignment.",
        );
      }
      inner.set!.call(this, typeof value === "string" && policy ? policy(value) : value);
    },
  });
  return {
    violations,
    policies,
    stop() {
      Object.defineProperty(Element.prototype, "innerHTML", inner);
      g.trustedTypes = before;
    },
  };
}
