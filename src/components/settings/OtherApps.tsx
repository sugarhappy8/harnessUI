import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo, ModelLoadInfo, SetupStep, ShareApp, ShareSetup, ShareStatus } from "../../lib/types";
import { FIT_WORDS, isAppleComputer, tokensWithWords } from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { reloadModel } from "../../store/modelChange";
import { ErrorBanner } from "../ErrorBanner";
import { ModelSettings } from "../ModelSettings";

// Settings → Other apps (0.0.5 item 3, direction A, experimental, off by default): Claude
// Code, Codex or the Claude app on this computer may use the model loaded here, through the
// app's own proxy on a loopback port (`src-tauri/src/share/`). This page shows the state and
// the setup text the backend builds; it builds no setup text itself and keeps no secret.
//
// - The switch changes only on the user's click (`set_share_enabled`).
// - The key is read only when asked for (Show key, Copy) and is never kept outside React
//   state. The setup cards contain it too, so a card is fetched only when its app is chosen
//   and forgotten when it is closed or sharing is switched off.
// - The page asks `share_status` again every POLL_MS while it is open and visible (there is
//   no push event); a late answer never overwrites a newer one.
// - What a copied setup assumed (the model's context and the port, never the key) is kept in
//   this webview's local storage, so "Copy the setup again" is still said after a restart.

/** How often the page asks for the sharing status while it is open. */
export const POLL_MS = 2000;
/** Local storage: what the last copied setup assumed (`{ctx, port}`; no key). */
export const COPIED_KEY = "otherApps.copiedSetup";
/** Claude Code and Codex need at least this much context. */
const WANTED_CTX = 32_768;
/** What a setup assumes when no model is loaded (`setup_text::ASSUMED_CTX`). */
const ASSUMED_CTX = 32_768;
const LOWEST_PORT = 1024;
const HIGHEST_PORT = 65_535;

const SWITCH_LABEL = "Let other apps on this computer use the loaded model";
const WARNING =
  "Claude Code and Codex send long instructions. Set How much it can remember to at least 32,768 tokens (about 24,000 words) for this model.";
const TOO_MUCH = "This model can't remember that much on this computer. Try Save memory or a smaller model.";
const SIZE_CHANGED = "You changed how much this model can remember. Copy the setup again.";
const PORT_CHANGED = "You changed the port. Copy the setup again.";
const OLD_KEY = "Apps set up with the old key stop working until you copy the new one.";
const BAD_PORT = "Choose a port from 1024 to 65535.";

const APPS: { app: ShareApp; label: string }[] = [
  { app: "claudeCode", label: "Claude Code" },
  { app: "codex", label: "Codex" },
  { app: "claudeApp", label: "Claude app" },
];

type Tab = "windows" | "mac";
const TAB_LABELS: Record<Tab, string> = { windows: "Windows", mac: "Mac" };

/** What the last copied setup assumed. `ctx` is null for the Claude app, whose setup has none. */
interface Copied {
  ctx: number | null;
  port: number;
}

function readCopied(): Copied | null {
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(COPIED_KEY) ?? "null");
    if (value && typeof value === "object") {
      const { ctx, port } = value as Record<string, unknown>;
      if (typeof port === "number" && (ctx === null || typeof ctx === "number")) return { ctx, port };
    }
  } catch {
    // Unreadable or unavailable: nothing is known about a copied setup.
  }
  return null;
}

function writeCopied(copied: Copied): void {
  try {
    window.localStorage.setItem(COPIED_KEY, JSON.stringify(copied));
  } catch {
    // Not kept: the note just won't survive a restart.
  }
}

async function copyText(text: string): Promise<boolean> {
  try {
    if (ipc.isTauri()) await writeText(text);
    else await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}

/** A Copy button that says "Copied" for a moment. */
function CopyButton(props: { label: string; text: () => Promise<string> | string; onCopied?: () => void }) {
  const [copied, setCopied] = useState(false);
  async function copy() {
    let text: string;
    try {
      text = await props.text();
    } catch {
      return;
    }
    if (!(await copyText(text))) return;
    setCopied(true);
    props.onCopied?.();
    window.setTimeout(() => setCopied(false), 1500);
  }
  return (
    <button
      type="button"
      aria-label={props.label}
      onClick={() => void copy()}
      className="shrink-0 rounded-md border border-border px-2 py-0.5 text-xs hover:bg-bg-muted"
    >
      {copied ? "Copied" : "Copy"}
    </button>
  );
}

type ErrorPlace = "switch" | "port" | "key" | "memory";

/** Settings → Other apps. */
export function OtherApps() {
  const [status, setStatus] = useState<ShareStatus | null>(null);
  const [error, setError] = useState<{ place: ErrorPlace; error: ErrorInfo } | null>(null);
  const [working, setWorking] = useState<string | null>(null);
  // The port chooser.
  const [choosing, setChoosing] = useState(false);
  const [portText, setPortText] = useState("");
  // The key: shown only on request.
  const [key, setKey] = useState<string | null>(null);
  const [confirmNewKey, setConfirmNewKey] = useState(false);
  const [keyReplaced, setKeyReplaced] = useState(false);
  const [keyVersion, setKeyVersion] = useState(0);
  // Setup cards.
  const [app, setApp] = useState<ShareApp | null>(null);
  const [card, setCard] = useState<{ app: ShareApp; setup: ShareSetup; ctx: number; port: number | null; version: number } | null>(
    null,
  );
  const [cardError, setCardError] = useState<{ app: ShareApp; error: ErrorInfo } | null>(null);
  const [copied, setCopied] = useState<Copied | null>(readCopied);
  // The loaded model's settings, for the context warning's fit mark.
  const [loadInfo, setLoadInfo] = useState<{ path: string; info: ModelLoadInfo } | null>(null);
  // The loaded model's own settings panel, opened over this page (inside the settings window,
  // so the window stays open under it and Escape closes only the panel).
  const [panelFor, setPanelFor] = useState<string | null>(null);
  const runtime = useApp((s) => s.runtime);
  const modelPath = runtime?.state === "ready" ? runtime.modelPath : null;

  // Every status answer is numbered; one that arrives after a newer one is dropped.
  const asked = useRef(0);
  const shown = useRef(0);
  const track = useCallback(async (call: () => Promise<ShareStatus>): Promise<ShareStatus> => {
    const n = ++asked.current;
    const next = await call();
    if (n > shown.current) {
      shown.current = n;
      setStatus(next);
    }
    return next;
  }, []);

  useEffect(() => {
    track(ipc.shareStatus).catch((e) => setError({ place: "switch", error: toErrorInfo(e) }));
    const timer = window.setInterval(() => {
      if (document.visibilityState === "hidden") return;
      track(ipc.shareStatus).catch(() => undefined);
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [track]);

  const enabled = status?.enabled ?? false;
  const port = status?.port ?? null;
  const modelCtx = status?.modelCtx ?? null;

  // The chosen app's card, fetched again when what it contains changes (port, size, key).
  useEffect(() => {
    if (!app || !enabled) return;
    let alive = true;
    const ctx = modelCtx ?? ASSUMED_CTX;
    ipc
      .shareSetup(app)
      .then((setup) => {
        if (!alive) return;
        setCard({ app, setup, ctx, port, version: keyVersion });
        setCardError(null);
      })
      .catch((e) => alive && setCardError({ app, error: toErrorInfo(e) }));
    return () => {
      alive = false;
    };
  }, [app, enabled, port, modelCtx, keyVersion]);

  // The context warning needs the loaded model's fit marks.
  const short = enabled && modelCtx !== null && modelCtx < WANTED_CTX;
  useEffect(() => {
    if (!short || !modelPath) return;
    let alive = true;
    ipc
      .modelLoadInfo(modelPath)
      .then((info) => alive && setLoadInfo({ path: modelPath, info }))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [short, modelPath, modelCtx]);

  /** Opens one app's card, or closes it. A closed card is forgotten: it contains the key. */
  function chooseApp(next: ShareApp | null) {
    setApp(next);
    setCard(null);
    setCardError(null);
  }

  function fail(place: ErrorPlace, e: unknown) {
    setError({ place, error: toErrorInfo(e) });
  }

  function errorAt(place: ErrorPlace) {
    return error?.place === place ? <ErrorBanner error={error.error} onDismiss={() => setError(null)} /> : null;
  }

  async function toggle(on: boolean) {
    setWorking(on ? "Switching on…" : "Switching off…");
    setError(null);
    try {
      await track(() => ipc.setShareEnabled(on));
      if (!on) {
        setKey(null);
        setChoosing(false);
        setConfirmNewKey(false);
        chooseApp(null);
      }
    } catch (e) {
      fail("switch", e);
    } finally {
      setWorking(null);
    }
  }

  const portNumber = /^\d{1,5}$/.test(portText.trim()) ? Number(portText.trim()) : NaN;
  const portValid = portNumber >= LOWEST_PORT && portNumber <= HIGHEST_PORT;

  async function choosePort(next: number) {
    setWorking("Changing the port…");
    setError(null);
    try {
      await track(() => ipc.setSharePort(next));
      setChoosing(false);
      setPortText("");
    } catch (e) {
      fail("port", e);
    } finally {
      setWorking(null);
    }
  }

  async function showKey() {
    setError(null);
    try {
      setKey(await ipc.revealShareKey());
    } catch (e) {
      fail("key", e);
    }
  }

  async function makeNewKey() {
    setConfirmNewKey(false);
    setWorking("Making a new key…");
    setError(null);
    try {
      await track(ipc.newShareKey);
      setKeyReplaced(true);
      setKeyVersion((v) => v + 1);
      if (key !== null) setKey(await ipc.revealShareKey());
    } catch (e) {
      fail("key", e);
    } finally {
      setWorking(null);
    }
  }

  /** A step of the setup was copied: remember what that setup assumed. */
  function setupCopied() {
    if (!card) return;
    const next: Copied = {
      ctx: card.app === "claudeApp" ? (copied?.ctx ?? null) : card.ctx,
      port: card.port ?? copied?.port ?? 0,
    };
    writeCopied(next);
    setCopied(next);
    if (card.version === keyVersion) setKeyReplaced(false);
  }

  /**
   * Saves 32,768 tokens for the loaded model and restarts it (asking first when it is busy,
   * D3). The model's other settings are read again first, so a change made in its panel
   * since the warning appeared is kept.
   */
  async function applyWanted(path: string) {
    setWorking("Saving…");
    setError(null);
    try {
      const { overrides } = await ipc.modelLoadInfo(path);
      await ipc.setModelLoadSettings(path, { ...overrides, context: { fixed: WANTED_CTX } });
      setWorking(null);
      await reloadModel(useApp.getState().runtime, false, () => setWorking("Restarting the model…"));
    } catch (e) {
      fail("memory", e);
    } finally {
      setWorking(null);
    }
  }

  const info = loadInfo && loadInfo.path === modelPath ? loadInfo.info : null;
  const shownCard = card && card.app === app ? card : null;
  const shownCardError = cardError && cardError.app === app && !shownCard ? cardError.error : null;
  const sizeChanged = copied !== null && copied.ctx !== null && modelCtx !== null && modelCtx !== copied.ctx;
  const portChanged = copied !== null && port !== null && port !== copied.port;

  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Other apps</h2>
      <p className="text-fg-muted">
        Lets Claude Code, Codex or the Claude app on this computer use the model loaded here instead of an online
        model. Only apps that have the key can use it.
      </p>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          role="switch"
          aria-checked={enabled}
          checked={enabled}
          disabled={status === null || working !== null}
          onChange={(e) => void toggle(e.target.checked)}
          className="h-4 w-4"
        />
        <span className={enabled ? "font-medium" : ""}>{SWITCH_LABEL}</span>
        <span className="rounded-full border border-warning/50 px-2 py-0.5 text-xs text-warning">Experimental</span>
      </label>
      {errorAt("switch")}
      {working !== null && (
        <p className="text-xs text-fg-muted" role="status">
          {working}
        </p>
      )}

      {status?.enabled && (
        <>
          <section className="space-y-2" aria-label="Sharing">
            {status.listening ? (
              <p>On at port {status.port}. Apps on this computer that have the key can use the loaded model.</p>
            ) : (
              <p className="rounded-md border border-danger/40 bg-danger/5 px-3 py-2 text-danger" role="alert">
                {status.error ?? "Sharing is not reachable now. Switch it off and on again."}
              </p>
            )}
            {status.listening && status.inUse && <p className="text-accent">Another app is using the model now.</p>}
            {status.listening && modelCtx === null && (
              <p className="text-fg-muted">
                {runtime?.state === "starting"
                  ? "The model is still loading. Apps get an answer saying so until it is ready."
                  : "No model is loaded now. Apps get an answer saying so until you load one."}
              </p>
            )}
            {choosing ? (
              <div className="space-y-2 rounded-md border border-border p-3">
                <div className="flex flex-wrap items-center gap-2">
                  <label htmlFor="share-port" className="text-sm">
                    Port number
                  </label>
                  <input
                    id="share-port"
                    type="number"
                    inputMode="numeric"
                    min={LOWEST_PORT}
                    max={HIGHEST_PORT}
                    value={portText}
                    onChange={(e) => setPortText(e.target.value)}
                    className="w-28 rounded-md border border-border bg-bg px-2 py-1 text-sm"
                  />
                  <button
                    type="button"
                    disabled={!portValid || working !== null}
                    onClick={() => void choosePort(portNumber)}
                    className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted disabled:opacity-40"
                  >
                    Use this port
                  </button>
                  <button
                    type="button"
                    disabled={working !== null}
                    onClick={() => void choosePort(0)}
                    className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted disabled:opacity-40"
                  >
                    Pick a free port
                  </button>
                  <button
                    type="button"
                    onClick={() => {
                      setChoosing(false);
                      setPortText("");
                      if (error?.place === "port") setError(null);
                    }}
                    className="px-2 py-1 text-fg-muted underline"
                  >
                    Cancel
                  </button>
                </div>
                {portText.trim() !== "" && !portValid && <p className="text-xs text-danger">{BAD_PORT}</p>}
                {errorAt("port")}
              </div>
            ) : (
              <button
                type="button"
                onClick={() => setChoosing(true)}
                className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
              >
                Choose another port
              </button>
            )}
            <p>{`Requests refused since sharing was switched on: ${status.refused}`}</p>
            <p className="text-xs text-fg-muted">
              A request is refused when it has no key or a wrong key, or comes from a web page. Each answered request
              is listed in the Audit log, without what was asked.
            </p>
          </section>

          {short && (
            <section
              className="space-y-2 rounded-md border border-warning/40 bg-warning/5 px-3 py-2"
              aria-label="How much the model can remember"
            >
              <p>{WARNING}</p>
              {info && <MemoryChoice info={info} disabled={working !== null} onUse={() => void applyWanted(info.path)} />}
              {modelPath && (
                <button
                  type="button"
                  onClick={() => setPanelFor(modelPath)}
                  className="text-xs text-fg-muted underline hover:text-fg"
                >
                  Model settings…
                </button>
              )}
              {errorAt("memory")}
            </section>
          )}

          <section className="space-y-2" aria-labelledby="share-key-title">
            <h3 id="share-key-title" className="font-medium">
              Key
            </h3>
            <p className="text-xs text-fg-muted">Apps need this key to use the model. Keep it to yourself.</p>
            {key !== null && <p className="font-mono text-xs break-all">{key}</p>}
            <div className="flex flex-wrap items-center gap-2">
              {key === null ? (
                <button
                  type="button"
                  onClick={() => void showKey()}
                  className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
                >
                  Show key
                </button>
              ) : (
                <button
                  type="button"
                  onClick={() => setKey(null)}
                  className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
                >
                  Hide key
                </button>
              )}
              <CopyButton
                label="Copy the key"
                text={() => key ?? ipc.revealShareKey()}
                onCopied={() => setKeyReplaced(false)}
              />
              {!confirmNewKey && (
                <button
                  type="button"
                  disabled={working !== null}
                  onClick={() => setConfirmNewKey(true)}
                  className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted disabled:opacity-40"
                >
                  Make a new key
                </button>
              )}
            </div>
            {confirmNewKey && (
              <div role="group" aria-label="Make a new key?" className="space-y-2 rounded-md border border-border p-3">
                <p>{OLD_KEY}</p>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => setConfirmNewKey(false)}
                    className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
                  >
                    Cancel
                  </button>
                  <button
                    type="button"
                    onClick={() => void makeNewKey()}
                    className="rounded-md bg-danger px-3 py-1 text-white"
                  >
                    Make a new key
                  </button>
                </div>
              </div>
            )}
            {keyReplaced && !confirmNewKey && <p className="text-warning">{OLD_KEY}</p>}
            {errorAt("key")}
          </section>

          <section className="space-y-2" aria-labelledby="share-setup-title">
            <h3 id="share-setup-title" className="font-medium">
              Set up an app
            </h3>
            <p className="text-xs text-fg-muted">
              Choose an app to see how to set it up. The setup includes the key.
            </p>
            {sizeChanged && <p className="text-warning">{SIZE_CHANGED}</p>}
            {portChanged && <p className="text-warning">{PORT_CHANGED}</p>}
            <div className="flex flex-wrap gap-2">
              {APPS.map((a) => (
                <button
                  key={a.app}
                  type="button"
                  aria-pressed={app === a.app}
                  onClick={() => chooseApp(app === a.app ? null : a.app)}
                  className={
                    "rounded-md border px-3 py-1 " +
                    (app === a.app ? "border-accent bg-accent text-accent-fg" : "border-border hover:bg-bg-muted")
                  }
                >
                  {a.label}
                </button>
              ))}
            </div>
            {shownCardError && <ErrorBanner error={shownCardError} onDismiss={() => setCardError(null)} />}
            {app && !shownCard && !shownCardError && <p className="text-xs text-fg-muted">Getting the setup…</p>}
            {shownCard && <SetupCard key={shownCard.app} setup={shownCard.setup} onCopied={setupCopied} />}
          </section>
        </>
      )}
      {panelFor !== null && (
        <ModelSettings
          key={panelFor}
          path={panelFor}
          onClose={() => setPanelFor(null)}
          onSaved={(saved) => setLoadInfo({ path: panelFor, info: saved })}
        />
      )}
    </div>
  );
}

/** The button that sets 32,768 tokens, with its fit mark, or why it cannot. */
function MemoryChoice(props: { info: ModelLoadInfo; disabled: boolean; onUse: () => void }) {
  const { info } = props;
  if (info.trainedCtx !== null && info.trainedCtx < WANTED_CTX) {
    return (
      <p>{`This model can remember at most ${tokensWithWords(info.trainedCtx)}. Try a model that can remember more.`}</p>
    );
  }
  const option = info.contextOptions.find((o) => typeof o.choice === "object" && o.choice.fixed === WANTED_CTX);
  if (!option) return null;
  if (option.fit === "red") return <p>{TOO_MUCH}</p>;
  return (
    <button
      type="button"
      disabled={props.disabled}
      onClick={props.onUse}
      className="flex items-center gap-1.5 rounded-md border border-border bg-surface px-3 py-1 hover:bg-bg-muted disabled:opacity-40"
    >
      <span
        aria-hidden
        className={`inline-block h-2 w-2 shrink-0 rounded-full ${option.fit === "green" ? "bg-success" : "bg-warning"}`}
      />
      {`Use 32,768 tokens and restart · ${FIT_WORDS[option.fit]}`}
    </button>
  );
}

/** One app's setup: Windows and Mac tabs (when the ways differ), steps with Copy buttons, and the notes. */
function SetupCard(props: { setup: ShareSetup; onCopied: () => void }) {
  const { setup } = props;
  const tabs = (["windows", "mac"] as const).filter((t) => setup.ways.some((w) => w.system === t));
  const [chosen, setChosen] = useState<Tab>(isAppleComputer() ? "mac" : "windows");
  const tab: Tab | null = tabs.includes(chosen) ? chosen : (tabs[0] ?? null);
  const ways = setup.ways.filter((w) => w.system === "any" || tab === null || w.system === tab);

  function onTabKey(e: KeyboardEvent<HTMLButtonElement>) {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const at = tab === null ? 0 : tabs.indexOf(tab);
    const next = tabs[(at + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length];
    if (!next) return;
    setChosen(next);
    document.getElementById(`share-tab-${setup.app}-${next}`)?.focus();
  }

  const body = (
    <div className="space-y-3">
      {ways.map((way, w) => (
        <div key={`${way.system}-${w}`} className="space-y-2">
          <h5 className="text-sm font-medium">{way.title}</h5>
          <ol className="list-decimal space-y-2 pl-5">
            {way.steps.map((step, i) => (
              <Step key={i} step={step} name={`step ${i + 1} of ${way.title}`} onCopied={props.onCopied} />
            ))}
          </ol>
        </div>
      ))}
    </div>
  );

  return (
    <section aria-label={`Set up ${setup.title}`} className="space-y-3 rounded-md border border-border p-3">
      <div className="flex flex-wrap items-center gap-2">
        <h4 className="font-medium">{setup.title}</h4>
        {setup.notTried && (
          <span className="rounded-full border border-warning/50 px-2 py-0.5 text-xs text-warning">Not tried yet</span>
        )}
      </div>
      {tab !== null && tabs.length > 1 ? (
        <>
          <div role="tablist" aria-label="Computer" className="flex gap-1 border-b border-border">
            {tabs.map((t) => (
              <button
                key={t}
                id={`share-tab-${setup.app}-${t}`}
                type="button"
                role="tab"
                aria-selected={tab === t}
                aria-controls={`share-panel-${setup.app}`}
                tabIndex={tab === t ? 0 : -1}
                onClick={() => setChosen(t)}
                onKeyDown={onTabKey}
                className={
                  "-mb-px border-b-2 px-3 py-1 text-sm " +
                  (tab === t ? "border-accent font-medium" : "border-transparent text-fg-muted hover:text-fg")
                }
              >
                {TAB_LABELS[t]}
              </button>
            ))}
          </div>
          <div
            id={`share-panel-${setup.app}`}
            role="tabpanel"
            aria-labelledby={`share-tab-${setup.app}-${tab}`}
          >
            {body}
          </div>
        </>
      ) : (
        body
      )}
      {setup.notes.length > 0 && (
        <ul className="list-disc space-y-1 pl-5 text-xs text-fg-muted">
          {setup.notes.map((note, i) => (
            <li key={i}>{note}</li>
          ))}
        </ul>
      )}
    </section>
  );
}

function Step(props: { step: SetupStep; name: string; onCopied: () => void }) {
  const { step } = props;
  return (
    <li className="space-y-1">
      <p>{step.text}</p>
      {step.file !== null && (
        <div className="flex items-center gap-2">
          <code className="min-w-0 font-mono text-xs break-all">{step.file}</code>
          <CopyButton label={`Copy the path of ${props.name}`} text={() => step.file ?? ""} />
        </div>
      )}
      {step.code !== null && (
        <div className="flex items-start gap-2">
          <pre className="min-w-0 flex-1 overflow-x-auto rounded-md bg-bg-muted p-2 font-mono text-xs whitespace-pre">
            {step.code}
          </pre>
          <CopyButton label={`Copy ${props.name}`} text={() => step.code ?? ""} onCopied={props.onCopied} />
        </div>
      )}
    </li>
  );
}
