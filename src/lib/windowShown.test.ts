import { beforeEach, describe, expect, it, vi } from "vitest";

type Handler = (event: { payload: unknown }) => void;
const listenMock = vi.hoisted(() =>
  vi.fn<(name: string, handler: Handler) => Promise<() => void>>(async () => () => undefined),
);
vi.mock("@tauri-apps/api/event", () => ({ listen: listenMock }));
const tauri = vi.hoisted(() => ({ inApp: false }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => tauri.inApp }));

const { MAIN_WINDOW_SHOWN, onMainWindowShown } = await import("./windowShown");

async function flush() {
  for (let i = 0; i < 3; i++) await Promise.resolve();
}

describe("onMainWindowShown (0.0.7)", () => {
  beforeEach(() => {
    listenMock.mockClear();
  });

  it("does nothing outside the app", () => {
    tauri.inApp = false;
    const stop = onMainWindowShown(() => undefined);
    expect(listenMock).not.toHaveBeenCalled();
    stop();
  });

  it("calls the handler for each time the app brought the window back, until stopped", async () => {
    tauri.inApp = true;
    let send: () => void = () => undefined;
    const unlisten = vi.fn();
    listenMock.mockImplementation(async (_name, handler) => {
      send = () => handler({ payload: null });
      return unlisten;
    });
    const handler = vi.fn();
    const stop = onMainWindowShown(handler);
    expect(listenMock).toHaveBeenCalledWith(MAIN_WINDOW_SHOWN, expect.any(Function));
    await flush();
    send();
    send();
    expect(handler).toHaveBeenCalledTimes(2);
    stop();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });
});
