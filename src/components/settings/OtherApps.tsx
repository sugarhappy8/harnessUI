import { useCallback, useEffect, useRef, useState, type KeyboardEvent } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo, ModelBusy, ModelLoadInfo, SetupStep, ShareApp, ShareSetup, ShareStatus } from "../../lib/types";
import { FIT_WORDS, isAppleComputer, tokensWithWords, waitingWords } from "../../lib/words";
import { toErrorInfo, useApp } from "../../store/app";
import { reloadModel, useWaiting } from "../../store/modelChange";
import { ErrorBanner } from "../ErrorBanner";
import { ModelSettings } from "../ModelSettings";

// Settings → Other apps (0.0.5 item 3, direction A, experimental, off by default): Claude
// Code, Codex or the Claude app on this computer may use the model loaded here, through the
// app's own proxy on a loopback port (`src-tauri/src/share/`). This page shows the state and
// the setup text the backend builds; it builds no setup text itself and keeps no secret.
//
// - The switch changes only on the user's click (`set_share_enabled`).
// - The key is read only when asked for (Show key, Copy) and is never kept outside React
//   state. Every key request is numbered: one that answers after the key was hidden, after
//   sharing went off or after a new key was made, is dropped. The setup cards contain the
//   key too: a card is fetched only when its app is chosen, shows the key as dots until Show
//   key (Copy copies the backend's text exactly), is forgotten when it is closed or sharing
//   goes off, and is shown only while it matches the current key, port and size.
// - The page asks `share_status` again every POLL_MS while it is open and visible (there is
//   no push event); a late answer never overwrites a newer one.
// - What each app's copied setup assumed (the model's context, the port and the model file,
//   never the key) is kept in this webview's local storage, so "Copy the setup again" is
//   still said after a restart, for the apps it concerns.

/** How often the page asks for the sharing status while it is open. */
export const POLL_MS = 2000;
/** Local storage: what each app's copied setup assumed (`{claudeCode: {ctx, port, model}, …}`; no key). */
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
const MODEL_CHANGED =
  "The loaded model can remember a different amount than when you copied the setup. Copy the setup again.";
const PORT_CHANGED = "You changed the port. Copy the setup again.";
const OLD_KEY = "Apps set up with the old key stop working until you copy the new one.";
const BAD_PORT = "Choose a port from 1024 to 65535.";
const COPY_FAILED = "Could not copy";
/** The sharing key inside setup text (`harnessui-share-` and hex digits). */
const KEY_IN_TEXT = /harnessui-share-[0-9A-Za-z]+/g;
/** How a card shows the key until the user asks to see it. */
const HIDDEN_KEY = "harnessui-share-••••••••";

const APPS: { app: ShareApp; label: string }[] = [
  { app: "claudeCode", label: "Claude Code" },
  { app: "codex", label: "Codex" },
  { app: "claudeApp", label: "Claude app" },
];

type Tab = "windows" | "mac";
const TAB_LABELS: Record<Tab, string> = { windows: "Windows", mac: "Mac" };

/**
 * What one app's copied setup assumed. `ctx` is null for the Claude app, whose setup has
 * none; `model` is the model file loaded then (null when none was).
 */
interface Copied {
  ctx: number | null;
  port: number;
  model: string | null;
}
type CopiedSetups = Partial<Record<ShareApp, Copied>>;

function readCopied(): CopiedSetups {
  const out: CopiedSetups = {};
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(COPIED_KEY) ?? "null");
    if (value && typeof value === "object") {
      for (const { app } of APPS) {
        const entry: unknown = (value as Record<string, unknown>)[app];
        if (!entry || typeof entry !== "object") continue;
        const { ctx, port, model } = entry as Record<string, unknown>;
        const known =
          typeof port === "number" && (ctx === null || typeof ctx === "number") && (model === null || typeof model === "string");
        if (known) out[app] = { ctx, port, model };
      }
    }
  } catch {
    // Unreadable or unavailable: nothing is known about a copied setup.
  }
  return out;
}

function writeCopied(copied: CopiedSetups): void {
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

/** "Ann", "Ann and Bo", "Ann, Bo and Cy". */
function listWords(names: string[]): string {
  return names.length <= 1 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/** A Copy button that says "Copied", or "Could not copy", for a moment. */
function CopyButton(props: {
  label: string;
  text: () => Promise<string> | string;
  onCopied?: () => void;
  /** The text could not be had (e.g. the key could not be read). */
  onError?: (e: unknown) => void;
  disabled?: boolean;
}) {
  const [shown, setShown] = useState<"copy" | "copied" | "failed">("copy");
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  function flash(next: "copied" | "failed") {
    setShown(next);
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setShown("copy"), next === "copied" ? 1500 : 3000);
  }
  async function copy() {
    let text: string;
    try {
      text = await props.text();
    } catch (e) {
      flash("failed");
      props.onError?.(e);
      return;
    }
    if (!(await copyText(text))) {
      flash("failed");
      return;
    }
    flash("copied");
    props.onCopied?.();
  }
  return (
    <button
      type="button"
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => void copy()}
      className="shrink-0 rounded-md border border-border px-2 py-0.5 text-xs hover:bg-bg-muted disabled:opacity-40"
    >
      {shown === "copied" ? "Copied" : shown === "failed" ? COPY_FAILED : "Copy"}
    </button>
  );
}

type ErrorPlace = "read" | "switch" | "port" | "key" | "memory";

/** A setup card, and what it was made for (it is shown only while these are still true). */
interface Made {
  app: ShareApp;
  ctx: number;
  port: number | null;
  version: number;
}

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
  const [keyVersion, setKeyVersion] = useState(0);
  // After a new key (made for `version`): the apps whose setup was copied with the old key
  // and not yet with the new one. Each is crossed off when a step with the key in it is
  // copied for it; with none known, the note stays until the new key is copied at all.
  // Copy the key ends it for every app. Null: no note.
  const [oldKey, setOldKey] = useState<{ version: number; apps: ShareApp[] } | null>(null);
  // Setup cards.
  const [app, setApp] = useState<ShareApp | null>(null);
  const [card, setCard] = useState<(Made & { setup: ShareSetup }) | null>(null);
  const [cardError, setCardError] = useState<(Made & { error: ErrorInfo }) | null>(null);
  const [copied, setCopied] = useState<CopiedSetups>(readCopied);
  // The copied setups as they are now, for a new key that is made while one is copied.
  const copiedNow = useRef(copied);
  // Windows or Mac, for every card: kept when a card is made again (new key, port or size).
  const [tab, setTab] = useState<Tab>(() => (isAppleComputer() ? "mac" : "windows"));
  // The loaded model's settings, for the context warning's fit mark.
  const [loadInfo, setLoadInfo] = useState<{ path: string; info: ModelLoadInfo } | null>(null);
  // The loaded model's own settings panel, opened over this page (inside the settings window,
  // so the window stays open under it and Escape closes only the panel).
  const [panelFor, setPanelFor] = useState<string | null>(null);
  // This page's restart (Use 32,768 tokens and restart): "asking" until the D3 question is
  // answered, then "running" until the change ends (it may wait for the model to be free).
  const [restart, setRestart] = useState<"asking" | "running" | null>(null);
  const [busy, setBusy] = useState<ModelBusy | null>(null);
  const waitingChange = useWaiting((s) => s.change);
  const runningChange = useWaiting((s) => s.kind);
  const runtime = useApp((s) => s.runtime);
  const modelPath = runtime?.state === "ready" ? runtime.modelPath : null;

  // Every key request is numbered; hiding the key or sharing going off moves the number on,
  // so an answer that arrives after that is dropped instead of showing the key again.
  const keyAsk = useRef(0);

  // Every status answer is numbered; one that arrives after a newer one is dropped.
  const asked = useRef(0);
  const shown = useRef(0);
  const track = useCallback(async (call: () => Promise<ShareStatus>): Promise<ShareStatus> => {
    const n = ++asked.current;
    const next = await call();
    if (n > shown.current) {
      shown.current = n;
      setStatus(next);
      if (!next.enabled) {
        // Off (by the switch or as the backend reports it): the key and the cards, which
        // contain it, are forgotten, and a key still on its way is dropped.
        keyAsk.current += 1;
        setKey(null);
        setChoosing(false);
        setPortText("");
        setConfirmNewKey(false);
        setApp(null);
        setCard(null);
        setCardError(null);
      }
    }
    return next;
  }, []);

  useEffect(() => {
    // A read that works clears the error of one that did not.
    const read = () => track(ipc.shareStatus).then(() => setError((e) => (e?.place === "read" ? null : e)));
    read().catch((e) => setError({ place: "read", error: toErrorInfo(e) }));
    const timer = window.setInterval(() => {
      if (document.visibilityState === "hidden") return;
      read().catch(() => undefined);
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [track]);

  const enabled = status?.enabled ?? false;
  const port = status?.port ?? null;
  const modelCtx = status?.modelCtx ?? null;
  const cardCtx = modelCtx ?? ASSUMED_CTX;

  // The chosen app's card, fetched again when what it contains changes (port, size, key).
  useEffect(() => {
    if (!app || !enabled) return;
    let alive = true;
    const made: Made = { app, ctx: modelCtx ?? ASSUMED_CTX, port, version: keyVersion };
    ipc
      .shareSetup(app)
      .then((setup) => {
        if (!alive) return;
        setCard({ ...made, setup });
        setCardError(null);
      })
      .catch((e) => {
        if (!alive) return;
        setCard(null);
        setCardError({ ...made, error: toErrorInfo(e) });
      });
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

  // This page's restart waits for the model to be free (D3: Restart when it ends, or the
  // backend waiting on its own for a request that started after the check).
  const waitingHere =
    restart === "running" && runtime?.state === "ready" && (waitingChange !== null || runtime.reloadWaiting);
  // What it waits for, polled while the line shows (as the runtime indicator does).
  useEffect(() => {
    if (!waitingHere) return;
    let alive = true;
    const read = () =>
      ipc
        .modelBusy()
        .then((b) => alive && setBusy(b))
        .catch(() => undefined);
    void read();
    const timer = window.setInterval(() => {
      if (document.visibilityState !== "hidden") void read();
    }, POLL_MS);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [waitingHere]);

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

  function hideKey() {
    keyAsk.current += 1;
    setKey(null);
  }

  /** Reads the key and shows it, unless it was hidden (or sharing went off) meanwhile. */
  async function revealKey() {
    const n = ++keyAsk.current;
    try {
      const next = await ipc.revealShareKey();
      if (n === keyAsk.current) setKey(next);
    } catch (e) {
      if (n === keyAsk.current) fail("key", e);
    }
  }

  async function toggle(on: boolean) {
    setWorking(on ? "Switching on…" : "Switching off…");
    setError(null);
    try {
      // Switched off, `track` forgets the key and the cards.
      await track(() => ipc.setShareEnabled(on));
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
    await revealKey();
  }

  async function makeNewKey() {
    setConfirmNewKey(false);
    setWorking("Making a new key…");
    setError(null);
    // A Show key read still on its way would answer with the old key: it is dropped. A key
    // shown now stays (Copy waits) until the new one is made, which is then shown in its
    // place, unless the user hides it meanwhile.
    keyAsk.current += 1;
    const shownAt = key !== null ? keyAsk.current : null;
    try {
      await track(ipc.newShareKey);
      // The old key is refused from now on: it leaves the page even if the new one cannot be read.
      setKey(null);
      const version = keyVersion + 1;
      setOldKey({ version, apps: APPS.filter((a) => copiedNow.current[a.app] !== undefined).map((a) => a.app) });
      setKeyVersion(version);
    } catch (e) {
      fail("key", e);
      return;
    } finally {
      setWorking(null);
    }
    if (shownAt !== null && shownAt === keyAsk.current) await revealKey();
  }

  const shownCard =
    card && card.app === app && card.version === keyVersion && card.port === port && card.ctx === cardCtx ? card : null;
  const shownCardError =
    !shownCard &&
    cardError &&
    cardError.app === app &&
    cardError.version === keyVersion &&
    cardError.port === port &&
    cardError.ctx === cardCtx
      ? cardError.error
      : null;

  /**
   * A step of the shown card was copied: remember what that app's setup assumed, and, when
   * the step has the key in it, that the app now has the key the card was made for.
   */
  function setupCopied(withKey: boolean) {
    if (!shownCard) return;
    const { app: done, version } = shownCard;
    // `shownCard` is the card the step was copied from (this is the click's own call). A
    // copy that was still running when Make a new key finished came from a card made for an
    // older key: that app counts as set up with the old key, as it would have had the copy
    // ended a moment sooner, so it is listed to set up again.
    setOldKey((k) => {
      if (k === null || version >= k.version || k.apps.includes(done)) return k;
      return { ...k, apps: [...k.apps, done] };
    });
    const claudeApp = shownCard.app === "claudeApp";
    const next: CopiedSetups = {
      ...copied,
      [shownCard.app]: {
        ctx: claudeApp ? null : shownCard.ctx,
        port: shownCard.port ?? 0,
        model: claudeApp ? null : modelPath,
      },
    };
    writeCopied(next);
    setCopied(next);
    copiedNow.current = next;
    if (!withKey) return;
    setOldKey((k) => {
      if (k === null || k.version !== version) return k;
      const apps = k.apps.filter((a) => a !== done);
      if (k.apps.length > 0 && apps.length === k.apps.length) return k;
      return apps.length === 0 ? null : { ...k, apps };
    });
  }

  /**
   * Saves 32,768 tokens for the loaded model and restarts it (asking first when it is busy,
   * D3). The model's other settings are read again first, so a change made in its panel
   * since the warning appeared is kept. While the restart waits for the model to be free,
   * only this button waits: the rest of the page stays usable.
   */
  async function applyWanted(path: string) {
    setWorking("Saving…");
    setError(null);
    try {
      const { overrides } = await ipc.modelLoadInfo(path);
      await ipc.setModelLoadSettings(path, { ...overrides, context: { fixed: WANTED_CTX } });
    } catch (e) {
      fail("memory", e);
      return;
    } finally {
      setWorking(null);
    }
    setRestart("asking");
    try {
      await reloadModel(useApp.getState().runtime, false, () => setRestart("running"));
    } catch (e) {
      fail("memory", e);
    } finally {
      setRestart(null);
    }
  }

  async function cancelWait() {
    try {
      await ipc.cancelPendingReload();
    } catch (e) {
      fail("memory", e);
    }
  }

  const info = loadInfo && loadInfo.path === modelPath ? loadInfo.info : null;
  const shownLine = working ?? (restart === "running" && !waitingHere ? "Restarting the model…" : null);
  // Which copied setups no longer match: the size (Claude Code, Codex) or the port (all).
  const sizeStale = (c: Copied) => c.ctx !== null && modelCtx !== null && c.ctx !== modelCtx;
  const portStale = (c: Copied) => port !== null && c.port !== port;
  const staleBySize = APPS.filter((a) => {
    const c = copied[a.app];
    return c !== undefined && sizeStale(c);
  });
  const sizeNote =
    staleBySize.length === 0
      ? null
      : staleBySize.every((a) => copied[a.app]?.model === modelPath)
        ? SIZE_CHANGED
        : MODEL_CHANGED;
  const portChanged = APPS.some((a) => {
    const c = copied[a.app];
    return c !== undefined && portStale(c);
  });
  // Apps to set up again: those whose copied setup no longer matches, or has the old key.
  const setUpAgain = APPS.filter((a) => {
    const c = copied[a.app];
    return (c !== undefined && (sizeStale(c) || portStale(c))) || (oldKey?.apps.includes(a.app) ?? false);
  }).map((a) => a.label);

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
      {errorAt("read")}
      {errorAt("switch")}
      {shownLine !== null && (
        <p className="text-xs text-fg-muted" role="status">
          {shownLine}
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
                {runtime?.state === "starting" || runtime?.state === "stopping"
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
            <p>{`Requests refused since sharing was switched on: ${status.refused.toLocaleString("en-US")}`}</p>
            <p className="text-xs text-fg-muted">
              A request is refused when it has no key or a wrong key, or comes from a web page. Each request for a reply
              is listed in the Audit log, without what was asked.
            </p>
          </section>

          {short && (
            <section
              className="space-y-2 rounded-md border border-warning/40 bg-warning/5 px-3 py-2"
              aria-label="How much the model can remember"
            >
              <p>{WARNING}</p>
              {info && (
                <MemoryChoice
                  info={info}
                  disabled={working !== null || restart !== null}
                  onUse={() => void applyWanted(info.path)}
                />
              )}
              {waitingHere && (
                <p className="flex flex-wrap items-center gap-1.5 text-xs text-warning" role="status">
                  <span>{waitingWords(waitingChange ?? runningChange ?? "reload", busy)}</span>
                  <span aria-hidden>·</span>
                  <button
                    type="button"
                    aria-label="Cancel the restart"
                    onClick={() => void cancelWait()}
                    className="underline hover:text-fg"
                  >
                    Cancel
                  </button>
                </p>
              )}
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
                  disabled={working !== null}
                  onClick={() => void showKey()}
                  className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted disabled:opacity-40"
                >
                  Show key
                </button>
              ) : (
                <button
                  type="button"
                  onClick={hideKey}
                  className="rounded-md border border-border px-3 py-1 hover:bg-bg-muted"
                >
                  Hide key
                </button>
              )}
              <CopyButton
                label="Copy the key"
                disabled={working !== null}
                text={() => key ?? ipc.revealShareKey()}
                // A copy that began before a newer key was made copied the old one.
                onCopied={() => setOldKey((k) => (k?.version === keyVersion ? null : k))}
                onError={(e) => fail("key", e)}
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
            {oldKey !== null && !confirmNewKey && <p className="text-warning">{OLD_KEY}</p>}
            {errorAt("key")}
          </section>

          <section className="space-y-2" aria-labelledby="share-setup-title">
            <h3 id="share-setup-title" className="font-medium">
              Set up an app
            </h3>
            <p className="text-xs text-fg-muted">
              Choose an app to see how to set it up. The setup includes the key, shown as dots until you choose Show
              key.
            </p>
            {sizeNote !== null && <p className="text-warning">{sizeNote}</p>}
            {portChanged && <p className="text-warning">{PORT_CHANGED}</p>}
            {setUpAgain.length > 0 && (
              <p className="text-xs text-warning">{`Apps to set up again: ${listWords(setUpAgain)}.`}</p>
            )}
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
            {shownCard && (
              <SetupCard
                key={shownCard.app}
                setup={shownCard.setup}
                tab={tab}
                onTab={setTab}
                keyShown={key !== null}
                onCopied={setupCopied}
              />
            )}
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

/**
 * One app's setup: Windows and Mac tabs (when the ways differ; the choice is the page's),
 * steps with Copy buttons, and the notes. The key shows as dots unless `keyShown`; Copy
 * copies the backend's text as it is.
 */
function SetupCard(props: {
  setup: ShareSetup;
  tab: Tab;
  onTab: (tab: Tab) => void;
  keyShown: boolean;
  /** A step was copied; `withKey`: the step has the key in it. */
  onCopied: (withKey: boolean) => void;
}) {
  const { setup } = props;
  const tabs = (["windows", "mac"] as const).filter((t) => setup.ways.some((w) => w.system === t));
  const chosen = props.tab;
  const setChosen = props.onTab;
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
              <Step
                key={i}
                step={step}
                name={`step ${i + 1} of ${way.title}`}
                keyShown={props.keyShown}
                onCopied={props.onCopied}
              />
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

function Step(props: { step: SetupStep; name: string; keyShown: boolean; onCopied: (withKey: boolean) => void }) {
  const { step } = props;
  const shown = (text: string) => (props.keyShown ? text : text.replace(KEY_IN_TEXT, HIDDEN_KEY));
  return (
    <li className="space-y-1">
      <p>{shown(step.text)}</p>
      {step.file !== null && (
        <div className="flex items-center gap-2">
          <code className="min-w-0 font-mono text-xs break-all">{step.file}</code>
          <CopyButton label={`Copy the path of ${props.name}`} text={() => step.file ?? ""} />
        </div>
      )}
      {step.code !== null && (
        <div className="flex items-start gap-2">
          <pre className="min-w-0 flex-1 overflow-x-auto rounded-md bg-bg-muted p-2 font-mono text-xs whitespace-pre">
            {shown(step.code)}
          </pre>
          <CopyButton
            label={`Copy ${props.name}`}
            text={() => step.code ?? ""}
            // `search` ignores the pattern's /g state (`test` would not).
            onCopied={() => props.onCopied((step.code ?? "").search(KEY_IN_TEXT) !== -1)}
          />
        </div>
      )}
    </li>
  );
}
