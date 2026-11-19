import { useState } from "react";
import { AlertDialog } from "radix-ui";
import type { ErrorInfo } from "../../lib/types";
import { LOCAL_NOTICE } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";

// The note about local models (0.1.0 Task B14, local addendum L1), word for word: once before
// the first local download, where *I understand* records it with the backend (which refuses
// local downloads until then) and *Cancel* downloads nothing; and read only, with *Close*,
// from Get local models' footer and Settings → Studio.

/** The note; `onAccepted` follows *I understand* once the backend recorded it. Cancel, Close
 *  or Esc call `onClose`. */
export function LocalNotice(props: { open: boolean; readOnly?: boolean; onAccepted(): void; onClose(): void }) {
  const { open, onClose } = props;
  return (
    <AlertDialog.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[30rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 text-sm shadow-pop">
          <Notice readOnly={props.readOnly ?? false} onAccepted={props.onAccepted} />
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

/** The words and the buttons; what *I understand* is waiting for, or why it failed, goes
 *  with the dialog when it closes. */
function Notice(props: { readOnly: boolean; onAccepted(): void }) {
  const acceptNotice = useStudio((s) => s.acceptNotice);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  function accept() {
    setBusy(true);
    setError(null);
    acceptNotice().then(
      () => {
        setBusy(false);
        props.onAccepted();
      },
      (e: unknown) => {
        setBusy(false);
        setError(toErrorInfo(e));
      },
    );
  }

  return (
    <>
      {/* One paragraph, its first sentence in bold, as the notice is written. */}
      <p className="leading-relaxed">
        <AlertDialog.Title asChild>
          <strong className="font-semibold">{LOCAL_NOTICE.lead}</strong>
        </AlertDialog.Title>{" "}
        <AlertDialog.Description asChild>
          <span>{LOCAL_NOTICE.body}</span>
        </AlertDialog.Description>
      </p>
      {error && (
        <div className="mt-3">
          <ErrorBanner error={error} onDismiss={() => setError(null)} />
        </div>
      )}
      <div className="mt-4 flex justify-end gap-2">
        {props.readOnly ? (
          <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
            {LOCAL_NOTICE.close}
          </AlertDialog.Cancel>
        ) : (
          <>
            <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
              {LOCAL_NOTICE.cancel}
            </AlertDialog.Cancel>
            <button
              type="button"
              disabled={busy}
              onClick={accept}
              className="btn-primary rounded-md px-3 py-1.5 font-medium disabled:opacity-60"
            >
              {LOCAL_NOTICE.understand}
            </button>
          </>
        )}
      </div>
    </>
  );
}
