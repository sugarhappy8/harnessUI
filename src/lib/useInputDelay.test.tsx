import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";

// The app's "main window shown" event, sent by hand here.
const shown = vi.hoisted(() => ({ handlers: new Set<() => void>() }));
vi.mock("./windowShown", () => ({
  MAIN_WINDOW_SHOWN: "main-window-shown",
  onMainWindowShown: (handler: () => void) => {
    shown.handlers.add(handler);
    return () => shown.handlers.delete(handler);
  },
}));

const { useInputDelay } = await import("./useInputDelay");

const DELAY = 700;

describe("useInputDelay (0.0.7)", () => {
  let visibility: DocumentVisibilityState = "visible";
  beforeEach(() => {
    vi.useFakeTimers();
    visibility = "visible";
    Object.defineProperty(document, "visibilityState", { configurable: true, get: () => visibility });
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    Reflect.deleteProperty(document, "visibilityState");
  });

  const wait = (ms: number) =>
    act(() => {
      vi.advanceTimersByTime(ms);
    });

  it("arms after the delay, and again only after the delay each time the window comes back", () => {
    const { result } = renderHook(() => useInputDelay(DELAY));
    expect(result.current.armed).toBe(false);
    expect(result.current.armedRef.current).toBe(false);
    wait(DELAY + 10);
    expect(result.current.armed).toBe(true);
    expect(result.current.armedRef.current).toBe(true);
    // Before the window appears and once it is there: the ref changes at once, before
    // React draws again.
    let atOnce: boolean | undefined;
    act(() => {
      for (const h of [...shown.handlers]) h();
      atOnce = result.current.armedRef.current;
    });
    expect(atOnce).toBe(false);
    expect(result.current.armed).toBe(false);
    wait(DELAY - 100);
    expect(result.current.armedRef.current).toBe(false);
    wait(110);
    expect(result.current.armedRef.current).toBe(true);
  });

  it("does not count the time while the page is hidden", () => {
    visibility = "hidden";
    const { result } = renderHook(() => useInputDelay(DELAY));
    wait(5000);
    expect(result.current.armedRef.current).toBe(false);
    act(() => {
      visibility = "visible";
      document.dispatchEvent(new Event("visibilitychange"));
    });
    wait(DELAY + 10);
    expect(result.current.armed).toBe(true);
  });

  it("stops listening when it goes, and a late event starts nothing", () => {
    const { unmount } = renderHook(() => useInputDelay(DELAY));
    const handler = [...shown.handlers][0];
    unmount();
    expect(shown.handlers.size).toBe(0);
    act(() => handler?.());
    expect(vi.getTimerCount()).toBe(0);
  });
});
