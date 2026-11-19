import { useState } from "react";
import { formatBytes } from "../../lib/format";
import type { DownloadGroup } from "../../lib/types";
import {
  amountGb,
  downloadProgressWords,
  LOCAL_FIT_WORDS,
  LOCAL_MODELS,
  STUDIO_ACTIONS,
  STUDIO_TIME,
} from "../../lib/words";
import { useDownloads } from "../../store/downloads";
import { useStudio } from "../../store/studio";
import { FitDot } from "../FitBadge";
import { visiblePath } from "../FolderChip";
import { ConfirmDialog } from "./ConfirmDialog";
import { etaText } from "./eta";
import { activeDownload, licenceBadge, offerForEntry } from "./localOffers";
import { CARD_BUTTON, PRIMARY_BUTTON } from "./styles";
import type { InstalledLocal, Licence, LocalEntry, LocalFit, LocalModels, MediaKind } from "./types";

// One model in Get local models (0.1.0 Task B13, local addendum Wireframe L2): a recommended
// entry (name and what it is good at, publisher, total size, the licence in plain words, the fit
// word with its reason, the time here, and *Download*, or *Installed* with *Download again* and
// *Delete*), or an installed model the list does not hold (one from a search, one whose entry
// left the catalog, one this version cannot read). *Download* only sets the store's `offer`
// (localOffers.ts): Task B14's dialog shows every part and licence before anything is fetched.
// While the model downloads (or downloads again), its row shows how far it got in Models →
// Downloads' words, in place of *Download* or *Download again*. The pieces a searched file
// shows too (the licence badge, the fit line, the time, the download) are exported for
// LocalSearch.

const TONE_DOT: Record<"green" | "amber" | "red", string> = {
  green: "bg-success",
  amber: "bg-warning",
  red: "bg-danger",
};

/** The licence badge: a colour dot and the words (never colour alone). */
export function LicenceBadge(props: { licence: Licence }) {
  const { tone, text } = licenceBadge(props.licence);
  return (
    <span
      data-licence={tone}
      title={props.licence.name}
      className="inline-flex items-center gap-1.5 rounded-full border border-border px-2 py-0.5 text-xs"
    >
      <span className={`inline-block h-2 w-2 shrink-0 rounded-full ${TONE_DOT[tone]}`} aria-hidden />
      <span>{text}</span>
    </span>
  );
}

/** The fit word with its colour, then why it does not fit and at what size it would. */
export function FitLine(props: { fit: LocalFit }) {
  const { fit } = props;
  const more = [fit.reason, fit.suggestion].filter((t): t is string => t !== null && t !== "");
  return (
    <div className="flex flex-wrap items-start gap-x-1.5 text-xs text-fg-muted">
      <FitDot verdict={fit.verdict} word={LOCAL_FIT_WORDS[fit.word]} />
      {more.map((t) => (
        <span key={t} className="mt-0.5 flex gap-1.5">
          <span aria-hidden>·</span>
          <span>{t}</span>
        </span>
      ))}
    </div>
  );
}

/** How long a run takes here: "about 2 min a picture", "(first guess)" until measured on this
 *  computer, else "Time shown after your first run here". Nothing for a model that does not
 *  fit. */
export function TimeLine(props: { fit: LocalFit; kind: MediaKind }) {
  const { fit, kind } = props;
  if (fit.word === "notHere") return null;
  const per = kind === "image" ? STUDIO_TIME.perPicture : STUDIO_TIME.aClip;
  return <p className="text-xs text-fg-muted">{etaText(fit.eta, per) ?? LOCAL_MODELS.noTime}</p>;
}

/** The download of local model `genModelId` while it runs, from the app's download groups
 *  (followed while Get local models is open); `null` when none is. */
export function useModelDownload(genModelId: string): DownloadGroup | null {
  return useDownloads((s) => activeDownload(s.groups, genModelId));
}

/** A model's download while it runs (Pause and Cancel are in Models → Downloads): how far it
 *  got, in Models → Downloads' words and sizes (the same download reads the same in both), its
 *  bar, and why it stopped. The offer's and the engine's sizes keep Studio's own. */
export function DownloadProgress(props: { group: DownloadGroup; name: string }) {
  const { group: g } = props;
  const pct = g.bytesTotal > 0 ? Math.min(100, (g.bytesDone / g.bytesTotal) * 100) : 0;
  return (
    <div className="flex w-full flex-col gap-1">
      <p className="text-xs text-fg-muted">{downloadProgressWords(g, formatBytes)}</p>
      <div
        className="h-1.5 overflow-hidden rounded bg-bg-muted"
        role="progressbar"
        aria-label={LOCAL_MODELS.downloading(props.name)}
        aria-valuenow={Math.round(pct)}
        aria-valuemin={0}
        aria-valuemax={100}
      >
        <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
      </div>
      {g.error && (
        <p className="text-xs text-danger">
          {g.error.message} {g.error.nextStep}
        </p>
      )}
    </div>
  );
}

/** *Download again*: the installed model's parts are checked, and what is missing or damaged is
 *  fetched again; then a line says so. When a licence has to be accepted first, the download
 *  dialog shows it instead. */
function RepairButton(props: { id: string; onError: (e: unknown) => void }) {
  const repairLocal = useStudio((s) => s.repairLocal);
  const [busy, setBusy] = useState(false);
  const [started, setStarted] = useState(false);
  if (started) return <p className="text-xs text-fg-muted">{LOCAL_MODELS.repairing}</p>;
  return (
    <button
      type="button"
      className={CARD_BUTTON}
      disabled={busy}
      onClick={() => {
        setBusy(true);
        repairLocal(props.id)
          .then(setStarted, props.onError)
          .finally(() => setBusy(false));
      }}
    >
      {STUDIO_ACTIONS.downloadAgain}
    </button>
  );
}

/** *Delete*, which asks first; the local models are read again after it. */
function DeleteButton(props: { id: string; name: string; onError: (e: unknown) => void }) {
  const deleteLocal = useStudio((s) => s.deleteLocal);
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  return (
    <>
      <button type="button" className={CARD_BUTTON} disabled={busy} onClick={() => setAsking(true)}>
        {LOCAL_MODELS.delete}
      </button>
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={LOCAL_MODELS.deleteTitle(props.name)}
        body={LOCAL_MODELS.deleteBody}
        cancelLabel={LOCAL_MODELS.cancel}
        actionLabel={LOCAL_MODELS.delete}
        destructive
        onConfirm={() => {
          setBusy(true);
          deleteLocal(props.id)
            .catch(props.onError)
            .finally(() => setBusy(false));
        }}
      />
    </>
  );
}

/** What an installed model offers: *Download again* when this version can run it (else why it
 *  cannot, unless the fit line says so already) and `repair` is allowed, and *Delete*. While it
 *  downloads again (`download`), how far that got instead of *Download again*. */
function InstalledActions(props: {
  id: string;
  name: string;
  installed: InstalledLocal | undefined;
  fit: LocalFit;
  repair: boolean;
  download: DownloadGroup | null;
  onError: (e: unknown) => void;
}) {
  const { installed, fit, download } = props;
  const usable = installed?.model.usable ?? null;
  return (
    <>
      <span className="text-xs font-medium text-fg-muted">{LOCAL_MODELS.installed}</span>
      {usable !== null && usable !== fit.reason && <p className="text-xs text-fg-muted">{usable}</p>}
      {props.repair && installed && usable === null && !download && (
        <RepairButton id={props.id} onError={props.onError} />
      )}
      <DeleteButton id={props.id} name={props.name} onError={props.onError} />
      {download && <DownloadProgress group={download} name={props.name} />}
    </>
  );
}

const ROW = "flex flex-col gap-1.5 rounded-xl border border-border bg-surface p-3.5 shadow-soft";

/** A recommended entry, or one that does not fit this computer (`notHere`: no *Download*, and
 *  when installed no *Download again*, only *Delete*). While it downloads, how far that got in
 *  place of *Download*. */
export function EntryRow(props: {
  entry: LocalEntry;
  local: LocalModels;
  notHere: boolean;
  onError: (e: unknown) => void;
}) {
  const { entry, local, notHere } = props;
  const setOffer = useStudio((s) => s.setOffer);
  const download = useModelDownload(entry.id);
  const installed = local.installed.find((m) => m.id === entry.id);
  const downloadable = !entry.installed && !notHere && entry.fit.word !== "notHere";
  return (
    <article aria-label={entry.name} className={ROW}>
      <div className="flex flex-wrap items-baseline gap-x-2">
        <h3 className="text-sm font-medium">{entry.name}</h3>
        {entry.recommended && <span className="ml-auto text-xs font-medium text-accent">{LOCAL_MODELS.recommended}</span>}
      </div>
      <div className="flex flex-wrap items-baseline justify-between gap-x-3 text-sm text-fg-muted">
        <p>{entry.summary}</p>
        <p className="text-xs">{entry.publisher}</p>
      </div>
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <span>{amountGb(entry.sizeBytes)}</span>
        <LicenceBadge licence={entry.licence} />
      </div>
      <FitLine fit={entry.fit} />
      <TimeLine fit={entry.fit} kind={entry.kind} />
      {(downloadable || entry.installed || download) && (
        <div className="flex flex-wrap items-center gap-2 pt-1">
          {entry.installed ? (
            <InstalledActions
              id={entry.id}
              name={entry.name}
              installed={installed}
              fit={entry.fit}
              repair={!notHere}
              download={download}
              onError={props.onError}
            />
          ) : download ? (
            <DownloadProgress group={download} name={entry.name} />
          ) : (
            <button type="button" className={PRIMARY_BUTTON} onClick={() => setOffer(offerForEntry(entry, local))}>
              {LOCAL_MODELS.download}
            </button>
          )}
        </div>
      )}
    </article>
  );
}

/** An installed model the list does not hold: from a search, an entry that left the catalog,
 *  or one this version cannot read. */
export function InstalledRow(props: { row: InstalledLocal; onError: (e: unknown) => void }) {
  const { row } = props;
  const download = useModelDownload(row.id);
  // A name from a search can hold any character: shown with invisible ones as codes.
  const name = visiblePath(row.name);
  return (
    <article aria-label={name} className={ROW}>
      <h3 className="text-sm font-medium">
        <bdi dir="ltr">{name}</bdi>
      </h3>
      <div className="text-xs">
        <span>{amountGb(row.sizeBytes)}</span>
      </div>
      <FitLine fit={row.fit} />
      <div className="flex flex-wrap items-center gap-2 pt-1">
        <InstalledActions
          id={row.id}
          name={name}
          installed={row}
          fit={row.fit}
          repair
          download={download}
          onError={props.onError}
        />
      </div>
    </article>
  );
}
