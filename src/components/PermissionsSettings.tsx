import { useEffect, useState } from "react";
import { RadioGroup } from "radix-ui";
import { FolderPlus, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ApprovedFolder, AuditRow, ErrorInfo, HotkeyStatus } from "../lib/types";
import { toErrorInfo } from "../store/app";
import { useApprovalMode, type ApprovalModeName } from "../lib/approvalMode";
import { APPROVAL_MODES, AllowAllConfirm, FullAccessConfirm, MODE_LOOK } from "./AllowAllButton";
import { ErrorBanner } from "./ErrorBanner";

/** Approved folders for every chat, and the stop hotkey. */
export function PermissionsSettings() {
  const [folders, setFolders] = useState<ApprovedFolder[]>([]);
  const [hotkey, setHotkey] = useState<HotkeyStatus | null>(null);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<ErrorInfo | null>(null);

  useEffect(() => {
    let alive = true;
    Promise.all([ipc.listApprovedFolders(null), ipc.stopHotkeyStatus()])
      .then(([f, h]) => {
        if (!alive) return;
        setFolders(f);
        setHotkey(h);
        setDraft(h.accelerator);
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function add() {
    try {
      const f = await ipc.approveFolder(null);
      if (f) setFolders(await ipc.listApprovedFolders(null));
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  async function remove(id: number) {
    try {
      await ipc.removeApprovedFolder(id);
      setFolders(await ipc.listApprovedFolders(null));
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  async function saveHotkey() {
    try {
      setHotkey(await ipc.setStopHotkey(draft));
    } catch (err) {
      setError(toErrorInfo(err));
      setHotkey(await ipc.stopHotkeyStatus());
    }
  }

  return (
    <div className="space-y-5 text-sm">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <DefaultApprovalMode onError={setError} />
      <section>
        <h3 className="mb-1 font-medium">Folders tools may use in every chat</h3>
        <p className="mb-2 text-xs text-fg-muted">
          File tools can only read and write inside approved folders. To approve a folder for one
          chat only, open Tools in that chat and use Folders for this chat. The app&apos;s own data and
          program folders are never reachable.
        </p>
        <ul className="space-y-1">
          {folders.map((f) => (
            <li key={f.id} className="flex items-center gap-2 rounded-md border border-border px-3 py-1.5">
              <span className="min-w-0 flex-1 truncate font-mono text-xs" title={f.path}>
                {f.path}
              </span>
              <button type="button" aria-label={`Remove ${f.path}`} onClick={() => void remove(f.id)} className="text-fg-muted hover:text-danger">
                <Trash2 size={13} aria-hidden />
              </button>
            </li>
          ))}
          {folders.length === 0 && <li className="text-xs text-fg-muted">No folders approved for all chats.</li>}
        </ul>
        <button type="button" onClick={() => void add()} className="mt-2 flex items-center gap-1 text-accent">
          <FolderPlus size={14} aria-hidden /> Approve a folder…
        </button>
      </section>
      <section>
        <h3 className="mb-1 font-medium">Stop hotkey</h3>
        <p className="mb-2 text-xs text-fg-muted">
          Stops whatever the model is doing, even when this window is in the background.
        </p>
        {hotkey && !hotkey.registered && (
          <p className="mb-2 rounded-md border border-warning/60 bg-warning/10 px-2 py-1 text-xs">
            The stop hotkey is not active: {hotkey.error ?? "it could not be registered"}. Choose a
            different combination.
          </p>
        )}
        <div className="flex gap-2">
          <label className="sr-only" htmlFor="hotkey">
            Stop hotkey
          </label>
          <input
            id="hotkey"
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            className="w-64 rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
          />
          <button type="button" onClick={() => void saveHotkey()} className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted">
            Save
          </button>
        </div>
        <p className="mt-1 text-xs text-fg-muted">
          Format: Control+Alt+Backspace. Avoid Ctrl+Alt with a letter, which some keyboards use to
          type characters.
        </p>
      </section>
      <TerminalPassThrough onError={setError} />
    </div>
  );
}

/**
 * The mode every chat starts in: Ask first (where a new install starts), Allow all or Full
 * access. Ask first saves at once; the other two ask first, as in the chat header.
 */
function DefaultApprovalMode(props: { onError: (e: ErrorInfo) => void }) {
  const { onError } = props;
  const mode = useApprovalMode((s) => s.defaultMode);
  const [confirm, setConfirm] = useState<"allowAll" | "fullAccess" | null>(null);
  useEffect(() => {
    let alive = true;
    useApprovalMode
      .getState()
      .loadDefault()
      .catch((e) => alive && onError(toErrorInfo(e)));
    return () => {
      alive = false;
    };
  }, [onError]);
  async function save(next: ApprovalModeName) {
    try {
      await useApprovalMode.getState().setDefault(next);
    } catch (e) {
      onError(toErrorInfo(e));
    }
  }
  // Even a step down from Full access to Allow all asks: the new default also applies to
  // chats switched by hand, and some of those may have been in Ask first.
  function choose(next: ApprovalModeName) {
    if (next === mode) return;
    if (next === "ask") void save("ask");
    else setConfirm(next);
  }
  const closeConfirm = (open: boolean) => {
    if (!open) setConfirm(null);
  };
  return (
    <section>
      <h3 className="mb-1 font-medium">Approvals</h3>
      <p className="mb-2 text-xs text-fg-muted">
        Choose how much the model may do without asking you: Ask first, Allow all or Full access. Every chat starts
        in the mode you choose here, and changing it also changes chats you switched by hand. To change one chat
        only, use the shield button in the chat&apos;s header.
      </p>
      {/* Controlled by the saved default: a choice that asks first shows only once confirmed. */}
      <RadioGroup.Root
        value={mode}
        onValueChange={(v) => choose(v as ApprovalModeName)}
        aria-label="Every chat starts in"
        className="space-y-1"
      >
        {APPROVAL_MODES.map((m) => (
          <RadioGroup.Item
            key={m.mode}
            value={m.mode}
            className="flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left hover:bg-bg-muted"
          >
            <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full border border-border-strong">
              <RadioGroup.Indicator className="h-2 w-2 rounded-full bg-fg" />
            </span>
            <span>
              <span className={mode === m.mode && m.mode !== "ask" ? "font-medium " + MODE_LOOK[m.mode].text : ""}>
                {m.label}
              </span>
              <span className="block text-xs text-fg-muted">{m.description}</span>
            </span>
          </RadioGroup.Item>
        ))}
      </RadioGroup.Root>
      <AllowAllConfirm
        open={confirm === "allowAll"}
        scope="every"
        onOpenChange={closeConfirm}
        onConfirm={() => void save("allowAll")}
      />
      <FullAccessConfirm
        open={confirm === "fullAccess"}
        scope="every"
        onOpenChange={closeConfirm}
        onConfirm={() => void save("fullAccess")}
      />
    </section>
  );
}

/** Variables the terminal passes through although their names look secret (9.3). */
function TerminalPassThrough(props: { onError: (e: ErrorInfo) => void }) {
  const [text, setText] = useState("");
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    void ipc
      .getTerminalPassThrough()
      .then((names) => setText(names.join(" ")))
      .catch(() => undefined);
  }, []);
  async function save() {
    try {
      const names = await ipc.setTerminalPassThrough(text.split(/[\s,]+/).filter(Boolean));
      setText(names.join(" "));
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      props.onError(toErrorInfo(e));
    }
  }
  return (
    <section>
      <h3 className="mb-1 font-medium">Terminal environment</h3>
      <p className="mb-2 text-xs text-fg-muted">
        Commands the terminal tool runs get your environment without variables whose names look secret (*_TOKEN,
        *_KEY, *_SECRET, *PASSWORD*). List variables that commands need anyway, separated by spaces. Anything listed
        here can end up in the model&apos;s context.
      </p>
      <div className="flex gap-2">
        <label className="sr-only" htmlFor="pass-through">
          Variables passed through
        </label>
        <input
          id="pass-through"
          value={text}
          onChange={(e) => setText(e.target.value)}
          placeholder="e.g. NPM_TOKEN"
          className="w-80 rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
        />
        <button type="button" onClick={() => void save()} className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted">
          {saved ? "Saved" : "Save"}
        </button>
      </div>
    </section>
  );
}

const DECISIONS = [
  ["", "Any decision"],
  ["allow_once", "Allowed once"],
  ["allow_for_chat", "Allowed for the chat"],
  ["allowed_by_grant", "Allowed by an earlier grant"],
  ["allowed_by_mode", "Allowed by Allow all or Full access"],
  ["allowed_without_prompt", "Allowed without asking"],
  ["denied", "Denied"],
  ["cancelled", "Cancelled"],
  ["refused_by_policy", "Refused by policy"],
] as const;

/** The note on an audit row of an action that ran because the chat was in Full access.
 *  The stored decision is `allowed_by_mode`, as for Allow all (FULL_ACCESS_NOTE in
 *  src-tauri/src/approvals.rs; keep the two in step). */
export const FULL_ACCESS_NOTE = "Full access";

/** Whether the row's note only names the mode that let the call through. */
function noteNamesMode(decision: string, note: string | null): boolean {
  return decision === "allowed_by_mode" && note === FULL_ACCESS_NOTE;
}

/** How a stored decision reads in the table. */
export function decisionLabel(decision: string, note: string | null = null): string {
  if (decision === "allowed_by_mode") {
    return noteNamesMode(decision, note) ? "allowed by Full access" : "allowed by Allow all";
  }
  return decision.replaceAll("_", " ");
}

/** Every tool call with its decision and result (13.7). */
export function AuditLogView() {
  const [rows, setRows] = useState<AuditRow[]>([]);
  const [tool, setTool] = useState("");
  const [decision, setDecision] = useState("");
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [saved, setSaved] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    ipc
      .listAudit({ tool: tool || null, decision: decision || null, limit: 500 })
      .then((r) => alive && setRows(r))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, [tool, decision]);

  async function exportAs(format: "jsonl" | "csv") {
    try {
      const path = await ipc.exportAudit({ tool: tool || null, decision: decision || null }, format);
      if (path) setSaved(path);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  return (
    <div className="space-y-3 text-sm">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <div className="flex flex-wrap items-center gap-2">
        <label className="sr-only" htmlFor="audit-tool">
          Tool
        </label>
        <input
          id="audit-tool"
          value={tool}
          placeholder="Filter by tool"
          onChange={(e) => setTool(e.target.value)}
          className="rounded-md border border-border bg-bg px-2 py-1 text-xs"
        />
        <label className="sr-only" htmlFor="audit-decision">
          Decision
        </label>
        <select
          id="audit-decision"
          value={decision}
          onChange={(e) => setDecision(e.target.value)}
          className="rounded-md border border-border bg-bg px-2 py-1 text-xs"
        >
          {DECISIONS.map(([v, l]) => (
            <option key={v} value={v}>
              {l}
            </option>
          ))}
        </select>
        <div className="ml-auto flex gap-2 text-xs">
          <button type="button" className="underline" onClick={() => void exportAs("jsonl")}>
            Export JSON Lines
          </button>
          <button type="button" className="underline" onClick={() => void exportAs("csv")}>
            Export CSV
          </button>
        </div>
      </div>
      {saved && <p className="text-xs text-fg-muted">Saved to {saved}</p>}
      <div className="max-h-80 overflow-auto rounded-md border border-border">
        <table className="w-full text-left text-xs">
          <thead className="sticky top-0 bg-bg-subtle">
            <tr>
              <th className="px-2 py-1">Time</th>
              <th className="px-2 py-1">Tool</th>
              <th className="px-2 py-1">What</th>
              <th className="px-2 py-1">Decision</th>
              <th className="px-2 py-1">Result</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.id} className="border-t border-border align-top">
                <td className="px-2 py-1 whitespace-nowrap">{new Date(r.ts).toLocaleString()}</td>
                <td className="px-2 py-1">{r.tool}</td>
                <td className="max-w-xs truncate px-2 py-1" title={r.preview}>
                  {r.preview}
                </td>
                <td className="px-2 py-1">
                  {decisionLabel(r.decision, r.note)}
                  {r.note && !noteNamesMode(r.decision, r.note) ? ` (“${r.note}”)` : ""}
                </td>
                <td className="px-2 py-1">{r.result_status ?? "—"}</td>
              </tr>
            ))}
            {rows.length === 0 && (
              <tr>
                <td colSpan={5} className="px-2 py-3 text-center text-fg-muted">
                  No tool calls yet.
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </div>
  );
}
