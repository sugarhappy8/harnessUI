import { useState } from "react";
import { Check, Pin } from "lucide-react";
import { toErrorInfo, useApp } from "../../store/app";
import { pinMessage } from "./api";

/** The "Pin" action on one of the user's own messages: pins its text for the open chat.
 *  Only the message's id goes to the app, which reads the text from the database and
 *  refuses anything that is not the user's own message in this chat. */
export function PinMessageButton(props: { messageId: number }) {
  const [done, setDone] = useState(false);
  const [busy, setBusy] = useState(false);
  async function pin() {
    const chatId = useApp.getState().activeChatId;
    if (chatId === null) return;
    setBusy(true);
    try {
      await pinMessage(chatId, props.messageId);
      setDone(true);
      setTimeout(() => setDone(false), 1500);
      useApp.getState().openWorkbench("context");
    } catch (err) {
      useApp.getState().showError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }
  const label = done ? "Pinned for this chat" : "Pin for this chat";
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      disabled={busy}
      onClick={() => void pin()}
      className="text-fg-muted hover:text-fg disabled:opacity-30"
      data-pin-message={props.messageId}
    >
      {done ? <Check size={13} aria-hidden className="text-accent" /> : <Pin size={13} aria-hidden />}
    </button>
  );
}
