import { AlertDialog } from "radix-ui";

/** Asks before the chat's task is removed. */
export function ClearTaskDialog(props: { open: boolean; onOpenChange: (open: boolean) => void; onConfirm: () => void }) {
  return (
    <AlertDialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[26rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 text-sm shadow-pop">
          <AlertDialog.Title className="text-base font-semibold">Clear the task?</AlertDialog.Title>
          <AlertDialog.Description className="mt-2 text-fg-muted">
            The goal, constraints, steps, changed files, checks and next action of this chat are removed. The chat and
            its files stay as they are.
          </AlertDialog.Description>
          <div className="mt-4 flex justify-end gap-2">
            <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
              Keep it
            </AlertDialog.Cancel>
            <AlertDialog.Action
              onClick={props.onConfirm}
              className="rounded-md border border-danger/60 bg-danger/10 px-3 py-1.5 font-medium text-danger hover:bg-danger/20"
            >
              Remove the task
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
