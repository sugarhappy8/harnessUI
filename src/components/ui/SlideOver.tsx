import { useRef, type ReactNode, type RefObject } from "react";
import { Dialog } from "radix-ui";
import { X } from "lucide-react";
import { UI } from "../../lib/words";
import { FOCUS_RING } from "./styles";

/** A panel that slides in from the right. Modal by default: focus stays inside, Esc closes it
 *  and focus returns to what opened it. `modal: false` leaves the page behind it usable. Focus
 *  goes to `initialFocus` as it opens, when given (a search box), else to its first control. */
export function SlideOver(props: {
  open: boolean;
  onOpenChange(open: boolean): void;
  title: string;
  width: "panel" | "half";
  modal?: boolean;
  initialFocus?: RefObject<HTMLElement | null>;
  children: ReactNode;
}) {
  const modal = props.modal ?? true;
  // The panel has no Dialog.Trigger (its owner opens it), so Radix has nothing to give focus
  // back to: remember what had it as the panel opens, and return it there as it closes.
  const opener = useRef<Element | null>(null);
  return (
    <Dialog.Root open={props.open} onOpenChange={props.onOpenChange} modal={modal}>
      <Dialog.Portal>
        {modal && <Dialog.Overlay className="fixed inset-0 z-40 bg-fg/30" />}
        <Dialog.Content
          aria-describedby={undefined}
          onOpenAutoFocus={(e) => {
            opener.current = document.activeElement;
            const first = props.initialFocus?.current;
            if (first) {
              e.preventDefault();
              first.focus();
            }
          }}
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            if (opener.current instanceof HTMLElement) opener.current.focus();
          }}
          onInteractOutside={modal ? undefined : (e) => e.preventDefault()}
          className={`fixed inset-y-0 right-0 z-50 flex max-w-full flex-col border-l border-border bg-surface shadow-pop ${
            props.width === "half" ? "w-1/2 min-w-[28rem]" : "w-[26rem]"
          } ${FOCUS_RING}`}
        >
          <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
            <Dialog.Title className="text-title">{props.title}</Dialog.Title>
            <Dialog.Close
              aria-label={UI.close}
              className={`rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
            >
              <X className="size-4" aria-hidden />
            </Dialog.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto p-4">{props.children}</div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
