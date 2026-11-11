import { useEffect, useState } from "react";
import { Dialog } from "radix-ui";
import { X } from "lucide-react";
import * as ipc from "../lib/ipc";

const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform);
const MOD = isMac ? "⌘" : "Ctrl";

const ROWS: [string, string][] = [
  [`${MOD} N`, "New chat"],
  [`${MOD} K`, "Search chats"],
  [`${MOD} L`, "Focus the message box"],
  [`${MOD} M`, "Models"],
  [`${MOD} B`, "Show or hide the chat list"],
  [`${MOD} ,`, "Settings"],
  [`${MOD} /`, "This list"],
  ["Enter", "Send (Shift+Enter: new line)"],
  ["Esc", "Stop the reply and any running tool"],
];

/** The keyboard shortcut sheet. */
export function ShortcutsDialog(props: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [hotkey, setHotkey] = useState<string | null>(null);
  useEffect(() => {
    if (props.open && ipc.isTauri())
      void ipc
        .stopHotkeyStatus()
        .then((s) => setHotkey(s.registered ? s.accelerator : null))
        .catch(() => setHotkey(null));
  }, [props.open]);
  return (
    <Dialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay fixed inset-0" />
        <Dialog.Content className="fixed top-1/2 left-1/2 w-[28rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 shadow-pop">
          <div className="mb-3 flex items-center justify-between">
            <Dialog.Title className="font-semibold">Keyboard shortcuts</Dialog.Title>
            <Dialog.Close aria-label="Close" className="text-fg-muted hover:text-fg">
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">Keyboard shortcuts in this app</Dialog.Description>
          <table className="w-full text-sm">
            <tbody>
              {ROWS.map(([k, what]) => (
                <tr key={k} className="border-b border-border last:border-0">
                  <td className="py-1.5 pr-4 font-mono text-xs whitespace-nowrap">{k}</td>
                  <td className="py-1.5">{what}</td>
                </tr>
              ))}
              <tr>
                <td className="py-1.5 pr-4 font-mono text-xs whitespace-nowrap">
                  {hotkey ? hotkey.replace(/\+/g, " ") : "not set"}
                </td>
                <td className="py-1.5">Stop from anywhere, even when another app is in front (change it in Settings)</td>
              </tr>
            </tbody>
          </table>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
