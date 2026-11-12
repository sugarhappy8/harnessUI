import { useEffect, useRef, useState, type ReactNode, type RefObject } from "react";
import { AlertDialog, DropdownMenu } from "radix-ui";
import { Check, ShieldAlert, ShieldCheck, ShieldOff, type LucideIcon } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ApprovalModeName } from "../lib/ipc";
import { useApprovalMode, useChatMode } from "../lib/approvalMode";
import { toErrorInfo, useApp } from "../store/app";

/** Clicks on the confirm button are ignored for this long after the dialog opens, so a
 *  double click on the button that opened it cannot land on it. */
export const CONFIRM_INPUT_DELAY_MS = 700;

/** The three modes, safest first, as the header menu and Settings list them. */
export const APPROVAL_MODES: readonly { mode: ApprovalModeName; label: string; description: string }[] = [
  { mode: "ask", label: "Ask first", description: "Asks before it changes anything." },
  { mode: "allowAll", label: "Allow all", description: "Asks only before deleting and a few other things." },
  {
    mode: "fullAccess",
    label: "Full access",
    description: "Never asks. Can change or delete anything on this computer.",
  },
];

export function modeLabel(mode: ApprovalModeName): string {
  return APPROVAL_MODES.find((m) => m.mode === mode)?.label ?? mode;
}

/** How each mode looks: muted while it asks, the warning colour for Allow all and the
 *  danger colour for Full access, so a mode that asks less is never easy to miss. */
export const MODE_LOOK: Record<ApprovalModeName, { icon: LucideIcon; text: string; button: string }> = {
  ask: { icon: ShieldCheck, text: "text-fg-muted", button: "text-fg-muted hover:bg-bg-muted hover:text-fg" },
  allowAll: {
    icon: ShieldAlert,
    text: "text-warning",
    button: "bg-warning/10 font-medium text-warning hover:bg-warning/20",
  },
  fullAccess: {
    icon: ShieldOff,
    text: "text-danger",
    button: "bg-danger/10 font-medium text-danger hover:bg-danger/20",
  },
};

/** How much each mode lets through without asking. Switching to a safer mode needs no
 *  confirmation; switching to one that asks less always does. */
const RISK: Record<ApprovalModeName, number> = { ask: 0, allowAll: 1, fullAccess: 2 };

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

/** Full access in one sentence, under the title of its confirmation. */
export const FULL_ACCESS_SUMMARY =
  "The model will do everything without asking, anywhere on this computer. It also gets tools to create, move and delete folders.";

/** What Full access means, as the list the user reads before switching it on. It follows
 *  src-tauri/src/approvals.rs (the whole computer, minus the app's own folders); keep the
 *  two in step. */
export const FULL_ACCESS_RISKS: readonly string[] = [
  "Nothing asks first, not even deleting, paying or publishing.",
  "It can run any command, and read, create, change, move or delete any file or folder on this computer, not only in your approved folders.",
  "Deleted files do not go to the trash. They are gone.",
  "It can use the browser and the accounts you are logged in to.",
  "Something it reads, such as a web page, a file or a tool result, could trick it into doing something harmful.",
  "The app's own data (its database and logs) and your saved keys stay out of reach of its tools. A program it runs has your full rights, so this is a guard, not a lock.",
];

type Tone = "warning" | "danger";

/** The shared look of the confirmations. */
function ConfirmDialog(props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tone: Tone;
  icon: LucideIcon;
  title: string;
  description: string;
  confirmLabel: string;
  /** The label of the button that has the focus first and closes the dialog. */
  cancelLabel?: string;
  onConfirm: () => void;
  /** Where the focus goes when the dialog closes (it has no trigger of its own). */
  returnFocus?: RefObject<HTMLElement | null>;
  children: ReactNode;
}) {
  const { tone, returnFocus } = props;
  const Icon = props.icon;
  return (
    <AlertDialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
        <AlertDialog.Content
          onCloseAutoFocus={(e) => {
            if (!returnFocus?.current) return;
            e.preventDefault();
            returnFocus.current.focus();
          }}
          className={
            "fixed top-1/2 left-1/2 flex max-h-[90vh] w-[34rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border bg-surface text-sm shadow-pop " +
            (tone === "danger" ? "border-danger/60" : "border-border")
          }
        >
          {/* The text scrolls; the buttons stay in view. */}
          <div className="min-h-0 flex-1 overflow-y-auto p-5 pb-3">
            <AlertDialog.Title className="flex items-center gap-2 text-base font-semibold">
              <Icon size={18} className={"shrink-0 " + (tone === "danger" ? "text-danger" : "text-warning")} aria-hidden />
              {props.title}
            </AlertDialog.Title>
            <AlertDialog.Description className="mt-2 text-fg-muted">{props.description}</AlertDialog.Description>
            {props.children}
          </div>
          <ConfirmButtons
            label={props.confirmLabel}
            cancelLabel={props.cancelLabel ?? "Cancel"}
            tone={tone}
            onConfirm={() => {
              // Confirmed first, then closed: a dialog where closing is itself an answer
              // (FullAccessStartCheck) can tell the two apart.
              props.onConfirm();
              props.onOpenChange(false);
            }}
          />
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

/**
 * The confirmation shown before Allow all is switched on, for one chat or for every chat.
 * Cancel has the focus when it opens, and the confirm button ignores input for a moment.
 */
export function AllowAllConfirm(props: {
  open: boolean;
  scope: "chat" | "every";
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  returnFocus?: RefObject<HTMLElement | null>;
}) {
  const { scope } = props;
  const where = scope === "chat" ? "this chat" : "every chat";

  return (
    <ConfirmDialog
      open={props.open}
      onOpenChange={props.onOpenChange}
      tone="warning"
      icon={ShieldAlert}
      title={`Allow all in ${where}?`}
      description={NO_LONGER_ASKS}
      confirmLabel={`Allow all in ${where}`}
      onConfirm={props.onConfirm}
      returnFocus={props.returnFocus}
    >
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
    </ConfirmDialog>
  );
}

/**
 * The confirmation shown before Full access is switched on, for one chat or for every
 * chat: the same safeguards as Allow all's (Cancel focused, a delay on the confirm
 * button), in the danger colour, with what the model can then do.
 */
export function FullAccessConfirm(props: {
  open: boolean;
  scope: "chat" | "every";
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  returnFocus?: RefObject<HTMLElement | null>;
}) {
  const { scope } = props;
  return (
    <ConfirmDialog
      open={props.open}
      onOpenChange={props.onOpenChange}
      tone="danger"
      icon={ShieldOff}
      title={`Turn on Full access in ${scope === "chat" ? "this chat" : "every chat"}?`}
      description={FULL_ACCESS_SUMMARY}
      confirmLabel="Turn on Full access"
      onConfirm={props.onConfirm}
      returnFocus={props.returnFocus}
    >
      <p className="mt-3 font-medium">What this means:</p>
      <ul aria-label="Risks" className="mt-1 list-disc space-y-1 pl-5">
        {FULL_ACCESS_RISKS.map((line) => (
          <li key={line}>{line}</li>
        ))}
        <li>
          {scope === "chat"
            ? "It stays on in this chat until you switch it off or quit the app."
            : "It applies to every chat, also the ones you switched by hand, and stays on after a restart, until you change it here."}
        </li>
      </ul>
    </ConfirmDialog>
  );
}

/** Why the app asks again at start before every chat starts in Full access. */
export const FULL_ACCESS_ASKS_AGAIN =
  "Your setting says every chat starts in Full access. For your safety the app asks again each time it starts, because a program on this computer could have changed that setting.";

/**
 * Asked once when the main window starts, if the saved setting says every chat starts in
 * Full access. The app does not apply that setting on its own: it is a row in the database
 * that a program on this computer could have written. Until the user answers, every chat
 * starts in Ask first. Closing the dialog or pressing Escape is the same as Ask first.
 */
export function FullAccessStartCheck() {
  const [open, setOpen] = useState(false);
  // One answer only: confirming also closes the dialog, and closing means Ask first.
  const answered = useRef(false);

  useEffect(() => {
    let alive = true;
    ipc
      .getFullAccessDefaultPending()
      .then((pending) => {
        if (alive && pending) setOpen(true);
      })
      // Not known: every chat stays in Ask first, which is the safe side.
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  const answer = (on: boolean) => {
    if (answered.current) return;
    answered.current = true;
    setOpen(false);
    void ipc
      .answerFullAccessDefault(on)
      // Settings and the hint under the message box show the mode every chat starts in now.
      .then(() => useApprovalMode.getState().loadDefault())
      .catch((e) => useApp.getState().showError(toErrorInfo(e)));
  };

  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(next) => {
        if (!next) answer(false);
      }}
      tone="danger"
      icon={ShieldOff}
      title="Start every chat in Full access again?"
      description={FULL_ACCESS_ASKS_AGAIN}
      confirmLabel="Turn on Full access"
      cancelLabel="Start in Ask first"
      onConfirm={() => answer(true)}
    >
      <p className="mt-3 font-medium">What Full access means:</p>
      <ul aria-label="Risks" className="mt-1 list-disc space-y-1 pl-5">
        {FULL_ACCESS_RISKS.map((line) => (
          <li key={line}>{line}</li>
        ))}
        <li>It applies to every chat until you quit the app. The app asks again the next time it starts.</li>
      </ul>
      <p className="mt-3 text-fg-muted">
        Start in Ask first also changes your setting to Ask first. You can change it again in Settings.
      </p>
    </ConfirmDialog>
  );
}

const CONFIRM_TONE: Record<Tone, string> = {
  warning: "border-warning/60 bg-warning/10 text-warning hover:bg-warning/20",
  danger: "border-danger/60 bg-danger/10 text-danger hover:bg-danger/20",
};

/** Cancel and confirm. Mounted with the dialog, so the delay starts when it opens. */
function ConfirmButtons(props: { label: string; cancelLabel: string; tone: Tone; onConfirm: () => void }) {
  const [armed, setArmed] = useState(false);
  useEffect(() => {
    const t = window.setTimeout(() => setArmed(true), CONFIRM_INPUT_DELAY_MS);
    return () => window.clearTimeout(t);
  }, []);
  return (
    <div className="flex shrink-0 justify-end gap-2 border-t border-border px-5 py-3">
      <AlertDialog.Cancel className="rounded-md border border-border px-3 py-1.5 hover:bg-bg-muted">
        {props.cancelLabel}
      </AlertDialog.Cancel>
      <button
        type="button"
        aria-disabled={!armed}
        onClick={() => armed && props.onConfirm()}
        className={"rounded-md border px-3 py-1.5 font-medium aria-disabled:opacity-50 " + CONFIRM_TONE[props.tone]}
      >
        {props.label}
      </button>
    </div>
  );
}

/** The end of the hint under the message box, for each mode. */
export const APPROVAL_HINTS: Record<ApprovalModeName, string> = {
  ask: "the model asks before it changes anything",
  allowAll: "Allow all is on: the model asks only before deleting and a few other things",
  fullAccess: "Full access is on: the model does everything without asking",
};

/**
 * The end of the hint under the message box: what the open chat's approval mode means
 * (for a chat that is about to be started, what the mode every chat starts in means).
 */
export function ApprovalHint() {
  const chatId = useApp((s) => s.activeChatId);
  const mode = useApprovalMode((s) =>
    chatId === null ? s.defaultMode : (s.modes[chatId] ?? s.defaultMode),
  );
  useEffect(() => {
    void useApprovalMode
      .getState()
      .loadDefault()
      .catch(() => undefined);
  }, []);
  // Full access stays in sight while the user types.
  return mode === "fullAccess" ? (
    <span className="text-danger">{APPROVAL_HINTS[mode]}</span>
  ) : (
    <>{APPROVAL_HINTS[mode]}</>
  );
}

/** The header button's tooltip, for each mode. */
const MODE_TITLE: Record<ApprovalModeName, string> = {
  ask: "The model asks before it changes anything. Click to change.",
  allowAll: "Allow all is on: the model asks only before deleting and a few other things. Click to change.",
  fullAccess: "Full access is on: the model does everything without asking. Click to change.",
};

/**
 * The chat's approval mode, in the chat header: "Ask first" (muted) is where every chat
 * starts, "Allow all" shows in the warning colour and "Full access" in the danger colour
 * for as long as they are on. The button opens a menu of the three modes. A mode that asks
 * less asks first (Allow all, Full access); a safer one takes one click.
 */
export function ApprovalModeMenu(props: { chatId: number }) {
  const { chatId } = props;
  const mode = useChatMode(chatId);
  const [confirm, setConfirm] = useState<"allowAll" | "fullAccess" | null>(null);
  const button = useRef<HTMLButtonElement>(null);
  // Set when a choice opens a confirmation, so the closing menu leaves the focus to the
  // dialog's Cancel button instead of moving it back to the header button.
  const confirming = useRef(false);

  useEffect(() => {
    void useApprovalMode
      .getState()
      .load(chatId)
      .catch((e) => useApp.getState().showError(toErrorInfo(e)));
  }, [chatId]);
  // The app says when a mode changed elsewhere ("Forget everything I allowed in this
  // chat", the choice in Settings): read it again, so the button never shows a stale mode.
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

  const switchTo = (next: ApprovalModeName) =>
    void useApprovalMode
      .getState()
      .setMode(chatId, next)
      .catch((e) => useApp.getState().showError(toErrorInfo(e)));

  const choose = (next: ApprovalModeName) => {
    if (next === mode) return;
    if (next === "ask" || RISK[next] < RISK[mode]) {
      switchTo(next);
      return;
    }
    confirming.current = true;
    setConfirm(next);
  };

  const look = MODE_LOOK[mode];
  const Icon = look.icon;
  const label = modeLabel(mode);
  const closeConfirm = (open: boolean) => {
    if (!open) setConfirm(null);
  };

  return (
    <>
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button
            ref={button}
            type="button"
            aria-label={label}
            title={MODE_TITLE[mode]}
            className={
              "flex shrink-0 items-center gap-1.5 rounded-lg px-2 py-1.5 text-sm whitespace-nowrap " + look.button
            }
          >
            <Icon size={15} aria-hidden />
            <span className={mode === "ask" ? "hidden @3xl:inline" : ""}>{label}</span>
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <DropdownMenu.Content
            align="end"
            sideOffset={4}
            onCloseAutoFocus={(e) => {
              if (!confirming.current) return;
              confirming.current = false;
              e.preventDefault();
            }}
            className="z-50 w-80 max-w-[92vw] rounded-xl border border-border bg-surface p-1.5 shadow-pop"
          >
            <DropdownMenu.Label className="px-2 py-1 text-xs text-fg-muted">Approvals in this chat</DropdownMenu.Label>
            <DropdownMenu.RadioGroup value={mode} onValueChange={(v) => choose(v as ApprovalModeName)}>
              {APPROVAL_MODES.map((m) => {
                const ItemIcon = MODE_LOOK[m.mode].icon;
                const colour = MODE_LOOK[m.mode].text;
                return (
                  <DropdownMenu.RadioItem
                    key={m.mode}
                    value={m.mode}
                    className="flex cursor-default items-start gap-2.5 rounded-lg px-2.5 py-2 text-sm outline-none data-[highlighted]:bg-bg-muted"
                  >
                    <ItemIcon size={15} className={"mt-0.5 shrink-0 " + colour} aria-hidden />
                    <span className="min-w-0 flex-1">
                      <span className={"block font-medium " + (m.mode === "ask" ? "" : colour)}>{m.label}</span>
                      <span className="block text-xs text-fg-muted">{m.description}</span>
                    </span>
                    <DropdownMenu.ItemIndicator className="mt-0.5 shrink-0">
                      <Check size={14} aria-hidden />
                    </DropdownMenu.ItemIndicator>
                  </DropdownMenu.RadioItem>
                );
              })}
            </DropdownMenu.RadioGroup>
          </DropdownMenu.Content>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
      <AllowAllConfirm
        open={confirm === "allowAll"}
        scope="chat"
        onOpenChange={closeConfirm}
        onConfirm={() => switchTo("allowAll")}
        returnFocus={button}
      />
      <FullAccessConfirm
        open={confirm === "fullAccess"}
        scope="chat"
        onOpenChange={closeConfirm}
        onConfirm={() => switchTo("fullAccess")}
        returnFocus={button}
      />
    </>
  );
}
