import { useEffect, useRef, useState } from "react";
import { ShieldAlert, ShieldQuestion } from "lucide-react";
import type { ApprovalRequest, KeepAsking } from "../lib/types";
import { useAllowAll, useApprovalMode } from "../lib/approvalMode";
import { toErrorInfo, useApp } from "../store/app";
import { AllowAllConfirm } from "./AllowAllButton";

/** Clicks and keys on a card are ignored for this long after it appears (13.3). */
export const CARD_INPUT_DELAY_MS = 700;

const REASON_TEXT: Record<ApprovalRequest["reason"], string | null> = {
  normal: null,
  alwaysAsk: "This kind of action always asks, even if you allowed similar ones before.",
  recoveredFromText:
    "The model wrote this call as text instead of a real tool call, so it was recovered from its answer. Check it carefully.",
  untrustedContentThenOutbound:
    "This chat has read content from files, pages or tools in this turn, and this action sends data out. Make sure it is what you asked for.",
  // Completed below with the kind of action it is.
  stillAsksInAllowAll: "Allow all is on in this chat, but this kind of action always asks.",
};

/** Why an action asks although the chat is in Allow all, by its kind. */
export const KEEP_ASKING_TEXT: Record<KeepAsking, string> = {
  delete: "it deletes files or folders, or leaves a file empty",
  destroy: "it can throw away work or change the disk or the system",
  sensitive: "it writes to a sensitive place (a .git folder, start-up files, git or ssh settings, autostart)",
  publish: "it pays, sends or publishes something in your name",
  protectedApp: "it sends input to a terminal, system settings, a password manager or outside the application in front",
  markedDestructive: "its server marks this tool as destructive, or you set it to always ask",
  hidden: "what it will do cannot be checked",
};

/** Why a card shows although the chat is in Allow all and the action is not one that always asks. */
export const ALLOW_ALL_AFTER_UNTRUSTED =
  "Allow all is on in this chat, but the model has read content from files, pages or tools in this turn, and this action can send data out or cannot be fully checked. Make sure it is what you asked for.";

function reasonText(request: ApprovalRequest, allowAll: boolean): string | null {
  if (request.reason === "stillAsksInAllowAll" && request.keepAsking) {
    return `Allow all is on in this chat, but this still asks: ${KEEP_ASKING_TEXT[request.keepAsking]}.`;
  }
  if (request.reason === "untrustedContentThenOutbound" && allowAll) return ALLOW_ALL_AFTER_UNTRUSTED;
  return REASON_TEXT[request.reason];
}

/**
 * An approval card. Every string on it comes from the model or a server, so it is shown
 * as plain text (already escaped by the backend: control, bidi and zero-width characters
 * are visible). Allow never has default focus or an Enter binding, and input is ignored
 * for a short time after the card appears.
 */
export function ApprovalCard(props: {
  request: ApprovalRequest;
  onDecide: (decision: "allowOnce" | "allowForChat" | "deny", note: string | null) => void;
}) {
  const { request } = props;
  const [armed, setArmed] = useState(false);
  const [note, setNote] = useState("");
  const [showNote, setShowNote] = useState(false);
  const [confirmAll, setConfirmAll] = useState(false);
  const allowAll = useAllowAll(request.chatId);
  const denyRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const t = window.setTimeout(() => setArmed(true), CARD_INPUT_DELAY_MS);
    return () => window.clearTimeout(t);
  }, []);

  const reason = reasonText(request, allowAll);
  const always = request.card === "onceOnly";
  const decide = (d: "allowOnce" | "allowForChat" | "deny") => {
    if (!armed) return;
    props.onDecide(d, d === "deny" && note.trim() ? note.trim() : null);
  };
  // "Allow all in this chat": the chat is switched first, and only then is this card
  // approved. If the switch is refused, nothing is approved.
  const allowAllInChat = () => {
    void useApprovalMode
      .getState()
      .setMode(request.chatId, true)
      .then(() => props.onDecide("allowOnce", null))
      .catch((e) => useApp.getState().showError(toErrorInfo(e)));
  };

  return (
    <>
    <div
      role="alertdialog"
      aria-labelledby={`approval-${request.id}-title`}
      aria-describedby={`approval-${request.id}-preview`}
      className="enter my-2 rounded-xl border border-warning/60 bg-surface p-3.5 text-sm shadow-float"
      onKeyDown={(e) => {
        // Enter must never approve.
        if (e.key === "Enter" && (e.target as HTMLElement).tagName !== "TEXTAREA") e.preventDefault();
      }}
    >
      <div className="flex items-start gap-2">
        {always ? (
          <ShieldAlert size={16} className="mt-0.5 shrink-0 text-warning" aria-hidden />
        ) : (
          <ShieldQuestion size={16} className="mt-0.5 shrink-0 text-fg-muted" aria-hidden />
        )}
        <div className="min-w-0 flex-1">
          <p id={`approval-${request.id}-title`} className="font-medium break-words">
            {request.summary}
          </p>
          <p className="text-xs text-fg-muted">
            {request.tool} · {request.server}
            {request.reason === "recoveredFromText" && " · recovered from text"}
          </p>
          {reason && <p className="mt-1 text-xs">{reason}</p>}
        </div>
      </div>
      <pre
        id={`approval-${request.id}-preview`}
        className="mt-2 max-h-64 overflow-auto rounded-md bg-bg p-2 font-mono text-xs whitespace-pre-wrap break-all"
      >
        {request.preview}
      </pre>
      {showNote && (
        <label className="mt-2 block text-xs">
          Note for the model (optional)
          <textarea
            value={note}
            onChange={(e) => setNote(e.target.value)}
            rows={2}
            className="mt-1 w-full rounded-md border border-border bg-bg p-2 text-sm"
          />
        </label>
      )}
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => decide("allowOnce")}
          aria-disabled={!armed}
          className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted aria-disabled:opacity-50"
        >
          Allow once
        </button>
        {!always && (
          <button
            type="button"
            onClick={() => decide("allowForChat")}
            aria-disabled={!armed}
            className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted aria-disabled:opacity-50"
          >
            Allow for this chat
          </button>
        )}
        {!always && !allowAll && (
          <button
            type="button"
            onClick={() => armed && setConfirmAll(true)}
            aria-disabled={!armed}
            className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted aria-disabled:opacity-50"
          >
            Allow all in this chat
          </button>
        )}
        <button
          ref={denyRef}
          type="button"
          onClick={() => decide("deny")}
          aria-disabled={!armed}
          className="rounded-md bg-danger px-3 py-1 text-white aria-disabled:opacity-50"
        >
          Deny
        </button>
        {!showNote && (
          <button type="button" className="text-xs underline" onClick={() => setShowNote(true)}>
            Add a note
          </button>
        )}
      </div>
    </div>
    {/* Outside the card: the card swallows Enter, and here Enter on Cancel must cancel. */}
    <AllowAllConfirm open={confirmAll} scope="chat" onOpenChange={setConfirmAll} onConfirm={allowAllInChat} />
    </>
  );
}
