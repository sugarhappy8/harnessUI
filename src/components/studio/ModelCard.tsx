import { useEffect, useId, useState, type ReactNode } from "react";
import clsx from "clsx";
import {
  ArrowRight,
  CircleCheck,
  CircleHelp,
  CircleSlash,
  Download,
  Heart,
  History,
  Layers,
  LockKeyhole,
  Trash2,
  TriangleAlert,
  Wrench,
} from "lucide-react";
import { formatBytes } from "../../lib/format";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import {
  DOWNLOAD_LOCAL,
  FILE_LABEL,
  GET_MODELS,
  LICENCE_BADGE,
  LOCAL_FIT_WORDS,
  LOCAL_MODELS,
  LOCAL_WARNING,
  MODEL_SHEET,
  REGIONAL_USE,
  revenueLimit,
  SETUP,
  STUDIO_DOWNLOADS,
  STUDIO_TIME,
  typeName,
  updatedWhen,
  VERDICT,
  verdictBadgeWords,
} from "../../lib/words";
import { canPick, useStudio } from "../../store/studio";
import { visiblePath } from "../FolderChip";
import { Tip } from "../ui/Tip";
import { FOCUS_RING } from "../ui/styles";
import { BrowserLink, modelPage } from "./BrowserLink";
import { ConfirmDialog } from "./ConfirmDialog";
import { DownloadState, pickNewModelAndFocus, useModelDownload, useShownDownload } from "./DownloadState";
import { etaText } from "./eta";
import { sizeWords, typeLabel } from "./sheetModels";
import { CARD_BUTTON, MODEL_CARD, PRIMARY_BUTTON } from "./styles";
import type {
  InstalledLocal,
  Licence,
  LocalEntry,
  LocalFit,
  LocalRepo,
  LocalSearchHit,
  Lora,
  MediaKind,
  Verdict,
} from "./types";

// The cards of Get models (Task U5, design 10.3), each an <article> named by its model:
// - CatalogCard: a model of Studio's list, with what it is good at, its size, licence, fit and
//   time here, and Download (on a model that may not fit too: a warning, never a refusal, design
//   9.2), which opens the detail panel (ModelDetail.tsx, Task U6: the review before the
//   download); installed, it says so and offers *Use it* in place of Download, also once the
//   person set it up again as their own (ruling S9-edited-catalog-generic: the backend counts it
//   installed by its row's id, and the card keeps the catalog's name). A gated model with no
//   Hugging Face token saved says what to do in place of a Download that would only fail (ruling
//   S7).
// - ResultCard: a Hugging Face result, its name, author, downloads, likes, when it was updated
//   and its Studio verdict (VerdictBadge): a hint from its name until it is opened, exact once
//   its files were read; opened, a gated one says it needs a token or the model's terms. Its
//   heading button opens the detail panel.
// - InstalledCard: a model on this computer, its type, size and fit, with Repair and Delete.
// - LoraCard: a LoRA on this computer, what it is for, with Delete.
// While a model downloads, its card shows the download (DownloadState.tsx, Task U7) with Pause
// and Cancel, and once it is done "Ready" with *Use it*, which chooses it in the composer, back
// at the results (a result's card for any of its files). Also here, for these cards and the
// detail panel: the licence badge, the fit line, the time line, a gated model's line, and
// whether a Hugging Face token is saved.

/** A licence's badge: its colour (as the backend judged it) and what it allows for what you
 *  make, in plain words. A kind this version does not know reads "Check the licence". */
export function licenceBadge(l: Licence): { tone: "green" | "amber" | "red"; text: string } {
  if (l.commercial === "yes" && l.excludedRegions?.length) return { tone: "amber", text: REGIONAL_USE };
  let text: string;
  switch (l.commercial) {
    case "yes":
      text = LICENCE_BADGE.yes;
      break;
    case "underRevenue":
      text = LICENCE_BADGE.underRevenue(l.revenueLimitUsd === null ? null : revenueLimit(l.revenueLimitUsd));
      break;
    case "registration":
      text = LICENCE_BADGE.registration;
      break;
    case "withUseRules":
      text = LICENCE_BADGE.withUseRules;
      break;
    case "unclear":
      text = LICENCE_BADGE.unclear;
      break;
    case "no":
      text = LICENCE_BADGE.no;
      break;
    default:
      // The backend sends it as a string: a kind this version does not know is not trusted.
      text = LICENCE_BADGE.unclear;
  }
  return { tone: l.badge, text };
}

/** What stops an opened repository's download before it starts: none of its files could be
 *  read for want of a Hugging Face token, or of the model's terms accepted (Review Focus 2:
 *  its files say so, and it offers no file to download); else `null`. */
export function gateProblem(repo: LocalRepo): "hf_needs_token" | "hf_needs_terms" | null {
  if (repo.files.length > 0) return null;
  const reasons = repo.fileViews.flatMap((f) => ("notRead" in f.label ? [f.label.notRead.reason] : []));
  if (reasons.includes("needsToken")) return "hf_needs_token";
  if (reasons.includes("needsTerms")) return "hf_needs_terms";
  return null;
}

/** Whether a Hugging Face token is saved, read while `needed` (a gated catalog model is listed,
 *  or shown in the detail panel), and again when Settings closes, where it is saved; `null`
 *  while not known or not needed. */
export function useTokenSaved(needed: boolean): boolean | null {
  const [saved, setSaved] = useState<boolean | null>(null);
  useEffect(() => {
    if (!needed || !ipc.isTauri()) return;
    let alive = true;
    const read = () =>
      ipc.hubTokenStatus().then(
        (t) => alive && setSaved(t.saved),
        (e: unknown) => console.warn("Studio: whether a Hugging Face token is saved could not be read", e),
      );
    void read();
    window.addEventListener("app:settings-closed", read);
    return () => {
      alive = false;
      window.removeEventListener("app:settings-closed", read);
    };
  }, [needed]);
  return saved;
}

/** A licence badge's or a fit's colour, as a dot. */
export const TONE_DOT: Record<"green" | "amber" | "red", string> = {
  green: "bg-success",
  amber: "bg-warning",
  red: "bg-danger",
};

/** A fit word's colour: green only for "Fits" (critique §6: "Fits" beside an amber dot read as
 *  a contradiction), amber for a condition, red when it may not fit. */
export const FIT_DOT: Record<LocalFit["word"], string> = {
  fits: TONE_DOT.green,
  afterUnload: TONE_DOT.amber,
  slow: TONE_DOT.amber,
  notHere: TONE_DOT.red,
};

/** The colour of a fit dot, wherever one is drawn (the model sheet, a card, a file choice):
 *  by the fit word (`FIT_DOT`). A video made on the processor never reads green, since its
 *  warning says it is very slow. */
export function fitDotClass(fit: Pick<LocalFit, "word" | "videoOnProcessor">): string {
  if (fit.word === "fits" && fit.videoOnProcessor) return TONE_DOT.amber;
  return FIT_DOT[fit.word] ?? TONE_DOT.amber;
}

/** What may go badly on this computer, as the warnings say it: a model that may not fit
 *  ("May not fit this computer: needs 6 GB of graphics memory. It may fail or be very slow."),
 *  a video made on the processor. Warnings, never refusals; nothing here turns a button off. */
export function fitWarnings(fit: Pick<LocalFit, "word" | "reason" | "videoOnProcessor">): string[] {
  const out: string[] = [];
  if (fit.word === "notHere") out.push(fit.reason ? LOCAL_WARNING.mayNotFit(fit.reason) : LOCAL_FIT_WORDS.notHere);
  if (fit.videoOnProcessor) out.push(LOCAL_WARNING.videoOnProcessor);
  return out;
}

/** Warning lines, one per sentence, in one polite status region: beside Create, under a card's
 *  fit and in the detail panel's plan. Nothing when there are none. */
export function WarningLines(props: { lines: string[]; id?: string; className?: string }) {
  if (props.lines.length === 0) return null;
  return (
    <div id={props.id} role="status" data-fit-warning className={clsx("flex flex-col gap-1", props.className)}>
      {props.lines.map((line) => (
        <p key={line} className="flex items-start gap-1.5 text-xs text-warning">
          <TriangleAlert size={14} aria-hidden className="mt-px shrink-0" />
          <span className="min-w-0">{line}</span>
        </p>
      ))}
    </div>
  );
}

/** The warning of a fit, if it has one. */
export function FitWarning(props: { fit: Pick<LocalFit, "word" | "reason" | "videoOnProcessor">; id?: string; className?: string }) {
  return <WarningLines lines={fitWarnings(props.fit)} id={props.id} className={props.className} />;
}

/** The licence badge: a colour dot and the words (never colour alone); its tooltip names the
 *  licence. */
export function LicenceBadge(props: { licence: Licence }) {
  const { tone, text } = licenceBadge(props.licence);
  return (
    <Tip content={props.licence.name}>
      <span
        data-licence={tone}
        tabIndex={0}
        className={`inline-flex max-w-full items-center gap-1.5 rounded-full border border-border px-2 py-0.5 text-xs ${FOCUS_RING}`}
      >
        <span className={`inline-block size-2 shrink-0 rounded-full ${TONE_DOT[tone]}`} aria-hidden />
        <span className="truncate">{text}</span>
      </span>
    </Tip>
  );
}

/** The fit word with its colour (and, given the model's `kind`, its time here under it, at
 *  body size on a card), then why it may not fit and at what size it would. */
export function FitLine(props: { fit: LocalFit; kind?: MediaKind }) {
  const { fit, kind } = props;
  // The reason of a model that may not fit is part of its warning, below; not said twice.
  const more = [fit.word === "notHere" ? null : fit.reason, fit.suggestion].filter((t): t is string => t !== null && t !== "");
  const time = kind === undefined ? null : timeWords(fit, kind);
  return (
    <div className="flex flex-col gap-0.5 text-xs text-fg-muted">
      <p className={clsx("flex flex-wrap items-center gap-x-1.5", kind !== undefined && "text-sm")}>
        <span className={`inline-block size-2 shrink-0 rounded-full ${fitDotClass(fit)}`} aria-hidden />
        <span className="font-medium text-fg">{LOCAL_FIT_WORDS[fit.word] ?? LOCAL_FIT_WORDS.notHere}</span>
      </p>
      {time !== null && <p className="pl-3.5 text-sm">{time}</p>}
      {more.map((t) => (
        <p key={t} className="pl-3.5">
          {t}
        </p>
      ))}
      <FitWarning fit={fit} className="pl-3.5" />
    </div>
  );
}

/** How long a run takes here: "about 2 min a picture", "(first guess)" until measured on this
 *  computer, else "Time shown after your first run here"; `null` for a model that may not fit. */
function timeWords(fit: LocalFit, kind: MediaKind): string | null {
  if (fit.word === "notHere") return null;
  const per = kind === "image" ? STUDIO_TIME.perPicture : STUDIO_TIME.aClip;
  return etaText(fit.eta, per) ?? LOCAL_MODELS.noTime;
}

/** The time line on its own (an opened result's file). Nothing for a model that may not fit. */
export function TimeLine(props: { fit: LocalFit; kind: MediaKind }) {
  const words = timeWords(props.fit, props.kind);
  return words === null ? null : <p className="text-xs text-fg-muted">{words}</p>;
}

/** What a verdict is, for its badge's colour and icon. */
type VerdictTone = "works" | "needsFiles" | "lora" | "cantRun" | "unknown";

function verdictTone(verdict: Verdict): VerdictTone {
  if (verdict === "unknown") return "unknown";
  if ("works" in verdict) return "works";
  if ("needsFiles" in verdict) return "needsFiles";
  if ("lora" in verdict) return "lora";
  return "cantRun";
}

const VERDICT_STYLE: Record<VerdictTone, { exact: string; hint: string; icon: typeof CircleCheck }> = {
  works: { exact: "border-success/30 bg-success/10 text-success", hint: "border-success/40 text-success", icon: CircleCheck },
  needsFiles: { exact: "border-warning/30 bg-warning/10 text-warning", hint: "border-warning/40 text-warning", icon: TriangleAlert },
  lora: { exact: "border-accent/30 bg-accent/10 text-accent", hint: "border-accent/40 text-accent", icon: Layers },
  cantRun: { exact: "border-danger/30 bg-danger/10 text-danger", hint: "border-danger/40 text-danger", icon: CircleSlash },
  unknown: { exact: "border-border bg-bg-muted text-fg-muted", hint: "border-border-strong text-fg-muted", icon: CircleHelp },
};

/** A result's Studio verdict in plain words ("Works in Studio · SDXL", "LoRA for FLUX.1",
 *  "Can't run: Lumina 2", "Open to check"). A hint from its name reads lighter (dashed, no
 *  fill), with the tooltip "From the name: opening it reads the files"; an exact "Can't run"
 *  has the whole reason as its tooltip. */
export function VerdictBadge(props: { verdict: Verdict; exact: boolean }) {
  const { verdict, exact } = props;
  const tone = verdictTone(verdict);
  const style = VERDICT_STYLE[tone];
  const Icon = style.icon;
  const words = verdictBadgeWords(verdict, exact);
  const tip = !exact
    ? VERDICT.hint
    : verdict !== "unknown" && "cannotRun" in verdict
      ? FILE_LABEL.cannotRun(verdict.cannotRun.reason)
      : verdict !== "unknown" && "needsFiles" in verdict
        ? VERDICT.needsFilesTip
        : null;
  const badge = (
    <span
      data-verdict={tone}
      data-exact={exact}
      tabIndex={tip === null ? undefined : 0}
      className={clsx(
        "inline-flex max-w-full items-center gap-1.5 rounded-full border px-2.5 py-0.5 text-sm font-medium",
        exact ? style.exact : ["border-dashed", style.hint],
        tip !== null && FOCUS_RING,
      )}
    >
      <Icon size={14} aria-hidden className="shrink-0" />
      <span className="truncate">{words}</span>
      {!exact && <span className="sr-only">{` (${VERDICT.hint})`}</span>}
    </span>
  );
  return tip === null ? badge : <Tip content={tip}>{badge}</Tip>;
}

/** A gated model's problem in SETUP's words, and its next step: Settings → Hugging Face for a
 *  token, or the model's page for its terms. Shown in place of a Download that would fail (on a
 *  card, at `xs`; in the detail panel, at body size). */
export function GateLine(props: { problem: "hf_needs_token" | "hf_needs_terms"; repo: string | null; size?: "xs" | "sm" }) {
  const [error, setError] = useState<ErrorInfo | null>(null);
  return (
    <div
      data-gate={props.problem}
      className={clsx(
        "relative z-10 flex flex-col gap-1.5 rounded-xl bg-warning/10 px-3 py-2",
        props.size === "sm" ? "text-sm" : "text-xs",
      )}
    >
      <p className="flex items-start gap-1.5 text-fg">
        <LockKeyhole size={13} aria-hidden className="mt-0.5 shrink-0 text-warning" />
        <span>{SETUP.problem[props.problem]}</span>
      </p>
      {props.problem === "hf_needs_token" ? (
        <button
          type="button"
          className={`self-start text-accent underline underline-offset-2 hover:text-fg ${FOCUS_RING}`}
          onClick={() => window.dispatchEvent(new CustomEvent("app:open-settings", { detail: "huggingface" }))}
        >
          {GET_MODELS.openSettings}
        </button>
      ) : (
        props.repo !== null && (
          <span className="self-start">
            <BrowserLink href={modelPage(props.repo)} onError={setError}>
              {DOWNLOAD_LOCAL.openModelPage}
            </BrowserLink>
          </span>
        )
      )}
      {error && (
        <p className="text-danger">
          {error.message} {error.nextStep}
        </p>
      )}
    </div>
  );
}

/** A small pill: the size, a type, an untested note. */
function Pill(props: { children: ReactNode; tone?: "plain" | "warning" | "accent" }) {
  return (
    <span
      className={clsx(
        "inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-xs whitespace-nowrap",
        props.tone === "warning"
          ? "bg-warning/10 text-warning"
          : props.tone === "accent"
            ? "bg-accent/10 font-medium text-accent"
            : "bg-bg-muted text-fg-muted",
      )}
    >
      {props.children}
    </span>
  );
}

const ACTION = `${CARD_BUTTON} inline-flex items-center gap-1.5 ${FOCUS_RING}`;
const PRIMARY = `${PRIMARY_BUTTON} inline-flex items-center gap-1.5 ${FOCUS_RING}`;

/** "Installed", with a check: a catalog model already on this computer. */
function InstalledMark() {
  return (
    <span className="inline-flex items-center gap-1.5 text-sm font-medium text-success">
      <CircleCheck size={15} aria-hidden />
      {GET_MODELS.installed}
    </span>
  );
}

/** *Use it* on an installed model's card: chosen in its tab's composer, back at the results. */
function UseItButton(props: { genModelId: string; nameId: string }) {
  return (
    <button
      type="button"
      aria-describedby={props.nameId}
      className={PRIMARY}
      onClick={() => pickNewModelAndFocus(props.genModelId)}
    >
      {STUDIO_DOWNLOADS.useIt}
      <ArrowRight size={14} aria-hidden />
    </button>
  );
}

/** A model of Studio's list: name, publisher, what it is good at, fit and time, licence, size,
 *  and Download, which opens its detail panel (or how far its download got, "Ready" or
 *  "Installed" with *Use it*, or what a gated model needs first).
 *  `tokenSaved`: whether a Hugging Face token is saved (`null` while not known). */
export function CatalogCard(props: { entry: LocalEntry; tokenSaved: boolean | null }) {
  const { entry } = props;
  const openDetail = useStudio((s) => s.openDetail);
  const pickable = useStudio((s) => canPick(s.local, entry.id));
  const download = useShownDownload((id) => id === entry.id);
  const nameId = useId();
  // Gated: Hugging Face refuses the files without a token (ruling S7).
  const gated = entry.gate.mode !== "none" && props.tokenSaved === false && !entry.installed;
  return (
    <article aria-label={entry.name} data-catalog-card={entry.id} className={MODEL_CARD}>
      <div className="flex items-start gap-2">
        <div className="min-w-0 flex-1">
          <h3 id={nameId} className="text-[15px] leading-snug font-semibold">
            {entry.name}
          </h3>
          <p className="text-meta">{entry.publisher}</p>
        </div>
        {entry.recommended && <Pill tone="accent">{GET_MODELS.topPick}</Pill>}
      </div>
      <p className="line-clamp-2 text-sm text-fg-muted">{entry.summary}</p>
      <FitLine fit={entry.fit} kind={entry.kind} />
      <div className="flex min-w-0 flex-wrap items-center gap-1.5">
        <LicenceBadge licence={entry.licence} />
      </div>
      <div className="mt-auto flex min-h-8 flex-wrap items-center justify-between gap-2 pt-1">
        {download ? (
          <DownloadState
            group={download}
            name={entry.name}
            onUse={pickable ? () => pickNewModelAndFocus(entry.id) : undefined}
          />
        ) : entry.installed ? (
          <>
            <span className="inline-flex items-center gap-3">
              <span className="text-sm text-fg-muted tabular-nums">{formatBytes(entry.sizeBytes)}</span>
              <InstalledMark />
            </span>
            {pickable && <UseItButton genModelId={entry.id} nameId={nameId} />}
          </>
        ) : (
          <>
            <span className="text-sm text-fg-muted tabular-nums">{formatBytes(entry.sizeBytes)}</span>
            {gated ? null : (
              <button
                type="button"
                aria-haspopup="dialog"
                className={PRIMARY}
                onClick={(e) => {
                  // Focused first (WebKit does not focus a pressed button), so focus returns
                  // here as the detail panel closes.
                  e.currentTarget.focus();
                  void openDetail({ entryId: entry.id });
                }}
              >
                <Download size={14} aria-hidden />
                {GET_MODELS.download}
              </button>
            )}
          </>
        )}
      </div>
      {gated && !download && <GateLine problem="hf_needs_token" repo={null} />}
    </article>
  );
}

/** A Hugging Face result: its name (the card's heading button opens it), author, downloads,
 *  likes, when it was updated, and its verdict: exact once it was opened this session (it is in
 *  `repos`), else its name's hint; opened and gated, what it needs first. */
export function ResultCard(props: { hit: LocalSearchHit; onOpen(): void }) {
  const { hit } = props;
  const opened = useStudio((s) => s.repos[hit.repo] ?? null);
  // A download of one of its files (`hf:<repo>:<file>`), running or just finished.
  const download = useShownDownload((id) => id.startsWith(`hf:${hit.repo}:`));
  const downloadedId = download?.genModelId ?? null;
  const pickable = useStudio((s) => downloadedId !== null && canPick(s.local, downloadedId));
  const verdict = opened ? opened.verdict : hit.verdict;
  const exact = opened !== null || hit.exact;
  const gate = opened ? gateProblem(opened) : null;
  // A name from Hugging Face can hold any character: shown with invisible ones as codes.
  const repo = visiblePath(hit.repo);
  const slash = repo.indexOf("/");
  const name = slash < 0 ? repo : repo.slice(slash + 1);
  const author = visiblePath(hit.author);
  const updated = hit.updatedAt === null ? Number.NaN : Date.parse(hit.updatedAt);
  return (
    <article
      aria-label={repo}
      data-result-card
      className={`${MODEL_CARD} relative transition-[border-color,box-shadow] hover:border-border-strong hover:shadow-float`}
    >
      {/* Above the button's area, so its tooltip shows on hover too. */}
      <div className="relative z-10 self-start">
        <VerdictBadge verdict={verdict} exact={exact} />
      </div>
      <div className="min-w-0">
        <h3 className="text-[15px] leading-snug font-semibold">
          {/* The whole card opens the result: the button's area covers it. */}
          <button
            type="button"
            aria-label={repo}
            onClick={(e) => {
              // Focused first (WebKit does not focus a pressed button), so focus returns here
              // as the opened result closes.
              e.currentTarget.focus();
              props.onOpen();
            }}
            className="max-w-full truncate text-left underline-offset-2 outline-none before:absolute before:inset-0 before:rounded-2xl hover:underline focus-visible:before:ring-2 focus-visible:before:ring-accent/40"
          >
            <bdi dir="ltr">{name}</bdi>
          </button>
        </h3>
        <p className="truncate text-sm text-fg-muted">
          <bdi dir="ltr">{author}</bdi>
        </p>
      </div>
      <p className="mt-auto flex flex-wrap items-center gap-x-3 gap-y-0.5 text-xs text-fg-muted">
        <span className="inline-flex items-center gap-1">
          <Download size={12} aria-hidden />
          {GET_MODELS.downloads(hit.downloads)}
        </span>
        <span className="inline-flex items-center gap-1">
          <Heart size={12} aria-hidden />
          {GET_MODELS.likes(hit.likes)}
        </span>
        {Number.isFinite(updated) && (
          <span className="inline-flex items-center gap-1">
            <History size={12} aria-hidden />
            {GET_MODELS.updated(updatedWhen(updated))}
          </span>
        )}
      </p>
      {gate && <GateLine problem={gate} repo={hit.repo} />}
      {download && (
        // Above the heading button's area, so its buttons can be pressed.
        <div className="relative z-10 border-t border-border pt-2.5">
          <DownloadState
            group={download}
            name={name}
            onUse={pickable && downloadedId !== null ? () => pickNewModelAndFocus(downloadedId) : undefined}
          />
        </div>
      )}
    </article>
  );
}

/** *Repair*: the installed model's files are checked, and what is missing or damaged is
 *  fetched again; a model whose files are all on this computer is checked at once. */
function RepairButton(props: { id: string; onError: (e: unknown) => void }) {
  const repairLocal = useStudio((s) => s.repairLocal);
  const [busy, setBusy] = useState(false);
  const [started, setStarted] = useState<"downloading" | "checked" | null>(null);
  if (started !== null) {
    return (
      <p className="text-xs text-fg-muted">
        {started === "downloading" ? LOCAL_MODELS.repairing : LOCAL_MODELS.repairedNow}
      </p>
    );
  }
  return (
    <Tip content={GET_MODELS.repairHint}>
      <button
        type="button"
        className={ACTION}
        disabled={busy}
        onClick={() => {
          setBusy(true);
          repairLocal(props.id)
            .then((group) => setStarted(group === null ? "checked" : "downloading"), props.onError)
            .finally(() => setBusy(false));
        }}
      >
        <Wrench size={14} aria-hidden className="text-fg-muted" />
        {GET_MODELS.repair}
      </button>
    </Tip>
  );
}

/** *Delete*, which asks first (`title`, `body`); `remove` deletes it. */
function DeleteButton(props: {
  title: string;
  body: string;
  remove: () => Promise<void>;
  onError: (e: unknown) => void;
}) {
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  return (
    <>
      <button type="button" className={ACTION} disabled={busy} onClick={() => setAsking(true)}>
        <Trash2 size={14} aria-hidden className="text-fg-muted" />
        {GET_MODELS.delete}
      </button>
      <ConfirmDialog
        open={asking}
        onOpenChange={setAsking}
        title={props.title}
        body={props.body}
        cancelLabel={LOCAL_MODELS.cancel}
        actionLabel={GET_MODELS.delete}
        destructive
        onConfirm={() => {
          setBusy(true);
          props
            .remove()
            .catch(props.onError)
            .finally(() => setBusy(false));
        }}
      />
    </>
  );
}

/** A model on this computer: its name, type, size and fit, then *Repair* (when this version can
 *  run it; else why it cannot) and *Delete*. While it downloads again, how far that got in place
 *  of *Repair*. */
export function InstalledCard(props: { row: InstalledLocal; onError: (e: unknown) => void }) {
  const { row } = props;
  const deleteLocal = useStudio((s) => s.deleteLocal);
  const download = useModelDownload(row.id);
  // A name from a search can hold any character: shown with invisible ones as codes.
  const name = visiblePath(row.name);
  const usable = row.model.usable;
  const type = typeLabel(row);
  return (
    <article aria-label={name} data-installed-card={row.id} className={MODEL_CARD}>
      <div className="min-w-0">
        <h3 className="truncate text-[15px] leading-snug font-semibold">
          <bdi dir="ltr">{name}</bdi>
        </h3>
        <p className="text-meta">{type ?? MODEL_SHEET.needsSetup}</p>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        <Pill>{sizeWords(row)}</Pill>
        {row.untested && <Pill tone="warning">{SETUP.untested}</Pill>}
      </div>
      {!row.needsSetup && <FitLine fit={row.fit} kind={row.kind} />}
      {usable !== null && (row.needsSetup || usable !== row.fit.reason) && <p className="text-xs text-fg-muted">{usable}</p>}
      <div className="mt-auto flex flex-wrap items-center gap-2 pt-1">
        {download ? (
          <DownloadState group={download} name={name} />
        ) : (
          usable === null && <RepairButton id={row.id} onError={props.onError} />
        )}
        <DeleteButton
          title={LOCAL_MODELS.deleteTitle(name)}
          body={
            row.inPlace
              ? LOCAL_MODELS.deleteBodyOwn
              : row.source === "import"
                ? LOCAL_MODELS.deleteBodyCopy
                : LOCAL_MODELS.deleteBody
          }
          remove={() => deleteLocal(row.id)}
          onError={props.onError}
        />
      </div>
    </article>
  );
}

/** A LoRA on this computer: its name, what model type it is for, its size and where it came
 *  from, and *Delete*. */
export function LoraCard(props: { lora: Lora; onError: (e: unknown) => void }) {
  const { lora } = props;
  const deleteLora = useStudio((s) => s.deleteLora);
  const name = visiblePath(lora.name);
  const type = VERDICT.lora(lora.base === null ? null : typeName(lora.base, null));
  return (
    <article aria-label={name} data-lora-card={lora.id} className={MODEL_CARD}>
      <div className="min-w-0">
        <h3 className="truncate text-[15px] leading-snug font-semibold">
          <bdi dir="ltr">{name}</bdi>
        </h3>
        <p className="text-meta">{type}</p>
      </div>
      <div className="flex min-w-0 items-center gap-1.5">
        <Pill>{formatBytes(lora.sizeBytes)}</Pill>
        <p className="min-w-0 truncate text-xs text-fg-muted">
          {lora.repo === null ? GET_MODELS.loraAdded : <bdi dir="ltr">{GET_MODELS.loraFrom(visiblePath(lora.repo))}</bdi>}
        </p>
      </div>
      {lora.missing && <p className="text-xs text-warning">{GET_MODELS.loraMissing}</p>}
      <div className="mt-auto flex flex-wrap items-center gap-2 pt-1">
        <DeleteButton
          title={GET_MODELS.deleteLoraTitle(name)}
          body={GET_MODELS.deleteLoraBody}
          remove={() => deleteLora(lora.id)}
          onError={props.onError}
        />
      </div>
    </article>
  );
}
