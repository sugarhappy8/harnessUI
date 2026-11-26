import { useEffect } from "react";
import { DropdownMenu } from "radix-ui";
import { Check, ShieldAlert, ShieldCheck, ShieldOff, type LucideIcon } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ApprovalModeName } from "../lib/ipc";
import { useApprovalMode, useChatMode } from "../lib/approvalMode";
import { toErrorInfo, useApp } from "../store/app";

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
 * for as long as they are on. The button opens a menu of the three modes; a choice applies
 * at once.
 */
export function ApprovalModeMenu(props: { chatId: number }) {
  const { chatId } = props;
  const mode = useChatMode(chatId);

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

  const choose = (next: ApprovalModeName) => {
    if (next === mode) return;
    void useApprovalMode
      .getState()
      .setMode(chatId, next)
      .catch((e) => useApp.getState().showError(toErrorInfo(e)));
  };

  const look = MODE_LOOK[mode];
  const Icon = look.icon;
  const label = modeLabel(mode);

  return (
    <>
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button
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
    </>
  );
}
