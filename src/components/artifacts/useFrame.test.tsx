import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import type { FrameInfo } from "./types";

const artifactFrame = vi.fn<(chatId: number, id: number, version: number) => Promise<FrameInfo>>();
vi.mock("./api", () => ({ artifactFrame: (...a: [number, number, number]) => artifactFrame(...a) }));

const { runsScripts, useFrame } = await import("./useFrame");

function Probe(props: { id: number | null; version: number | null; again: string }) {
  const frame = useFrame(1, props.id, props.version, props.again);
  return <output>{frame ? `${frame.url} ${frame.running}` : "none"}</output>;
}

let issued = 0;
beforeEach(() => {
  issued = 0;
  artifactFrame.mockReset();
  artifactFrame.mockImplementation((_chat, id, version) => {
    issued += 1;
    return Promise.resolve({ url: `artifact://sandbox.localhost/${id}/${version}?t=${issued}`, running: "page" });
  });
});
afterEach(cleanup);

describe("useFrame", () => {
  it("asks for a new address for every frame that is created", async () => {
    const view = render(<Probe id={7} version={2} again="0" />);
    expect(screen.getByRole("status").textContent).toBe("none");
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("artifact://sandbox.localhost/7/2?t=1 page"));
    // The same frame again: nothing is asked.
    view.rerender(<Probe id={7} version={2} again="0" />);
    expect(artifactFrame).toHaveBeenCalledTimes(1);
    // The user reloads the preview: the old address is used up, so a new one is asked for.
    view.rerender(<Probe id={7} version={2} again="1" />);
    expect(screen.getByRole("status").textContent).toBe("none");
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("artifact://sandbox.localhost/7/2?t=2 page"));
    // Another version is another frame.
    view.rerender(<Probe id={7} version={3} again="1" />);
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("artifact://sandbox.localhost/7/3?t=3 page"));
    expect(artifactFrame).toHaveBeenCalledTimes(3);
  });

  it("asks for nothing when there is nothing to frame, and shows nothing when the app refuses", async () => {
    const view = render(<Probe id={null} version={null} again="0" />);
    expect(artifactFrame).not.toHaveBeenCalled();
    artifactFrame.mockRejectedValueOnce(new Error("no such version"));
    view.rerender(<Probe id={7} version={9} again="0" />);
    await waitFor(() => expect(artifactFrame).toHaveBeenCalledTimes(1));
    expect(screen.getByRole("status").textContent).toBe("none");
  });

  it("does not use an answer that arrives after the frame has changed", async () => {
    let answerFirst: (f: FrameInfo) => void = () => undefined;
    artifactFrame.mockImplementationOnce(() => new Promise<FrameInfo>((resolve) => (answerFirst = resolve)));
    const view = render(<Probe id={7} version={2} again="0" />);
    view.rerender(<Probe id={8} version={1} again="0" />);
    await waitFor(() => expect(screen.getByRole("status").textContent).toBe("artifact://sandbox.localhost/8/1?t=1 page"));
    answerFirst({ url: "artifact://sandbox.localhost/7/2?t=late", running: "page" });
    await Promise.resolve();
    expect(screen.getByRole("status").textContent).toBe("artifact://sandbox.localhost/8/1?t=1 page");
  });

  it("knows in which frames something runs", () => {
    const frame = (running: FrameInfo["running"]): FrameInfo => ({ url: "u", running });
    expect(runsScripts(frame("page"))).toBe(true);
    expect(runsScripts(frame("appOnly"))).toBe(true);
    expect(runsScripts(frame("held"))).toBe(false);
    expect(runsScripts(frame("notProven"))).toBe(false);
  });
});
