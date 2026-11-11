import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("../lib/ipc", () => ({ minimizeMainWindow: vi.fn(async () => undefined) }));

const { ToolCallCard } = await import("./ToolCallCard");
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
