import { useEffect, useState } from "react";
import { FolderPlus, Trash2 } from "lucide-react";
import * as ipc from "../lib/ipc";
import type { ApprovedFolder, AuditRow, ErrorInfo, HotkeyStatus } from "../lib/types";
import { toErrorInfo } from "../store/app";
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
      <section>
        <h3 className="mb-1 font-medium">Folders tools may use in every chat</h3>
        <p className="mb-2 text-xs text-fg-muted">
          File tools can only read and write inside approved folders. You can also approve a folder
          for a single chat from that chat's permissions. The app's own data and program folders are
          never reachable.
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
    </div>
  );
}

const DECISIONS = [
  ["", "Any decision"],
  ["allow_once", "Allowed once"],
  ["allow_for_chat", "Allowed for the chat"],
  ["allowed_by_grant", "Allowed by an earlier grant"],
  ["allowed_without_prompt", "Allowed without asking"],
  ["denied", "Denied"],
  ["cancelled", "Cancelled"],
  ["refused_by_policy", "Refused by policy"],
] as const;

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
                  {r.decision.replaceAll("_", " ")}
                  {r.note ? ` (“${r.note}”)` : ""}
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
