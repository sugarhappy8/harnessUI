import { useCallback, useEffect, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronRight, Plus, RotateCw, Trash2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import * as ipc from "../lib/ipc";
import type { McpImportPreview, McpServerDto, ToolDto } from "../lib/types";
import { toErrorInfo } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";
import type { ErrorInfo } from "../lib/types";

/** What to install when a server's runtime is missing (section 15). */
function missingRuntimeHint(reason: string): { label: string; url: string } | null {
  if (reason.includes("Node.js")) return { label: "Install Node.js", url: "https://nodejs.org/en/download" };
  if (reason.includes("uv (Python)")) return { label: "Install uv", url: "https://docs.astral.sh/uv/getting-started/installation/" };
  if (reason.includes("Docker")) return { label: "Install Docker Desktop", url: "https://www.docker.com/products/docker-desktop/" };
  return null;
}

function stateText(s: McpServerDto): { text: string; tone: string } {
  switch (s.state.type) {
    case "ready":
      return { text: "Running", tone: "text-success" };
    case "starting":
      return { text: "Starting…", tone: "text-fg-muted" };
    case "failed":
      return { text: "Failed", tone: "text-danger" };
    default:
      return { text: s.enabled ? "Stopped" : "Off", tone: "text-fg-muted" };
  }
}

/** One external tool's switches (D6, D10). */
function ToolRow(props: { tool: ToolDto; onChange: () => void; onError: (e: ErrorInfo) => void }) {
  const t = props.tool;
  async function save(enabled: boolean, allowAnyArgs: boolean, alwaysAsk: boolean) {
    try {
      await ipc.setMcpToolPref(t.server, t.original, enabled, allowAnyArgs, alwaysAsk);
      props.onChange();
    } catch (e) {
      props.onError(toErrorInfo(e));
    }
  }
  return (
    <li className="rounded-md border border-border p-2">
      <div className="flex items-start gap-2">
        <input
          type="checkbox"
          className="mt-1"
          aria-label={`Offer ${t.original} to the model`}
          checked={t.available}
          disabled={t.schemaError !== null}
          onChange={(e) => void save(e.target.checked, t.allowAnyArgs, t.alwaysAsk)}
        />
        <div className="min-w-0 flex-1">
          <div className="font-mono text-xs">
            {t.original}
            {t.destructive && <span className="ml-2 text-warning">destructive</span>}
            {t.changed && <span className="ml-2 text-warning">changed since you last reviewed it</span>}
          </div>
          <p className="text-xs whitespace-pre-wrap text-fg-muted">{t.description}</p>
          {t.schemaError && <p className="text-xs text-danger">Disabled: {t.schemaError}</p>}
          <div className="mt-1 flex gap-4 text-xs">
            <label className="flex items-center gap-1">
              <input
                type="checkbox"
                checked={t.allowAnyArgs}
                onChange={(e) => void save(t.available, e.target.checked, t.alwaysAsk)}
              />
              Allow any arguments when allowed for a chat
            </label>
            <label className="flex items-center gap-1">
              <input
                type="checkbox"
                checked={t.alwaysAsk}
                onChange={(e) => void save(t.available, t.allowAnyArgs, e.target.checked)}
              />
              Always ask
            </label>
          </div>
        </div>
        <span className="text-xs text-fg-muted">~{t.tokenEstimate} tokens</span>
      </div>
    </li>
  );
}

function ServerRow(props: { server: McpServerDto; onChange: () => void; onError: (e: ErrorInfo) => void }) {
  const s = props.server;
  const [open, setOpen] = useState(false);
  const [tools, setTools] = useState<ToolDto[]>([]);
  const st = stateText(s);
  const loadTools = useCallback(() => {
    if (!s.builtin) void ipc.listServerTools(s.name).then(setTools).catch(() => setTools([]));
  }, [s.builtin, s.name]);
  useEffect(() => {
    if (open) loadTools();
  }, [open, loadTools, s.toolCount]);

  const act = (f: () => Promise<void>) => () =>
    void f()
      .then(props.onChange)
      .catch((e) => props.onError(toErrorInfo(e)));

  return (
    <li className="rounded-lg border border-border">
      <div className="flex items-center gap-2 px-3 py-2">
        <button type="button" aria-expanded={open} aria-label="Show details" onClick={() => setOpen(!open)}>
          {open ? <ChevronDown size={14} aria-hidden /> : <ChevronRight size={14} aria-hidden />}
        </button>
        <div className="min-w-0 flex-1">
          <div className="font-medium">
            {s.name} {s.builtin && <span className="text-xs font-normal text-fg-muted">built in</span>}
          </div>
          <div className="truncate font-mono text-xs text-fg-muted" title={s.summary}>
            {s.summary}
          </div>
        </div>
        <span className={`text-xs ${st.tone}`}>{st.text}</span>
        <span className="text-xs text-fg-muted">{s.toolCount} tools</span>
        {!s.builtin && (
          <>
            <label className="flex items-center gap-1 text-xs">
              <input
                type="checkbox"
                checked={s.enabled}
                onChange={(e) => act(() => ipc.setMcpServerEnabled(s.name, e.target.checked))()}
              />
              On
            </label>
            <button type="button" title="Restart" aria-label="Restart" onClick={act(() => ipc.restartMcpServer(s.name))}>
              <RotateCw size={14} aria-hidden />
            </button>
            <button
              type="button"
              title="Remove"
              aria-label="Remove"
              onClick={act(async () => {
                if (window.confirm(`Remove ${s.name}? Its saved secrets are deleted from the keychain.`))
                  await ipc.removeMcpServer(s.name);
              })}
            >
              <Trash2 size={14} aria-hidden />
            </button>
          </>
        )}
      </div>
      {open && (
        <div className="space-y-2 border-t border-border px-3 py-2 text-sm">
          {s.state.type === "failed" && (
            <div className="space-y-1">
              <p className="text-danger whitespace-pre-wrap">{s.state.reason}</p>
              {(() => {
                const hint = missingRuntimeHint(s.state.reason);
                return hint ? (
                  <p className="text-xs">
                    <button type="button" className="text-accent underline" onClick={() => void openUrl(hint.url)}>
                      {hint.label}
                    </button>
                    , then restart the server.
                  </p>
                ) : null;
              })()}
            </div>
          )}
          {s.protocolVersion && <p className="text-xs text-fg-muted">Protocol {s.protocolVersion}</p>}
          {!s.builtin && (
            <>
              <p className="text-xs text-fg-muted">
                Tools of a new server start off. Turn on the ones you want offered to the model; each chat can still
                switch them off.
              </p>
              <ul className="space-y-1">
                {tools.map((t) => (
                  <ToolRow key={t.exposed} tool={t} onChange={loadTools} onError={props.onError} />
                ))}
              </ul>
            </>
          )}
          {s.stderrTail.length > 0 && (
            <details>
              <summary className="cursor-pointer text-xs text-fg-muted">Server log (last lines)</summary>
              <pre className="max-h-48 overflow-auto rounded bg-bg p-2 font-mono text-xs whitespace-pre-wrap">
                {s.stderrTail.join("\n")}
              </pre>
            </details>
          )}
        </div>
      )}
    </li>
  );
}

/** Pasting a config: preview, the "runs as you" warning, then add (9.2, 13.4). */
function AddServers(props: { onDone: () => void; onError: (e: ErrorInfo) => void }) {
  const [json, setJson] = useState("");
  const [preview, setPreview] = useState<McpImportPreview[] | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [moveCredentials, setMoveCredentials] = useState(true);
  const [ack, setAck] = useState(false);

  async function check() {
    try {
      const p = await ipc.previewMcpConfig(json);
      setPreview(p);
      setChosen(new Set(p.filter((x) => !x.exists).map((x) => x.name)));
      setAck(false);
    } catch (e) {
      props.onError(toErrorInfo(e));
    }
  }

  async function add() {
    try {
      await ipc.addMcpServers(json, [...chosen], moveCredentials);
      setJson("");
      setPreview(null);
      props.onDone();
    } catch (e) {
      props.onError(toErrorInfo(e));
    }
  }

  return (
    <div className="space-y-2 rounded-lg border border-border p-3">
      <label className="block text-sm font-medium" htmlFor="mcp-json">
        Paste a configuration (mcpServers or servers JSON, as used by Claude Desktop, Cursor or VS Code)
      </label>
      <textarea
        id="mcp-json"
        value={json}
        onChange={(e) => (setJson(e.target.value), setPreview(null))}
        rows={6}
        spellCheck={false}
        className="w-full rounded-md border border-border bg-bg p-2 font-mono text-xs"
        placeholder='{ "mcpServers": { "example": { "command": "npx", "args": ["-y", "some-mcp-server"] } } }'
      />
      {!preview && (
        <button
          type="button"
          disabled={!json.trim()}
          onClick={() => void check()}
          className="rounded-md bg-accent px-3 py-1 text-sm text-accent-fg disabled:opacity-40"
        >
          Check
        </button>
      )}
      {preview && (
        <div className="space-y-2">
          <ul className="space-y-1">
            {preview.map((p) => (
              <li key={p.name} className="rounded-md border border-border p-2 text-sm">
                <label className="flex items-start gap-2">
                  <input
                    type="checkbox"
                    className="mt-1"
                    disabled={p.exists}
                    checked={chosen.has(p.name)}
                    onChange={(e) => {
                      const next = new Set(chosen);
                      if (e.target.checked) next.add(p.name);
                      else next.delete(p.name);
                      setChosen(next);
                    }}
                  />
                  <span className="min-w-0 flex-1">
                    <span className="font-medium">{p.name}</span>{" "}
                    <span className="text-xs text-fg-muted">{p.kind}</span>
                    {p.exists && <span className="ml-2 text-xs text-warning">a server with this name exists</span>}
                    <span className="block font-mono text-xs break-all text-fg-muted">{p.summary}</span>
                    {p.findings.length > 0 && (
                      <span className="block text-xs text-warning">
                        Looks like credentials: {p.findings.map((f) => f.what).join("; ")}
                      </span>
                    )}
                    {(p.envKeys.length > 0 || p.headerKeys.length > 0) && (
                      <span className="block text-xs text-fg-muted">
                        Saved in the system keychain: {[...p.envKeys, ...p.headerKeys].join(", ")}
                      </span>
                    )}
                  </span>
                </label>
              </li>
            ))}
          </ul>
          {preview.some((p) => p.findings.length > 0) && (
            <label className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={moveCredentials} onChange={(e) => setMoveCredentials(e.target.checked)} />
              Move the credentials found above to the system keychain (recommended)
            </label>
          )}
          <div className="flex items-start gap-2 rounded-md border border-warning/60 p-2 text-sm">
            <AlertTriangle size={16} className="mt-0.5 shrink-0 text-warning" aria-hidden />
            <label className="flex items-start gap-2">
              <input type="checkbox" className="mt-1" checked={ack} onChange={(e) => setAck(e.target.checked)} />
              <span>
                A local server runs as a program on this computer with your permissions: it can read and change your
                files and use your network, whatever the model asks. A remote server receives what the model sends it.
                Only add servers you trust.
              </span>
            </label>
          </div>
          <div className="flex gap-2">
            <button
              type="button"
              disabled={!ack || chosen.size === 0}
              onClick={() => void add()}
              className="rounded-md bg-accent px-3 py-1 text-sm text-accent-fg disabled:opacity-40"
            >
              Add {chosen.size === 1 ? "server" : `${chosen.size} servers`}
            </button>
            <button type="button" onClick={() => setPreview(null)} className="px-3 py-1 text-sm">
              Back
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

export function McpSettings() {
  const [servers, setServers] = useState<McpServerDto[]>([]);
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  const refresh = useCallback(() => {
    void ipc
      .listMcpServers()
      .then(setServers)
      .catch((e) => setError(toErrorInfo(e)));
  }, []);

  useEffect(() => {
    refresh();
    let un: (() => void) | undefined;
    void ipc.onMcpChanged(refresh).then((u) => (un = u));
    return () => un?.();
  }, [refresh]);

  return (
    <div className="space-y-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">MCP servers</h2>
        {!adding && (
          <button
            type="button"
            onClick={() => setAdding(true)}
            className="flex items-center gap-1 rounded-md border border-border px-2 py-1 text-sm"
          >
            <Plus size={14} aria-hidden /> Add servers
          </button>
        )}
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {adding && (
        <AddServers
          onDone={() => {
            setAdding(false);
            refresh();
          }}
          onError={setError}
        />
      )}
      <ul className="space-y-2">
        {servers.map((s) => (
          <ServerRow key={s.name} server={s} onChange={refresh} onError={setError} />
        ))}
      </ul>
    </div>
  );
}
