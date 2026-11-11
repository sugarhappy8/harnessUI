import { useEffect, useState } from "react";
import { AlertDialog } from "radix-ui";
import { ShieldAlert, ShieldCheck } from "lucide-react";
import * as ipc from "../lib/ipc";
import { useAllowAll, useApprovalMode } from "../lib/approvalMode";
import { toErrorInfo, useApp } from "../store/app";

/** Clicks on the confirm button are ignored for this long after the dialog opens, so a
 *  double click on the button that opened it cannot land on it. */
export const CONFIRM_INPUT_DELAY_MS = 700;

/** What the model may do without asking once Allow all is on. */
export const NO_LONGER_ASKS =
  "The model will no longer ask before it reads, writes or edits files in your approved folders, runs commands, commits with git, browses, or uses its other tools.";

/** What still asks in Allow all, as the list the user reads before switching it on.
 *  It follows the rules in crates/policy (allow_all.rs); keep the two in step. */
export const STILL_ASKS: readonly string[] = [
  "Deleting files or folders, however it is done, and emptying a file.",
  "Throwing away work or changing the computer: git reset --hard, a forced push, disk and system commands, registry edits, and anything run as administrator.",
  "Commands that hide what they run.",
  "Writing to sensitive places: a .git folder, shell start-up files, git and ssh settings, autostart folders.",
  "Paying, sending and publishing: web page buttons that say pay, buy, order, checkout, transfer, delete, remove, send, post or publish, and git push.",
  "Typing or clicking in a terminal, in system settings or in a password manager.",
  "Tools from added servers that are marked destructive, or that you set to always ask.",
  "Sending data out after the model has read a web page, a file or a tool result in the same turn: opening another site, typing into a page, tools from added servers, git pull and clone, and commands that use the network or cannot be checked.",
];

/** What Allow all does not change, and what it cannot see. */
export const LIMITS: readonly string[] = [
  "File tools still cannot reach folders you have not approved.",
  "Commands run with your rights and can also change things outside the approved folders.",
  "The app checks the command it is about to run, not what a script or program does once it is running.",
];

/**
 * The confirmation shown before Allow all is switched on, for one chat or for every chat.
 * Cancel has the focus when it opens, and the confirm button ignores input for a moment.
 */
export function AllowAllConfirm(props: {
  open: boolean;
  scope: "chat" | "every";
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const { open, scope } = props;
  const where = scope === "chat" ? "this chat" : "every chat";

  return (
    <AlertDialog.Root open={open} onOpenChange={props.onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 flex max-h-[90vh] w-[34rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface text-sm shadow-pop">
          {/* The text scrolls; the buttons stay in view. */}
          <div className="min-h-0 flex-1 overflow-y-auto p-5 pb-3">
          <AlertDialog.Title className="flex items-center gap-2 text-base font-semibold">
            <ShieldAlert size={18} className="shrink-0 text-warning" aria-hidden />
            Allow all in {where}?
          </AlertDialog.Title>
          <AlertDialog.Description className="mt-2 text-fg-muted">{NO_LONGER_ASKS}</AlertDialog.Description>
          <p className="mt-3 font-medium">It still asks before:</p>
          <ul aria-label="Still asks" className="mt-1 list-disc space-y-1 pl-5 text-fg-muted">
            {STILL_ASKS.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
          <p className="mt-3 font-medium">Good to know:</p>
          <ul aria-label="Limits" className="mt-1 list-disc space-y-1 pl-5 text-fg-muted">
            {LIMITS.map((line) => (
              <li key={line}>{line}</li>
            ))}
            <li>
              {scope === "chat"
                ? "It stays on in this chat until you switch it off or quit the app."
                : "It applies to every chat and stays on after a restart, until you switch it off here."}
            </li>
          </ul>
          </div>
          <ConfirmButtons
            label={`Allow all in ${where}`}
            onConfirm={() => {
              props.onOpenChange(false);
              props.onConfirm();
            }}
          />
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

/** Cancel and confirm. Mounted with the dialog, so the delay starts when it opens. */
function ConfirmButtons(props: { label: string; onConfirm: () => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const t = window.setTimeout(() => setArmed(true), CONFIRM_INPUT_DELAY_MS);
    return () => window.clearTimeout(t);
  }, []);
  return (
    <div className="flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3">
      <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
        Cancel
      </AlertDialog.Cancel>
      <button
        type="button"
        aria-disabled={!armed}
        onClick={() => armed && props.onConfirm()}
        className="rounded-md border border-warning/60 bg-warning/10 px-3 py-1.5 font-medium text-warning hover:bg-warning/20 aria-disabled:opacity-50"
      >
        {props.label}
      </button>
    </div>
  );
}

/**
 * The end of the hint under the message box: what the open chat's approval mode means
 * (for a chat that is about to be started, what "Allow all in every chat" means).
 */
export function ApprovalHint() {
  const chatId = useApp((s) => s.activeChatId);
  const allowAll = useApprovalMode((s) =>
    chatId === null ? s.defaultOn : (s.modes[chatId] ?? s.defaultOn),
  );
  useEffect(() => {
    void useApprovalMode
      .getState()
      .loadDefault()
      .catch(() => undefined);
  }, []);
  return (
    <>
      {allowAll
        ? "Allow all is on: the model asks only before deleting and a few other things"
        : "the model asks before it changes anything"}
    </>
  );
}

/**
 * The chat's approval mode, in the chat header. "Ask first" (muted) is where every chat
 * starts; "Allow all" is shown in the warning colour for as long as it is on. Switching
 * it on asks first; switching it off takes one click.
 */
export function AllowAllButton(props: { chatId: number }) {
  const { chatId } = props;
  const allowAll = useAllowAll(chatId);
  const [confirm, setConfirm] = useState(false);

  useEffect(() => {
    void useApprovalMode
      .getState()
      .load(chatId)
      .catch((e) => useApp.getState().showError(toErrorInfo(e)));
  }, [chatId]);
  // The app says when a mode changed elsewhere ("Forget everything I allowed in this
  // chat", the switch in Settings): read it again, so the button never shows a stale mode.
  useEffect(() => {
    let off: (() => void) | undefined;
    let gone = false;
    void ipc
      .onApprovalModeChanged((changed) => void useApprovalMode.getState().refresh(changed).catch(() => undefined))
      .then((unlisten) => (gone ? unlisten() : (off = unlisten)))
      .catch(() => undefined);
    return () => {
      gone = true;
      off?.();
    };
  }, []);

  const switchTo = (on: boolean) =>
    void useApprovalMode
      .getState()
      .setMode(chatId, on)
      .catch((e) => useApp.getState().showError(toErrorInfo(e)));

  return (
    <>
      <button
        type="button"
        aria-label={allowAll ? "Allow all" : "Ask first"}
        title={
          allowAll
            ? "Allow all is on: the model acts without asking, except for deleting and a few other things. Click to go back to asking first."
            : "The model asks before it acts. Click to allow all in this chat."
        }
        onClick={() => (allowAll ? switchTo(false) : setConfirm(true))}
        className={
          "flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm whitespace-nowrap " +
          (allowAll
            ? "bg-warning/10 font-medium text-warning hover:bg-warning/20"
            : "text-fg-muted hover:bg-bg-muted hover:text-fg")
        }
      >
        {allowAll ? <ShieldAlert size={15} aria-hidden /> : <ShieldCheck size={15} aria-hidden />}
        <span className={allowAll ? "" : "hidden @3xl:inline"}>{allowAll ? "Allow all" : "Ask first"}</span>
      </button>
      <AllowAllConfirm open={confirm} scope="chat" onOpenChange={setConfirm} onConfirm={() => switchTo(true)} />
    </>
  );
}
