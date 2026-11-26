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
const { useStudio } = await import("../store/studio");
const { LOCAL_RUN } = await import("./studio/fixtures/models");

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

const box = () => screen.queryByRole("region", { name: "Update ready" });

/** Gives elements the page positions a real window would (jsdom lays nothing out). */
function placeOnPage(rects: { match: (el: HTMLElement) => boolean; rect: Partial<DOMRect> }[]) {
  return vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    const found = rects.find((r) => r.match(this));
    return { top: 0, bottom: 0, left: 0, right: 0, width: 0, height: 0, x: 0, y: 0, ...found?.rect } as DOMRect;
  });
}

describe("the Update ready box", () => {
  beforeEach(() => {
    statusNow = status();
    installNow = async () => undefined;
    window.sessionStorage.clear();
    useApp.setState({ generation: null, runningTurn: null });
    useStudio.setState({ live: {} });
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

  // 0.2.0 (design 9.2): installing stops a local Studio job instead of being refused, so the
  // box says so first, as a note above the install button.
  it("the warning line shows while a local job runs", async () => {
    statusNow = status({ ready: info("0.0.9") });
    await show();
    const warning = "Installing stops the picture being made";
    expect(screen.queryByText(warning)).toBeNull();
    act(() => useStudio.setState({ live: { [LOCAL_RUN.id]: { ...LOCAL_RUN, status: "running" } } }));
    const note = screen.getByRole("note");
    expect(note.textContent).toBe(warning);
    // Above the install button, which stays on.
    const install = screen.getByRole("button", { name: "Restart and update" }) as HTMLButtonElement;
    expect(note.compareDocumentPosition(install) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(install.disabled).toBe(false);
    // A run waiting for its turn is stopped too.
    act(() => useStudio.setState({ live: { [LOCAL_RUN.id]: { ...LOCAL_RUN, status: "queued" } } }));
    expect(screen.getByRole("note").textContent).toBe(warning);
    // A cloud run, or a local one that ended, is not stopped by installing.
    act(() => useStudio.setState({ live: { [LOCAL_RUN.id]: { ...LOCAL_RUN, status: "done" } } }));
    expect(screen.queryByText(warning)).toBeNull();
    act(() => useStudio.setState({ live: { a: { ...LOCAL_RUN, id: "a", runsOn: "cloud", status: "running" } } }));
    expect(screen.queryByText(warning)).toBeNull();
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

  it("is a region named by its heading, and a separate status says a version is ready", async () => {
    await show();
    const status = screen.getByRole("status");
    expect(status.textContent).toBe("");
    act(() => onReady(info("0.0.9")));
    const b = box() as HTMLElement;
    expect(b.getAttribute("role")).toBe("region");
    expect(b.getAttribute("aria-live")).toBeNull();
    expect(b.contains(status)).toBe(false);
    expect(status.textContent).toBe("Update ready. harnessUI 0.0.9 is ready to install.");
    // What changes inside the box later is not inside a live region.
    act(() => useApp.setState({ runningTurn: 3 }));
    expect(status.contains(screen.getByText("Updating stops the current reply."))).toBe(false);
  });

  it("puts the caret back in the message box after Later", async () => {
    const focus = vi.fn();
    window.addEventListener("app:focus-input", focus);
    try {
      statusNow = status({ ready: info("0.0.9") });
      await show();
      fireEvent.click(screen.getByRole("button", { name: "Later" }));
      expect(focus).toHaveBeenCalledTimes(1);
      expect(screen.getByRole("status").textContent).toBe("");
    } finally {
      window.removeEventListener("app:focus-input", focus);
    }
  });

  it("over the sidebar, grows no higher than the chat list and leaves room at the list's end", async () => {
    const nav = document.createElement("nav");
    nav.setAttribute("aria-label", "Chat history");
    document.body.appendChild(nav);
    const rects = placeOnPage([
      { match: (el) => el === nav, rect: { top: 136, bottom: 724 } },
      { match: (el) => el.hasAttribute("data-update-box"), rect: { top: 562, bottom: 724 } },
    ]);
    try {
      statusNow = status({ ready: info("0.0.9") });
      await show("sidebar");
      // From the box's bottom edge up to 8px under the top of the chat list.
      expect((box() as HTMLElement).style.maxHeight).toBe("580px");
      // The list's end is covered from the box's top: that much room, plus the gap.
      const root = document.documentElement;
      expect(root.style.getPropertyValue("--update-box-space")).toBe("170px");
      fireEvent.click(screen.getByRole("button", { name: "Later" }));
      expect(root.style.getPropertyValue("--update-box-space")).toBe("");
    } finally {
      rects.mockRestore();
      nav.remove();
    }
  });

  it("under the header, ends above the message box", async () => {
    const main = document.createElement("main");
    const header = document.createElement("header");
    main.appendChild(header);
    const composer = document.createElement("div");
    composer.setAttribute("data-composer", "");
    main.appendChild(composer);
    document.body.appendChild(main);
    const rects = placeOnPage([
      { match: (el) => el === header, rect: { bottom: 66 } },
      { match: (el) => el === composer, rect: { top: 395 } },
    ]);
    try {
      statusNow = status({ ready: info("0.0.9") });
      await show("below-header");
      const b = box() as HTMLElement;
      expect(b.style.top).toBe("74px");
      expect(b.style.maxHeight).toBe(`${395 - 8 - 74}px`);
      // The release notes give way first; the heading and the buttons keep their size.
      expect(b.className.split(/\s+/)).toEqual(expect.arrayContaining(["flex", "flex-col", "overflow-y-auto"]));
      fireEvent.click(screen.getByRole("button", { name: "What's new" }));
      const notes = screen.getByRole("region", { name: "Release notes" });
      expect(notes.className.split(/\s+/)).toContain("min-h-12");
      expect(screen.getByRole("button", { name: "Later" }).parentElement?.className.split(/\s+/)).toContain("shrink-0");
      expect(document.documentElement.style.getPropertyValue("--update-box-space")).toBe("0px");
    } finally {
      rects.mockRestore();
      main.remove();
    }
  });

  it("in the corner of the first-run screen, leaves room at the screen's end", async () => {
    const rects = placeOnPage([{ match: (el) => el.hasAttribute("data-update-box"), rect: { top: window.innerHeight - 146 } }]);
    try {
      statusNow = status({ ready: info("0.0.9") });
      await show("corner");
      expect((box() as HTMLElement).style.maxHeight).toBe("");
      expect(document.documentElement.style.getPropertyValue("--update-box-space")).toBe("154px");
    } finally {
      rects.mockRestore();
    }
  });
});
