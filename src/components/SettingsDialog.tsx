import { useCallback, useEffect, useState } from "react";
import { Dialog, Tabs } from "radix-ui";
import { X, RefreshCw, Trash2, Plus } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { applyTheme, loadTheme, type ThemeChoice } from "../lib/theme";
import * as ipc from "../lib/ipc";
import type { AppInfo } from "../lib/ipc";
import type {
  FirstRunInfo,
  Provider,
  ProviderKind,
  UpdateInfo,
  UpdateStatus,
} from "../lib/types";
import { APP_NAME } from "../lib/app";
import { toErrorInfo, useApp } from "../store/app";
import { ErrorBanner } from "./ErrorBanner";
import { AuditLogView, PermissionsSettings } from "./PermissionsSettings";
import { McpSettings } from "./McpSettings";
import { SkillsSettings } from "./SkillsSettings";
import { BrowserSettings } from "./BrowserSettings";
import { ComputerSettings } from "./ComputerSettings";
import { GitSettings } from "./GitSettings";
import { SearchSettings } from "./SearchSettings";
import { Benchmarks } from "./evals/Benchmarks";
import { ModelCheck } from "./evals/ModelCheck";
import { LocalModels } from "./settings/LocalModels";
import { OtherApps } from "./settings/OtherApps";
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
          {info
            ? `${info.name} ${info.version}${info.commit ? ` (build ${info.commit})` : ""} · ${info.os} ${info.arch}`
            : APP_NAME}
        </p>
        <p className="mt-1 text-xs text-fg-muted">
          Runs locally. No account, no telemetry. Licenses for bundled software are in
          THIRD-PARTY-NOTICES.md next to the app.
        </p>
      </section>
      {ipc.isTauri() && <StepsPerTurn />}
      {ipc.isTauri() && <DefaultSystemPrompt />}
      {ipc.isTauri() && <Updates />}
      {ipc.isTauri() && <ModelCatalog />}
    </div>
  );
}

const STEP_CHOICES = [25, 50, 100, 200];
const DEFAULT_STEPS = 50;

/** How many times the model may be called in one turn (setting "stepsPerTurn"). */
export function StepsPerTurn() {
  const [steps, setSteps] = useState<number>(DEFAULT_STEPS);
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    let alive = true;
    ipc
      .getSetting<unknown>("stepsPerTurn")
      .then((v) => alive && setSteps(typeof v === "number" ? v : DEFAULT_STEPS))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);
  async function choose(n: number) {
    const before = steps;
    setSteps(n);
    setError(null);
    try {
      await ipc.setSetting("stepsPerTurn", n);
    } catch (err) {
      setSteps(before);
      setError(toErrorInfo(err));
    }
  }
  // A value set some other way still shows as the current choice.
  const choices = STEP_CHOICES.includes(steps) ? STEP_CHOICES : [...STEP_CHOICES, steps].sort((a, b) => a - b);
  return (
    <fieldset>
      <legend className="mb-1 text-sm font-medium">Steps per turn</legend>
      <p className="mb-2 text-xs text-fg-muted">
        How many times the model may act before it stops and reports. A long task, such as installing
        software, needs more. When the limit is reached you can choose Continue.
      </p>
      <div className="flex gap-2" role="radiogroup" aria-label="Steps per turn">
        {choices.map((n) => (
          <button
            key={n}
            type="button"
            role="radio"
            aria-checked={steps === n}
            onClick={() => void choose(n)}
            className={
              "rounded-md border px-3 py-1.5 text-sm " +
              (steps === n ? "border-accent bg-accent text-accent-fg" : "border-border hover:bg-bg-muted")
            }
          >
            {n}
          </button>
        ))}
      </div>
      {error && (
        <div className="mt-2">
          <ErrorBanner error={error} onDismiss={() => setError(null)} />
        </div>
      )}
    </fieldset>
  );
}

/** The system prompt for chats without their own (setting "defaultSystemPrompt"). */
function DefaultSystemPrompt() {
  const [text, setText] = useState("");
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    let alive = true;
    ipc
      .getSetting<unknown>("defaultSystemPrompt")
      .then((v) => alive && setText(typeof v === "string" ? v : ""))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);
  async function save() {
    setError(null);
    try {
      // An empty box means no default of our own: the built-in prompt is used.
      await ipc.setSetting("defaultSystemPrompt", text.trim() ? text : null);
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (err) {
      setError(toErrorInfo(err));
    }
  }
  return (
    <section aria-label="Default system prompt">
      <label htmlFor="default-system-prompt" className="mb-1 block text-sm font-medium">
        Default system prompt
      </label>
      <p id="default-system-prompt-help" className="mb-2 text-xs text-fg-muted">
        Used by chats that have no system prompt of their own. Leave empty for the built-in one.
      </p>
      <textarea
        id="default-system-prompt"
        aria-describedby="default-system-prompt-help"
        value={text}
        onChange={(e) => setText(e.target.value)}
        rows={5}
        className="w-full rounded-md border border-border bg-bg-subtle p-2 font-mono text-xs"
      />
      <button
        type="button"
        onClick={() => void save()}
        className="mt-1 rounded-md border border-border px-3 py-1 text-sm hover:bg-bg-muted"
      >
        {saved ? "Saved" : "Save"}
      </button>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </section>
  );
}

function catalogInfo(i: FirstRunInfo) {
  return { sequence: i.catalogSequence, disabledReason: i.refreshDisabledReason };
}

/** The model catalog's version, and a signed refresh from the project's releases (11). */
function ModelCatalog() {
  const [info, setInfo] = useState<ReturnType<typeof catalogInfo> | null>(null);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    let alive = true;
    ipc
      .firstRunInfo()
      .then((i) => alive && setInfo(catalogInfo(i)))
      .catch((err) => alive && setError(toErrorInfo(err)));
    return () => {
      alive = false;
    };
  }, []);
  async function refresh() {
    setBusy(true);
    setError(null);
    setResult(null);
    const before = info?.sequence;
    try {
      const now = await ipc.refreshCatalog();
      setResult(now === before ? "The catalog is up to date." : `Updated to version ${now}.`);
      setInfo(catalogInfo(await ipc.firstRunInfo()));
    } catch (err) {
      setError(toErrorInfo(err));
    } finally {
      setBusy(false);
    }
  }
  return (
    <section aria-label="Model catalog">
      <h3 className="mb-1 text-sm font-medium">Model catalog</h3>
      <p className="mb-2 text-xs text-fg-muted">
        The list of suggested models. Refresh downloads a newer signed list from github.com.
      </p>
      <div className="flex flex-wrap items-center gap-2 text-sm">
        {info && <span>Catalog version {info.sequence}</span>}
        <button
          type="button"
          disabled={busy || !info || info.disabledReason !== null}
          aria-describedby={info?.disabledReason ? "catalog-refresh-reason" : undefined}
          onClick={() => void refresh()}
          className="flex items-center gap-1 rounded-md border border-border px-3 py-1 hover:bg-bg-muted disabled:opacity-40"
        >
          <RefreshCw size={12} aria-hidden className={busy ? "animate-spin" : ""} /> Refresh catalog
        </button>
        {result !== null && <span className="text-fg-muted">{result}</span>}
      </div>
      {info?.disabledReason && (
        <p id="catalog-refresh-reason" className="mt-1 text-xs text-fg-muted">
          {info.disabledReason}
        </p>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </section>
  );
}

/** Opt-in updates: nothing is checked unless the user asks or turns on automatic checks. */
function Updates() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [found, setFound] = useState<UpdateInfo | null | undefined>(undefined);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  useEffect(() => {
    let alive = true;
    let off: (() => void) | undefined;
    ipc
      .updateStatus()
      .then((s) => {
        if (!alive) return;
        setStatus(s);
        // Found by the check at start (or an earlier "Check now") since the app started.
        const available = s.available;
        if (available) setFound((cur) => cur ?? available);
      })
      .catch(() => alive && setStatus(null));
    void ipc.onUpdateAvailable(setFound).then((u) => {
      if (alive) off = u;
      else u();
    });
    return () => {
      alive = false;
      off?.();
    };
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
      <RemoteContext onError={setError} />
    </div>
  );
}

const REMOTE_CTX_OPTIONS = [2048, 4096, 8192, 16384, 32768];
const REMOTE_CTX_DEFAULT = 4096;

/** Context size for an endpoint whose model reports none (8.4; setting "remoteContextLength"). */
function RemoteContext(props: { onError: (e: ErrorInfo) => void }) {
  const [ctx, setCtx] = useState<number | null>(null);
  const { onError } = props;
  useEffect(() => {
    let alive = true;
    ipc
      .getSetting<unknown>("remoteContextLength")
      .then((v) => {
        if (!alive) return;
        // Missing or invalid means the default, as on the Rust side.
        setCtx(typeof v === "number" && Number.isInteger(v) && v > 0 ? v : REMOTE_CTX_DEFAULT);
      })
      .catch((err) => {
        if (!alive) return;
        setCtx(REMOTE_CTX_DEFAULT);
        onError(toErrorInfo(err));
      });
    return () => {
      alive = false;
    };
  }, [onError]);
  if (ctx === null) return null;
  const options = REMOTE_CTX_OPTIONS.includes(ctx)
    ? REMOTE_CTX_OPTIONS
    : [...REMOTE_CTX_OPTIONS, ctx].sort((a, b) => a - b);
  async function save(n: number) {
    const before = ctx;
    setCtx(n);
    try {
      await ipc.setSetting("remoteContextLength", n);
    } catch (err) {
      setCtx(before);
      onError(toErrorInfo(err));
    }
  }
  return (
    <section className="space-y-1 border-t border-border pt-4 text-sm">
      <label htmlFor="remote-ctx" className="block font-medium">
        Context size when a server does not report one
      </label>
      <p id="remote-ctx-help" className="text-xs text-fg-muted">
        Most servers say how much context their model uses. For one that does not, the app plans
        with this size. If it is larger than what the server really uses, older messages can be
        dropped without warning.
      </p>
      <select
        id="remote-ctx"
        aria-describedby="remote-ctx-help"
        value={ctx}
        onChange={(e) => void save(Number(e.target.value))}
        className="rounded-md border border-border bg-bg px-2 py-1"
      >
        {options.map((n) => (
          <option key={n} value={n}>
            {n.toLocaleString()} tokens{n === REMOTE_CTX_DEFAULT ? " (default)" : ""}
          </option>
        ))}
      </select>
    </section>
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
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => void run()}
          disabled={busy}
          className="rounded-md bg-accent px-3 py-1.5 text-sm text-accent-fg disabled:opacity-50"
        >
          {busy ? "Running… (this can take a few minutes)" : "Run backend check"}
        </button>
        <button
          type="button"
          onClick={() => void ipc.openLogsFolder().catch((err) => setError(toErrorInfo(err)))}
          className="rounded-md border border-border px-3 py-1.5 text-sm hover:bg-bg-muted"
        >
          Open logs folder
        </button>
      </div>
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
        <Dialog.Overlay className="dialog-overlay fixed inset-0" />
        <Dialog.Content className="fixed top-1/2 left-1/2 flex h-[34rem] w-[48rem] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface shadow-pop">
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
              <Tabs.Trigger value="otherApps" className={tabClass}>Other apps</Tabs.Trigger>
              <Tabs.Trigger value="endpoints" className={tabClass}>Endpoints</Tabs.Trigger>
              <Tabs.Trigger value="huggingface" className={tabClass}>Hugging Face</Tabs.Trigger>
              <Tabs.Trigger value="mcp" className={tabClass}>MCP servers</Tabs.Trigger>
              <Tabs.Trigger value="skills" className={tabClass}>Skills</Tabs.Trigger>
              <Tabs.Trigger value="git" className={tabClass}>Git</Tabs.Trigger>
              <Tabs.Trigger value="browser" className={tabClass}>Browser</Tabs.Trigger>
              <Tabs.Trigger value="computer" className={tabClass}>Computer use</Tabs.Trigger>
              <Tabs.Trigger value="search" className={tabClass}>Search</Tabs.Trigger>
              <Tabs.Trigger value="permissions" className={tabClass}>Permissions</Tabs.Trigger>
              <Tabs.Trigger value="audit" className={tabClass}>Audit log</Tabs.Trigger>
              <Tabs.Trigger value="diagnostics" className={tabClass}>Diagnostics</Tabs.Trigger>
            </Tabs.List>
            <div className="min-w-0 flex-1 overflow-y-auto p-5">
              <Tabs.Content value="general"><General /></Tabs.Content>
              <Tabs.Content value="runtime"><LocalModels /><ModelCheck /></Tabs.Content>
              <Tabs.Content value="otherApps"><OtherApps /></Tabs.Content>
              <Tabs.Content value="endpoints"><Endpoints /></Tabs.Content>
              <Tabs.Content value="huggingface"><HuggingFace /></Tabs.Content>
              <Tabs.Content value="mcp"><McpSettings /></Tabs.Content>
              <Tabs.Content value="skills"><SkillsSettings /></Tabs.Content>
              <Tabs.Content value="git"><GitSettings /></Tabs.Content>
              <Tabs.Content value="browser"><BrowserSettings /></Tabs.Content>
              <Tabs.Content value="computer"><ComputerSettings /></Tabs.Content>
              <Tabs.Content value="search"><SearchSettings /></Tabs.Content>
              <Tabs.Content value="permissions"><PermissionsSettings /></Tabs.Content>
              <Tabs.Content value="audit"><AuditLogView /></Tabs.Content>
              <Tabs.Content value="diagnostics"><Diagnostics /><Benchmarks /></Tabs.Content>
            </div>
          </Tabs.Root>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
