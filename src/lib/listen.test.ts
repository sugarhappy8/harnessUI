import { describe, expect, it, vi } from "vitest";
import { listenInEffect } from "./listen";

/** A listener that starts when the test says so, as Tauri's `listen` does (asynchronously). */
function delayedListen() {
  const unlisten = vi.fn();
  let ready: () => void = () => undefined;
  const start = vi.fn(
    () =>
      new Promise<() => void>((resolve) => {
        ready = () => resolve(unlisten);
      }),
  );
  return { start, unlisten, ready: () => ready() };
}

async function flush() {
  for (let i = 0; i < 3; i++) await Promise.resolve();
}

describe("listenInEffect (0.0.7 P6)", () => {
  it("stops a listener that starts after the cleanup ran", async () => {
    const l = delayedListen();
    const cleanup = listenInEffect(l.start);
    cleanup();
    expect(l.unlisten).not.toHaveBeenCalled();
    l.ready();
    await flush();
    expect(l.unlisten).toHaveBeenCalledTimes(1);
  });

  it("keeps a listener until the cleanup, and stops it once", async () => {
    const l = delayedListen();
    const cleanup = listenInEffect(l.start);
    l.ready();
    await flush();
    expect(l.unlisten).not.toHaveBeenCalled();
    cleanup();
    cleanup();
    expect(l.unlisten).toHaveBeenCalledTimes(1);
  });

  it("is quiet when listening cannot start", async () => {
    const cleanup = listenInEffect(() => Promise.reject(new Error("no app")));
    await flush();
    expect(() => cleanup()).not.toThrow();
  });
});
