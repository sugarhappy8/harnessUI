import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

const ipcMock = vi.hoisted(() => ({
  minimizeMainWindow: vi.fn(async () => undefined),
  isTauri: vi.fn(() => false),
  getToolOutput: vi.fn<(chatId: number, id: number) => Promise<import("../lib/ipc").SavedToolOutput>>(),
}));
vi.mock("../lib/ipc", () => ipcMock);
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn(async () => undefined) }));

const { ToolCallCard, savedOutputId } = await import("./ToolCallCard");
const { useApp } = await import("../store/app");
type View = import("./ToolCallCard").ToolCallView;

function call(over: Partial<View> = {}): View {
  return {
    callId: "c1",
    name: "run_subagent",
    arguments: { task: "Read notes.txt and report the secret word." },
    recovered: false,
    status: "running",
    text: "",
    progress: "",
    ...over,
  };
}

describe("ToolCallCard: a helper's card", () => {
  afterEach(cleanup);

  it("shows the task in its header", () => {
    render(<ToolCallCard call={call()} />);
    expect(screen.getByText("Read notes.txt and report the secret word.")).toBeTruthy();
  });

  it("shows what the helper is doing right now without being opened", () => {
    const progress = "read_file notes.txt — done\nlist_dir . — done\nthe secret word is";
    const { rerender } = render(<ToolCallCard call={call({ progress })} />);
    expect(screen.getByText("the secret word is")).toBeTruthy();
    expect(screen.queryByText(/read_file notes.txt/)).toBeNull();
    // A line that just ended is still the last thing that happened.
    rerender(<ToolCallCard call={call({ progress: "read_file notes.txt — done\n" })} />);
    expect(screen.getByText("read_file notes.txt — done")).toBeTruthy();
    // Waiting for an approval card: still shown.
    rerender(<ToolCallCard call={call({ status: "waiting", progress: "write_file a.txt" })} />);
    expect(screen.getByText("write_file a.txt")).toBeTruthy();
  });

  it("opened, shows every line so far; finished, shows the answer instead", () => {
    const progress = "read_file notes.txt — done\nthe secret word is heron\n";
    const { rerender } = render(<ToolCallCard call={call({ progress })} />);
    fireEvent.click(screen.getByRole("button", { expanded: false }));
    expect(screen.getByText("Output so far")).toBeTruthy();
    expect(document.querySelectorAll("pre")[1]?.textContent).toBe(progress);
    rerender(<ToolCallCard call={call({ status: "ok", text: "the secret word is heron", progress })} />);
    expect(screen.getByText("Result")).toBeTruthy();
    expect(document.querySelectorAll("pre")[1]?.textContent).toBe("the secret word is heron");
  });

  it("shows no live line once the helper is done, and none for other tools", () => {
    const progress = "read_file notes.txt — done\n";
    const { rerender } = render(<ToolCallCard call={call({ status: "ok", text: "heron", progress })} />);
    expect(screen.queryByText("read_file notes.txt — done")).toBeNull();
    rerender(<ToolCallCard call={call({ name: "run_command", arguments: { command: "ls" }, progress: "a.txt\n" })} />);
    expect(screen.queryByText("a.txt")).toBeNull();
  });
});

const SHORTENED =
  '[Shortened: 10 of 100 lines shown. Full output is saved as output #42. Read more with read_output {"id": 42, "lines": "11-30"} or {"id": 42, "grep": "text"}.]';

function finished(over: Partial<View> = {}): View {
  return call({
    name: "run_command",
    arguments: { command: "npm test" },
    status: "error",
    text: `Exit code 1 (2.0 s)\nline 1\n[... 90 lines not shown ...]\n${SHORTENED}`,
    ...over,
  });
}

describe("ToolCallCard: a shortened result", () => {
  afterEach(() => {
    cleanup();
    ipcMock.getToolOutput.mockReset();
  });

  it("finds the saved output only on the last line, in the app's exact form", () => {
    expect(savedOutputId(`text\n${SHORTENED}`)).toBe(42);
    expect(savedOutputId(`text\n${SHORTENED}\n`)).toBe(42);
    expect(savedOutputId(`${SHORTENED}\nmore text`)).toBeNull();
    expect(savedOutputId("[Shortened: output #42]")).toBeNull();
    expect(savedOutputId("Exit code 0 (0.1 s)")).toBeNull();
  });

  it("shows Full output, opens the saved text of this chat in the card, and copies it", async () => {
    useApp.setState({ activeChatId: 7 });
    const text = Array.from({ length: 100 }, (_, i) => `line ${i + 1}`).join("\n");
    ipcMock.getToolOutput.mockResolvedValue({ id: 42, tool: "run_command", lines: 100, bytes: text.length, text });
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    render(<ToolCallCard call={finished()} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Full output" }));
    });
    expect(ipcMock.getToolOutput).toHaveBeenCalledWith(7, 42);
    expect(screen.getByLabelText("Output #42").textContent).toBe(text);
    expect(screen.getByText("Output #42 · 100 lines")).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Copy output" }));
    });
    expect(writeText).toHaveBeenCalledWith(text);
    fireEvent.click(screen.getByRole("button", { name: "Hide" }));
    expect(screen.queryByLabelText("Output #42")).toBeNull();
    expect(screen.getByRole("button", { name: "Full output" })).toBeTruthy();
  });

  it("says so when the saved output cannot be opened", async () => {
    useApp.setState({ activeChatId: 7 });
    ipcMock.getToolOutput.mockRejectedValue({ message: "saved output not found" });
    render(<ToolCallCard call={finished()} />);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Full output" }));
    });
    expect(
      screen.getByText("The full output could not be opened. A chat keeps only its 200 newest saved outputs."),
    ).toBeTruthy();
  });

  it("shows no button for a result that was not shortened, or while the call runs", () => {
    useApp.setState({ activeChatId: 7 });
    const { rerender } = render(<ToolCallCard call={finished({ text: "Exit code 0 (0.1 s)\nok" })} />);
    expect(screen.queryByRole("button", { name: "Full output" })).toBeNull();
    rerender(<ToolCallCard call={finished({ status: "running" })} />);
    expect(screen.queryByRole("button", { name: "Full output" })).toBeNull();
  });
});
