import { useState } from "react";
import { Dialog } from "radix-ui";
import { X } from "lucide-react";

export function SystemPromptDialog(props: {
  open: boolean;
  initial: string;
  onOpenChange: (open: boolean) => void;
  onSave: (text: string | null) => void;
}) {
  const [text, setText] = useState(props.initial);
  return (
    <Dialog.Root
      open={props.open}
      onOpenChange={(o) => {
        if (o) setText(props.initial);
        props.onOpenChange(o);
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 w-[40rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-lg border border-border bg-bg p-5 shadow-xl">
          <div className="mb-3 flex items-center justify-between">
            <Dialog.Title className="font-semibold">System prompt for this chat</Dialog.Title>
            <Dialog.Close aria-label="Close" className="text-fg-muted hover:text-fg">
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <Dialog.Description className="mb-2 text-sm text-fg-muted">
            Instructions the model sees before every message in this chat. Leave empty for the
            default from Settings → General.
          </Dialog.Description>
          <textarea
            aria-label="System prompt"
            value={text}
            onChange={(e) => setText(e.target.value)}
            rows={12}
            className="w-full rounded-md border border-border bg-bg-subtle p-3 font-mono text-[13px]"
          />
          <div className="mt-3 flex justify-end gap-2">
            <Dialog.Close className="rounded-md border border-border px-3 py-1.5 text-sm">Cancel</Dialog.Close>
            <button
              type="button"
              className="rounded-md bg-accent px-3 py-1.5 text-sm text-accent-fg"
              onClick={() => {
                props.onSave(text.trim() ? text : null);
                props.onOpenChange(false);
              }}
            >
              Save
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
