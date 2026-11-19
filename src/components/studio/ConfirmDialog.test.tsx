// Studio's shared question before a step that cannot be undone: the title, what the step does
// (with a quieter note when there is one), Cancel and the action. Its callers (Settings → Studio,
// the engine line, deleting results, deleting a local model) test their own words.
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { ConfirmDialog } from "./ConfirmDialog";
import { alertDialogsAnimate, endAnimation } from "./fixtures/dialogs";

afterEach(() => cleanup());

/** The dialog opened by a button, as its callers use it. */
function Asking(props: { note?: string; destructive?: boolean; onConfirm(): void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button type="button" onClick={() => setOpen(true)}>
        Ask
      </button>
      <ConfirmDialog
        open={open}
        onOpenChange={setOpen}
        title="Delete it?"
        body="It goes."
        note={props.note}
        cancelLabel="Cancel"
        actionLabel="Delete"
        destructive={props.destructive}
        onConfirm={props.onConfirm}
      />
    </>
  );
}

async function click(el: HTMLElement) {
  await act(async () => {
    fireEvent.click(el);
  });
}

describe("ConfirmDialog", () => {
  it("asks, and the action confirms and closes it", async () => {
    const onConfirm = vi.fn();
    render(<Asking destructive onConfirm={onConfirm} />);
    await click(screen.getByRole("button", { name: "Ask" }));
    const dialog = screen.getByRole("alertdialog", { name: "Delete it?" });
    expect(dialog.getAttribute("aria-describedby")).toBeTruthy();
    within(dialog).getByText("It goes.");
    const action = within(dialog).getByRole("button", { name: "Delete" });
    expect(action.className).toContain("bg-danger");
    await click(action);
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("while it closes, its action does nothing more", async () => {
    // In the app the closing dialog stays on screen for its 130 ms animation, buttons and all;
    // a double-click's second click lands on it.
    alertDialogsAnimate();
    const onConfirm = vi.fn();
    render(<Asking destructive onConfirm={onConfirm} />);
    await click(screen.getByRole("button", { name: "Ask" }));
    const dialog = screen.getByRole("alertdialog");
    const action = within(dialog).getByRole("button", { name: "Delete" });
    await click(action);
    expect(onConfirm).toHaveBeenCalledTimes(1);
    expect(dialog.getAttribute("data-state")).toBe("closed");
    expect(dialog.isConnected).toBe(true);
    await click(action);
    expect(onConfirm).toHaveBeenCalledTimes(1);
    await endAnimation(dialog);
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("Cancel closes it without confirming", async () => {
    const onConfirm = vi.fn();
    render(<Asking onConfirm={onConfirm} />);
    await click(screen.getByRole("button", { name: "Ask" }));
    await click(within(screen.getByRole("alertdialog")).getByRole("button", { name: "Cancel" }));
    expect(onConfirm).not.toHaveBeenCalled();
    expect(screen.queryByRole("alertdialog")).toBeNull();
  });

  it("a note reads beneath the body, and an action that deletes nothing is not red", async () => {
    render(<Asking note="The audit log keeps a line." onConfirm={() => {}} />);
    await click(screen.getByRole("button", { name: "Ask" }));
    const dialog = screen.getByRole("alertdialog");
    const body = within(dialog).getByText("It goes.");
    const note = within(dialog).getByText("The audit log keeps a line.");
    expect(body.compareDocumentPosition(note) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(note.className).toContain("text-fg-muted");
    expect(within(dialog).getByRole("button", { name: "Delete" }).className).not.toContain("bg-danger");
  });
});
