import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { UpdateInfo, UpdateStatus } from "../lib/types";

// The event handler the box subscribes, what `update_status` answers, and what installing does.
let onReady: (u: UpdateInfo) => void = () => undefined;
let statusNow: UpdateStatus;
let installNow: () => Promise<void>;

vi.mock("../lib/ipc", () => ({
  isTauri: () => true,
  onUpdateReady: vi.fn(async (h: (u: UpdateInfo) => void) => {
    onReady = h;
    return () => undefined;
  }),
  updateStatus: vi.fn(async () => statusNow),
  installUpdate: vi.fn(() => installNow()),
}));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => undefined) }));

const ipc = await import("../lib/ipc");
const { UpdateBox } = await import("./UpdateBox");
const { useApp } = await import("../store/app");

function info(version: string, notes: string | null = "Fixes."): UpdateInfo {
  return { version, date: null, notes };
}

function status(patch: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    configured: true,
    autoCheck: true,
    currentVersion: "0.0.8",
    checking: false,
    checked: true,
    downloading: null,
    available: null,
    ready: null,
    error: null,
    ...patch,
  };
}

async function show(placement: "sidebar" | "below-header" | "corner" = "sidebar") {
  await act(async () => {
    render(<UpdateBox placement={placement} />);
  });
}

const box = () => screen.queryByRole("status", { name: "Update ready" });

describe("the Update ready box", () => {
  beforeEach(() => {
    statusNow = status();
    installNow = async () => undefined;
    window.sessionStorage.clear();
    useApp.setState({ generation: null, runningTurn: null });
    vi.mocked(ipc.installUpdate).mockClear();
  });
  afterEach(cleanup);

  it("shows nothing while no version is ready", async () => {
    statusNow = status({ available: info("0.0.9"), downloading: "0.0.9" });
    await show();
    expect(box()).toBeNull();
    expect(screen.queryByText(/ready to install/)).toBeNull();
  });

  it("shows a version that was ready before the window listened", async () => {
    statusNow = status({ ready: info("0.0.9") });
    await show();
    const b = box();
    expect(b).not.toBeNull();
    expect(within(b as HTMLElement).getByRole("heading", { name: "Update ready" })).toBeTruthy();
    expect(screen.getByText("harnessUI 0.0.9 is ready to install.")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Restart and update" })).toBeTruthy();
    expect(screen.getByRole("button", { name: "Later" })).toBeTruthy();
  });

  it("appears when the ready event arrives", async () => {
    await show();
    expect(box()).toBeNull();
    act(() => onReady(info("0.0.9")));
    expect(screen.getByText("harnessUI 0.0.9 is ready to install.")).toBeTruthy();
  });

  it("hides on Later until a newer version is ready", async () => {
    await show();
    act(() => onReady(info("0.0.9")));
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    expect(box()).toBeNull();
    // The same version again (a later check) stays hidden.
    act(() => onReady(info("0.0.9")));
    expect(box()).toBeNull();
    act(() => onReady(info("0.0.10")));
    expect(screen.getByText("harnessUI 0.0.10 is ready to install.")).toBeTruthy();
  });

  it("keeps Later when the window draws the box again (until the app starts again)", async () => {
    statusNow = status({ ready: info("0.0.9") });
    await show();
    fireEvent.click(screen.getByRole("button", { name: "Later" }));
    cleanup();
    await show();
    expect(box()).toBeNull();
  });

  it("installs on Restart and update, with both buttons disabled meanwhile", async () => {
    let finish: () => void = () => undefined;
    installNow = () => new Promise<void>((resolve) => (finish = resolve));
    statusNow = status({ ready: info("0.0.9") });
    await show();
    fireEvent.click(screen.getByRole("button", { name: "Restart and update" }));
    expect(ipc.installUpdate).toHaveBeenCalledTimes(1);
    const updating = screen.getByRole("button", { name: "Updating…" }) as HTMLButtonElement;
    expect(updating.disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Later" }) as HTMLButtonElement).disabled).toBe(true);
    await act(async () => finish());
  });

  it("shows why installing failed inside the box, and can try again", async () => {
    installNow = async () => {
      throw {
        code: "update_install",
        message: "The update could not be installed.",
        nextStep: "Move harnessUI to the Applications folder, open it from there, and try again.",
        details: "Read-only file system (os error 30)",
      };
    };
    statusNow = status({ ready: info("0.0.9") });
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Restart and update" }));
    });
    const b = box() as HTMLElement;
    expect(within(b).getByRole("alert").textContent).toMatch(/could not be installed.*Applications folder/);
    const again = within(b).getByRole("button", { name: "Restart and update" }) as HTMLButtonElement;
    expect(again.disabled).toBe(false);
  });

  it("shows the release notes as plain text on What's new", async () => {
    const notes = "<b>Bold?</b>\n  - Indented line\n<script>alert(1)</script>";
    statusNow = status({ ready: info("0.0.9", notes) });
    await show();
    expect(screen.queryByRole("region", { name: "Release notes" })).toBeNull();
    const toggle = screen.getByRole("button", { name: "What's new" });
    expect(toggle.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(toggle);
    expect(toggle.getAttribute("aria-expanded")).toBe("true");
    const region = screen.getByRole("region", { name: "Release notes" });
    expect(region.textContent).toBe(notes);
    expect(region.querySelector("b, script")).toBeNull();
    expect(region.className.split(/\s+/)).toEqual(expect.arrayContaining(["whitespace-pre-wrap", "overflow-y-auto"]));
  });

  it("offers no What's new without notes", async () => {
    statusNow = status({ ready: info("0.0.9", "  ") });
    await show();
    expect(screen.queryByRole("button", { name: "What's new" })).toBeNull();
  });

  it("says that updating stops a reply that is running", async () => {
    statusNow = status({ ready: info("0.0.9") });
    await show();
    expect(screen.queryByText("Updating stops the current reply.")).toBeNull();
    act(() => useApp.setState({ runningTurn: 3 }));
    expect(screen.getByText("Updating stops the current reply.")).toBeTruthy();
  });

  it("floats over the foot of the sidebar, narrower than the sidebar", async () => {
    statusNow = status({ ready: info("0.0.9") });
    await show("sidebar");
    const classes = (box() as HTMLElement).className.split(/\s+/);
    expect(classes).toEqual(expect.arrayContaining(["fixed", "left-2", "w-60", "bottom-[4.75rem]"]));
  });

  it("goes under the chat's header when the sidebar is hidden", async () => {
    const main = document.createElement("main");
    const header = document.createElement("header");
    header.getBoundingClientRect = () => ({ bottom: 57 }) as DOMRect;
    main.appendChild(header);
    document.body.appendChild(main);
    try {
      statusNow = status({ ready: info("0.0.9") });
      await show("below-header");
      const b = box() as HTMLElement;
      expect(b.style.top).toBe("65px");
      expect(b.className.split(/\s+/)).not.toContain("bottom-4");
    } finally {
      main.remove();
    }
  });
});
