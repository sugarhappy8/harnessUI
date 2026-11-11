import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";
import { ArtifactFrame, type Settled } from "./ArtifactFrame";
import type { Problem } from "./types";

const URL = "artifact://sandbox.localhost/7/2";

function show(over: { scripts?: boolean; hidden?: boolean; kind?: "html" | "svg" } = {}) {
  const onProblems = vi.fn<(p: Problem[]) => void>();
  const onSettled = vi.fn<(o: Settled) => void>();
  const view = render(
    <ArtifactFrame
      id={7}
      version={2}
      kind={over.kind ?? "html"}
      url={URL}
      title="Preview of Tips"
      scripts={over.scripts ?? true}
      hidden={over.hidden}
      onProblems={onProblems}
      onSettled={onSettled}
    />,
  );
  const frame = view.container.querySelector("iframe") as HTMLIFrameElement;
  return { frame, onProblems, onSettled };
}

/** A message as the browser delivers it, from `source`. */
function post(data: unknown, source: Window | null) {
  window.dispatchEvent(new MessageEvent("message", { data, source }));
}

const problem = { source: "artifact", id: 7, version: 2, kind: "problem", type: "error", message: "boom", line: 3, col: 1 };
const ready = { source: "artifact", id: 7, version: 2, kind: "ready" };

describe("ArtifactFrame", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("is a sandboxed frame that sends no referrer", () => {
    const { frame } = show();
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
    expect(frame.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(frame.getAttribute("src")).toBe(URL);
    expect(frame.getAttribute("srcdoc")).toBeNull();
  });

  it("allows nothing at all while scripts are off", () => {
    const { frame, onProblems, onSettled } = show({ scripts: false });
    // An empty sandbox attribute is the strictest one; a missing attribute would be none.
    expect(frame.hasAttribute("sandbox")).toBe(true);
    expect(frame.getAttribute("sandbox")).toBe("");
    post(problem, frame.contentWindow);
    vi.advanceTimersByTime(10_000);
    expect(onProblems).not.toHaveBeenCalled();
    expect(onSettled).not.toHaveBeenCalled();
  });

  it("takes messages only from its own frame's window", () => {
    const { frame, onProblems } = show();
    const other = document.createElement("iframe");
    document.body.appendChild(other);
    // The app's own window, another frame, and no source at all: none is this preview.
    post(problem, window);
    post(problem, other.contentWindow);
    post(problem, null);
    expect(onProblems).not.toHaveBeenCalled();
    post(problem, frame.contentWindow);
    expect(onProblems).toHaveBeenCalledTimes(1);
    expect(onProblems).toHaveBeenLastCalledWith([{ type: "error", message: "boom", line: 3, col: 1 }]);
    other.remove();
  });

  it("ignores what is not a message of this preview, and repeats", () => {
    const { frame, onProblems } = show();
    const own = frame.contentWindow;
    post("junk", own);
    post({ ...problem, id: 8 }, own);
    post({ ...problem, version: 1 }, own);
    post({ ...problem, source: "isolation-probe" }, own);
    expect(onProblems).not.toHaveBeenCalled();
    post(problem, own);
    post(problem, own);
    expect(onProblems).toHaveBeenCalledTimes(1);
  });

  it("reports once, a moment after the page said it has loaded", () => {
    const { frame, onProblems, onSettled } = show();
    post(problem, frame.contentWindow);
    post(ready, frame.contentWindow);
    vi.advanceTimersByTime(1400);
    expect(onSettled).not.toHaveBeenCalled();
    // A problem inside that moment is part of the report.
    post({ ...problem, message: "late", line: 9 }, frame.contentWindow);
    vi.advanceTimersByTime(200);
    expect(onSettled).toHaveBeenCalledTimes(1);
    expect(onSettled.mock.calls[0]?.[0]).toEqual({
      problems: [
        { type: "error", message: "boom", line: 3, col: 1 },
        { type: "error", message: "late", line: 9, col: 1 },
      ],
      unavailable: false,
    });
    // Later problems are still shown, but nothing is reported twice.
    post({ ...problem, message: "on click" }, frame.contentWindow);
    post(ready, frame.contentWindow);
    vi.advanceTimersByTime(10_000);
    expect(onSettled).toHaveBeenCalledTimes(1);
    expect(onProblems).toHaveBeenCalledTimes(3);
  });

  it("does not wait for ever for a page that never says it has loaded", () => {
    const { onSettled } = show();
    vi.advanceTimersByTime(3100);
    expect(onSettled).not.toHaveBeenCalled();
    vi.advanceTimersByTime(200);
    expect(onSettled).toHaveBeenCalledWith({ problems: [], unavailable: false });
  });

  it("passes on that a page cannot draw here, and settles a picture sooner", () => {
    const first = show({ kind: "svg" });
    post({ source: "artifact", id: 7, version: 2, kind: "unavailable", message: "x" }, first.frame.contentWindow);
    expect(first.onSettled).toHaveBeenCalledWith({ problems: [], unavailable: true });
    cleanup();
    const second = show({ kind: "svg" });
    post(ready, second.frame.contentWindow);
    vi.advanceTimersByTime(350);
    expect(second.onSettled).toHaveBeenCalledWith({ problems: [], unavailable: false });
  });

  it("looks at only so many messages from one page", () => {
    const { frame, onProblems } = show();
    for (let i = 0; i < 500; i += 1) post({ ...problem, message: `m${i}` }, frame.contentWindow);
    // Ten are kept; a flood changes nothing after that, and costs nothing after 200.
    expect(onProblems).toHaveBeenCalledTimes(10);
  });

  it("is kept out of sight when it only renders for a report", () => {
    const { frame } = show({ hidden: true });
    expect(frame.className).toBe("hidden");
    expect(frame.getAttribute("aria-hidden")).toBe("true");
    expect(frame.getAttribute("sandbox")).toBe("allow-scripts");
  });
});
