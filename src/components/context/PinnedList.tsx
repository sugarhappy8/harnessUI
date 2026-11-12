import { useCallback, useEffect, useState, type KeyboardEvent, type ReactNode } from "react";
import { ArrowDown, ArrowUp, Pencil, Pin as PinIcon, X } from "lucide-react";
import { toErrorInfo } from "../../store/app";
import * as api from "./api";
import type { Pin, PinList } from "./types";

/** Characters as the app counts them: code points, so "🙂" is one, as in the store. */
export function charCount(text: string): number {
  return [...text].length;
}

function grouped(n: number): string {
  return n.toLocaleString("en-US");
}

/** "12 of 1,000 characters", in the warning colour past the limit. */
function CharCounter(props: { text: string; max: number; id: string }) {
  const n = charCount(props.text);
  const over = n > props.max;
  return (
    <span id={props.id} className={"text-xs tabular-nums " + (over ? "font-medium text-danger" : "text-fg-muted")}>
      {grouped(n)} of {grouped(props.max)} characters
      {over && ": too long"}
    </span>
  );
}

/** A pin's text can be kept when it has something in it and is not too long. */
function keepable(text: string, max: number): boolean {
  return text.trim() !== "" && charCount(text) <= max;
}

function ScopeSwitch(props: { pin: Pin; busy: boolean; onChange: (everyChat: boolean) => void }) {
  const option = (everyChat: boolean, label: string) => {
    const on = props.pin.everyChat === everyChat;
    return (
      <button
        type="button"
        aria-pressed={on}
        disabled={props.busy}
        onClick={() => !on && props.onChange(everyChat)}
        className={
          "px-2 py-0.5 text-xs " +
          (on ? "bg-bg-muted font-medium text-fg" : "text-fg-muted hover:bg-bg-muted/70 hover:text-fg")
        }
      >
        {label}
      </button>
    );
  };
  return (
    <span
      role="group"
      aria-label="Applies to"
      className="inline-flex overflow-hidden rounded-md border border-border"
      data-scope={props.pin.everyChat ? "every" : "chat"}
    >
      {option(false, "This chat")}
      {option(true, "Every chat")}
    </span>
  );
}

function IconButton(props: { label: string; disabled?: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      aria-label={props.label}
      title={props.label}
      disabled={props.disabled}
      onClick={props.onClick}
      className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg disabled:opacity-40 disabled:hover:bg-transparent"
    >
      {props.children}
    </button>
  );
}

function PinItem(props: {
  pin: Pin;
  index: number;
  maxChars: number;
  first: boolean;
  last: boolean;
  busy: boolean;
  run: (call: () => Promise<PinList>) => Promise<boolean>;
  chatId: number;
}) {
  const { pin, chatId, run } = props;
  const [draft, setDraft] = useState<string | null>(null);
  const editing = draft !== null;
  const counterId = `pin-${pin.id}-count`;

  const keep = async () => {
    if (draft === null || !keepable(draft, props.maxChars)) return;
    if (await run(() => api.editPin(chatId, pin.id, draft))) setDraft(null);
  };
  const onKey = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Escape") {
      e.preventDefault();
      setDraft(null);
    } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void keep();
    }
  };

  return (
    <li
      className="rounded-xl border border-border bg-surface p-3 text-sm shadow-soft"
      data-pin={pin.id}
      data-scope={pin.everyChat ? "every" : "chat"}
      aria-label={`Pinned instruction ${props.index + 1}`}
    >
      {editing ? (
        <div className="space-y-2">
          <textarea
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={onKey}
            rows={3}
            autoFocus
            aria-label="Pinned instruction"
            aria-describedby={counterId}
            className="w-full resize-y rounded-lg border border-border bg-bg p-2 text-sm outline-none focus:border-accent"
          />
          <div className="flex items-center gap-2">
            <CharCounter text={draft} max={props.maxChars} id={counterId} />
            <div className="flex-1" />
            <button
              type="button"
              onClick={() => setDraft(null)}
              className="rounded-lg px-2.5 py-1 text-xs text-fg-muted hover:bg-bg-muted hover:text-fg"
            >
              Discard changes
            </button>
            <button
              type="button"
              disabled={props.busy || !keepable(draft, props.maxChars)}
              onClick={() => void keep()}
              className="btn-primary rounded-lg px-2.5 py-1 text-xs font-medium"
            >
              Keep changes
            </button>
          </div>
        </div>
      ) : (
        <p className="whitespace-pre-wrap break-words" data-pin-text>
          {pin.text}
        </p>
      )}
      {!editing && (
        <div className="mt-2 flex flex-wrap items-center gap-1">
          <ScopeSwitch
            pin={pin}
            busy={props.busy}
            onChange={(everyChat) => void run(() => api.setPinScope(chatId, pin.id, everyChat))}
          />
          <div className="flex-1" />
          <IconButton
            label="Move up"
            disabled={props.busy || props.first}
            onClick={() => void run(() => api.movePin(chatId, pin.id, true))}
          >
            <ArrowUp size={14} aria-hidden />
          </IconButton>
          <IconButton
            label="Move down"
            disabled={props.busy || props.last}
            onClick={() => void run(() => api.movePin(chatId, pin.id, false))}
          >
            <ArrowDown size={14} aria-hidden />
          </IconButton>
          <IconButton label="Edit pin" disabled={props.busy} onClick={() => setDraft(pin.text)}>
            <Pencil size={14} aria-hidden />
          </IconButton>
          <IconButton
            label="Unpin"
            disabled={props.busy}
            onClick={() => void run(() => api.removePin(chatId, pin.id))}
          >
            <X size={14} aria-hidden />
          </IconButton>
        </div>
      )}
    </li>
  );
}

/** The pinned instructions of a chat: those for every chat, then the chat's own, as the
 *  model gets them. Add, edit in place, reorder, unpin, and switch between this chat and
 *  every chat. */
export function PinnedList(props: { chatId: number }) {
  const { chatId } = props;
  const [list, setList] = useState<PinList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [text, setText] = useState("");
  const [everyChat, setEveryChat] = useState(false);

  useEffect(() => {
    let gone = false;
    let unlisten: (() => void) | undefined;
    const reload = () =>
      api.listPins(chatId).then(
        (pins) => !gone && setList(pins),
        (err: unknown) => !gone && setError(toErrorInfo(err).message),
      );
    void reload();
    void api
      .onPinsChanged((changed) => {
        if (changed === null || changed === chatId) void reload();
      })
      .then((u) => {
        if (gone) u();
        else unlisten = u;
      });
    return () => {
      gone = true;
      unlisten?.();
    };
  }, [chatId]);

  /** Runs one change; true when it went through. */
  const run = useCallback(async (call: () => Promise<PinList>) => {
    setBusy(true);
    setError(null);
    try {
      setList(await call());
      return true;
    } catch (err) {
      setError(toErrorInfo(err).message);
      return false;
    } finally {
      setBusy(false);
    }
  }, []);

  const max = list?.max ?? 20;
  const maxChars = list?.maxChars ?? 1000;
  const pins = list?.pins ?? [];
  const full = pins.length >= max;

  const add = async () => {
    if (!keepable(text, maxChars) || full) return;
    if (await run(() => api.addPin(chatId, text, everyChat))) setText("");
  };

  return (
    <section className="space-y-3 p-4" aria-labelledby="pinned-heading">
      <header className="flex items-baseline gap-2">
        <h2 id="pinned-heading" className="flex items-center gap-1.5 text-sm font-medium">
          <PinIcon size={14} aria-hidden className="text-accent" />
          Pinned instructions
        </h2>
        <span className="text-xs text-fg-muted tabular-nums" data-pin-count>
          {pins.length} of {max}
        </span>
      </header>
      <p className="text-xs text-fg-muted">
        Sent with every message, right after the system prompt. Stored as plain text: do not put passwords or keys in
        a pin.
      </p>

      {pins.length === 0 && list !== null && <p className="text-sm text-fg-muted">Nothing is pinned yet.</p>}
      {pins.length > 0 && (
        <ol className="space-y-2" aria-label="Pinned instructions">
          {pins.map((pin, i) => (
            <PinItem
              key={pin.id}
              pin={pin}
              index={i}
              chatId={chatId}
              maxChars={maxChars}
              first={i === 0 || pins[i - 1]!.everyChat !== pin.everyChat}
              last={i === pins.length - 1 || pins[i + 1]!.everyChat !== pin.everyChat}
              busy={busy}
              run={run}
            />
          ))}
        </ol>
      )}

      {error && (
        <p className="text-sm text-danger" data-pin-error>
          {error}
        </p>
      )}

      <div className="space-y-2 rounded-xl border border-border bg-surface p-3 shadow-soft">
        <textarea
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
              e.preventDefault();
              void add();
            }
          }}
          rows={3}
          disabled={full}
          placeholder={full ? `${max} of ${max}: unpin one to add another.` : "An instruction to keep, such as “Answer in British English.”"}
          aria-label="New pinned instruction"
          aria-describedby="new-pin-count"
          className="w-full resize-y rounded-lg border border-border bg-bg p-2 text-sm outline-none focus:border-accent disabled:opacity-60"
        />
        <div className="flex flex-wrap items-center gap-3">
          <CharCounter text={text} max={maxChars} id="new-pin-count" />
          <div className="flex-1" />
          <fieldset className="flex items-center gap-3 text-xs" disabled={full}>
            <legend className="sr-only">Pin for</legend>
            <label className="flex items-center gap-1">
              <input type="radio" name="pin-scope" checked={!everyChat} onChange={() => setEveryChat(false)} />
              This chat
            </label>
            <label className="flex items-center gap-1">
              <input type="radio" name="pin-scope" checked={everyChat} onChange={() => setEveryChat(true)} />
              Every chat
            </label>
          </fieldset>
          <button
            type="button"
            disabled={busy || full || !keepable(text, maxChars)}
            onClick={() => void add()}
            className="btn-primary rounded-lg px-3 py-1 text-xs font-medium"
          >
            Pin it
          </button>
        </div>
      </div>
    </section>
  );
}
