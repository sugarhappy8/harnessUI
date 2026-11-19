import type { ReactNode } from "react";
import { AlertDialog } from "radix-ui";

// Studio's question before a step that cannot be undone (deleting results, removing the
// Studio engine, deleting a local model): a title, what the step does, a quieter note when there
// is one, then Cancel and the action. Cancel, Esc and the action itself close it. A question
// that shows more than this (ConfirmRun's estimate, the quit question's failure line) keeps its
// own markup.

/** The question: `title`, then `body`; with a `note`, `body` reads plain and the note quieter
 *  beneath it, else `body` reads quieter alone. `onConfirm` follows the action; the dialog then
 *  closes (`onOpenChange(false)`), as it does on Cancel or Esc. `destructive` colours the action
 *  as a deletion. */
export function ConfirmDialog(props: {
  open: boolean;
  onOpenChange(open: boolean): void;
  title: ReactNode;
  body: ReactNode;
  note?: ReactNode;
  cancelLabel: string;
  actionLabel: string;
  destructive?: boolean;
  onConfirm(): void;
}) {
  const { note } = props;
  return (
    <AlertDialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[26rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 text-sm shadow-pop">
          <AlertDialog.Title className="text-base font-semibold">{props.title}</AlertDialog.Title>
          {note === undefined ? (
            <AlertDialog.Description className="mt-2 text-fg-muted">{props.body}</AlertDialog.Description>
          ) : (
            <AlertDialog.Description asChild>
              <div className="mt-2 flex flex-col gap-1.5">
                <p>{props.body}</p>
                <p className="text-fg-muted">{note}</p>
              </div>
            </AlertDialog.Description>
          )}
          <div className="mt-4 flex justify-end gap-2">
            <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
              {props.cancelLabel}
            </AlertDialog.Cancel>
            <AlertDialog.Action
              onClick={props.onConfirm}
              className={
                props.destructive
                  ? "rounded-md bg-danger px-3 py-1.5 font-medium text-white hover:opacity-90"
                  : "btn-primary rounded-md px-3 py-1.5 font-medium"
              }
            >
              {props.actionLabel}
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}
