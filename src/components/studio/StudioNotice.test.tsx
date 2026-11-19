// The notice in the chat for a video that finished while the chat was shown (0.1.0 Task B8):
// it says so, and Open goes to Studio's Video tab with the viewer on its first result.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { RUN } from "./fixtures/models";
import type { Run } from "./types";

vi.mock("../../lib/ipc", () => ({ isTauri: () => true }));
vi.mock("./api");

const { StudioNotice } = await import("./StudioNotice");
const { useStudio } = await import("../../store/studio");

const INITIAL = useStudio.getState();

/** A finished video at OpenRouter, with the fixture run's results. */
const VIDEO: Run = { ...RUN, id: "v", kind: "video", model: "google/veo-3.1", modelLabel: "Veo 3.1" };

beforeEach(() => {
  useStudio.setState(INITIAL, true);
});

afterEach(cleanup);

describe("StudioNotice", () => {
  it("a video finished while in chat says so, with a way to open it", () => {
    useStudio.getState().applyProgress({ ...VIDEO, status: "waiting", watching: true });
    useStudio.getState().applyProgress(VIDEO);
    expect(useStudio.getState().readyNotice).toBe("v");
    render(<StudioNotice />);
    screen.getByText("Your video is ready");
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Open" }));
    });
    expect(useStudio.getState()).toMatchObject({ view: "studio", tab: "video", viewer: "o1", readyNotice: null });
    expect(screen.queryByText("Your video is ready")).toBeNull();
  });

  it("shows nothing without a finished video, and can be put away", () => {
    const { container } = render(<StudioNotice />);
    expect(container.textContent).toBe("");
    act(() => useStudio.getState().applyProgress(VIDEO));
    screen.getByText("Your video is ready");
    act(() => {
      fireEvent.click(screen.getByRole("button", { name: "Put the notice away" }));
    });
    expect(useStudio.getState()).toMatchObject({ view: "chat", readyNotice: null });
  });

  it("opening Studio another way puts the notice away", () => {
    useStudio.getState().applyProgress(VIDEO);
    useStudio.getState().showStudio("image");
    expect(useStudio.getState().readyNotice).toBeNull();
  });
});
