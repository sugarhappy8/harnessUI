// Test helper (hand-written): jsdom runs no CSS, so Radix unmounts a closed alert dialog at once.
// In the app, styles.css gives a closing `[role="alertdialog"]` a 130 ms `dialog-out` animation,
// and Radix keeps it on screen, clickable, until that ends. `alertDialogsAnimate()` makes jsdom
// report those animation names, so a test can look at (and click in) a dialog while it closes.
import { act } from "@testing-library/react";
import { onTestFinished, vi } from "vitest";

/** For the rest of the test, alert dialogs report styles.css's animations: `dialog-in` while
 *  open, `dialog-out` once closed, so a closed one stays until `endAnimation`. */
export function alertDialogsAnimate() {
  const real = window.getComputedStyle.bind(window);
  const spy = vi.spyOn(window, "getComputedStyle").mockImplementation((el, pseudo) => {
    const styles = real(el, pseudo);
    if (el.getAttribute("role") !== "alertdialog") return styles;
    return new Proxy(styles, {
      get(target, prop) {
        if (prop === "animationName") return el.getAttribute("data-state") === "open" ? "dialog-in" : "dialog-out";
        const value: unknown = Reflect.get(target, prop, target);
        return typeof value === "function" ? (value as (...a: unknown[]) => unknown).bind(target) : value;
      },
    });
  });
  onTestFinished(() => spy.mockRestore());
}

/** The closing animation of `el` (an alert dialog) ends, and Radix removes it. */
export async function endAnimation(el: Element) {
  // jsdom has no AnimationEvent; Radix reads only `animationName` and `target`.
  const event = Object.assign(new Event("animationend", { bubbles: true }), { animationName: "dialog-out" });
  await act(async () => {
    el.dispatchEvent(event);
  });
}
