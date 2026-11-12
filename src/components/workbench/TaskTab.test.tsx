import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { Task, TaskEdit, TaskStatus } from "../task/types";

const apiMock = vi.hoisted(() => ({
  getTask: vi.fn<(chatId: number) => Promise<Task | null>>(),
  editTask: vi.fn<(chatId: number, edit: TaskEdit) => Promise<Task | null>>(),
  setTaskStatus: vi.fn<(chatId: number, status: TaskStatus) => Promise<Task | null>>(),
  clearTask: vi.fn<(chatId: number) => Promise<void>>(),
  onTaskChanged: vi.fn(async () => () => undefined),
}));
vi.mock("../task/api", () => apiMock);

const { TaskTab, RESUME_MESSAGE } = await import("./TaskTab");
const { resetTasks } = await import("../task/state");
const { useApp } = await import("../../store/app");

function task(over: Partial<Task> = {}): Task {
  return {
    chatId: 7,
    status: "active",
    updatedAt: Date.UTC(2026, 10, 12, 9, 30),
    goal: "Put a dark mode switch in the header",
    goalByUser: false,
    constraints: [
      { text: "No new dependencies", byUser: true },
      { text: "Keep the tests green", byUser: false },
    ],
    steps: [
      { text: "Read the header", status: "done", byUser: false },
      { text: "Add the switch", status: "doing", byUser: false },
      { text: "Run the tests", status: "todo", byUser: true },
    ],
    changedFiles: [
      { path: "/work/src/Switch.tsx", shown: "/work/src/Switch.tsx", kind: "created", at: 3 },
      { path: "/work/src/evil‮txt.exe", shown: "/work/src/evil\\u{202E}txt.exe", kind: "edited", at: 2 },
    ],
    filesNotListed: 0,
    checks: [
      { command: "npm test", passed: false, finished: true, at: 5, summary: "1 failed, 11 passed", exitCode: 1 },
      { command: "npm run lint", passed: true, finished: true, at: 4, summary: "", exitCode: 0 },
      { command: "cargo build", passed: false, finished: false, at: 3, summary: "Timed out", exitCode: null },
    ],
    nextAction: "Add the switch to the header",
    nextActionByUser: false,
    notes: [],
    ...over,
  };
}

async function show(t: Task | null) {
  apiMock.getTask.mockResolvedValue(t);
  let result!: ReturnType<typeof render>;
  await act(async () => {
    result = render(<TaskTab chatId={7} />);
  });
  return result;
}

const stop = vi.fn(async () => undefined);
const send = vi.fn(async () => undefined);

describe("TaskTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetTasks();
    useApp.setState({ activeChatId: 7, generation: null, stop, send });
  });
  afterEach(() => {
    cleanup();
    useApp.setState({ activeChatId: null, generation: null });
  });

  it("says what will appear when the chat has no task, and lets the user write a goal", async () => {
    await show(null);
    expect(screen.getByText(/No task in this chat yet/)).toBeTruthy();
    apiMock.editTask.mockResolvedValue(task({ goal: "Tidy the notes", goalByUser: true, steps: [], constraints: [] }));
    fireEvent.click(screen.getByRole("button", { name: "Write a goal" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Goal" }), { target: { value: "Tidy the notes" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    });
    expect(apiMock.editTask).toHaveBeenCalledWith(7, { goal: "Tidy the notes" });
    expect(screen.getByText("Tidy the notes")).toBeTruthy();
    expect(screen.getByText("written by you")).toBeTruthy();
  });

  it("shows the goal, constraints, steps, files, checks, next action and status", async () => {
    await show(task());
    expect(screen.getByLabelText("Status: active")).toBeTruthy();
    expect(screen.getByText("Put a dark mode switch in the header")).toBeTruthy();
    const constraints = screen.getByRole("region", { name: "Constraints" });
    expect(within(constraints).getAllByText("written by you")).toHaveLength(1);
    const steps = screen.getAllByRole("listitem").filter((li) => li.dataset.stepStatus);
    expect(steps.map((li) => li.dataset.stepStatus)).toEqual(["done", "doing", "todo"]);
    // The path shown hides nothing; the one copied is the real one.
    expect(screen.getByText("/work/src/evil\\u{202E}txt.exe")).toBeTruthy();
    const checks = screen.getByRole("list", { name: "Checks" });
    expect(within(checks).getByText("failed (exit code 1)")).toBeTruthy();
    expect(within(checks).getByText("passed")).toBeTruthy();
    expect(within(checks).getByText("did not finish")).toBeTruthy();
    expect(within(checks).getByText("1 failed, 11 passed")).toBeTruthy();
    expect(screen.getByText("Add the switch to the header")).toBeTruthy();
    expect(screen.getByText(/Only changes made with the file tools are listed/)).toBeTruthy();
    expect(screen.getByText(/^Last updated/)).toBeTruthy();
  });

  it("copies a changed file's path when it is clicked", async () => {
    const writeText = vi.fn(async () => undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    await show(task());
    await act(async () => {
      fireEvent.click(screen.getByText("/work/src/Switch.tsx"));
    });
    expect(writeText).toHaveBeenCalledWith("/work/src/Switch.tsx");
    expect(screen.getByText("copied")).toBeTruthy();
  });

  it("pauses the task and stops a reply that runs in this chat", async () => {
    useApp.setState({ generation: { chatId: 7 } as never });
    await show(task());
    apiMock.setTaskStatus.mockResolvedValue(task({ status: "paused" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    });
    expect(apiMock.setTaskStatus).toHaveBeenCalledWith(7, "paused");
    expect(stop).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText("Status: paused")).toBeTruthy();
    expect(send).not.toHaveBeenCalled();
  });

  it("does not stop a reply that runs in another chat", async () => {
    useApp.setState({ generation: { chatId: 99 } as never });
    await show(task());
    apiMock.setTaskStatus.mockResolvedValue(task({ status: "paused" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    });
    expect(stop).not.toHaveBeenCalled();
  });

  it("resumes with an ordinary message, and not while a reply runs", async () => {
    await show(task({ status: "paused" }));
    apiMock.setTaskStatus.mockResolvedValue(task({ status: "active" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    });
    expect(apiMock.setTaskStatus).toHaveBeenCalledWith(7, "active");
    expect(send).toHaveBeenCalledWith(RESUME_MESSAGE);
    expect(RESUME_MESSAGE).toBe("Continue the task. Start with the next action.");

    cleanup();
    resetTasks();
    useApp.setState({ generation: { chatId: 99 } as never });
    await show(task({ status: "paused" }));
    expect((screen.getByRole("button", { name: "Resume" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("marks the task done, and offers to reopen a done task", async () => {
    await show(task());
    apiMock.setTaskStatus.mockResolvedValue(task({ status: "done" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Mark done" }));
    });
    expect(apiMock.setTaskStatus).toHaveBeenCalledWith(7, "done");
    expect(screen.getByLabelText("Status: done")).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Pause" })).toBeNull();
    expect(screen.getByRole("button", { name: "Reopen" })).toBeTruthy();
  });

  it("clears the task only after the user confirms", async () => {
    await show(task());
    apiMock.clearTask.mockResolvedValue(undefined);
    fireEvent.click(screen.getByRole("button", { name: "Clear task" }));
    expect(screen.getByRole("alertdialog")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Keep it" }));
    expect(apiMock.clearTask).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Clear task" }));
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove the task" }));
    });
    expect(apiMock.clearTask).toHaveBeenCalledWith(7);
    expect(screen.getByText(/No task in this chat yet/)).toBeTruthy();
  });

  it("saves the user's corrections to the steps and the next action", async () => {
    await show(task());
    apiMock.editTask.mockResolvedValue(task());
    // A click on a step's mark moves it on.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Step 3 is to do. Change it." }));
    });
    expect(apiMock.editTask).toHaveBeenLastCalledWith(7, {
      steps: [
        { text: "Read the header", status: "done" },
        { text: "Add the switch", status: "doing" },
        { text: "Run the tests", status: "doing" },
      ],
    });
    fireEvent.click(screen.getByRole("button", { name: "Edit the next action" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Next action" }), { target: { value: "Ask about the footer" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    });
    expect(apiMock.editTask).toHaveBeenLastCalledWith(7, { nextAction: "Ask about the footer" });
    fireEvent.click(screen.getByRole("button", { name: "Edit the constraints" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Constraints, one per line" }), {
      target: { value: "No new dependencies\n\n  Use tabs  " },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    });
    expect(apiMock.editTask).toHaveBeenLastCalledWith(7, { constraints: ["No new dependencies", "Use tabs"] });
  });

  it("offers to clear a task that cannot be read", async () => {
    apiMock.getTask.mockRejectedValue({
      code: "task_unreadable",
      message: "The saved task of this chat could not be read.",
    });
    await act(async () => {
      render(<TaskTab chatId={7} />);
    });
    expect(screen.getByRole("alert").textContent).toContain("could not be read");
    expect(screen.getByRole("button", { name: "Clear task" })).toBeTruthy();
  });
});
