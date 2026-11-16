import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

/** What the backend has stored; `setSetting` checks like the real command. */
let stored: unknown = null;

const ipcMock = vi.hoisted(() => ({
  getSetting: vi.fn<(key: string) => Promise<unknown>>(),
  setSetting: vi.fn<(key: string, value: unknown) => Promise<void>>(),
}));
vi.mock("../../lib/ipc", () => ipcMock);
vi.mock("../../store/app", () => ({
  toErrorInfo: (e: unknown) => ({ code: "bad_request", message: String(e), nextStep: "", details: null }),
}));

const { SameCallLimit, sameCallLimitFrom, storedSameCallLimit } = await import("./SameCallLimit");

async function show() {
  await act(async () => {
    render(<SameCallLimit />);
  });
}

function box(): HTMLInputElement {
  return screen.getByRole("spinbutton", {
    name: /Stop when the model makes the same call this many times in a row/,
  }) as HTMLInputElement;
}

async function typeAndLeave(text: string) {
  await act(async () => {
    fireEvent.change(box(), { target: { value: text } });
  });
  await act(async () => {
    fireEvent.blur(box());
  });
}

describe("SameCallLimit", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    stored = null;
    ipcMock.getSetting.mockImplementation(async () => stored);
    ipcMock.setSetting.mockImplementation(async (key, value) => {
      if (key !== "sameCallLimit" || !Number.isInteger(value) || (value as number) < 2 || (value as number) > 100) {
        throw new Error("The number of identical calls in a row must be a whole number from 2 to 100.");
      }
      stored = value;
    });
  });
  afterEach(cleanup);

  it("shows 5 by default, with a one-line help in plain words", async () => {
    await show();
    expect(box().value).toBe("5");
    expect(box().min).toBe("2");
    expect(box().max).toBe("100");
    expect(
      screen.getByText(/The model is warned two calls before the limit \(one call before at 3, no warning at 2\)\./),
    ).toBeTruthy();
    expect(ipcMock.getSetting).toHaveBeenCalledWith("sameCallLimit");
  });

  it("shows the stored number, held to 2 to 100 as a turn reads it", async () => {
    stored = 12;
    await show();
    expect(box().value).toBe("12");
    cleanup();
    stored = 1000;
    await show();
    expect(box().value).toBe("100");
    cleanup();
    stored = "7";
    await show();
    expect(box().value).toBe("5");
    // [0.0.7 review] A stored negative number is not sensible: 5, as a turn reads it.
    cleanup();
    stored = -3;
    await show();
    expect(box().value).toBe("5");
  });

  it("saves a whole number when the box is left or Enter is pressed", async () => {
    await show();
    await typeAndLeave("8");
    expect(ipcMock.setSetting).toHaveBeenCalledWith("sameCallLimit", 8);
    expect(stored).toBe(8);
    await act(async () => {
      fireEvent.change(box(), { target: { value: "20" } });
    });
    await act(async () => {
      fireEvent.keyDown(box(), { key: "Enter" });
    });
    expect(ipcMock.setSetting).toHaveBeenLastCalledWith("sameCallLimit", 20);
    // The same number again is not saved again.
    await typeAndLeave("20");
    expect(ipcMock.setSetting).toHaveBeenCalledTimes(2);
  });

  it("holds a number outside 2 to 100 to the nearest end, and puts back what is not a number", async () => {
    await show();
    await typeAndLeave("1");
    expect(box().value).toBe("2");
    expect(ipcMock.setSetting).toHaveBeenLastCalledWith("sameCallLimit", 2);
    await typeAndLeave("250");
    expect(box().value).toBe("100");
    expect(ipcMock.setSetting).toHaveBeenLastCalledWith("sameCallLimit", 100);
    const calls = ipcMock.setSetting.mock.calls.length;
    await typeAndLeave("");
    expect(box().value).toBe("100");
    await typeAndLeave("3.5");
    expect(box().value).toBe("100");
    expect(ipcMock.setSetting).toHaveBeenCalledTimes(calls);
  });

  it("says why when saving fails, and shows the number that is still in force", async () => {
    stored = 6;
    await show();
    ipcMock.setSetting.mockImplementationOnce(async () => {
      throw new Error("The settings could not be saved.");
    });
    await typeAndLeave("9");
    expect(box().value).toBe("6");
    expect(screen.getByText(/The settings could not be saved\./)).toBeTruthy();
  });

  it("reads a stored value as a turn does", () => {
    expect(storedSameCallLimit(12)).toBe(12);
    expect(storedSameCallLimit(0)).toBe(2);
    expect(storedSameCallLimit(1000)).toBe(100);
    expect(storedSameCallLimit(-3)).toBe(5);
    expect(storedSameCallLimit(3.5)).toBe(5);
    expect(storedSameCallLimit("7")).toBe(5);
    expect(storedSameCallLimit(null)).toBe(5);
    // [0.0.7 verification] As the backend reads them (settings.rs tests the same values).
    expect(storedSameCallLimit(42.0)).toBe(42);
    expect(storedSameCallLimit(-0)).toBe(2);
    expect(storedSameCallLimit(2 ** 64)).toBe(100);
    expect(storedSameCallLimit(1e300)).toBe(100);
  });

  it("reads typed text as a whole number from 2 to 100", () => {
    expect(sameCallLimitFrom("5")).toBe(5);
    expect(sameCallLimitFrom(" 42 ")).toBe(42);
    expect(sameCallLimitFrom("0")).toBe(2);
    expect(sameCallLimitFrom("-4")).toBe(2);
    expect(sameCallLimitFrom("101")).toBe(100);
    expect(sameCallLimitFrom("")).toBeNull();
    expect(sameCallLimitFrom("2.5")).toBeNull();
    expect(sameCallLimitFrom("five")).toBeNull();
  });
});
