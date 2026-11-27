// The error boundary (0.2.1): an error thrown while its children are drawn shows the fallback
// instead of unmounting the app, `reset` and a change of `resetKeys` draw them again, and the
// console report carries only the component, the error's name and its cleaned message.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { ErrorBoundary } from "./ErrorBoundary";

let console_: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  console_ = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  console_.mockRestore();
});

const box = { broken: true };

function Child(props: { label?: string }) {
  if (box.broken) throw new TypeError("Cannot read properties of undefined (reading 'length')");
  return <p>{props.label ?? "drawn"}</p>;
}

function fallback({ error, reset }: { error: Error; reset: () => void }) {
  return (
    <div role="alert">
      <span>{`${error.name}: ${error.message}`}</span>
      <button type="button" onClick={reset}>
        Try again
      </button>
    </div>
  );
}

describe("ErrorBoundary", () => {
  it("shows the fallback in place of a child that throws, and leaves its siblings drawn", () => {
    box.broken = true;
    render(
      <div>
        <p>sibling</p>
        <ErrorBoundary name="Child" fallback={fallback}>
          <Child />
        </ErrorBoundary>
      </div>,
    );
    expect(screen.getByRole("alert").textContent).toContain("TypeError: Cannot read properties of undefined");
    expect(screen.getByText("sibling")).toBeTruthy();
    expect(screen.queryByText("drawn")).toBeNull();
  });

  it("draws the children again on reset, and calls onReset", () => {
    box.broken = true;
    const onReset = vi.fn();
    render(
      <ErrorBoundary name="Child" fallback={fallback} onReset={onReset}>
        <Child />
      </ErrorBoundary>,
    );
    box.broken = false;
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(screen.getByText("drawn")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(onReset).toHaveBeenCalledTimes(1);
  });

  it("shows the fallback again when the reset children throw again", () => {
    box.broken = true;
    render(
      <ErrorBoundary name="Child" fallback={fallback}>
        <Child />
      </ErrorBoundary>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(screen.getByRole("alert")).toBeTruthy();
  });

  it("draws the children again when a reset key changes, and not before", () => {
    box.broken = true;
    const tree = (model: string, other = 0) => (
      <ErrorBoundary name="Child" fallback={fallback} resetKeys={[model, other]}>
        <Child label={model} />
      </ErrorBoundary>
    );
    const { rerender } = render(tree("chatrlabs/a"));
    expect(screen.getByRole("alert")).toBeTruthy();
    box.broken = false;
    rerender(tree("chatrlabs/a")); // Same keys: still the fallback.
    expect(screen.getByRole("alert")).toBeTruthy();
    rerender(tree("city96/b")); // Another model chosen.
    expect(screen.getByText("city96/b")).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("settles on the fallback when the change of keys itself makes the children throw", () => {
    box.broken = false;
    const tree = (model: string) => (
      <ErrorBoundary name="Child" fallback={fallback} resetKeys={[model]}>
        <Child label={model} />
      </ErrorBoundary>
    );
    const { rerender } = render(tree("a"));
    box.broken = true;
    act(() => rerender(tree("b")));
    expect(screen.getByRole("alert")).toBeTruthy();
  });

  it("tells the fallback when a Try again failed, and forgets it once the children draw", () => {
    box.broken = true;
    const seen: boolean[] = [];
    const tree = (label: string) => (
      <ErrorBoundary
        name="Child"
        fallback={({ again, reset }) => {
          seen.push(again);
          return (
            <button type="button" onClick={reset}>
              Try again
            </button>
          );
        }}
      >
        <Child label={label} />
      </ErrorBoundary>
    );
    const { rerender } = render(tree("one"));
    expect(seen.at(-1)).toBe(false);
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(seen.at(-1)).toBe(true);
    box.broken = false;
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(screen.getByText("one")).toBeTruthy();
    // A later error, after the children drew, is a first one again.
    box.broken = true;
    rerender(tree("two"));
    expect(seen.at(-1)).toBe(false);
  });

  it("tells onReset whether Try again or a change of keys drew the children again", () => {
    box.broken = true;
    const onReset = vi.fn();
    const { rerender } = render(
      <ErrorBoundary name="Child" fallback={fallback} resetKeys={[1]} onReset={onReset}>
        <Child />
      </ErrorBoundary>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(onReset).toHaveBeenLastCalledWith("user");
    rerender(
      <ErrorBoundary name="Child" fallback={fallback} resetKeys={[2]} onReset={onReset}>
        <Child />
      </ErrorBoundary>,
    );
    expect(onReset).toHaveBeenLastCalledWith("keys");
  });

  it("turns anything thrown into an Error", () => {
    function Throws(): never {
      throw "a plain string";
    }
    render(
      <ErrorBoundary name="Throws" fallback={fallback}>
        <Throws />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert").textContent).toContain("Error: a plain string");
  });

  it("reports the component, the error's name and its cleaned message, never a path or the stack", () => {
    function Leaky(): never {
      throw new Error("ENOENT: no such file '/home/alice/.config/harnessUI/x.json' key hf_abcdefghijklmnop");
    }
    render(
      <ErrorBoundary name="ModelDetail" fallback={fallback}>
        <Leaky />
      </ErrorBoundary>,
    );
    const ours = console_.mock.calls.map((c: unknown[]) => String(c[0])).filter((t: string) => t.startsWith("[crash]"));
    expect(ours).toEqual(["[crash] ModelDetail could not be drawn: Error: ENOENT: no such file '…' key …"]);
    expect(ours.join()).not.toMatch(/alice|hf_|at Leaky|\.tsx/);
  });
});
