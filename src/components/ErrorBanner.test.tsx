import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

// [0.0.7] The app's "main window shown" event, sent by hand here.
const shown = vi.hoisted(() => ({ handlers: new Set<() => void>() }));
vi.mock("../lib/windowShown", () => ({
  MAIN_WINDOW_SHOWN: "main-window-shown",
  onMainWindowShown: (handler: () => void) => {
    shown.handlers.add(handler);
    return () => shown.handlers.delete(handler);
  },
}));

const { ErrorBanner } = await import("./ErrorBanner");

const error = { code: "stepCap", message: "The model reached the step limit for one turn.", nextStep: "Choose Continue.", details: "" };

describe("ErrorBanner", () => {
  afterEach(cleanup);

  it("is an alert by default, with no action", () => {
    render(<ErrorBanner error={{ ...error, code: "http_500" }} />);
    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
  });

  // A stop at the step limit is not a failure: it is a notice with the way forward.
  it("shows a notice with its action", () => {
    const onClick = vi.fn();
    render(<ErrorBanner error={error} tone="notice" action={{ label: "Continue", onClick }} />);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("status").textContent).toContain("step limit");
    fireEvent.click(screen.getByRole("button", { name: "Continue" }));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  // [0.0.7] Continue appears as a step-capped turn ends, when the app may bring its window
  // back after stepping aside for computer use: a stray click must not set the agent going.
  it("with a delay, ignores clicks at first and again when the app brings its window back", () => {
    vi.useFakeTimers();
    try {
      const onClick = vi.fn();
      render(<ErrorBanner error={error} tone="notice" action={{ label: "Continue", onClick, delayMs: 700 }} />);
      const button = screen.getByRole("button", { name: "Continue" });
      fireEvent.click(button);
      expect(onClick).not.toHaveBeenCalled();
      expect(button.getAttribute("aria-disabled")).toBe("true");
      act(() => {
        vi.advanceTimersByTime(710);
      });
      act(() => {
        for (const h of [...shown.handlers]) h();
        button.click();
      });
      expect(onClick).not.toHaveBeenCalled();
      act(() => {
        vi.advanceTimersByTime(710);
      });
      fireEvent.click(button);
      expect(onClick).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
