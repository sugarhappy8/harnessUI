// Trusted Types for tests, enforced on the HTML sinks as `require-trusted-types-for 'script'`
// makes Chromium (WebView2) enforce them: jsdom has none, so without this a string written to
// `innerHTML` that the app's real window would refuse goes through unseen.
//
// While enforced, a string written to `Element.innerHTML`, `Element.outerHTML` or passed to
// `Element.insertAdjacentHTML` throws the browser's TypeError unless a `default` policy exists
// (window.trustedTypes.createPolicy("default", ...)); with one, the string goes through the
// policy first, and a policy that throws refuses it. The empty string is checked too, as
// Chromium checks it. Other sinks (srcdoc, Range.createContextualFragment, DOMParser) are not
// covered: the app reaches none of them.
//
// Every test runs under it (src/lib/testing/enforceTrustedTypesSetup.ts, a setup file in
// vite.config.ts). A test that calls `enforceTrustedTypes` itself while that is in force gets the
// same enforcement (its `stop` then does nothing).

type Policy = { createHTML?: (s: string) => string };

export type Enforcement = {
  /** Each string refused for want of a policy, as `<tag>.sink = "start of the string"`. */
  violations: string[];
  policies: Map<string, Policy>;
  stop(): void;
};

let active: Enforcement | null = null;

const REFUSED = (sink: string, owner: string) =>
  `Failed to ${sink === "insertAdjacentHTML" ? "execute" : "set"} the '${sink}' property on '${owner}': This document requires 'TrustedHTML' assignment.`;

/** Starts enforcing; the returned `stop` puts jsdom back as it was. */
export function enforceTrustedTypes(): Enforcement {
  if (active) {
    const shared = active;
    return { violations: shared.violations, policies: shared.policies, stop() {} };
  }
  const policies = new Map<string, Policy>();
  const violations: string[] = [];
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

  /** `value` as the sink may take it: through the default policy, or refused. */
  const vet = (el: Element, sink: string, value: unknown): unknown => {
    if (typeof value !== "string") return value;
    const policy = policies.get("default")?.createHTML;
    if (!policy) {
      violations.push(`<${el.localName}>.${sink} = ${JSON.stringify(value.slice(0, 60))}`);
      throw new TypeError(REFUSED(sink, "Element"));
    }
    return policy(value);
  };

  const inner = Object.getOwnPropertyDescriptor(Element.prototype, "innerHTML")!;
  const outer = Object.getOwnPropertyDescriptor(Element.prototype, "outerHTML")!;
  const adjacent = Object.getOwnPropertyDescriptor(Element.prototype, "insertAdjacentHTML")!;
  const setter = (sink: string, real: PropertyDescriptor): PropertyDescriptor => ({
    configurable: true,
    get: real.get,
    set(this: Element, value: unknown) {
      real.set!.call(this, vet(this, sink, value));
    },
  });
  Object.defineProperty(Element.prototype, "innerHTML", setter("innerHTML", inner));
  Object.defineProperty(Element.prototype, "outerHTML", setter("outerHTML", outer));
  Object.defineProperty(Element.prototype, "insertAdjacentHTML", {
    ...adjacent,
    value(this: Element, where: InsertPosition, html: unknown) {
      return (adjacent.value as Element["insertAdjacentHTML"]).call(this, where, vet(this, "insertAdjacentHTML", html) as string);
    },
  });

  const handle: Enforcement = {
    violations,
    policies,
    stop() {
      Object.defineProperty(Element.prototype, "innerHTML", inner);
      Object.defineProperty(Element.prototype, "outerHTML", outer);
      Object.defineProperty(Element.prototype, "insertAdjacentHTML", adjacent);
      g.trustedTypes = before;
      if (active === handle) active = null;
    },
  };
  active = handle;
  return handle;
}
