// The question before quitting while a local job works (0.1.0 Task B8): the backend sends
// `studio-quit-requested` when the main window is closed, or the app quit, while a picture or
// video is being made on this computer; Quit and stop stops it and quits, once.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { LOCAL_RUN } from "./fixtures/models";

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const api = await import("./api");
const { QuitWhileMaking } = await import("./QuitWhileMaking");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();
/** What the component listens with for `studio-quit-requested`. */
let asked: (() => void) | null = null;
const unlisten = vi.fn();

async function flush() {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve();
  });
}

async function mount() {
  await act(async () => {
    render(<QuitWhileMaking />);
  });
  await flush();
}

beforeEach(() => {
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  asked = null;
  vi.mocked(api.onQuitRequested).mockImplementation(async (handler) => {
    asked = handler;
    return unlisten;
  });
  vi.mocked(api.studioQuitNow).mockResolvedValue(undefined);
});

afterEach(cleanup);

describe("QuitWhileMaking", () => {
  it("quitting while a local job works asks first", async () => {
    useStudio.setState({ live: { v: { ...LOCAL_RUN, id: "v", kind: "video", status: "running" } } });
    await mount();
    expect(screen.queryByText(/Quitting stops it/)).toBeNull();
    act(() => asked!());
    screen.getByText("A video is being made on this computer. Quitting stops it.");
    screen.getByRole("button", { name: "Keep working" });
    fireEvent.click(screen.getByRole("button", { name: "Quit and stop" }));
    fireEvent.click(screen.getByRole("button", { name: "Quit and stop" }));
    await flush();
    expect(api.studioQuitNow).toHaveBeenCalledTimes(1);
  });

  it("names a picture, and Keep working quits nothing", async () => {
    useStudio.setState({ live: { p: { ...LOCAL_RUN, id: "p", status: "running" } } });
    await mount();
    act(() => asked!());
    screen.getByText("A picture is being made on this computer. Quitting stops it.");
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Keep working" }));
    });
    expect(screen.queryByText(/Quitting stops it/)).toBeNull();
    expect(api.studioQuitNow).not.toHaveBeenCalled();
  });

  it("stops listening when it goes", async () => {
    await mount();
    cleanup();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});
