import { useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import { STUDIO_ACTIONS, STUDIO_DELETE, STUDIO_SETTINGS, STUDIO_TOPBAR } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import * as api from "../studio/api";
import { ConfirmDialog } from "../studio/ConfirmDialog";
import { EngineLine } from "../studio/EngineLine";
import { LocalNotice } from "../studio/LocalNotice";
import { BOX } from "../studio/styles";
import type { GpuPolicy, LocalModels, StudioEndpoint, StudioPrefs, StudioPrefsPatch, Usage } from "../studio/types";

// Settings → Studio (0.1.0 Task B10; design §4 and §5, local addendum L1/L2). Local first:
// the picture and video models on this computer and the disk they take, with *Get local
// models* (Studio's own view); the Studio engine's line; *What local models mean*, the content
// notice read only (Task B14); what happens to the chat model when a local job needs the
// graphics card, and whether it loads again after Studio. Then what the
// gallery holds and where, with *Open folder*, *Change…* (new results only) and *Delete
// stopped and failed runs* (which asks first); the confirmations and the daily limit for cloud
// runs; and when each cloud model list was updated, with *Update now*. Opening the page reads
// what is saved (the store's `reread`) and sends nothing; each change sends only its own field,
// and applies to the next run at once.

/** The amounts the backend takes (`prefs::DAILY_LIMIT_RANGE`, `prefs::CONFIRM_AT_RANGE`). */
const DAILY_LIMIT = { min: 1, max: 1000 } as const;
const CONFIRM_AT = { min: 0.25, max: 100 } as const;
/** The daily limit when it is turned on again with no amount of its own this session. */
const DEFAULT_LIMIT_USD = 10;

const GPU_POLICIES: readonly GpuPolicy[] = ["unload", "ask", "processor"];

const BUTTON = "rounded-md border border-border px-3 py-1 text-sm hover:bg-bg-muted disabled:opacity-60";
const HEADING = "text-sm font-medium";

/** Commas between groups of three digits ("1,000", "1,000.00"), which only separate thousands. */
const THOUSANDS = /^\d{1,3}(,\d{3})+(\.\d*)?$/;

/** A typed amount in US dollars ("2.5", "$15", "$1,000.00"); `null` when it is not a number.
 *  A comma that does not separate thousands ("1,5") is not read, so a decimal comma is never
 *  taken for an amount ten times too big. */
function amountFrom(text: string): number | null {
  const bare = text.trim().replace(/^\$\s*/, "");
  if (bare === "") return null;
  const n = Number(THOUSANDS.test(bare) ? bare.replaceAll(",", "") : bare);
  return Number.isFinite(n) ? n : null;
}

/** Writes `patch` (only the fields it names); a refusal goes to `onError`. */
function changePrefs(patch: StudioPrefsPatch, onError: (e: ErrorInfo | null) => void, onRefused?: () => void) {
  onError(null);
  useStudio
    .getState()
    .setPrefs(patch)
    .catch((e: unknown) => {
      onError(toErrorInfo(e));
      onRefused?.();
    });
}

export function StudioSettings() {
  const local = useStudio((s) => s.local);
  const prefs = useStudio((s) => s.prefs);
  const usage = useStudio((s) => s.usage);
  const endpoints = useStudio((s) => s.endpoints);
  const [error, setError] = useState<ErrorInfo | null>(null);

  // What is saved: the lists, local models, preferences and the gallery's size. Never a fetch.
  // Outside the app there is nothing to read.
  useEffect(() => {
    if (!ipc.isTauri()) return;
    let alive = true;
    useStudio
      .getState()
      .reread()
      .catch((e: unknown) => alive && setError(toErrorInfo(e)));
    return () => {
      alive = false;
    };
  }, []);

  return (
    <div className="space-y-6 text-sm">
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <OnThisComputer local={local} prefs={prefs} />
      <Results usage={usage} />
      {prefs && <Spending prefs={prefs} />}
      <ModelLists endpoints={endpoints} />
    </div>
  );
}

/** The local block: models, the engine, the content notice, and the chat model when the
 *  graphics card is needed. */
function OnThisComputer(props: { local: LocalModels | null; prefs: StudioPrefs | null }) {
  const { local, prefs } = props;
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [notice, setNotice] = useState(false);
  return (
    <section aria-labelledby="studio-local" className="space-y-3">
      <h3 id="studio-local" className={HEADING}>
        {STUDIO_SETTINGS.local}
      </h3>
      {local && (
        <>
          <div className="flex flex-wrap items-center gap-2">
            <span>{STUDIO_SETTINGS.models(local.installed.length, local.installedBytes)}</span>
            <button
              type="button"
              className={BUTTON}
              // Studio opens on Get models; Settings closes as it does.
              onClick={() => window.dispatchEvent(new CustomEvent("app:open-studio", { detail: "local" }))}
            >
              {STUDIO_TOPBAR.getModels}
            </button>
          </div>
          <EngineLine engine={local.engine} hasModel={local.installed.length > 0} removable />
        </>
      )}
      <p>
        <button type="button" className="text-fg-muted underline hover:text-fg" onClick={() => setNotice(true)}>
          {STUDIO_SETTINGS.whatLocalMeans}
        </button>
      </p>
      {/* Read only: it is accepted before the first local download. */}
      <LocalNotice open={notice} readOnly onAccepted={() => setNotice(false)} onClose={() => setNotice(false)} />
      {prefs && (
        <fieldset>
          <legend className="mb-1 font-medium">{STUDIO_SETTINGS.gpu}</legend>
          <p className="mb-2 text-xs text-fg-muted">{STUDIO_SETTINGS.gpuHelp}</p>
          <div className="space-y-1">
            {GPU_POLICIES.map((policy) => (
              <label key={policy} className="flex items-center gap-2">
                <input
                  type="radio"
                  name="studio-gpu-policy"
                  checked={prefs.gpuPolicy === policy}
                  onChange={() => changePrefs({ gpuPolicy: policy }, setError)}
                />
                {STUDIO_SETTINGS.gpuChoice[policy]}
              </label>
            ))}
          </div>
          <label className="mt-3 flex items-center gap-2">
            <input
              type="checkbox"
              checked={prefs.reloadChatAfter}
              aria-describedby="studio-reload-help"
              onChange={(e) => changePrefs({ reloadChatAfter: e.target.checked }, setError)}
            />
            {STUDIO_SETTINGS.reloadAfter}
          </label>
          <p id="studio-reload-help" className="mt-1 text-xs text-fg-muted">
            {STUDIO_SETTINGS.reloadHelp}
          </p>
        </fieldset>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </section>
  );
}

/** What the gallery holds and where, its folder, and *Delete stopped and failed runs*. */
function Results(props: { usage: Usage | null }) {
  const { usage } = props;
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [asking, setAsking] = useState(false);
  const [deleted, setDeleted] = useState<number | null>(null);
  const [moved, setMoved] = useState(false);

  async function changeFolder() {
    setError(null);
    try {
      const folder = await openDialog({ directory: true, multiple: false });
      if (typeof folder !== "string") return;
      await useStudio.getState().setPrefs({ galleryDir: folder });
      setMoved(true);
      // The line above names the new folder.
      await useStudio.getState().reread();
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

  async function deleteUnfinished() {
    setError(null);
    setDeleted(null);
    try {
      setDeleted(await useStudio.getState().removeUnfinished());
    } catch (e) {
      setError(toErrorInfo(e));
    }
  }

  return (
    <section aria-labelledby="studio-results" className="space-y-2">
      <h3 id="studio-results" className={HEADING}>
        {STUDIO_SETTINGS.results}
      </h3>
      {usage && <p>{STUDIO_SETTINGS.usage(usage.bytes, visiblePath(usage.folder), usage.pictures, usage.videos)}</p>}
      <div className="flex flex-wrap items-center gap-2">
        <button
          type="button"
          className={BUTTON}
          onClick={() => api.studioOpenFolder().catch((e: unknown) => setError(toErrorInfo(e)))}
        >
          {STUDIO_SETTINGS.openFolder}
        </button>
        <button type="button" className={BUTTON} onClick={() => void changeFolder()}>
          {STUDIO_SETTINGS.changeFolder}
        </button>
      </div>
      {moved && <p className="text-xs text-fg-muted">{STUDIO_SETTINGS.folderChanged}</p>}
      <div className="flex flex-wrap items-center gap-2">
        <button type="button" className={BUTTON} onClick={() => setAsking(true)}>
          {STUDIO_SETTINGS.deleteUnfinished}
        </button>
        {deleted !== null && (
          <span role="status" className="text-fg-muted">
            {STUDIO_SETTINGS.deleted(deleted)}
          </span>
        )}
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={STUDIO_SETTINGS.deleteTitle}
        body={STUDIO_SETTINGS.deleteBody}
        note={STUDIO_DELETE.audit}
        cancelLabel={STUDIO_DELETE.cancel}
        actionLabel={STUDIO_DELETE.delete}
        destructive
        onConfirm={() => void deleteUnfinished()}
      />
    </section>
  );
}

/** An amount in US dollars, kept when the box is left or Enter is pressed; one outside
 *  `min`..`max` (or not a number) says `rangeWords` and is not sent. */
function AmountField(props: {
  id: string;
  /** The words before the box, as its label; else `ariaLabel` names it. */
  label?: string;
  ariaLabel?: string;
  /** Words after the box ("or more"). */
  after?: string;
  value: number;
  min: number;
  max: number;
  rangeWords: string;
  disabled?: boolean;
  onCommit(usd: number): void;
}) {
  // What is typed, until it is kept; `null` shows the saved amount.
  const [draft, setDraft] = useState<string | null>(null);
  const [bad, setBad] = useState(false);
  function commit() {
    if (draft === null) return;
    const usd = amountFrom(draft);
    if (usd === null || usd < props.min || usd > props.max) {
      setBad(true);
      return;
    }
    setBad(false);
    setDraft(null);
    if (usd !== props.value) props.onCommit(usd);
  }
  const rangeId = `${props.id}-range`;
  return (
    <div>
      <div className="flex flex-wrap items-center gap-1.5">
        {props.label && <label htmlFor={props.id}>{props.label}</label>}
        <span aria-hidden>{STUDIO_SETTINGS.dollar}</span>
        <input
          id={props.id}
          type="text"
          inputMode="decimal"
          aria-label={props.ariaLabel}
          aria-invalid={bad || undefined}
          aria-describedby={bad ? rangeId : undefined}
          value={draft ?? String(props.value)}
          disabled={props.disabled}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit();
          }}
          className={`${BOX} w-20`}
        />
        {props.after && <span>{props.after}</span>}
      </div>
      {bad && (
        <p id={rangeId} role="alert" className="mt-1 text-xs text-danger">
          {props.rangeWords}
        </p>
      )}
    </div>
  );
}

/** The confirmations and the daily limit for cloud runs. */
function Spending(props: { prefs: StudioPrefs }) {
  const { prefs } = props;
  const limit = prefs.dailyLimitUsd;
  const [error, setError] = useState<ErrorInfo | null>(null);
  // The amount the limit is turned on again at: the newest one, kept in the box here or saved.
  // Unticking leaves the box first, so an amount just kept there may not be saved yet (`limit`
  // still the old one) when the box is unticked: that amount, not `limit`, comes back.
  const [kept, setKept] = useState(limit ?? DEFAULT_LIMIT_USD);
  const [savedLimit, setSavedLimit] = useState(limit);
  if (limit !== savedLimit) {
    setSavedLimit(limit);
    if (limit !== null) setKept(limit);
  }
  return (
    <section aria-labelledby="studio-spending" className="space-y-3">
      <h3 id="studio-spending" className={HEADING}>
        {STUDIO_SETTINGS.spending}
      </h3>
      <label className="flex items-center gap-2">
        <input
          type="checkbox"
          checked={prefs.confirmEveryVideo}
          onChange={(e) => changePrefs({ confirmEveryVideo: e.target.checked }, setError)}
        />
        {STUDIO_SETTINGS.confirmVideo}
      </label>
      <AmountField
        id="studio-confirm-at"
        label={STUDIO_SETTINGS.confirmAt}
        after={STUDIO_SETTINGS.orMore}
        value={prefs.confirmAtUsd}
        min={CONFIRM_AT.min}
        max={CONFIRM_AT.max}
        rangeWords={STUDIO_SETTINGS.confirmAtRange}
        onCommit={(usd) => changePrefs({ confirmAtUsd: usd }, setError)}
      />
      <div>
        <div className="flex flex-wrap items-start gap-3">
          <label className="flex items-center gap-2 py-1">
            <input
              type="checkbox"
              checked={limit !== null}
              aria-describedby="studio-limit-help"
              onChange={(e) => {
                changePrefs({ dailyLimitUsd: e.target.checked ? kept : null }, setError);
              }}
            />
            {STUDIO_SETTINGS.limit}
          </label>
          <AmountField
            // A box turned off or on again starts from the saved amount.
            key={limit === null ? "off" : "on"}
            id="studio-limit"
            ariaLabel={STUDIO_SETTINGS.limitAmount}
            value={limit ?? kept}
            min={DAILY_LIMIT.min}
            max={DAILY_LIMIT.max}
            rangeWords={STUDIO_SETTINGS.limitRange}
            disabled={limit === null}
            onCommit={(usd) => {
              setKept(usd);
              // A refused amount is not kept: the limit comes back at the saved one.
              changePrefs({ dailyLimitUsd: usd }, setError, () =>
                setKept((now) => (now === usd ? (useStudio.getState().prefs?.dailyLimitUsd ?? limit ?? DEFAULT_LIMIT_USD) : now)),
              );
            }}
          />
        </div>
        <p id="studio-limit-help" className="mt-1 text-xs text-fg-muted">
          {STUDIO_SETTINGS.limitHelp}
        </p>
      </div>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </section>
  );
}

/** When each cloud endpoint's picture and video lists were saved, with *Update now*. */
function ModelLists(props: { endpoints: StudioEndpoint[] }) {
  const { endpoints } = props;
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [updating, setUpdating] = useState<number | null>(null);

  async function update(providerId: number) {
    setUpdating(providerId);
    setError(null);
    try {
      await useStudio.getState().refreshModels(providerId);
    } catch (e) {
      setError(toErrorInfo(e));
    } finally {
      setUpdating(null);
    }
  }

  return (
    <section aria-labelledby="studio-lists" className="space-y-2">
      <h3 id="studio-lists" className={HEADING}>
        {STUDIO_SETTINGS.lists}
      </h3>
      {endpoints.length === 0 ? (
        <p className="text-fg-muted">{STUDIO_SETTINGS.noEndpoint}</p>
      ) : (
        <ul className="space-y-1.5">
          {endpoints.map((e) => (
            <li key={e.providerId} className="flex flex-wrap items-center gap-2">
              <span id={`studio-list-${e.providerId}`}>
                {e.fetchedAt === null
                  ? STUDIO_SETTINGS.listNone(e.name)
                  : STUDIO_SETTINGS.listUpdated(e.name, e.fetchedAt)}
              </span>
              <button
                type="button"
                className={BUTTON}
                aria-describedby={`studio-list-${e.providerId}`}
                disabled={updating !== null}
                onClick={() => void update(e.providerId)}
              >
                {e.fetchedAt === null ? STUDIO_ACTIONS.getTheList : STUDIO_SETTINGS.updateNow}
              </button>
            </li>
          ))}
        </ul>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </section>
  );
}
