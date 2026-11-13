import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type {
  ModelBusy,
  ModelLoadInfo,
  ModelLoadSettings,
  RuntimeStatus,
  ShareApp,
  ShareSetup,
  ShareStatus,
} from "../../lib/types";
import { FINE_LAUNCH_NONE, fineFacts, processorPicture } from "../fine.fixtures";

const KEY = "harnessui-share-0123456789abcdef0123456789abcdef";
const NEW_KEY = "harnessui-share-fedcba9876543210fedcba9876543210";
const IDLE: ModelBusy = { chatTurns: 0, modelChecks: 0, otherApps: 0 };

const WARNING =
  "Claude Code and Codex send long instructions. Set How much it can remember to at least 32,768 tokens (about 24,000 words) for this model.";
const TOO_MUCH = "This model can't remember that much on this computer. Try Save memory or a smaller model.";
const SIZE_CHANGED = "You changed how much this model can remember. Copy the setup again.";
const OLD_KEY = "Apps set up with the old key stop working until you copy the new one.";
const PORT_TAKEN = "Port 51234 is used by another program. Choose another port.";
const MODEL_CHANGED =
  "The loaded model can remember a different amount than when you copied the setup. Copy the setup again.";
const PORT_CHANGED = "You changed the port. Copy the setup again.";
const HIDDEN_KEY = "harnessui-share-••••••••";
const KEYCHAIN = {
  code: "keychain",
  message: "The sharing key could not be read from or saved to the system keychain.",
  nextStep: "Unlock the keychain and try again.",
  details: "",
};

let statusNow: ShareStatus;

const ipcMock = vi.hoisted(() => ({
  isTauri: () => false,
  shareStatus: vi.fn<() => Promise<ShareStatus>>(),
  setShareEnabled: vi.fn<(on: boolean) => Promise<ShareStatus>>(),
  setSharePort: vi.fn<(port: number) => Promise<ShareStatus>>(),
  revealShareKey: vi.fn<() => Promise<string>>(),
  newShareKey: vi.fn<() => Promise<ShareStatus>>(),
  shareSetup: vi.fn<(app: ShareApp) => Promise<ShareSetup>>(),
  modelLoadInfo: vi.fn<(path: string) => Promise<ModelLoadInfo>>(),
  setModelLoadSettings: vi.fn<(path: string, o: ModelLoadSettings) => Promise<ModelLoadInfo>>(),
  modelBusy: vi.fn<() => Promise<ModelBusy>>(),
  reloadLocalModel: vi.fn<(force: boolean, whenIdle: boolean) => Promise<boolean>>(async () => true),
  cancelPendingReload: vi.fn<() => Promise<void>>(async () => undefined),
}));
vi.mock("../../lib/ipc", () => ipcMock);
const clipboard = vi.hoisted(() => ({ writeText: vi.fn<(text: string) => Promise<void>>(async () => undefined) }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => clipboard);
// The model's own panel (WP2), as a stand-in: what matters here is where it opens and what
// the page does with what it saves.
const panel = vi.hoisted(() => ({
  saved: null as ModelLoadInfo | null,
  opened: vi.fn<(path: string) => void>(),
}));
vi.mock("../ModelSettings", () => ({
  openModelSettings: panel.opened,
  ModelSettings: (props: { path: string; onClose: () => void; onSaved?: (info: ModelLoadInfo) => void }) => (
    <div role="dialog" aria-label="Settings for this model">
      <p>{props.path}</p>
      <button type="button" onClick={() => panel.saved && props.onSaved?.(panel.saved)}>
        Save
      </button>
      <button type="button" onClick={props.onClose}>
        Close
      </button>
    </div>
  ),
}));

const { OtherApps, POLL_MS, COPIED_KEY } = await import("./OtherApps");
const { useApp } = await import("../../store/app");
const { useQuestion, useWaiting, answerQuestion } = await import("../../store/modelChange");

const browserClipboard = vi.fn<(text: string) => Promise<void>>(async () => undefined);
Object.defineProperty(navigator, "clipboard", { value: { writeText: browserClipboard }, configurable: true });

function share(patch: Partial<ShareStatus> = {}): ShareStatus {
  return { enabled: false, port: null, listening: false, error: null, refused: 0, inUse: false, modelCtx: null, ...patch };
}

const ON = (patch: Partial<ShareStatus> = {}) => share({ enabled: true, port: 51234, listening: true, ...patch });

function runtime(patch: Partial<RuntimeStatus> = {}): RuntimeStatus {
  return {
    state: "ready",
    backend: "vulkan",
    device: "Vulkan0",
    modelPath: "/models/a.gguf",
    modelName: "a.gguf",
    progress: 1,
    stage: null,
    nCtx: 8192,
    supportsTools: true,
    preserveReasoning: false,
    thinking: false,
    speedTps: 30,
    slow: false,
    vision: false,
    offloadedLayers: null,
    totalLayers: null,
    error: null,
    loadStage: null,
    adjustments: [],
    ctxSource: "modelSetting",
    settingsChanged: false,
    reloadWaiting: false,
    unloadedPath: null,
    placement: null,
    ...patch,
  };
}

const OVERRIDES: ModelLoadSettings = {
  context: { fixed: 8192 },
  gpu: "default",
  saveMemory: "on",
  keepInMemory: "default",
  device: "default",
  thinkingLimit: "default",
};

function info(patch: Partial<ModelLoadInfo> = {}): ModelLoadInfo {
  return {
    path: "/models/a.gguf",
    fileName: "a.gguf",
    sizeBytes: 4_920_000_000,
    trainedCtx: 40_960,
    thinking: false,
    vision: false,
    overrides: OVERRIDES,
    defaults: {
      ctx: 8192,
      backend: null,
      device: null,
      contextAuto: true,
      gpu: "all",
      saveMemory: false,
      keepInMemory: false,
      thinkingLimit: "noLimit",
    },
    resolved: {
      ctx: 8192,
      ctxChoice: { fixed: 8192 },
      ctxSource: "modelSetting",
      gpu: "all",
      saveMemory: true,
      keepInMemory: false,
      thinkingLimit: "noLimit",
      backend: null,
      device: null,
      mmproj: null,
      fine: FINE_LAUNCH_NONE,
    },
    fit: "green",
    tooLarge: false,
    contextOptions: [
      { choice: "auto", tokens: 24_576, fit: "green" },
      { choice: { fixed: 8192 }, tokens: 8192, fit: "green" },
      { choice: { fixed: 16_384 }, tokens: 16_384, fit: "green" },
      { choice: { fixed: 32_768 }, tokens: 32_768, fit: "yellow" },
      { choice: { fixed: 40_960 }, tokens: 40_960, fit: "red" },
    ],
    gpuOptions: [],
    devices: [],
    loaded: true,
    olderSettingsLoaded: false,
    autoCtx: null,
    fineFacts: fineFacts(),
    picture: processorPicture(),
    ...patch,
  };
}

function setup(app: ShareApp, patch: Partial<ShareSetup> = {}): ShareSetup {
  if (app === "claudeApp") {
    return {
      app,
      title: "Claude app",
      notTried: true,
      ways: [
        {
          title: "In the Claude app",
          system: "any",
          steps: [
            { text: "Open Help, then Troubleshooting, then Enable Developer Mode.", code: null, file: null },
            { text: "Set Gateway base URL to:", code: "http://127.0.0.1:51234", file: null },
          ],
        },
      ],
      notes: ["To go back to your Claude app account, remove these lines (or close the window)."],
      ...patch,
    };
  }
  const title = app === "claudeCode" ? "Claude Code" : "Codex";
  return {
    app,
    title,
    notTried: false,
    ways: [
      {
        title: "For this window only",
        system: "windows",
        steps: [{ text: "Open Windows PowerShell and paste these lines.", code: `$env:A = '${KEY}'\nclaude`, file: null }],
      },
      {
        title: "For this window only",
        system: "mac",
        steps: [{ text: "Open Terminal and paste these lines.", code: `export A='${KEY}'\nclaude`, file: null }],
      },
      {
        title: "For every window",
        system: "windows",
        steps: [
          {
            text: "Open this file in a text editor, or create it if it is not there.",
            code: '{\n  "env": {}\n}',
            file: "%USERPROFILE%\\.claude\\settings.json",
          },
        ],
      },
      {
        title: "For every window",
        system: "mac",
        steps: [
          { text: "Open this file in a text editor, or create it if it is not there.", code: '{\n  "env": {}\n}', file: "~/.claude/settings.json" },
        ],
      },
    ],
    notes: [
      `To go back to your ${title} account, remove these lines (or close the window).`,
      "Keep harnessUI open with a model loaded while you use it.",
      "This stores the key as plain text in that app's settings.",
    ],
    ...patch,
  };
}

async function show() {
  await act(async () => {
    render(<OtherApps />);
  });
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

/** Advances the page's poll once. */
async function poll() {
  await act(async () => {
    vi.advanceTimersByTime(POLL_MS);
  });
  await act(async () => undefined);
}

/** A promise and the functions that settle it, for answers that arrive late. */
function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  let reject: (e: unknown) => void = () => undefined;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

/** Runs `body` as if on a Mac (`navigator.platform`), then puts the platform back. */
async function onAMac(body: () => Promise<void>) {
  Object.defineProperty(navigator, "platform", { value: "MacIntel", configurable: true });
  try {
    await body();
  } finally {
    // The own property shadows jsdom's getter; removing it brings that back.
    delete (navigator as { platform?: string }).platform;
  }
}

/** The first "Copy step 1 of …" button (of the first way shown). */
function firstCopy(inside?: HTMLElement): HTMLElement {
  const [button] = (inside ? within(inside) : screen).getAllByRole("button", { name: /^Copy step 1/ });
  if (!button) throw new Error("no Copy button");
  return button;
}

function theSwitch(): HTMLInputElement {
  return screen.getByRole("switch", { name: /Let other apps on this computer use the loaded model/ }) as HTMLInputElement;
}

beforeEach(() => {
  vi.clearAllMocks();
  try {
    window.localStorage.clear();
  } catch {
    // jsdom always has it.
  }
  statusNow = share();
  ipcMock.shareStatus.mockImplementation(async () => statusNow);
  ipcMock.setShareEnabled.mockImplementation(async (on) => {
    statusNow = on ? ON({ refused: 0 }) : share({ port: 51234 });
    return statusNow;
  });
  ipcMock.setSharePort.mockImplementation(async (port) => {
    statusNow = { ...statusNow, port: port === 0 ? 60001 : port, listening: statusNow.enabled, error: null };
    return statusNow;
  });
  ipcMock.revealShareKey.mockResolvedValue(KEY);
  ipcMock.newShareKey.mockImplementation(async () => {
    ipcMock.revealShareKey.mockResolvedValue(NEW_KEY);
    return statusNow;
  });
  ipcMock.shareSetup.mockImplementation(async (app) => setup(app));
  ipcMock.modelLoadInfo.mockResolvedValue(info());
  ipcMock.setModelLoadSettings.mockImplementation(async (_p, o) => info({ overrides: o, olderSettingsLoaded: true }));
  ipcMock.modelBusy.mockResolvedValue(IDLE);
  useApp.setState({ runtime: null });
  useQuestion.setState({ question: null, hosts: 0 });
  useWaiting.setState({ change: null, kind: null });
  panel.saved = null;
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("OtherApps: the switch", () => {
  it("is off by default, says it is experimental and what it does, and shows nothing else", async () => {
    await show();
    expect(theSwitch().checked).toBe(false);
    expect(screen.getByText("Experimental")).toBeTruthy();
    expect(screen.getByText(/Claude Code, Codex or the Claude app on this computer/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Show key" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Choose another port" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Claude Code" })).toBeNull();
    expect(ipcMock.revealShareKey).not.toHaveBeenCalled();
    expect(ipcMock.shareSetup).not.toHaveBeenCalled();
  });

  it("switches on with the user's click and shows the port, then off again", async () => {
    await show();
    await click(theSwitch());
    expect(ipcMock.setShareEnabled).toHaveBeenCalledWith(true);
    expect(theSwitch().checked).toBe(true);
    expect(screen.getByText(/On at port 51234\./)).toBeTruthy();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
    // The key is never read just because sharing is on.
    expect(ipcMock.revealShareKey).not.toHaveBeenCalled();
    await click(theSwitch());
    expect(ipcMock.setShareEnabled).toHaveBeenLastCalledWith(false);
    expect(theSwitch().checked).toBe(false);
    expect(screen.queryByText(/On at port/)).toBeNull();
  });

  it("shows why switching on failed and stays off", async () => {
    ipcMock.setShareEnabled.mockRejectedValue({
      code: "keychain",
      message: "The sharing key could not be read from or saved to the system keychain.",
      nextStep: "Unlock the keychain and try again.",
      details: "",
    });
    await show();
    await click(theSwitch());
    expect(screen.getByRole("alert").textContent).toContain("The sharing key could not be read from or saved to the system keychain.");
    expect(theSwitch().checked).toBe(false);
  });

  it("says when no model is loaded, when another app uses it, and counts refused requests", async () => {
    statusNow = ON({ refused: 3 });
    await show();
    expect(screen.getByText(/No model is loaded now/)).toBeTruthy();
    expect(screen.getByText("Requests refused since sharing was switched on: 3")).toBeTruthy();
    expect(screen.getByText(/A request is refused when it has no key or a wrong key, or comes from a web page\./)).toBeTruthy();
    expect(screen.queryByText("Another app is using the model now.")).toBeNull();
  });

  it("says when the model is still loading", async () => {
    useApp.setState({ runtime: runtime({ state: "starting", nCtx: null }) });
    statusNow = ON();
    await show();
    expect(screen.getByText("The model is still loading. Apps get an answer saying so until it is ready.")).toBeTruthy();
    expect(screen.queryByText(/No model is loaded now/)).toBeNull();
  });

  it("follows the backend while the page is open (polling)", async () => {
    vi.useFakeTimers();
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    expect(screen.queryByText("Another app is using the model now.")).toBeNull();
    statusNow = ON({ modelCtx: 32_768, inUse: true, refused: 1 });
    await poll();
    expect(screen.getByText("Another app is using the model now.")).toBeTruthy();
    expect(screen.getByText("Requests refused since sharing was switched on: 1")).toBeTruthy();
  });

  it("changes nothing by itself: while open it only reads, whatever the backend says", async () => {
    vi.useFakeTimers();
    useApp.setState({ runtime: runtime() });
    statusNow = share({ port: 51234 });
    await show();
    await poll();
    // Switched on elsewhere, with a short model and another app at work: still only reads.
    statusNow = ON({ modelCtx: 8192, inUse: true, refused: 4 });
    await poll();
    await poll();
    expect(screen.getByText(WARNING)).toBeTruthy();
    expect(ipcMock.setShareEnabled).not.toHaveBeenCalled();
    expect(ipcMock.setSharePort).not.toHaveBeenCalled();
    expect(ipcMock.newShareKey).not.toHaveBeenCalled();
    expect(ipcMock.revealShareKey).not.toHaveBeenCalled();
    expect(ipcMock.shareSetup).not.toHaveBeenCalled();
    expect(ipcMock.setModelLoadSettings).not.toHaveBeenCalled();
    expect(ipcMock.reloadLocalModel).not.toHaveBeenCalled();
    expect(ipcMock.shareStatus.mock.calls.length).toBeGreaterThanOrEqual(4);
  });

  it("does not let a poll that started before a click undo what the click did", async () => {
    vi.useFakeTimers();
    await show();
    // A poll goes out while sharing is off and answers late.
    let late: (s: ShareStatus) => void = () => undefined;
    ipcMock.shareStatus.mockImplementationOnce(() => new Promise((resolve) => (late = resolve)));
    await act(async () => {
      vi.advanceTimersByTime(POLL_MS);
    });
    await click(theSwitch());
    expect(theSwitch().checked).toBe(true);
    await act(async () => late(share()));
    expect(theSwitch().checked).toBe(true);
  });

  it("says the model is still loading while it stops for a restart, as the other app is told", async () => {
    // A restart unloads first; the proxy answers "still loading" then, not "no model".
    useApp.setState({ runtime: runtime({ state: "stopping", nCtx: null }) });
    statusNow = ON();
    await show();
    expect(screen.getByText("The model is still loading. Apps get an answer saying so until it is ready.")).toBeTruthy();
    expect(screen.queryByText(/No model is loaded now/)).toBeNull();
  });

  it("writes the refused count like other numbers, and says which requests the Audit log lists", async () => {
    statusNow = ON({ refused: 123_456 });
    await show();
    expect(screen.getByText("Requests refused since sharing was switched on: 123,456")).toBeTruthy();
    expect(
      screen.getByText(
        "A request is refused when it has no key or a wrong key, or comes from a web page. Each request for a reply is listed in the Audit log, without what was asked.",
      ),
    ).toBeTruthy();
  });

  it("says when sharing is on but cannot be reached, also with no reason from the backend", async () => {
    statusNow = ON({ listening: false, error: null });
    await show();
    expect(screen.getByRole("alert").textContent).toBe("Sharing is not reachable now. Switch it off and on again.");
    expect(screen.queryByText(/On at port/)).toBeNull();
  });

  it("drops the first read's error once a later read works", async () => {
    vi.useFakeTimers();
    ipcMock.shareStatus.mockRejectedValueOnce({ code: "internal", message: "Boom.", nextStep: "", details: "" });
    await show();
    expect(screen.getByRole("alert").textContent).toContain("Boom.");
    expect(theSwitch().disabled).toBe(true);
    await poll();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(theSwitch().disabled).toBe(false);
  });

  it("does not ask while the page is hidden, and asks again once it is visible", async () => {
    vi.useFakeTimers();
    await show();
    const calls = ipcMock.shareStatus.mock.calls.length;
    const visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue("hidden");
    try {
      await poll();
      await poll();
      expect(ipcMock.shareStatus.mock.calls.length).toBe(calls);
    } finally {
      visibility.mockRestore();
    }
    await poll();
    expect(ipcMock.shareStatus.mock.calls.length).toBe(calls + 1);
  });

  it("drops a key that arrives after sharing was switched off, and keeps Show key and Copy still while it switches", async () => {
    statusNow = ON();
    await show();
    // Show key goes out; the user switches off before it answers.
    const reveal = deferred<string>();
    ipcMock.revealShareKey.mockImplementationOnce(() => reveal.promise);
    await click(screen.getByRole("button", { name: "Show key" }));
    const off = deferred<ShareStatus>();
    ipcMock.setShareEnabled.mockImplementationOnce(() => off.promise);
    await click(theSwitch());
    // While it switches, the key cannot be asked for or copied.
    expect((screen.getByRole("button", { name: "Show key" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("button", { name: "Copy the key" }) as HTMLButtonElement).disabled).toBe(true);
    statusNow = share({ port: 51234 });
    await act(async () => off.resolve(statusNow));
    await act(async () => reveal.resolve(KEY));
    // On again: no key on the page until Show key is clicked again.
    await click(theSwitch());
    expect(theSwitch().checked).toBe(true);
    expect(screen.queryByText(KEY)).toBeNull();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
  });

  it("forgets the key and the card when the backend reports sharing off", async () => {
    vi.useFakeTimers();
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Show key" }));
    await click(screen.getByRole("button", { name: "Claude Code" }));
    expect(screen.getAllByText(new RegExp(KEY)).length).toBeGreaterThan(1);
    statusNow = share({ port: 51234 });
    await poll();
    statusNow = ON({ modelCtx: 32_768 });
    await poll();
    expect(screen.queryByText(new RegExp(KEY))).toBeNull();
    expect(screen.queryByRole("region", { name: "Set up Claude Code" })).toBeNull();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
  });
});

describe("OtherApps: the port", () => {
  it("says when the port is taken and lets the user choose another one", async () => {
    statusNow = ON({ listening: false, error: PORT_TAKEN });
    await show();
    expect(screen.getByRole("alert").textContent).toContain(PORT_TAKEN);
    await click(screen.getByRole("button", { name: "Choose another port" }));
    const input = screen.getByLabelText("Port number") as HTMLInputElement;
    const use = screen.getByRole("button", { name: "Use this port" }) as HTMLButtonElement;
    fireEvent.change(input, { target: { value: "80" } });
    expect(use.disabled).toBe(true);
    expect(screen.getByText("Choose a port from 1024 to 65535.")).toBeTruthy();
    fireEvent.change(input, { target: { value: "52000" } });
    expect(use.disabled).toBe(false);
    await click(use);
    expect(ipcMock.setSharePort).toHaveBeenCalledWith(52000);
    expect(screen.getByText(/On at port 52000\./)).toBeTruthy();
    expect(screen.queryByLabelText("Port number")).toBeNull();
    expect(screen.queryByText(PORT_TAKEN)).toBeNull();
  });

  it("can pick a free port, and keeps the chooser open when the port is taken", async () => {
    statusNow = ON();
    await show();
    await click(screen.getByRole("button", { name: "Choose another port" }));
    ipcMock.setSharePort.mockRejectedValueOnce({
      code: "share_port_taken",
      message: "Port 52001 is used by another program. Choose another port.",
      nextStep: "Choose another port.",
      details: "address in use",
    });
    fireEvent.change(screen.getByLabelText("Port number"), { target: { value: "52001" } });
    await click(screen.getByRole("button", { name: "Use this port" }));
    expect(screen.getByRole("alert").textContent).toContain("Port 52001 is used by another program. Choose another port.");
    expect(screen.getByLabelText("Port number")).toBeTruthy();
    await click(screen.getByRole("button", { name: "Pick a free port" }));
    expect(ipcMock.setSharePort).toHaveBeenLastCalledWith(0);
    expect(screen.getByText(/On at port 60001\./)).toBeTruthy();
  });

  it("Cancel closes the chooser and changes nothing", async () => {
    statusNow = ON();
    await show();
    await click(screen.getByRole("button", { name: "Choose another port" }));
    fireEvent.change(screen.getByLabelText("Port number"), { target: { value: "52000" } });
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByLabelText("Port number")).toBeNull();
    expect(ipcMock.setSharePort).not.toHaveBeenCalled();
    expect(screen.getByText(/On at port 51234\./)).toBeTruthy();
    // Opened again, it starts empty.
    await click(screen.getByRole("button", { name: "Choose another port" }));
    expect((screen.getByLabelText("Port number") as HTMLInputElement).value).toBe("");
  });
});

describe("OtherApps: the key", () => {
  it("shows the key only on request, hides it again and copies it", async () => {
    statusNow = ON();
    await show();
    expect(screen.queryByText(KEY)).toBeNull();
    await click(screen.getByRole("button", { name: "Show key" }));
    expect(ipcMock.revealShareKey).toHaveBeenCalledTimes(1);
    expect(screen.getByText(KEY)).toBeTruthy();
    await click(screen.getByRole("button", { name: "Hide key" }));
    expect(screen.queryByText(KEY)).toBeNull();
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(browserClipboard).toHaveBeenCalledWith(KEY);
    // Copying does not show it.
    expect(screen.queryByText(KEY)).toBeNull();
    expect(screen.getByRole("button", { name: "Copy the key" }).textContent).toBe("Copied");
  });

  it("asks before making a new key, and shows the new one", async () => {
    statusNow = ON();
    await show();
    await click(screen.getByRole("button", { name: "Show key" }));
    await click(screen.getByRole("button", { name: "Make a new key" }));
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    await click(screen.getByRole("button", { name: "Cancel" }));
    expect(ipcMock.newShareKey).not.toHaveBeenCalled();
    expect(screen.queryByText(OLD_KEY)).toBeNull();
    await click(screen.getByRole("button", { name: "Make a new key" }));
    const confirm = screen.getByRole("group", { name: "Make a new key?" });
    await click(within(confirm).getByRole("button", { name: "Make a new key" }));
    expect(ipcMock.newShareKey).toHaveBeenCalledTimes(1);
    expect(screen.getByText(NEW_KEY)).toBeTruthy();
    expect(screen.queryByText(KEY)).toBeNull();
    // Until the new key is copied, the page says the old one stopped working.
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(screen.queryByText(OLD_KEY)).toBeNull();
  });

  it("keeps the key hidden when Hide key is clicked while a new key is made", async () => {
    statusNow = ON();
    await show();
    await click(screen.getByRole("button", { name: "Show key" }));
    expect(screen.getByText(KEY)).toBeTruthy();
    const made = deferred<ShareStatus>();
    ipcMock.newShareKey.mockImplementationOnce(() => {
      ipcMock.revealShareKey.mockResolvedValue(NEW_KEY);
      return made.promise;
    });
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    // The user hides the key while the new one is made.
    await click(screen.getByRole("button", { name: "Hide key" }));
    await act(async () => made.resolve(statusNow));
    expect(screen.queryByText(NEW_KEY)).toBeNull();
    expect(screen.queryByText(KEY)).toBeNull();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
  });

  it("drops a Show key read still on its way when a new key is made, so the old key is never shown or copied", async () => {
    statusNow = ON();
    await show();
    // Show key goes out (the keychain is slow); the user makes a new key before it answers.
    const reveal = deferred<string>();
    ipcMock.revealShareKey.mockImplementationOnce(() => reveal.promise);
    await click(screen.getByRole("button", { name: "Show key" }));
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(ipcMock.newShareKey).toHaveBeenCalledTimes(1);
    // The read answers late, with the old key: it is not shown.
    await act(async () => reveal.resolve(KEY));
    expect(screen.queryByText(KEY)).toBeNull();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
    // Copy copies the new key, never the old one.
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(browserClipboard).toHaveBeenCalledWith(NEW_KEY);
    expect(browserClipboard).not.toHaveBeenCalledWith(KEY);
  });

  it("takes the old key off the page once a new one is made, even when the new one cannot be read", async () => {
    statusNow = ON();
    await show();
    await click(screen.getByRole("button", { name: "Show key" }));
    expect(screen.getByText(KEY)).toBeTruthy();
    ipcMock.newShareKey.mockImplementationOnce(async () => {
      ipcMock.revealShareKey.mockResolvedValue(NEW_KEY);
      ipcMock.revealShareKey.mockRejectedValueOnce(KEYCHAIN);
      return statusNow;
    });
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(screen.getByRole("alert").textContent).toContain(KEYCHAIN.message);
    expect(screen.queryByText(KEY)).toBeNull();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(browserClipboard).toHaveBeenCalledWith(NEW_KEY);
    expect(browserClipboard).not.toHaveBeenCalledWith(KEY);
  });

  it("keeps saying the old key stopped working when a copy of the key started before the new key was made", async () => {
    statusNow = ON();
    await show();
    // Copy the key goes out and reads the key slowly; a new key is made meanwhile.
    const read = deferred<string>();
    ipcMock.revealShareKey.mockImplementationOnce(() => read.promise);
    await click(screen.getByRole("button", { name: "Copy the key" }));
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    // That copy ends with the old key: the page still says to copy the new one.
    await act(async () => read.resolve(KEY));
    expect(browserClipboard).toHaveBeenLastCalledWith(KEY);
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(browserClipboard).toHaveBeenLastCalledWith(NEW_KEY);
    expect(screen.queryByText(OLD_KEY)).toBeNull();
  });

  it("shows why the key could not be read, copied or replaced", async () => {
    statusNow = ON();
    await show();
    ipcMock.revealShareKey.mockRejectedValueOnce(KEYCHAIN);
    await click(screen.getByRole("button", { name: "Show key" }));
    expect(screen.getByRole("alert").textContent).toContain(KEYCHAIN.message);
    expect(screen.queryByText(/harnessui-share-/)).toBeNull();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
    // Copy reads the key too: it says it could not copy, and why.
    ipcMock.revealShareKey.mockRejectedValueOnce(KEYCHAIN);
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(screen.getByRole("button", { name: "Copy the key" }).textContent).toBe("Could not copy");
    expect(browserClipboard).not.toHaveBeenCalled();
    expect(screen.getByRole("alert").textContent).toContain(KEYCHAIN.message);
    ipcMock.newShareKey.mockRejectedValueOnce(KEYCHAIN);
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(screen.getByRole("alert").textContent).toContain(KEYCHAIN.message);
    // The old key still works, so the page does not say it stopped working.
    expect(screen.queryByText(OLD_KEY)).toBeNull();
  });

  it("says when the clipboard refused a copy, instead of saying nothing", async () => {
    vi.useFakeTimers();
    browserClipboard.mockRejectedValueOnce(new Error("denied"));
    statusNow = ON();
    await show();
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(screen.getByRole("button", { name: "Copy the key" }).textContent).toBe("Could not copy");
    await act(async () => {
      vi.advanceTimersByTime(5000);
    });
    expect(screen.getByRole("button", { name: "Copy the key" }).textContent).toBe("Copy");
  });
});

describe("OtherApps: the context warning", () => {
  it("is not shown when the loaded model remembers 32,768 tokens or more, or no model is loaded", async () => {
    useApp.setState({ runtime: runtime({ nCtx: 32_768 }) });
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    expect(screen.queryByText(WARNING)).toBeNull();
    cleanup();
    statusNow = ON({ modelCtx: null });
    useApp.setState({ runtime: null });
    await show();
    expect(screen.queryByText(WARNING)).toBeNull();
    expect(ipcMock.modelLoadInfo).not.toHaveBeenCalled();
  });

  it("asks for 32,768 tokens with the fit mark, saves it for this model and restarts", async () => {
    useApp.setState({ runtime: runtime() });
    statusNow = ON({ modelCtx: 8192 });
    await show();
    expect(screen.getByText(WARNING)).toBeTruthy();
    expect(ipcMock.modelLoadInfo).toHaveBeenCalledWith("/models/a.gguf");
    const button = screen.getByRole("button", { name: "Use 32,768 tokens and restart · Fits with limits" });
    await click(button);
    expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/a.gguf", { ...OVERRIDES, context: { fixed: 32_768 } });
    // Nothing used the model, so it restarted at once (no question), without force. (WP2:
    // with no question the backend is told to wait, which acts at once when nothing uses it.)
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, true);
  });

  it("keeps the model's other settings as they are saved now, not as they were when the warning appeared", async () => {
    useApp.setState({ runtime: runtime() });
    statusNow = ON({ modelCtx: 8192 });
    await show();
    // Save memory was turned off in the model's panel after the warning appeared.
    const newer: ModelLoadSettings = { ...OVERRIDES, saveMemory: "off" };
    ipcMock.modelLoadInfo.mockResolvedValue(info({ overrides: newer }));
    await click(screen.getByRole("button", { name: /Use 32,768 tokens and restart/ }));
    expect(ipcMock.setModelLoadSettings).toHaveBeenCalledWith("/models/a.gguf", { ...newer, context: { fixed: 32_768 } });
  });

  it("asks first when another app is using the model (D3)", async () => {
    useApp.setState({ runtime: runtime() });
    useQuestion.setState({ question: null, hosts: 1 });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 0, otherApps: 1 });
    statusNow = ON({ modelCtx: 8192, inUse: true });
    await show();
    await click(screen.getByRole("button", { name: /Use 32,768 tokens and restart/ }));
    const q = useQuestion.getState().question;
    expect(q?.text).toBe("Another app is using the model. Stop its request and restart the model?");
    expect(ipcMock.reloadLocalModel).not.toHaveBeenCalled();
  });

  it("Restart when it ends: says what it waits for, offers Cancel, and leaves the rest of the page usable", async () => {
    useApp.setState({ runtime: runtime() });
    useQuestion.setState({ question: null, hosts: 1 });
    const otherApp: ModelBusy = { chatTurns: 0, modelChecks: 0, otherApps: 1 };
    ipcMock.modelBusy.mockResolvedValue(otherApp);
    const restarted = deferred<boolean>();
    ipcMock.reloadLocalModel.mockImplementationOnce(() => restarted.promise);
    statusNow = ON({ modelCtx: 8192, inUse: true });
    await show();
    await click(screen.getByRole("button", { name: /Use 32,768 tokens and restart/ }));
    const q = useQuestion.getState().question;
    if (!q) throw new Error("no question");
    await act(async () => answerQuestion(q.id, "wait"));
    // The backend sets reloadWaiting while it waits.
    await act(async () => useApp.setState({ runtime: runtime({ reloadWaiting: true }) }));
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, true);
    expect(screen.getByText("Restarts when the other app's request ends")).toBeTruthy();
    expect(screen.queryByText("Restarting the model…")).toBeNull();
    // Only the restart button waits; the switch, the port and the key still work.
    expect((screen.getByRole("button", { name: /Use 32,768 tokens and restart/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(theSwitch().disabled).toBe(false);
    expect((screen.getByRole("button", { name: "Make a new key" }) as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByRole("button", { name: "Show key" }) as HTMLButtonElement).disabled).toBe(false);
    // Cancel ends the wait (the backend answers the waiting call with "replaced").
    await click(screen.getByRole("button", { name: "Cancel the restart" }));
    expect(ipcMock.cancelPendingReload).toHaveBeenCalledTimes(1);
    await act(async () => {
      useApp.setState({ runtime: runtime({ reloadWaiting: false }) });
      restarted.resolve(false);
    });
    expect(screen.queryByText("Restarts when the other app's request ends")).toBeNull();
    expect((screen.getByRole("button", { name: /Use 32,768 tokens and restart/ }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("Stop and restart: says it restarts", async () => {
    useApp.setState({ runtime: runtime() });
    useQuestion.setState({ question: null, hosts: 1 });
    ipcMock.modelBusy.mockResolvedValue({ chatTurns: 0, modelChecks: 0, otherApps: 1 });
    const restarted = deferred<boolean>();
    ipcMock.reloadLocalModel.mockImplementationOnce(() => restarted.promise);
    statusNow = ON({ modelCtx: 8192, inUse: true });
    await show();
    await click(screen.getByRole("button", { name: /Use 32,768 tokens and restart/ }));
    const q = useQuestion.getState().question;
    if (!q) throw new Error("no question");
    await act(async () => answerQuestion(q.id, "now"));
    expect(ipcMock.reloadLocalModel).toHaveBeenCalledWith(false, false);
    expect(screen.getByRole("status").textContent).toBe("Restarting the model…");
    expect(screen.queryByRole("button", { name: "Cancel the restart" })).toBeNull();
    await act(async () => restarted.resolve(true));
    expect(screen.queryByText("Restarting the model…")).toBeNull();
  });

  it("says when 32,768 tokens do not fit on this computer, with no button to try", async () => {
    useApp.setState({ runtime: runtime() });
    ipcMock.modelLoadInfo.mockResolvedValue(
      info({
        contextOptions: [
          { choice: "auto", tokens: 8192, fit: "green" },
          { choice: { fixed: 8192 }, tokens: 8192, fit: "green" },
          { choice: { fixed: 32_768 }, tokens: 32_768, fit: "red" },
        ],
      }),
    );
    statusNow = ON({ modelCtx: 8192 });
    await show();
    expect(screen.getByText(WARNING)).toBeTruthy();
    expect(screen.getByText(TOO_MUCH)).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Use 32,768 tokens/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Model settings…" })).toBeTruthy();
  });

  it("opens the loaded model's settings over this page, and shows what was saved there", async () => {
    useApp.setState({ runtime: runtime() });
    ipcMock.modelLoadInfo.mockResolvedValue(
      info({
        contextOptions: [
          { choice: "auto", tokens: 8192, fit: "green" },
          { choice: { fixed: 32_768 }, tokens: 32_768, fit: "red" },
        ],
      }),
    );
    statusNow = ON({ modelCtx: 8192 });
    await show();
    expect(screen.getByText(TOO_MUCH)).toBeTruthy();
    await click(screen.getByRole("button", { name: "Model settings…" }));
    // Inside the page (so the settings window stays open under it), not the chat's panel.
    const opened = screen.getByRole("dialog", { name: "Settings for this model" });
    expect(within(opened).getByText("/models/a.gguf")).toBeTruthy();
    expect(panel.opened).not.toHaveBeenCalled();
    // Save memory turned on there: 32,768 tokens fit now, and the page says so at once.
    panel.saved = info({
      overrides: { ...OVERRIDES, saveMemory: "on" },
      contextOptions: [{ choice: { fixed: 32_768 }, tokens: 32_768, fit: "green" }],
    });
    await click(within(opened).getByRole("button", { name: "Save" }));
    expect(screen.queryByText(TOO_MUCH)).toBeNull();
    expect(screen.getByRole("button", { name: "Use 32,768 tokens and restart · Fits" })).toBeTruthy();
    await click(within(opened).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog", { name: "Settings for this model" })).toBeNull();
    expect(screen.getByText(WARNING)).toBeTruthy();
  });

  it("says when the model itself cannot remember that much", async () => {
    useApp.setState({ runtime: runtime() });
    ipcMock.modelLoadInfo.mockResolvedValue(
      info({ trainedCtx: 16_384, contextOptions: [{ choice: { fixed: 16_384 }, tokens: 16_384, fit: "green" }] }),
    );
    statusNow = ON({ modelCtx: 8192 });
    await show();
    expect(screen.getByText(WARNING)).toBeTruthy();
    expect(
      screen.getByText("This model can remember at most 16,384 tokens, about 12,000 words. Try a model that can remember more."),
    ).toBeTruthy();
    expect(screen.queryByRole("button", { name: /Use 32,768 tokens/ })).toBeNull();
  });
});

describe("OtherApps: setup cards", () => {
  it("shows a card only when asked, with Windows and Mac tabs and Copy buttons", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    expect(ipcMock.shareSetup).not.toHaveBeenCalled();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    expect(ipcMock.shareSetup).toHaveBeenCalledWith("claudeCode");
    const card = screen.getByRole("region", { name: "Set up Claude Code" });
    const windows = within(card).getByRole("tab", { name: "Windows" });
    const mac = within(card).getByRole("tab", { name: "Mac" });
    // jsdom is not a Mac, so Windows comes first.
    expect(windows.getAttribute("aria-selected")).toBe("true");
    expect(within(card).getByText("Open Windows PowerShell and paste these lines.")).toBeTruthy();
    expect(within(card).queryByText("Open Terminal and paste these lines.")).toBeNull();
    expect(within(card).getByText("%USERPROFILE%\\.claude\\settings.json")).toBeTruthy();
    await click(mac);
    expect(mac.getAttribute("aria-selected")).toBe("true");
    expect(within(card).getByText("Open Terminal and paste these lines.")).toBeTruthy();
    expect(within(card).queryByText("Open Windows PowerShell and paste these lines.")).toBeNull();
    expect(within(card).getByText("~/.claude/settings.json")).toBeTruthy();
    // Every card ends with its notes.
    expect(within(card).getByText("This stores the key as plain text in that app's settings.")).toBeTruthy();
    // Copy copies exactly the text of that step.
    await click(firstCopy(card));
    expect(browserClipboard).toHaveBeenCalledWith(`export A='${KEY}'\nclaude`);
    await click(within(card).getByRole("button", { name: "Copy the path of step 1 of For every window" }));
    expect(browserClipboard).toHaveBeenLastCalledWith("~/.claude/settings.json");
  });

  it("marks the Claude app as not tried yet and shows its steps on both systems", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude app" }));
    const card = screen.getByRole("region", { name: "Set up Claude app" });
    expect(within(card).getByText("Not tried yet")).toBeTruthy();
    // Only steps for any system: no tabs to choose from.
    expect(within(card).queryByRole("tab")).toBeNull();
    expect(within(card).getByText("Set Gateway base URL to:")).toBeTruthy();
  });

  it("shows the setup text as text, never as markup", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    ipcMock.shareSetup.mockResolvedValue(
      setup("codex", {
        ways: [{ title: "<b>x</b>", system: "windows", steps: [{ text: "<img src=x onerror=alert(1)>", code: "<script>", file: null }] }],
      }),
    );
    await show();
    await click(screen.getByRole("button", { name: "Codex" }));
    expect(screen.getByText("<img src=x onerror=alert(1)>")).toBeTruthy();
    expect(screen.getByText("<script>")).toBeTruthy();
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("b")).toBeNull();
  });

  it("says when the size changed after the setup was copied, until it is copied again (also after a restart of the page)", async () => {
    vi.useFakeTimers();
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(firstCopy());
    expect(screen.queryByText(SIZE_CHANGED)).toBeNull();
    statusNow = ON({ modelCtx: 16_384 });
    await poll();
    expect(screen.getByText(SIZE_CHANGED)).toBeTruthy();
    // The card was fetched again for the new size.
    expect(ipcMock.shareSetup).toHaveBeenCalledTimes(2);
    // Still said after the page is opened again.
    cleanup();
    await show();
    expect(screen.getByText(SIZE_CHANGED)).toBeTruthy();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(firstCopy());
    expect(screen.queryByText(SIZE_CHANGED)).toBeNull();
    expect(JSON.parse(window.localStorage.getItem(COPIED_KEY) ?? "null")).toEqual({
      claudeCode: { ctx: 16_384, port: 51234, model: null },
    });
  });

  it("assumes 32,768 tokens for a setup copied with no model loaded", async () => {
    vi.useFakeTimers();
    statusNow = ON({ modelCtx: null });
    await show();
    await click(screen.getByRole("button", { name: "Codex" }));
    await click(firstCopy());
    statusNow = ON({ modelCtx: 32_768 });
    await poll();
    expect(screen.queryByText(SIZE_CHANGED)).toBeNull();
    statusNow = ON({ modelCtx: 8192 });
    await poll();
    expect(screen.getByText(SIZE_CHANGED)).toBeTruthy();
  });

  it("says when the port changed after the setup was copied", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Codex" }));
    await click(firstCopy());
    await click(screen.getByRole("button", { name: "Choose another port" }));
    await click(screen.getByRole("button", { name: "Pick a free port" }));
    expect(screen.getByText("You changed the port. Copy the setup again.")).toBeTruthy();
    // The open card was fetched again with the new port.
    expect(ipcMock.shareSetup).toHaveBeenCalledTimes(2);
  });

  it("fetches the open card again after a new key, and says to copy it", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(ipcMock.shareSetup).toHaveBeenCalledTimes(2);
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    await click(firstCopy());
    expect(screen.queryByText(OLD_KEY)).toBeNull();
  });

  it("keeps the old-key note for each app set up before, until a step with the key is copied for that app", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    // Claude Code and Codex were set up with the old key.
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(firstCopy());
    await click(screen.getByRole("button", { name: "Codex" }));
    await click(firstCopy());
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    expect(screen.getByText("Apps to set up again: Claude Code and Codex.")).toBeTruthy();
    // A Codex step without the key in it does not set Codex up with the new key.
    const codex = screen.getByRole("region", { name: "Set up Codex" });
    await click(within(codex).getByRole("button", { name: "Copy step 1 of For every window" }));
    expect(browserClipboard).toHaveBeenLastCalledWith('{\n  "env": {}\n}');
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    expect(screen.getByText("Apps to set up again: Claude Code and Codex.")).toBeTruthy();
    // Codex's step with the key: Codex is done, Claude Code still has the old key.
    await click(firstCopy(codex));
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    expect(screen.getByText("Apps to set up again: Claude Code.")).toBeTruthy();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(firstCopy());
    expect(screen.queryByText(OLD_KEY)).toBeNull();
    expect(screen.queryByText(/Apps to set up again/)).toBeNull();
  });

  it("keeps the old-key note when a step without the key is copied, with no app set up before", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    // The Claude app's address has no key in it.
    await click(screen.getByRole("button", { name: "Claude app" }));
    await click(screen.getByRole("button", { name: "Copy step 2 of In the Claude app" }));
    expect(browserClipboard).toHaveBeenLastCalledWith("http://127.0.0.1:51234");
    expect(screen.getByText(OLD_KEY)).toBeTruthy();
    // The new key itself ends it.
    await click(screen.getByRole("button", { name: "Copy the key" }));
    expect(screen.queryByText(OLD_KEY)).toBeNull();
  });

  it("forgets a card (and the key in it) when its app is closed or sharing is switched off", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    expect(screen.getByRole("region", { name: "Set up Claude Code" })).toBeTruthy();
    // Closed: the next look fetches it again instead of showing a kept copy.
    let next: (s: ShareSetup) => void = () => undefined;
    ipcMock.shareSetup.mockImplementationOnce(() => new Promise((resolve) => (next = resolve)));
    await click(screen.getByRole("button", { name: "Claude Code" }));
    expect(screen.queryByRole("region", { name: "Set up Claude Code" })).toBeNull();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    expect(ipcMock.shareSetup).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("region", { name: "Set up Claude Code" })).toBeNull();
    expect(screen.getByText("Getting the setup…")).toBeTruthy();
    expect(screen.queryByText(/harnessui-share-/)).toBeNull();
    await act(async () => next(setup("claudeCode")));
    expect(screen.getByRole("region", { name: "Set up Claude Code" })).toBeTruthy();
    // Off and on again: no card and no key until asked for.
    await click(theSwitch());
    await click(theSwitch());
    expect(screen.queryByRole("region", { name: "Set up Claude Code" })).toBeNull();
    expect(screen.queryByText(/harnessui-share-/)).toBeNull();
    expect(screen.getByRole("button", { name: "Show key" })).toBeTruthy();
  });

  it("opens on the Mac tab on a Mac", async () => {
    await onAMac(async () => {
      statusNow = ON({ modelCtx: 32_768 });
      await show();
      await click(screen.getByRole("button", { name: "Claude Code" }));
      const card = screen.getByRole("region", { name: "Set up Claude Code" });
      expect(within(card).getByRole("tab", { name: "Mac" }).getAttribute("aria-selected")).toBe("true");
      expect(within(card).getByText("Open Terminal and paste these lines.")).toBeTruthy();
    });
  });

  it("shows the key in a card as dots until Show key, and Copy still copies it exactly", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    const card = screen.getByRole("region", { name: "Set up Claude Code" });
    expect(document.body.textContent).not.toContain(KEY);
    expect(within(card).getByText(`$env:A = '${HIDDEN_KEY}'`, { exact: false })).toBeTruthy();
    await click(firstCopy(card));
    expect(browserClipboard).toHaveBeenLastCalledWith(`$env:A = '${KEY}'\nclaude`);
    // Show key shows it in the card too; Hide key hides it everywhere again.
    await click(screen.getByRole("button", { name: "Show key" }));
    expect(within(card).getByText(`$env:A = '${KEY}'`, { exact: false })).toBeTruthy();
    await click(screen.getByRole("button", { name: "Hide key" }));
    expect(document.body.textContent).not.toContain(KEY);
  });

  it("never shows or copies a card made for an old key or port, and says why a new one could not be made", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    expect(screen.getByRole("region", { name: "Set up Claude Code" })).toBeTruthy();
    // A new key: while the card is made again, the old one (with the old key) is gone.
    const again = deferred<ShareSetup>();
    ipcMock.shareSetup.mockImplementationOnce(() => again.promise);
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(screen.queryByRole("region", { name: "Set up Claude Code" })).toBeNull();
    expect(screen.getByText("Getting the setup…")).toBeTruthy();
    // It fails: the page says why, and there is nothing to copy.
    await act(async () => again.reject(KEYCHAIN));
    expect(screen.getByRole("alert").textContent).toContain(KEYCHAIN.message);
    expect(screen.queryByRole("button", { name: /^Copy step/ })).toBeNull();
    // A new port: the card is made again for it.
    ipcMock.shareSetup.mockImplementation(async (app) => setup(app, { title: "Claude Code at the new port" }));
    await click(screen.getByRole("button", { name: "Choose another port" }));
    await click(screen.getByRole("button", { name: "Pick a free port" }));
    expect(screen.getByRole("region", { name: "Set up Claude Code at the new port" })).toBeTruthy();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("keeps the chosen Windows or Mac tab when the card is made again, and for the other apps", async () => {
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(within(screen.getByRole("region", { name: "Set up Claude Code" })).getByRole("tab", { name: "Mac" }));
    // A new key: the card is made again (with the new key) and still shows the Mac steps.
    await click(screen.getByRole("button", { name: "Make a new key" }));
    await click(within(screen.getByRole("group", { name: "Make a new key?" })).getByRole("button", { name: "Make a new key" }));
    expect(ipcMock.shareSetup).toHaveBeenCalledTimes(2);
    const again = screen.getByRole("region", { name: "Set up Claude Code" });
    expect(within(again).getByRole("tab", { name: "Mac" }).getAttribute("aria-selected")).toBe("true");
    expect(within(again).getByText("Open Terminal and paste these lines.")).toBeTruthy();
    // Codex opens on the Mac tab too.
    await click(screen.getByRole("button", { name: "Codex" }));
    const codex = screen.getByRole("region", { name: "Set up Codex" });
    expect(within(codex).getByRole("tab", { name: "Mac" }).getAttribute("aria-selected")).toBe("true");
  });

  it("remembers each app's copied setup on its own", async () => {
    vi.useFakeTimers();
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(firstCopy());
    await click(screen.getByRole("button", { name: "Claude app" }));
    await click(screen.getByRole("button", { name: "Copy step 2 of In the Claude app" }));
    statusNow = ON({ modelCtx: 16_384, port: 52000 });
    await poll();
    expect(screen.getByText(SIZE_CHANGED)).toBeTruthy();
    expect(screen.getByText(PORT_CHANGED)).toBeTruthy();
    expect(screen.getByText("Apps to set up again: Claude Code and Claude app.")).toBeTruthy();
    // Copying Codex (never copied before) changes nothing for the other two.
    await click(screen.getByRole("button", { name: "Codex" }));
    await click(firstCopy());
    expect(screen.getByText(SIZE_CHANGED)).toBeTruthy();
    expect(screen.getByText("Apps to set up again: Claude Code and Claude app.")).toBeTruthy();
    // Claude Code copied again: only the Claude app (port) is left; it has no size.
    await click(screen.getByRole("button", { name: "Claude Code" }));
    await click(firstCopy());
    expect(screen.queryByText(SIZE_CHANGED)).toBeNull();
    expect(screen.getByText(PORT_CHANGED)).toBeTruthy();
    expect(screen.getByText("Apps to set up again: Claude app.")).toBeTruthy();
    expect(JSON.parse(window.localStorage.getItem(COPIED_KEY) ?? "null")).toEqual({
      claudeCode: { ctx: 16_384, port: 52000, model: null },
      codex: { ctx: 16_384, port: 52000, model: null },
      claudeApp: { ctx: null, port: 51234, model: null },
    });
  });

  it("says a different model is loaded, rather than that its size was changed", async () => {
    vi.useFakeTimers();
    useApp.setState({ runtime: runtime({ modelPath: "/models/a.gguf", nCtx: 32_768 }) });
    statusNow = ON({ modelCtx: 32_768 });
    await show();
    await click(screen.getByRole("button", { name: "Codex" }));
    await click(firstCopy());
    // The same model, now at 16,384 tokens: the size was changed.
    statusNow = ON({ modelCtx: 16_384 });
    await poll();
    expect(screen.getByText(SIZE_CHANGED)).toBeTruthy();
    // Another model, with another size: it says so.
    await act(async () => useApp.setState({ runtime: runtime({ modelPath: "/models/b.gguf", nCtx: 16_384 }) }));
    expect(screen.queryByText(SIZE_CHANGED)).toBeNull();
    expect(screen.getByText(MODEL_CHANGED)).toBeTruthy();
    expect(screen.getByText("Apps to set up again: Codex.")).toBeTruthy();
  });

  it("shows why a card could not be made", async () => {
    statusNow = ON();
    ipcMock.shareSetup.mockRejectedValue({
      code: "bad_request",
      message: "Switch sharing on first, so the setup has a port and a key.",
      nextStep: "",
      details: "",
    });
    await show();
    await click(screen.getByRole("button", { name: "Codex" }));
    expect(screen.getByRole("alert").textContent).toContain("Switch sharing on first");
  });
});
