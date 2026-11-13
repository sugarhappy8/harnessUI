// Polling for the header's status lines (moved here from RuntimeIndicator.tsx for 0.0.6 item
// 6, so the usage line can use it too). There is no push event for these figures: the
// webview asks while a line that needs them is on screen, and never while the window is
// hidden.
import { useEffect, useState } from "react";

/** Whether the page is shown (polling stops while the window is hidden). */
export function usePageVisible(): boolean {
  const [visible, setVisible] = useState(() => typeof document === "undefined" || document.visibilityState === "visible");
  useEffect(() => {
    const on = () => setVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", on);
    return () => document.removeEventListener("visibilitychange", on);
  }, []);
  return visible;
}

/**
 * Calls `read` now and again after `delay(last result)` milliseconds while `active`; null
 * while inactive. `read` and `delay` must be stable (module-level) functions.
 */
export function usePoll<T>(active: boolean, read: () => Promise<T>, delay: (last: T | null) => number): T | null {
  const [value, setValue] = useState<T | null>(null);
  useEffect(() => {
    if (!active) return;
    let alive = true;
    let timer = 0;
    const tick = async () => {
      let last: T | null = null;
      try {
        last = await read();
        if (alive) setValue(last);
      } catch {
        // Not available now; asked again on the next tick.
      }
      if (alive) timer = window.setTimeout(() => void tick(), delay(last));
    };
    void tick();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [active, read, delay]);
  return active ? value : null;
}
