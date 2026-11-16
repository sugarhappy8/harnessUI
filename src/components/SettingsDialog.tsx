import { useEffect, useState } from "react";
import { Dialog, Tabs } from "radix-ui";
import { X, RefreshCw } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { applyTheme, loadTheme, type ThemeChoice } from "../lib/theme";
import * as ipc from "../lib/ipc";
import type { AppInfo } from "../lib/ipc";
import type { FirstRunInfo, UpdateStatus } from "../lib/types";
import { APP_NAME } from "../lib/app";
import { toErrorInfo, turnChatId, useApp } from "../store/app";
import { listenInEffect } from "../lib/listen";
import { askBeforeChange, backendWhenIdle, runChange } from "../store/modelChange";
import { ErrorBanner } from "./ErrorBanner";
import { visibleLines, visiblePath } from "./FolderChip";
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
import { Reverse } from "./settings/Reverse";
import { Endpoints } from "./settings/Endpoints";
import { SameCallLimit } from "./settings/SameCallLimit";
import type { DiagnosticsReport, ErrorInfo } from "../lib/types";

const THEMES: { value: ThemeChoice; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

const tabClass =
  "rounded-md px-3 py-1.5 text-left text-sm data-[state=active]:bg-bg-muted data-[state=active]:font-medium hover:bg-bg-muted";

/** The section list's ref: when the list appears (the dialog opens, or opens on another
 * section), it scrolls so the chosen section's row is in view, centred. In a short window the
 * list shows only part of itself, and a link elsewhere (the notice's Permissions link, the
 * Tools panel's Browser link) opens Settings on a section further down. Layout offsets, not
 * screen rectangles, because the dialog is scaled while it opens. */
function showChosenSection(list: HTMLDivElement | null) {
  const tab = list?.querySelector<HTMLElement>('[role="tab"][data-state="active"]');
  if (!list || !tab) return;
  const top = tab.offsetParent === list ? tab.offsetTop : tab.offsetTop - list.offsetTop - list.clientTop;
  if (top < list.scrollTop || top + tab.offsetHeight > list.scrollTop + list.clientHeight) {
    list.scrollTop = top - (list.clientHeight - tab.offsetHeight) / 2;
  }
}

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
      {ipc.isTauri() && <SameCallLimit />}
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

/** The line under "Check now": what the updater is doing or found since the app started. */
function updateLine(s: UpdateStatus): string | null {
  if (s.ready) return `Version ${s.ready.version} is ready to install.`;
  if (s.downloading) return `Downloading version ${s.downloading}…`;
  if (s.checking) return "Checking for updates…";
  // Only the automatic check tries again on its own.
  if (s.error) return `${s.error} ${s.autoCheck ? "The app tries again at the next check." : "Click Check now to try again."}`;
  if (s.available) return `Version ${s.available.version} is available.`;
  if (s.checked) return `You have the latest version (${s.currentVersion}).`;
  return null;
}

/**
 * Updates (0.0.8): the app checks github.com on its own, at start and every six hours, unless
 * the user turns that off here. A newer version is downloaded and its signature checked in the
 * background; then this section and the small "Update ready" box offer "Restart and update".
 * Nothing is installed without that click.
 */
export function Updates() {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const replying = useApp((s) => turnChatId(s) !== null);
  useEffect(() => {
    let alive = true;
    let loaded = false;
    // A change that arrives before the status does cannot be applied yet, and the status may
    // have been read just before it: then read the status again, after the change.
    let missed = false;
    const read = () =>
      ipc
        .updateStatus()
        .then((s) => {
          if (!alive) return;
          loaded = true;
          setStatus(s);
          if (missed) {
            missed = false;
            void read();
          }
        })
        .catch(() => {
          if (alive && !loaded) setStatus(null);
        });
    // Checking, downloading, ready or failed, also when the automatic check does it.
    const stop = listenInEffect(() =>
      ipc.onUpdateState((state) => {
        if (!alive) return;
        if (!loaded) missed = true;
        else setStatus((cur) => (cur ? { ...cur, ...state } : cur));
      }),
    );
    void read();
    return () => {
      alive = false;
      stop();
    };
  }, []);
  if (!status) return null;
  async function run(f: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await f();
    } catch (e) {
      setError(toErrorInfo(e));
    } finally {
      setBusy(false);
    }
  }
  async function checkNow() {
    await run(async () => {
      try {
        await ipc.checkForUpdate();
      } catch (e) {
        // A check that did not work is in the status line below; say it only once.
        if (toErrorInfo(e).code !== "update_check") throw e;
      }
      // What the check found, and whether a download started (it may already be running).
      const s = await ipc.updateStatus();
      setStatus(s);
    });
  }
  async function install() {
    setInstalling(true);
    setError(null);
    try {
      // On success the app restarts and this never returns.
      await ipc.installUpdate();
    } catch (e) {
      setError(toErrorInfo(e));
    } finally {
      setInstalling(false);
    }
  }
  const line = updateLine(status);
  // The notes of the version that is ready, or else of the one found.
  const shown = status.ready ?? status.available;
  const notes = shown?.notes?.trim() ? shown.notes : null;
  const working = busy || status.checking || status.downloading !== null;
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
              onChange={(e) => {
                const on = e.target.checked;
                void run(async () => {
                  await ipc.setUpdateAutoCheck(on);
                  setStatus((cur) => (cur ? { ...cur, autoCheck: on } : cur));
                });
              }}
            />
            Check for updates automatically (contacts github.com)
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              disabled={working || installing}
              onClick={() => void checkNow()}
              className="rounded-md border border-border px-3 py-1 disabled:opacity-40"
            >
              Check now
            </button>
            {line && <span className={status.ready ? undefined : "text-fg-muted"}>{line}</span>}
            {status.ready && (
              <button
                type="button"
                disabled={installing || busy}
                onClick={() => void install()}
                className="rounded-md bg-accent px-3 py-1 text-accent-fg disabled:opacity-40"
              >
                {installing ? "Updating…" : "Restart and update"}
              </button>
            )}
          </div>
          {status.ready && replying && <p className="text-xs text-fg-muted">Updating stops the current reply.</p>}
          {notes && (
            <div
              role="region"
              aria-label="Release notes"
              tabIndex={0}
              className="max-h-40 overflow-y-auto text-xs break-words whitespace-pre-wrap text-fg-muted"
            >
              {notes}
            </div>
          )}
        </div>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
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

export function Diagnostics() {
  const [report, setReport] = useState<string | null>(null);
  const [savedTo, setSavedTo] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const runtime = useApp((s) => s.runtime);

  async function run() {
    setError(null);
    // The timed generation replaces the loaded model: a reply, a model check or another
    // app's request using it is asked about first, the same as Reload (D3). Cancel runs
    // nothing; "Restart when it ends" waits, with the waiting line and its Cancel.
    const answer = await askBeforeChange(runtime);
    if (!answer) return;
    setBusy(true);
    try {
      let r = null as DiagnosticsReport | null;
      // The check goes on for minutes after its wait, so the waiting line follows the
      // backend's wait (`reloadWaiting`), named as a restart, and is not kept up for the
      // whole call: it shows, with a Cancel that works, only while the check waits.
      await runChange("reload", undefined, async () => {
        r = await ipc.runDiagnostics(runtime?.modelPath ?? null, backendWhenIdle(answer));
        return r !== null;
      });
      // null: the wait for the model was cancelled, which is not an error.
      if (r === null) return;
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
          {/* The report names the loaded model's path: hidden and direction-changing characters
              show as codes, its lines stay. Copy report copies it as it was written. */}
          <pre className="max-h-72 overflow-auto rounded-md bg-bg-muted p-3 text-xs whitespace-pre-wrap">
            {visibleLines(report)}
          </pre>
          <div className="flex gap-3 text-xs">
            <button type="button" className="underline" onClick={() => void writeText(report)}>
              Copy report
            </button>
            {savedTo && (
              <span className="text-fg-muted">
                Saved to <bdi dir="ltr">{visiblePath(savedTo)}</bdi>
              </span>
            )}
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
        <Dialog.Content className="fixed top-1/2 left-1/2 flex h-[40rem] max-h-[calc(100vh-9rem)] w-[48rem] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface shadow-pop">
          <div className="flex items-center justify-between border-b border-border px-5 py-3">
            <Dialog.Title className="text-base font-semibold">Settings</Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg" aria-label="Close settings">
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">App settings</Dialog.Description>
          <Tabs.Root key={props.initialTab ?? "general"} defaultValue={props.initialTab ?? "general"} orientation="vertical" className="flex min-h-0 flex-1">
            <Tabs.List ref={showChosenSection} className="flex min-h-0 w-48 flex-col gap-1 overflow-y-auto border-r border-border p-3" aria-label="Settings sections">
              <Tabs.Trigger value="general" className={tabClass}>General</Tabs.Trigger>
              <Tabs.Trigger value="runtime" className={tabClass}>Local models</Tabs.Trigger>
              <Tabs.Trigger value="reverse" className={tabClass}>Reverse engineering</Tabs.Trigger>
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
              <Tabs.Content value="reverse"><Reverse /></Tabs.Content>
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
