import { useEffect, useRef, useState } from "react";
import { onMainWindowShown } from "./windowShown";

/**
 * [0.0.7] The input delay of a button that must not take a click meant for something else:
 * the choices on an approval card, and the confirm button of the Allow all and Full access
 * confirmations. Clicks are ignored for `ms` after the button appears, and again each time
 * the app brings its window back on screen after stepping aside for computer use (it says
 * so just before the window appears and again once it is there, in front of where the user
 * may be clicking), and whenever the page becomes visible again. Time while the page is
 * hidden does not count. Gaining the focus does not restart it: the click that focuses a
 * card shown in front is the user's own.
 *
 * `armed` is for drawing the button. Click handlers read `armedRef`, which changes at once,
 * before React draws the button again.
 */
export function useInputDelay(ms: number): { armed: boolean; armedRef: { readonly current: boolean } } {
  const [armed, setArmed] = useState(false);
  const armedRef = useRef(false);
  useEffect(() => {
    let live = true;
    let timer: number | undefined;
    const set = (value: boolean) => {
      armedRef.current = value;
      setArmed(value);
    };
    const restart = () => {
      // A Tauri listener can still be called once after the cleanup.
      if (!live) return;
      window.clearTimeout(timer);
      set(false);
      if (document.visibilityState === "hidden") return;
      timer = window.setTimeout(() => set(true), ms);
    };
    restart();
    document.addEventListener("visibilitychange", restart);
    const stopShown = onMainWindowShown(restart);
    return () => {
      live = false;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", restart);
      stopShown();
    };
  }, [ms]);
  return { armed, armedRef };
}
