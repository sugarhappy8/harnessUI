import { useCallback, useEffect, useState } from "react";
import { Dialog, Tabs } from "radix-ui";
import { X, RefreshCw, Trash2, Plus } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { applyTheme, loadTheme, type ThemeChoice } from "../lib/theme";
import * as ipc from "../lib/ipc";
import type { AppInfo } from "../lib/ipc";
import type { BackendInfo, LocalModelSettings, Provider, ProviderKind, UpdateInfo, UpdateStatus } from "../lib/types";
import { APP_NAME } from "../lib/app";
import { toErrorInfo, useApp } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";
import { BackendDownloads } from "./BackendDownloads";
import { AuditLogView, PermissionsSettings } from "./PermissionsSettings";
import { McpSettings } from "./McpSettings";
import { SkillsSettings } from "./SkillsSettings";
import { BrowserSettings } from "./BrowserSettings";
import { ComputerSettings } from "./ComputerSettings";
import type { ErrorInfo } from "../lib/types";

const THEMES: { value: ThemeChoice; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

const tabClass =
  "rounded-md px-3 py-1.5 text-left text-sm data-[state=active]:bg-bg-muted data-[state=active]:font-medium hover:bg-bg-muted";

function General() {
  const [theme, setTheme] = useState<ThemeChoice>(loadTheme);
  const [info, setInfo] = useState<AppInfo | null>(null);
  useEffect(() => {
    if (ipc.isTauri()) ipc.appInfo().then(setInfo).catch(() => setInfo(null));
  }, []);
  return (
    <div className="space-y-5">
      <fieldset>
        <legend className="mb-2 text-sm font-medium">Theme</legend>
        <div className="flex gap-2" role="radiogroup" aria-label="Theme">
          {THEMES.map((t) => (
            <button
              key={t.value}
              type="button"
              role="radio"
              aria-checked={theme === t.value}
              onClick={() => {
                setTheme(t.value);
                applyTheme(t.value);
              }}
              className={
                "rounded-md border px-3 py-1.5 text-sm " +
                (theme === t.value
                  ? "border-accent bg-accent text-accent-fg"
                  : "border-border hover:bg-bg-muted")
              }
            >
              {t.label}
            </button>
          ))}
        </div>
      </fieldset>
      <section aria-label="About">
        <h3 className="mb-1 text-sm font-medium">About</h3>
        <p className="text-sm text-fg-muted">
          {info ? `${info.name} ${info.version} · ${info.os} ${info.arch}` : APP_NAME}
        </p>
        <p className="mt-1 text-xs text-fg-muted">
          Runs locally. No account, no telemetry. Licenses for bundled software are in
          THIRD-PARTY-NOTICES.md next to the app.
        </p>
      </section>
      {ipc.isTauri() && <Updates />}
    </div>
  );
}

/** Opt-in updates: nothing is checked unless the user asks or turns on automatic checks. */
function Updates() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [found, setFound] = useState<UpdateInfo | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    ipc.updateStatus().then(setStatus).catch(() => setStatus(null));
  }, []);
  if (!status) return null;
  async function run<T>(f: () => Promise<T>, then: (v: T) => void) {
    setBusy(true);
    setError(null);
    try {
      then(await f());
    } catch (e) {
      setError(toErrorInfo(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section aria-label="Updates">
      <h3 className="mb-1 text-sm font-medium">Updates</h3>
      {!status.configured ? (
        <p className="text-sm text-fg-muted">
          This build cannot update itself. New versions are on the project&apos;s GitHub releases page.
        </p>
      ) : (
        <div className="space-y-2 text-sm">
          <label className="flex items-center gap-2">
            <input
              type="checkbox"
              checked={status.autoCheck}
              onChange={(e) =>
                void run(
                  () => ipc.setUpdateAutoCheck(e.target.checked),
                  () => setStatus({ ...status, autoCheck: e.target.checked }),
                )
              }
            />
            Check for updates when the app starts (off by default; contacts github.com)
          </label>
          <div className="flex items-center gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() => void run(() => ipc.checkForUpdate(), setFound)}
              className="rounded-md border border-border px-3 py-1 disabled:opacity-40"
            >
              Check now
            </button>
            {found === null && <span className="text-fg-muted">You have the latest version ({status.currentVersion}).</span>}
            {found && (
              <>
                <span>Version {found.version} is available.</span>
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void run(() => ipc.installUpdate(), () => undefined)}
                  className="rounded-md bg-accent px-3 py-1 text-accent-fg disabled:opacity-40"
                >
                  Install and restart
                </button>
              </>
            )}
          </div>
          {found?.notes && <p className="whitespace-pre-wrap text-xs text-fg-muted">{found.notes}</p>}
        </div>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </section>
  );
}

function Runtime() {
  const [backends, setBackends] = useState<BackendInfo[] | null>(null);
  const [settings, setSettings] = useState<LocalModelSettings | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  const load = useCallback(async (reprobe: boolean) => {
    setBusy(true);
    try {
      setBackends(await ipc.listBackends(reprobe));
      setSettings(await ipc.getLocalModelSettings());
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }, []);
  const reprobe = useCallback(() => void load(true), [load]);
  useEffect(() => {
    let alive = true;
    Promise.all([ipc.listBackends(false), ipc.getLocalModelSettings()])
      .then(([b, s]) => {
        if (alive) {
          setBackends(b);
          setSettings(s);
        }
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function save(next: LocalModelSettings) {
    setSettings(next);
    try {
      await ipc.setLocalModelSettings(next);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  return (
    <div className="space-y-5">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      {settings && (
        <div className="grid grid-cols-[10rem_1fr] items-center gap-3 text-sm">
          <label htmlFor="backend">Backend</label>
          <select
            id="backend"
            value={settings.backend ?? "auto"}
            onChange={(e) =>
              void save({ ...settings, backend: e.target.value === "auto" ? null : e.target.value })
            }
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            <option value="auto">Auto (recommended)</option>
            {(backends ?? []).map((b) => (
              <option key={b.kind} value={b.kind} disabled={!b.available}>
                {b.kind}
                {b.available ? "" : " (unavailable)"}
              </option>
            ))}
          </select>
          <label htmlFor="ctx">Context size</label>
          <select
            id="ctx"
            value={settings.ctx}
            onChange={(e) => void save({ ...settings, ctx: Number(e.target.value) })}
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            {[2048, 4096, 8192, 16384, 32768, 65536].map((n) => (
              <option key={n} value={n}>
                {n.toLocaleString()} tokens
              </option>
            ))}
          </select>
        </div>
      )}
      <p className="text-xs text-fg-muted">
        Changes apply the next time a model loads. A larger context needs more memory.
      </p>
      <section>
        <div className="mb-2 flex items-center justify-between">
          <h3 className="text-sm font-medium">Detected backends</h3>
          <button
            type="button"
            onClick={() => void load(true)}
            disabled={busy}
            className="flex items-center gap-1 text-xs text-fg-muted hover:text-fg disabled:opacity-40"
          >
            <RefreshCw size={12} aria-hidden className={busy ? "animate-spin" : ""} /> Check again
          </button>
        </div>
        <ul className="space-y-2 text-sm">
          {(backends ?? []).map((b) => (
            <li key={b.kind} className="rounded-md border border-border px-3 py-2">
              <div className="flex justify-between">
                <span className="font-medium">{b.kind}</span>
                <span className={b.available ? "text-success" : "text-fg-muted"}>
                  {b.available ? "available" : "unavailable"}
                </span>
              </div>
              {b.devices.map((d) => (
                <p key={d.name} className="text-xs text-fg-muted">
                  {d.name}: {d.description}
                  {d.totalMib > 0 ? ` · ${(d.totalMib / 1024).toFixed(1)} GB` : ""}
                </p>
              ))}
              {b.error && <p className="text-xs text-fg-muted">{b.error}</p>}
            </li>
          ))}
          {backends?.length === 0 && (
            <li className="text-fg-muted">No llama.cpp runtime was found in this build.</li>
          )}
        </ul>
      </section>
      <BackendDownloads onChanged={reprobe} />
    </div>
  );
}

const KINDS: { value: ProviderKind; label: string; example: string }[] = [
  { value: "ollama", label: "Ollama", example: "http://127.0.0.1:11434" },
  { value: "lmstudio", label: "LM Studio", example: "http://127.0.0.1:1234" },
  { value: "vllm", label: "vLLM", example: "http://127.0.0.1:8000" },
  { value: "generic", label: "Other OpenAI-compatible", example: "https://example.com/v1" },
];

function Endpoints() {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [form, setForm] = useState<{ name: string; kind: ProviderKind; baseUrl: string; apiKey: string } | null>(null);
  const [status, setStatus] = useState<Record<number, string>>({});
  const [error, setError] = useState<ErrorInfo | null>(null);
  const refreshModelOptions = useApp((s) => s.refreshModelOptions);

  const load = useCallback(async () => {
    try {
      setProviders(await ipc.listProviders());
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }, []);
  useEffect(() => {
    let alive = true;
    ipc
      .listProviders()
      .then((p) => alive && setProviders(p))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function save() {
    if (!form) return;
    try {
      await ipc.saveProvider({
        id: null,
        name: form.name.trim() || KINDS.find((k) => k.value === form.kind)!.label,
        kind: form.kind,
        baseUrl: form.baseUrl.trim(),
        apiKey: form.apiKey.trim() || null,
        clearApiKey: false,
      });
      setForm(null);
      await load();
      await refreshModelOptions();
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  async function test(id: number) {
    setStatus((s) => ({ ...s, [id]: "Checking…" }));
    try {
      const models = await ipc.testProvider(id);
      setStatus((s) => ({ ...s, [id]: `Connected · ${models.length} model(s)` }));
      await refreshModelOptions();
    } catch (err) {
      setStatus((s) => ({ ...s, [id]: toErrorInfo(err).message }));
    }
  }

  return (
    <div className="space-y-4">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <p className="text-sm text-fg-muted">
        Connect Ollama, LM Studio, vLLM or any OpenAI-compatible server. API keys are kept in
        your system keychain.
      </p>
      <ul className="space-y-2">
        {providers.map((p) => (
          <li key={p.id} className="flex items-center gap-2 rounded-md border border-border px-3 py-2 text-sm">
            <div className="min-w-0 flex-1">
              <p className="font-medium">{p.name}</p>
              <p className="truncate text-xs text-fg-muted">
                {p.baseUrl}
                {p.hasApiKey ? " · API key saved" : ""}
              </p>
              {status[p.id] && <p className="text-xs">{status[p.id]}</p>}
            </div>
            <button type="button" className="text-xs underline" onClick={() => void test(p.id)}>
              Test
            </button>
            <button
              type="button"
              aria-label={`Remove ${p.name}`}
              onClick={() => void ipc.deleteProvider(p.id).then(load).then(refreshModelOptions)}
              className="text-fg-muted hover:text-danger"
            >
              <Trash2 size={14} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
      {form ? (
        <form
          className="grid grid-cols-[7rem_1fr] items-center gap-2 rounded-md border border-border p-3 text-sm"
          onSubmit={(e) => {
            e.preventDefault();
            void save();
          }}
        >
          <label htmlFor="p-kind">Type</label>
          <select
            id="p-kind"
            value={form.kind}
            onChange={(e) => {
              const kind = e.target.value as ProviderKind;
              setForm({ ...form, kind, baseUrl: KINDS.find((k) => k.value === kind)!.example });
            }}
            className="rounded-md border border-border bg-bg px-2 py-1"
          >
            {KINDS.map((k) => (
              <option key={k.value} value={k.value}>
                {k.label}
              </option>
            ))}
          </select>
          <label htmlFor="p-name">Name</label>
          <input id="p-name" value={form.name} onChange={(e) => setForm({ ...form, name: e.target.value })} className="rounded-md border border-border bg-bg px-2 py-1" />
          <label htmlFor="p-url">Base URL</label>
          <input id="p-url" required value={form.baseUrl} onChange={(e) => setForm({ ...form, baseUrl: e.target.value })} className="rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs" />
          <label htmlFor="p-key">API key</label>
          <input id="p-key" type="password" autoComplete="off" value={form.apiKey} placeholder="optional" onChange={(e) => setForm({ ...form, apiKey: e.target.value })} className="rounded-md border border-border bg-bg px-2 py-1" />
          <div className="col-span-2 flex justify-end gap-2">
            <button type="button" onClick={() => setForm(null)} className="px-2 py-1">
              Cancel
            </button>
            <button type="submit" className="rounded-md bg-accent px-3 py-1 text-accent-fg">
              Save
            </button>
          </div>
        </form>
      ) : (
        <button
          type="button"
          onClick={() => setForm({ name: "", kind: "ollama", baseUrl: KINDS[0]!.example, apiKey: "" })}
          className="flex items-center gap-1 text-sm text-accent"
        >
          <Plus size={14} aria-hidden /> Add an endpoint
        </button>
      )}
    </div>
  );
}

function HuggingFace() {
  const [saved, setSaved] = useState<boolean | null>(null);
  const [token, setToken] = useState("");
  const [account, setAccount] = useState<string | null>(null);
  const [mirror, setMirror] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  useEffect(() => {
    let alive = true;
    Promise.all([ipc.hubTokenStatus(), ipc.getHfEndpoint()])
      .then(([t, m]) => {
        if (!alive) return;
        setSaved(t.saved);
        setMirror(m ?? "");
      })
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);

  async function saveToken() {
    setBusy(true);
    setError(null);
    try {
      setAccount(await ipc.hubSetToken(token));
      setSaved(true);
      setToken("");
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }

  async function clearToken() {
    await ipc.hubClearToken();
    setSaved(false);
    setAccount(null);
  }

  async function saveMirror() {
    try {
      await ipc.setHfEndpoint(mirror.trim() || null);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }

  return (
    <div className="space-y-5 text-sm">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <section>
        <h3 className="mb-1 font-medium">Hugging Face token</h3>
        <p className="mb-2 text-xs text-fg-muted">
          Only needed for gated models. It is checked with Hugging Face, kept in your system
          keychain, and sent only to huggingface.co.
        </p>
        {saved ? (
          <div className="flex items-center gap-3">
            <span>{account ? `Saved (signed in as ${account})` : "A token is saved."}</span>
            <button type="button" className="text-xs underline" onClick={() => void clearToken()}>
              Remove
            </button>
          </div>
        ) : (
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              void saveToken();
            }}
          >
            <label className="sr-only" htmlFor="hf-token">
              Hugging Face token
            </label>
            <input
              id="hf-token"
              type="password"
              autoComplete="off"
              value={token}
              placeholder="hf_…"
              onChange={(e) => setToken(e.target.value)}
              className="flex-1 rounded-md border border-border bg-bg px-2 py-1"
            />
            <button
              type="submit"
              disabled={busy || !token.trim()}
              className="rounded-md bg-accent px-3 py-1 text-accent-fg disabled:opacity-40"
            >
              {busy ? "Checking…" : "Save"}
            </button>
          </form>
        )}
      </section>
      <section>
        <h3 className="mb-1 font-medium">Mirror (advanced)</h3>
        <p className="mb-2 text-xs text-fg-muted">
          For networks where huggingface.co is unreachable. Must be https. The token is never
          sent to a mirror, so gated models need the official site.
        </p>
        <div className="flex gap-2">
          <label className="sr-only" htmlFor="hf-mirror">
            Mirror address
          </label>
          <input
            id="hf-mirror"
            value={mirror}
            placeholder="https://huggingface.co (default)"
            onChange={(e) => setMirror(e.target.value)}
            className="flex-1 rounded-md border border-border bg-bg px-2 py-1 font-mono text-xs"
          />
          <button type="button" onClick={() => void saveMirror()} className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted">
            Save
          </button>
        </div>
      </section>
    </div>
  );
}

function Diagnostics() {
  const [report, setReport] = useState<string | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const runtime = useApp((s) => s.runtime);

  async function run() {
    setBusy(true);
    setError(null);
    try {
      const r = await ipc.runDiagnostics(runtime?.modelPath ?? null);
      setReport(r.text);
      setSavedTo(r.savedTo);
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="space-y-3">
      <p className="text-sm text-fg-muted">
        Checks every llama.cpp backend on this computer, lists the devices it finds and, if a
        model is loaded, times a short generation on each usable backend. Nothing is sent
        anywhere; the report is saved to the logs folder.
      </p>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <button
        type="button"
        onClick={() => void run()}
        disabled={busy}
        className="rounded-md bg-accent px-3 py-1.5 text-sm text-accent-fg disabled:opacity-50"
      >
        {busy ? "Running… (this can take a few minutes)" : "Run backend check"}
      </button>
      {report && (
        <>
          <pre className="max-h-72 overflow-auto rounded-md bg-bg-muted p-3 text-xs whitespace-pre-wrap">{report}</pre>
          <div className="flex gap-3 text-xs">
            <button type="button" className="underline" onClick={() => void writeText(report)}>
              Copy report
            </button>
            {savedTo && <span className="text-fg-muted">Saved to {savedTo}</span>}
          </div>
        </>
      )}
    </div>
  );
}

export function SettingsDialog(props: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialTab?: string;
}) {
  return (
    <Dialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 bg-black/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 flex h-[34rem] w-[48rem] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-lg border border-border bg-bg shadow-xl">
          <div className="flex items-center justify-between border-b border-border px-5 py-3">
            <Dialog.Title className="text-base font-semibold">Settings</Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg" aria-label="Close settings">
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">App settings</Dialog.Description>
          <Tabs.Root key={props.initialTab ?? "general"} defaultValue={props.initialTab ?? "general"} orientation="vertical" className="flex min-h-0 flex-1">
            <Tabs.List className="flex w-44 flex-col gap-1 border-r border-border p-3" aria-label="Settings sections">
              <Tabs.Trigger value="general" className={tabClass}>General</Tabs.Trigger>
              <Tabs.Trigger value="runtime" className={tabClass}>Local models</Tabs.Trigger>
              <Tabs.Trigger value="endpoints" className={tabClass}>Endpoints</Tabs.Trigger>
              <Tabs.Trigger value="huggingface" className={tabClass}>Hugging Face</Tabs.Trigger>
              <Tabs.Trigger value="mcp" className={tabClass}>MCP servers</Tabs.Trigger>
              <Tabs.Trigger value="skills" className={tabClass}>Skills</Tabs.Trigger>
              <Tabs.Trigger value="browser" className={tabClass}>Browser</Tabs.Trigger>
              <Tabs.Trigger value="computer" className={tabClass}>Computer use</Tabs.Trigger>
              <Tabs.Trigger value="permissions" className={tabClass}>Permissions</Tabs.Trigger>
              <Tabs.Trigger value="audit" className={tabClass}>Audit log</Tabs.Trigger>
              <Tabs.Trigger value="diagnostics" className={tabClass}>Diagnostics</Tabs.Trigger>
            </Tabs.List>
            <div className="min-w-0 flex-1 overflow-y-auto p-5">
              <Tabs.Content value="general"><General /></Tabs.Content>
              <Tabs.Content value="runtime"><Runtime /></Tabs.Content>
              <Tabs.Content value="endpoints"><Endpoints /></Tabs.Content>
              <Tabs.Content value="huggingface"><HuggingFace /></Tabs.Content>
              <Tabs.Content value="mcp"><McpSettings /></Tabs.Content>
              <Tabs.Content value="skills"><SkillsSettings /></Tabs.Content>
              <Tabs.Content value="browser"><BrowserSettings /></Tabs.Content>
              <Tabs.Content value="computer"><ComputerSettings /></Tabs.Content>
              <Tabs.Content value="permissions"><PermissionsSettings /></Tabs.Content>
              <Tabs.Content value="audit"><AuditLogView /></Tabs.Content>
              <Tabs.Content value="diagnostics"><Diagnostics /></Tabs.Content>
            </div>
          </Tabs.Root>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
