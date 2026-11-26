import { useId, useState, type ReactNode } from "react";
import clsx from "clsx";
import { CircleCheck, ExternalLink, LockKeyhole } from "lucide-react";
import { formatBytes } from "../../lib/format";
import type { ErrorInfo } from "../../lib/types";
import {
  DOWNLOAD_LOCAL,
  engineSizeWords,
  licenceWord,
  MODEL_DETAIL,
  REGIONAL_USE,
  revenueLimit,
  SLOT_LABEL,
} from "../../lib/words";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { FOCUS_RING } from "../ui/styles";
import { BrowserLink, modelPage } from "./BrowserLink";
import { fitWarnings, licenceBadge, TONE_DOT } from "./ModelCard";
import { CARD_BUTTON } from "./styles";
import type { Gate, Licence, LocalFit, MediaKind, PartRole, PlannedSlot } from "./types";

// A set-up plan's parts and licences as the review before a download shows them (0.2.0 Task U6;
// shared since Task U8's fix round 1): *Extra files Studio adds* (PlanParts), each part's role in
// plain words, its size and licence and whether it is already on this computer; and the licences
// in plain words with their links and what the pictures or videos you make may be used for
// (Licences, `verdict`). Get models' detail panel (ModelDetail.tsx) and the side panel's Files
// (ModelFilesSection.tsx), before a change that downloads, both draw them from here (design
// 7.4.3: nothing downloads that the plan does not list).

/** The id of the Studio engine's licence among a plan's licences (the backend's
 *  `engine_cmd::ENGINE_LICENCE_ID`): listed when the engine comes with the model. */
export const ENGINE_LICENCE_ID = "studio-engine";

/** How strict a licence is for what you make, from the least strict (the backend's
 *  `Commercial` order). */
const STRICTNESS: Record<Licence["commercial"], number> = {
  yes: 0,
  underRevenue: 1,
  registration: 2,
  withUseRules: 3,
  unclear: 4,
  no: 5,
};

/** `l` is stricter than `than`: a stricter kind, or under a lower revenue limit (an unknown
 *  limit counts as none), as the backend's `gen_catalog::strictness` judges. */
function stricter(l: Licence, than: Licence): boolean {
  const a = STRICTNESS[l.commercial];
  const b = STRICTNESS[than.commercial];
  if (a !== b) return a > b;
  return l.commercial === "underRevenue" && (l.revenueLimitUsd ?? 0) < (than.revenueLimitUsd ?? 0);
}

/** What the pictures (a `kind` "video" model: the videos) you make may be used for, by the
 *  strictest of `licences` (the first of equals): "Pictures you make: free for any use,
 *  including commercial.", the amber line or the red line; "" without a licence. */
export function verdict(licences: Licence[], kind: MediaKind): string {
  const strictest = licences.reduce<Licence | null>((best, l) => (best === null || stricter(l, best) ? l : best), null);
  if (strictest === null) return "";
  if (strictest.commercial === "yes" && licences.some((l) => l.excludedRegions?.length)) {
    return DOWNLOAD_LOCAL.verdictLine(kind, REGIONAL_USE.toLowerCase() + ".");
  }
  const words = DOWNLOAD_LOCAL.verdict[strictest.commercial];
  const allowed =
    typeof words === "string"
      ? words
      : words(strictest.revenueLimitUsd === null ? null : revenueLimit(strictest.revenueLimitUsd));
  return DOWNLOAD_LOCAL.verdictLine(kind, allowed);
}


/** A file's name from its path. */
export function nameOf(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}


/** The parts Studio adds to the main file: each with its file, and each needed one with none. */
function extraSlots(slots: PlannedSlot[]): PlannedSlot[] {
  return slots.filter((s) => s.source !== "main" && (s.part !== null || (s.required && s.source === "missing")));
}


/** What a licence's link says it opens: its name; the Studio engine's by its short word, since
 *  its name is the engine's whole licence line ("stable-diffusion.cpp MIT; ggml MIT; …" reads
 *  "MIT", ruling B8-engine-words). */
function linkName(l: Licence): string {
  return l.id === ENGINE_LICENCE_ID ? licenceWord(l.name) : l.name;
}


/** What may make a download go badly here, said by Download: a model that may not fit this
 *  computer, a video on the processor, little disk left. Warnings, never refusals (design 9.2). */
export function warningsFor(fit: LocalFit, lowDisk: boolean): string[] {
  const out = fitWarnings(fit);
  if (lowDisk) out.push(DOWNLOAD_LOCAL.lowDisk);
  return out;
}


/** A section of the panel: a heading, a quiet line under it, and what it holds. */
export function Section(props: { title: string; note?: ReactNode; children: ReactNode; labelId?: string }) {
  const ownId = useId();
  const id = props.labelId ?? ownId;
  return (
    <section aria-labelledby={id} className="flex flex-col gap-2.5">
      <div className="flex flex-col gap-0.5">
        <h3 id={id} className="text-section">
          {props.title}
        </h3>
        {props.note}
      </div>
      {props.children}
    </section>
  );
}


/** *Extra files Studio adds*: each part of the set-up other than the main file, with its role,
 *  size and licence, marked when it is already on this computer; a needed part with no file
 *  says so and offers *Choose a file* (while a new plan is asked for, `busy`). Nothing when the
 *  main file needs nothing else. */
export function PlanParts(props: {
  slots: PlannedSlot[];
  licences: Licence[];
  onChoose?(role: PartRole): void;
  busy?: boolean;
}) {
  const rows = extraSlots(props.slots);
  const licenceOf = new Map(props.licences.map((l) => [l.id, l]));
  if (rows.length === 0) return null;
  return (
    <Section
      title={MODEL_DETAIL.extras}
      note={
        props.busy && (
          <p role="status" className="text-sm text-fg-muted">
            {MODEL_DETAIL.planning}
          </p>
        )
      }
    >
      <ul className="flex flex-col divide-y divide-border overflow-hidden rounded-xl border border-border">
        {rows.map((s) => (
          <PartRow
            key={s.role}
            slot={s}
            licence={s.part ? (licenceOf.get(s.part.licenceId)?.name ?? null) : null}
            onChoose={props.onChoose}
            busy={props.busy ?? false}
          />
        ))}
      </ul>
    </Section>
  );
}

/** One extra part: what it does, its licence (or the file chosen for it), its size and whether
 *  it is here; with no file, *Choose a file*. */
function PartRow(props: { slot: PlannedSlot; licence: string | null; onChoose?(role: PartRole): void; busy: boolean }) {
  const { slot: s } = props;
  const labelId = useId();
  const p = s.part;
  const detail = p === null ? MODEL_DETAIL.noFile : p.local ? MODEL_DETAIL.chosen(visiblePath(nameOf(p.path))) : props.licence;
  return (
    <li data-part={s.role} className="flex items-center gap-3 px-3 py-2.5">
      <div className="min-w-0 flex-1">
        <p id={labelId} className="text-sm font-medium">
          {SLOT_LABEL[s.role]}
        </p>
        {detail && <p className={clsx("truncate text-xs", p === null ? "text-warning" : "text-fg-muted")}>{detail}</p>}
      </div>
      {p === null ? (
        props.onChoose && (
          <button
            type="button"
            aria-describedby={labelId}
            disabled={props.busy}
            className={`${CARD_BUTTON} shrink-0 ${FOCUS_RING}`}
            onClick={() => props.onChoose?.(s.role)}
          >
            {MODEL_DETAIL.chooseFile}
          </button>
        )
      ) : (
        <div className="flex shrink-0 flex-col items-end gap-0.5 text-right">
          <span className="text-sm tabular-nums">{formatBytes(p.sizeBytes)}</span>
          {p.alreadyHere && (
            <span className="inline-flex items-center gap-1 text-xs text-success">
              <CircleCheck size={12} aria-hidden />
              {MODEL_DETAIL.alreadyHere}
            </span>
          )}
        </div>
      )}
    </li>
  );
}

/** The licences, each in plain words with its colour (the licence badge's) and its link (with
 *  none, `none` says so), then what the pictures or videos you make may be used for (a `kind`
 *  of `null`: nothing Studio makes with it); the Studio engine when it comes with the model
 *  (its licence is about the engine, not what you make), and a gated repository's form. */
export function Licences(props: {
  licences: Licence[];
  kind: MediaKind | null;
  gate: Gate;
  pageRepo: string | null;
  engineBytes: number | null;
  engineLicence: string;
  none?: string;
}) {
  const [error, setError] = useState<ErrorInfo | null>(null);
  if (props.licences.length === 0 && props.none === undefined && props.gate.mode === "none" && props.engineBytes === null) {
    return null;
  }
  const said =
    props.kind === null
      ? ""
      : verdict(
          props.licences.filter((l) => l.id !== ENGINE_LICENCE_ID),
          props.kind,
        );
  return (
    <Section title={MODEL_DETAIL.licences}>
      {props.licences.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {props.licences.map((l) => (
            <LicenceRow key={l.id} licence={l} onError={setError} />
          ))}
        </ul>
      ) : (
        props.none !== undefined && (
          <p data-licence="none" className={clsx("flex items-start gap-2 rounded-xl border px-3 py-2.5 text-sm font-medium", LICENCE_TONE.red)}>
            <span className={`mt-1.5 inline-block size-2 shrink-0 rounded-full ${TONE_DOT.red}`} aria-hidden />
            <span>{props.none}</span>
          </p>
        )
      )}
      {said !== "" && <p className="text-sm font-medium">{said}</p>}
      {props.engineBytes !== null && (
        <p className="text-sm text-fg-muted">{DOWNLOAD_LOCAL.engine(engineSizeWords(props.engineBytes, props.engineLicence))}</p>
      )}
      {props.gate.mode !== "none" && (
        <div className="flex items-start gap-2 text-sm">
          <LockKeyhole size={15} aria-hidden className="mt-0.5 shrink-0 text-fg-muted" />
          <div className="flex flex-col gap-1">
            <p>{DOWNLOAD_LOCAL.gate[props.gate.mode]}</p>
            {props.gate.collects.length > 0 && <p className="text-fg-muted">{DOWNLOAD_LOCAL.collects(props.gate.collects)}</p>}
            {props.pageRepo && (
              <span>
                <BrowserLink href={modelPage(props.pageRepo)} onError={setError}>
                  {DOWNLOAD_LOCAL.openModelPage}
                </BrowserLink>
              </span>
            )}
          </div>
        </div>
      )}
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </Section>
  );
}

const LICENCE_TONE: Record<"green" | "amber" | "red", string> = {
  green: "border-border",
  amber: "border-warning/40 bg-warning/5",
  red: "border-danger/40 bg-danger/5",
};

/** One licence: what it allows in plain words (the backend's; the badge's words when it gave
 *  none), coloured as its badge is, its link, and an amber licence's duties and excluded
 *  regions. */
function LicenceRow(props: { licence: Licence; onError(e: ErrorInfo): void }) {
  const { licence: l } = props;
  const { tone, text } = licenceBadge(l);
  const words = l.plainWords.trim() !== "" ? l.plainWords : text;
  return (
    <li data-licence={tone} className={clsx("flex flex-col gap-1 rounded-xl border px-3 py-2.5", LICENCE_TONE[tone])}>
      <p className="flex items-start gap-2 text-sm font-medium">
        <span className={`mt-1.5 inline-block size-2 shrink-0 rounded-full ${TONE_DOT[tone]}`} aria-hidden />
        <span>{words}</span>
      </p>
      <p className="flex items-center gap-1 pl-4 text-sm">
        <BrowserLink href={l.url} title={l.name} onError={props.onError}>
          {linkName(l)}
        </BrowserLink>
        <ExternalLink size={12} aria-hidden className="shrink-0 text-accent" />
      </p>
      {tone === "amber" &&
        l.obligations.map((o) => (
          <p key={o} className="pl-4 text-sm text-fg-muted">
            {MODEL_DETAIL.duty(o)}
          </p>
        ))}
      {l.excludedRegions?.length ? (
        <p className="pl-4 text-sm text-warning">{DOWNLOAD_LOCAL.excludedRegions(l.name, l.excludedRegions)}</p>
      ) : null}
    </li>
  );
}
