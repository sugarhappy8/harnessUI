import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { RadioGroup } from "radix-ui";
import {
  AlertTriangle,
  ArrowRight,
  CircleCheck,
  CircleSlash,
  Download,
  ExternalLink,
  Layers,
  Sparkles,
} from "lucide-react";
import { formatBytes } from "../../lib/format";
import type { DownloadGroup, ErrorInfo } from "../../lib/types";
import {
  FILE_LABEL,
  fileLabelWords,
  GET_MODELS,
  KIND_WORD,
  LOCAL_FIT_WORDS,
  MODEL_DETAIL,
  SETUP,
  setupProblem,
  SLOT_LABEL,
  STUDIO_DOWNLOADS,
  VERDICT,
} from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useDownloads } from "../../store/downloads";
import { canPick, useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { SelectField } from "../ui/SelectField";
import { SlideOver } from "../ui/SlideOver";
import { FOCUS_RING } from "../ui/styles";
import * as api from "./api";
import { BrowserLink, modelPage } from "./BrowserLink";
import { LocalNotice } from "./LocalNotice";
import { DownloadState, pickNewModelAndFocus, useShownDownload } from "./DownloadState";
import { Licences, nameOf, PlanParts, Section, verdict, warningsFor } from "./PlanParts";
import { fitDotClass, GateLine, gateProblem, useTokenSaved, VerdictBadge, WarningLines } from "./ModelCard";
import { CARD_BUTTON } from "./styles";
import type {
  Gate,
  Licence,
  LocalEntry,
  LocalFileChoice,
  LocalFit,
  LocalModels,
  LocalRepo,
  LocalSource,
  MediaKind,
  PartRole,
  PlannedSlot,
  RepoFileView,
  SetupChoice,
  SetupPlan,
} from "./types";

// A result's or a catalog model's detail panel (0.2.0 Task U6, design 10.3): the review before a
// download, sliding over the right half of Get models. Top to bottom:
// 1. what it is (its verdict and who published it) and its summary;
// 2. *Main file*: one choice per quantisation or precision, each with its size and fit word,
//    *Best for this computer* marked and chosen first (with none, the smallest is). A file that
//    may not fit stays choosable: a warning, never a refusal (design 9.2);
// 3. *Model type*, when the file's version can run with more than one recipe;
// 4. *Extra files Studio adds* (PlanParts): each part's role, size, licence and whether it is
//    already on this computer; a part with no file offers *Choose a file* (.safetensors, .sft or
//    .gguf only: never a pickle file, ruling S9);
// 5. the licences in plain words with their links, what the pictures or videos you make may be
//    used for (`verdict`), then the total to download;
// 6. one Download, which starts the download at once: the panel already shows every file, its
//    role, size and licence, and the total, so no second dialog follows (spec correction 20,
//    coordinator ruling P34). The note about local models still comes first while it was never
//    accepted. Download is off, with the reason, while a part has no file or a gated repository
//    could not be read (with the next step: Settings → Hugging Face for a token, the model's
//    page for its terms, ruling S7). While it downloads, the panel shows how far it got, with
//    Pause and Cancel (DownloadState.tsx, Task U7); once done, "Ready" and *Use it*, which
//    chooses the model in the composer, back at the results; an installed model offers *Use it*
//    too;
// 7. OtherFiles: the LoRAs, each with *Download LoRA*, and the files Studio can't run (or only
//    adds to a model), each with what it is and a quiet *Download file anyway* (0.1.2's plain
//    download).
// A catalog model has one main file and no other files. Names from Hugging Face are shown with
// invisible characters as codes (`visiblePath`).

export { verdict };

/** The installed id a searched repository's file gets (`hf:<repo>:<file>`, the backend's set-up
 *  plan's entry): its download group carries it while it downloads. */
export function searchedModelId(repo: string, file: string): string {
  return `hf:${repo}:${file}`;
}

/** The parts that make the picture: a catalog model's main file is the first of them. */
const MAKES_THE_PICTURE: ReadonlySet<PartRole> = new Set(["diffusion", "high-noise-diffusion", "model"]);

/** The slot a plan's main file fills. */
function mainSlot(slots: PlannedSlot[]): PlannedSlot | null {
  return slots.find((s) => s.source === "main") ?? null;
}

/** A searched file's own size (the main part), as its choice says it. */
function sizeOf(repo: LocalRepo, f: LocalFileChoice): number {
  return (
    mainSlot(f.plan.slots)?.part?.sizeBytes ??
    repo.fileViews.find((v) => v.path === f.file)?.sizeBytes ??
    f.plan.totalBytes
  );
}

/** The main file chosen first: the best for this computer, else the smallest. */
function firstChoice(repo: LocalRepo): LocalFileChoice | null {
  const best = repo.files.find((f) => f.best);
  if (best) return best;
  return repo.files.reduce<LocalFileChoice | null>((small, f) => (small === null || sizeOf(repo, f) < sizeOf(repo, small) ? f : small), null);
}

/** Each main file's name on its choice: its quantisation, else (none, or two files with the
 *  same one) the file's name. */
function choiceLabels(files: LocalFileChoice[]): Map<string, string> {
  const count = new Map<string, number>();
  for (const f of files) count.set(f.quant, (count.get(f.quant) ?? 0) + 1);
  return new Map(files.map((f) => [f.file, f.quant !== "" && count.get(f.quant) === 1 ? f.quant : visiblePath(nameOf(f.file))]));
}

/** A repository's licences, with its set-up plan's when it has one: each licence once (by its
 *  id), the plan's first (the files a download fetches), then the repository's own, which
 *  shows whether or not Studio can set the repository up. */
export function repoLicences(repo: LocalRepo, plan: SetupPlan | null): Licence[] {
  const seen = new Set<string>();
  const out: Licence[] = [];
  for (const l of [...(plan?.licences ?? []), ...repo.licences, ...(repo.licence ? [repo.licence] : [])]) {
    if (seen.has(l.id)) continue;
    seen.add(l.id);
    out.push(l);
  }
  return out;
}

/** No gate: a gated repository that could not be read says what to do in its gated line. */
const NO_GATE: Gate = { mode: "none", collects: [] };

/** Runs a download once the note about local models was accepted: at once when it was, else
 *  after *I understand* (the backend refuses local downloads until then); *Cancel* runs
 *  nothing. `notice` is the note, to render. */
function useNoticeFirst(accepted: boolean): { run(action: () => void): void; notice: ReactNode } {
  const [waiting, setWaiting] = useState<(() => void) | null>(null);
  return {
    run: (action) => (accepted ? action() : setWaiting(() => action)),
    notice: (
      <LocalNotice
        open={waiting !== null}
        onAccepted={() => {
          setWaiting(null);
          waiting?.();
        }}
        onClose={() => setWaiting(null)}
      />
    ),
  };
}

/** The detail panel of the store's `detail`: a SlideOver over the right half, titled by the
 *  repository or the model's name; closed while there is none. */
export function ModelDetail() {
  const detail = useStudio((s) => s.detail);
  const detailError = useStudio((s) => s.detailError);
  const closeDetail = useStudio((s) => s.closeDetail);
  const local = useStudio((s) => s.local);
  const repo = useStudio((s) => (s.detail !== null && "repo" in s.detail ? (s.repos[s.detail.repo] ?? null) : null));
  const entryId = detail !== null && "entryId" in detail ? detail.entryId : null;
  const entry =
    entryId !== null && local !== null ? ([...local.recommended, ...local.notHere].find((e) => e.id === entryId) ?? null) : null;
  const title = detail === null ? "" : "repo" in detail ? visiblePath(detail.repo) : (entry?.name ?? "");
  return (
    <SlideOver open={detail !== null} onOpenChange={(open) => !open && closeDetail()} title={title} width="half">
      {detail !== null && "repo" in detail && (
        <div className="flex min-h-full flex-col gap-4">
          {detailError && <ErrorBanner error={detailError} onDismiss={closeDetail} />}
          {repo && local ? (
            <RepoReview key={`${repo.repo}@${repo.commit}`} repo={repo} local={local} />
          ) : (
            !detailError && (
              <p role="status" className="text-sm text-fg-muted">
                {MODEL_DETAIL.reading}
              </p>
            )
          )}
        </div>
      )}
      {entry && local && <EntryReview key={entry.id} entry={entry} local={local} />}
    </SlideOver>
  );
}

/** A Hugging Face result reviewed: its main files, model type and extra files as its plan for
 *  the chosen file says, the licences and the total, Download, and its other files. A change of
 *  model type or a file chosen for a part asks for a new plan; the main file changed after that
 *  keeps them (it is planned again with them). */
function RepoReview(props: { repo: LocalRepo; local: LocalModels }) {
  const { repo, local } = props;
  const setupPlan = useStudio((s) => s.setupPlan);
  const downloadSetup = useStudio((s) => s.downloadSetup);
  const chooseFile = useStudio((s) => s.chooseFile);
  const openRepo = useStudio((s) => s.openRepo);
  // The main file the person chose (`null`: the first choice), and the plan made for a file
  // with the person's set-up choice (`null`: each file's own plan, as it comes).
  const [picked, setPicked] = useState<string | null>(null);
  const [made, setMade] = useState<{ file: string; choice: SetupChoice; plan: SetupPlan } | null>(null);
  const [planning, setPlanning] = useState(false);
  // Why a plan or a download was refused.
  const [problem, setProblem] = useState<ErrorInfo | null>(null);
  const [starting, setStarting] = useState(false);
  const asked = useRef(0);
  const { run, notice } = useNoticeFirst(local.noticeAccepted);
  const chosen = repo.files.find((f) => f.file === picked) ?? firstChoice(repo);
  const current = made !== null && chosen !== null && made.file === chosen.file ? made : null;
  const plan = current?.plan ?? chosen?.plan ?? null;
  const choice = current?.choice ?? null;
  const gate = gateProblem(repo);
  const chosenId = chosen ? searchedModelId(repo.repo, chosen.file) : "";
  const group = useShownDownload((id) => id === chosenId);
  const pickable = useStudio((s) => chosenId !== "" && canPick(s.local, chosenId));

  // Settings closed (a token may have been saved there): a refusal is said no more, and a
  // repository that could not be read for want of a token is read again.
  useEffect(() => {
    const closed = () => {
      setProblem(null);
      if (gate === "hf_needs_token") {
        openRepo(repo.repo).catch((e: unknown) => console.warn("Studio: the repository could not be read again", e));
      }
    };
    window.addEventListener("app:settings-closed", closed);
    return () => window.removeEventListener("app:settings-closed", closed);
  }, [gate, openRepo, repo.repo]);

  const sourceOf = (f: LocalFileChoice): LocalSource => ({ search: { repo: repo.repo, commit: repo.commit, file: f.file } });

  /** Asks for `f`'s plan with `next`; only the last answer is kept, and a refusal leaves the
   *  plan shown as it was (Download sends the choice of the plan shown). */
  function replan(f: LocalFileChoice, next: SetupChoice) {
    const mine = ++asked.current;
    setPlanning(true);
    setProblem(null);
    setupPlan(sourceOf(f), next).then(
      (p) => {
        if (mine !== asked.current) return;
        setMade({ file: f.file, choice: next, plan: p });
        setPlanning(false);
      },
      (e: unknown) => {
        if (mine !== asked.current) return;
        setProblem(toErrorInfo(e));
        setPlanning(false);
      },
    );
  }

  function onFile(file: string) {
    const f = repo.files.find((x) => x.file === file);
    if (!f) return;
    setPicked(file);
    setProblem(null);
    // The person's model type and chosen files carry over to the other file.
    if (made !== null) {
      replan(f, made.choice);
    } else {
      // A plan still on its way is for the file before: dropped.
      asked.current++;
      setPlanning(false);
    }
  }

  function onRecipe(id: string) {
    if (!chosen) return;
    const overrides = choice?.overrides ?? [];
    if (id === chosen.plan.recipe.id && overrides.length === 0) {
      // Back to the file's own: its plan as it comes.
      asked.current++;
      setMade(null);
      setPlanning(false);
      return;
    }
    replan(chosen, { recipe: id, overrides });
  }

  async function onChoose(role: PartRole) {
    if (!chosen) return;
    const f = chosen;
    let path: string | null;
    try {
      path = await chooseFile("model");
    } catch (e) {
      setProblem(toErrorInfo(e));
      return;
    }
    if (path === null) return;
    const overrides = [...(choice?.overrides ?? []).filter((o) => o.role !== role), { role, file: { disk: { path } } }];
    replan(f, { recipe: choice?.recipe ?? null, overrides });
  }

  function download() {
    if (!chosen) return;
    const source = sourceOf(chosen);
    const sent = choice;
    run(() => {
      setStarting(true);
      setProblem(null);
      downloadSetup(source, sent)
        .catch((e: unknown) => setProblem(toErrorInfo(e)))
        .finally(() => setStarting(false));
    });
  }

  const refused = setupProblem(problem?.code);
  const why: ReactNode =
    gate !== null ? (
      <GateLine problem={gate} repo={repo.repo} size="sm" />
    ) : refused === "hf_needs_token" || refused === "hf_needs_terms" ? (
      <GateLine problem={refused} repo={repo.repo} size="sm" />
    ) : plan !== null && (!plan.canDownload || refused === "studio_setup_incomplete") ? (
      <Why>{MODEL_DETAIL.missingWhy}</Why>
    ) : null;
  // A refusal said in `why` keeps Download on (to try again), except for a missing part.
  const off = gate !== null || (plan !== null && !plan.canDownload) || planning;
  const kind: MediaKind = plan?.recipe.kind === "video" ? "video" : "image";
  const installed = chosen !== null && local.installed.some((m) => m.id === searchedModelId(repo.repo, chosen.file));
  const main = plan ? mainSlot(plan.slots) : null;
  const licences = repoLicences(repo, plan);
  const licenceOf = new Map(licences.map((l) => [l.id, l]));
  const labels = choiceLabels(repo.files);
  const lorasOnly = repo.files.length === 0 && repo.verdict !== "unknown" && "lora" in repo.verdict;

  return (
    <div className="flex min-h-full flex-col gap-6">
      <Summary
        badge={<VerdictBadge verdict={repo.verdict} exact />}
        by={visiblePath(repo.author)}
        summary={repo.summary}
        note={repo.labelMismatch}
        link={<FilesLink repo={repo} />}
      />
      {chosen && plan ? (
        <>
          <MainFileList
            choices={repo.files.map((f) => ({
              value: f.file,
              label: labels.get(f.file) ?? f.file,
              sizeBytes: sizeOf(repo, f),
              fit: f === chosen ? plan.fit : f.plan.fit,
              best: f.best,
            }))}
            value={chosen.file}
            onChange={onFile}
            path={chosen.file}
            role={main?.role ?? null}
            licence={main?.part ? (licenceOf.get(main.part.licenceId)?.name ?? null) : null}
          />
          {plan.recipes.length > 1 && (
            <div className="flex flex-wrap items-end gap-x-3 gap-y-1.5">
              <div className="min-w-56">
                <SelectField
                  label={SETUP.modelType}
                  value={plan.recipe.id}
                  options={plan.recipes.map((r) => ({ value: r.id, label: r.name, hint: KIND_WORD[r.kind] }))}
                  onChange={onRecipe}
                  disabled={planning}
                />
              </div>
              {plan.untested && (
                <span className="mb-1.5 rounded-full bg-warning/10 px-2 py-0.5 text-xs text-warning">{SETUP.untested}</span>
              )}
            </div>
          )}
          <PlanParts slots={plan.slots} licences={plan.licences} onChoose={(role) => void onChoose(role)} busy={planning} />
          <Licences
            licences={licences}
            kind={kind}
            gate={repo.gate}
            pageRepo={repo.repo}
            engineBytes={plan.needsEngine ? local.engine.downloadBytes : null}
            engineLicence={local.engine.licenseName}
            none={MODEL_DETAIL.noLicence}
          />
          <ActionBar
            total={plan.toFetchBytes + (plan.needsEngine ? local.engine.downloadBytes : 0)}
            warnings={warningsFor(plan.fit, plan.lowDisk)}
            why={why}
            off={off}
            busy={starting}
            planning={planning}
            installed={installed}
            group={group}
            name={visiblePath(nameOf(chosen.file))}
            onUse={pickable ? () => pickNewModelAndFocus(chosenId) : undefined}
            error={refused === null ? problem : null}
            onDismiss={() => setProblem(null)}
            onDownload={download}
          />
        </>
      ) : (
        // No set-up: the repository's own licence all the same, before its files and LoRAs
        // (their downloads stay offered), and a gated one's Download, off with what to do.
        <>
          {gate === null && !lorasOnly && <NoSetup repo={repo} />}
          <Licences
            licences={licences}
            kind={null}
            gate={gate === null ? repo.gate : NO_GATE}
            pageRepo={repo.repo}
            engineBytes={null}
            engineLicence={local.engine.licenseName}
            none={MODEL_DETAIL.noLicence}
          />
          {gate !== null && (
            <ActionBar
              total={null}
              warnings={[]}
              why={why}
              off
              busy={false}
              planning={false}
              installed={false}
              group={null}
              name={visiblePath(repo.repo)}
              error={null}
              onDismiss={() => undefined}
              onDownload={() => undefined}
            />
          )}
        </>
      )}
      <OtherFiles repo={repo} run={run} />
      {notice}
    </div>
  );
}

/** A catalog model reviewed: its one main file, the parts that come with it, the licences and
 *  the total, and Download. A gated one says what it needs first while no Hugging Face token is
 *  saved. */
function EntryReview(props: { entry: LocalEntry; local: LocalModels }) {
  const { entry, local } = props;
  const downloadSetup = useStudio((s) => s.downloadSetup);
  const [problem, setProblem] = useState<ErrorInfo | null>(null);
  const [starting, setStarting] = useState(false);
  const { run, notice } = useNoticeFirst(local.noticeAccepted);
  const tokenSaved = useTokenSaved(entry.gate.mode !== "none" && !entry.installed);
  const group = useShownDownload((id) => id === entry.id);
  const pickable = useStudio((s) => canPick(s.local, entry.id));
  const mainIndex = Math.max(
    0,
    entry.parts.findIndex((p) => MAKES_THE_PICTURE.has(p.role)),
  );
  const main = entry.parts[mainIndex] ?? null;
  const slots: PlannedSlot[] = entry.parts.map((part, i) => ({
    role: part.role,
    required: true,
    source: i === mainIndex ? "main" : "companion",
    part,
  }));
  const licenceOf = new Map(entry.licences.map((l) => [l.id, l]));
  const refused = setupProblem(problem?.code);
  const pageRepo = main?.repo || null;
  const gated = entry.gate.mode !== "none" && tokenSaved === false && !entry.installed;

  useEffect(() => {
    const closed = () => setProblem(null);
    window.addEventListener("app:settings-closed", closed);
    return () => window.removeEventListener("app:settings-closed", closed);
  }, []);

  function download() {
    run(() => {
      setStarting(true);
      setProblem(null);
      downloadSetup({ catalog: { entryId: entry.id } }, null)
        .catch((e: unknown) => setProblem(toErrorInfo(e)))
        .finally(() => setStarting(false));
    });
  }

  const why: ReactNode = gated ? (
    <GateLine problem="hf_needs_token" repo={pageRepo} size="sm" />
  ) : refused === "hf_needs_token" || refused === "hf_needs_terms" ? (
    <GateLine problem={refused} repo={pageRepo} size="sm" />
  ) : null;

  return (
    <div className="flex min-h-full flex-col gap-6">
      <Summary
        badge={
          entry.recommended ? (
            <span className="inline-flex items-center gap-1 rounded-full bg-accent/10 px-2.5 py-0.5 text-sm font-medium text-accent">
              <Sparkles size={14} aria-hidden />
              {GET_MODELS.topPick}
            </span>
          ) : null
        }
        by={entry.publisher}
        summary={entry.summary}
        note={null}
      />
      {main && (
        <MainFileList
          choices={[{ value: main.path, label: visiblePath(nameOf(main.path)), sizeBytes: main.sizeBytes, fit: entry.fit, best: false }]}
          value={main.path}
          path={main.path}
          role={main.role}
          licence={licenceOf.get(main.licenceId)?.name ?? null}
        />
      )}
      <PlanParts slots={slots} licences={entry.licences} />
      <Licences
        licences={entry.licences}
        kind={entry.kind}
        gate={entry.gate}
        pageRepo={pageRepo}
        engineBytes={entry.needsEngine ? local.engine.downloadBytes : null}
        engineLicence={local.engine.licenseName}
      />
      <ActionBar
        total={entry.toFetchBytes + (entry.needsEngine ? local.engine.downloadBytes : 0)}
        warnings={warningsFor(entry.fit, entry.lowDisk)}
        why={why}
        off={gated}
        busy={starting}
        planning={false}
        installed={entry.installed}
        group={group}
        name={entry.name}
        onUse={pickable ? () => pickNewModelAndFocus(entry.id) : undefined}
        error={refused === null ? problem : null}
        onDismiss={() => setProblem(null)}
        onDownload={download}
      />
      {notice}
    </div>
  );
}

/** What the model is, who published it (and a link to its files), its summary, and a note
 *  about its name. */
function Summary(props: { badge: ReactNode; by: string; summary: string | null; note: string | null; link?: ReactNode }) {
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        {props.badge}
        {props.by !== "" && (
          <span className="text-sm text-fg-muted">
            <bdi dir="ltr">{MODEL_DETAIL.by(props.by)}</bdi>
          </span>
        )}
        {props.link && <span className="ml-auto">{props.link}</span>}
      </div>
      {props.summary && <p className="text-sm leading-relaxed text-fg">{props.summary}</p>}
      {props.note && <p className="text-xs text-fg-muted">{props.note}</p>}
    </div>
  );
}

/** One main file to choose: its name on the choice, its own size, its fit here, and whether it
 *  is the best for this computer. */
interface MainChoice {
  value: string;
  label: string;
  sizeBytes: number;
  fit: LocalFit;
  best: boolean;
}

const TILE =
  "group relative flex min-w-0 flex-col gap-1 rounded-xl border border-border bg-surface px-3 pt-3 pb-2.5 text-left " +
  "transition-colors data-[state=checked]:border-accent data-[state=checked]:bg-accent/5 " +
  "data-[state=checked]:shadow-[inset_0_0_0_1px_var(--accent)]";

/** *Main file*: one choice per quantisation or precision (a radio group; one file alone is shown
 *  as chosen), each with its size and fit word, *Best for this computer* marked. Under them,
 *  the chosen file's path; above, its role and licence. */
export function MainFileList(props: {
  choices: MainChoice[];
  value: string;
  onChange?(value: string): void;
  path: string;
  role: PartRole | null;
  licence: string | null;
}) {
  const id = useId();
  const about = [props.role === null ? null : SLOT_LABEL[props.role], props.licence].filter((t): t is string => !!t);
  // The chosen file's path, unless its choice already says it whole.
  const path = visiblePath(props.path);
  const said = props.choices.find((c) => c.value === props.value)?.label === path;
  const tiles = "grid grid-cols-[repeat(auto-fill,minmax(min(100%,11.5rem),1fr))] gap-x-2 gap-y-3.5 pt-2.5";
  return (
    <Section
      title={MODEL_DETAIL.mainFile}
      labelId={id}
      note={about.length > 0 && <p className="text-sm text-fg-muted">{about.join(" · ")}</p>}
    >
      {props.choices.length > 1 ? (
        <RadioGroup.Root
          aria-labelledby={id}
          value={props.value}
          onValueChange={(v) => props.onChange?.(v)}
          className={tiles}
        >
          {props.choices.map((c) => (
            <RadioGroup.Item key={c.value} value={c.value} className={`${TILE} hover:border-border-strong ${FOCUS_RING}`}>
              <ChoiceBody choice={c} mark={<RadioMark />} />
            </RadioGroup.Item>
          ))}
        </RadioGroup.Root>
      ) : (
        <div className={tiles}>
          {props.choices.map((c) => (
            // One file alone takes the row, so its whole name shows.
            <div key={c.value} data-state="checked" className={`${TILE} col-span-full`}>
              <ChoiceBody choice={c} mark={<CircleCheck size={16} aria-hidden className="shrink-0 text-accent" />} />
            </div>
          ))}
        </div>
      )}
      {!said && (
        <p className="truncate text-xs text-fg-muted" title={path}>
          <bdi dir="ltr">{path}</bdi>
        </p>
      )}
    </Section>
  );
}

/** A radio's circle, filled while it is the one chosen. */
function RadioMark() {
  return (
    <span
      aria-hidden
      className="grid size-4 shrink-0 place-items-center rounded-full border border-border-strong bg-surface group-data-[state=checked]:border-accent"
    >
      <span className="size-2 scale-0 rounded-full bg-accent transition-transform group-data-[state=checked]:scale-100" />
    </span>
  );
}

/** A main file choice's words: its name and size, its fit word (with its colour), and the best
 *  for this computer's mark. */
function ChoiceBody(props: { choice: MainChoice; mark: ReactNode }) {
  const { choice: c } = props;
  return (
    <>
      <span className="flex min-w-0 items-center gap-2">
        {props.mark}
        <span className="min-w-0 flex-1 truncate text-sm font-semibold">
          <bdi dir="ltr">{c.label}</bdi>
        </span>
        <span className="shrink-0 text-sm text-fg-muted tabular-nums">{formatBytes(c.sizeBytes)}</span>
      </span>
      <span className="flex items-start gap-1.5 pl-6 text-sm text-fg-muted">
        <span className={`mt-1.5 inline-block size-2 shrink-0 rounded-full ${fitDotClass(c.fit)}`} aria-hidden />
        {LOCAL_FIT_WORDS[c.fit.word] ?? LOCAL_FIT_WORDS.notHere}
      </span>
      {c.best && (
        <span className="absolute -top-2.5 right-2.5 inline-flex items-center gap-1 rounded-full bg-accent px-2 py-px text-xs font-medium text-accent-fg shadow-soft">
          <Sparkles size={11} aria-hidden />
          {MODEL_DETAIL.best}
        </span>
      )}
    </>
  );
}

/** Why Download is off, in words. */
function Why(props: { children: ReactNode }) {
  return (
    <p className="flex items-start gap-2 text-sm font-medium text-warning">
      <AlertTriangle size={15} aria-hidden className="mt-0.5 shrink-0" />
      {props.children}
    </p>
  );
}

/** The panel's foot, kept in view at the bottom while the panel scrolls: the warnings, the
 *  total and Download (off, with `why`, while it would fail; "Installed" for a model already
 *  here, with *Use it* given `onUse`), or how far its download got, with Pause and Cancel, and
 *  once it is done, "Ready" with *Use it*. */
function ActionBar(props: {
  total: number | null;
  warnings: string[];
  why: ReactNode;
  off: boolean;
  busy: boolean;
  planning: boolean;
  installed: boolean;
  group: DownloadGroup | null;
  name: string;
  onUse?(): void;
  error: ErrorInfo | null;
  onDismiss(): void;
  onDownload(): void;
}) {
  const whyId = useId();
  return (
    <div
      data-action-bar
      className="sticky -bottom-4 z-10 -mx-4 mt-auto flex flex-col gap-2.5 border-t border-border bg-surface/95 px-4 pt-3 pb-4 backdrop-blur"
    >
      <WarningLines lines={props.warnings} />
      {props.error && <ErrorBanner error={props.error} onDismiss={props.onDismiss} />}
      {props.group ? (
        <DownloadState group={props.group} name={props.name} onUse={props.onUse} size="md" />
      ) : (
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-2">
          {props.total !== null && !props.installed && (
            <p className="text-sm font-semibold tabular-nums">{MODEL_DETAIL.total(formatBytes(props.total))}</p>
          )}
          {props.installed ? (
            <>
              <span className="inline-flex items-center gap-1.5 text-sm font-medium text-success">
                <CircleCheck size={16} aria-hidden />
                {MODEL_DETAIL.installed}
              </span>
              {props.onUse && (
                <button
                  type="button"
                  onClick={props.onUse}
                  className={`btn-primary ml-auto inline-flex h-9 items-center gap-2 rounded-xl px-4 text-sm font-semibold ${FOCUS_RING}`}
                >
                  {STUDIO_DOWNLOADS.useIt}
                  <ArrowRight size={16} aria-hidden />
                </button>
              )}
            </>
          ) : (
            <button
              type="button"
              disabled={props.off || props.busy}
              aria-describedby={props.why ? whyId : undefined}
              onClick={props.onDownload}
              className={`btn-primary ml-auto inline-flex h-9 items-center gap-2 rounded-xl px-4 text-sm font-semibold disabled:cursor-not-allowed ${FOCUS_RING}`}
            >
              <Download size={16} aria-hidden />
              {MODEL_DETAIL.download}
            </button>
          )}
        </div>
      )}
      {props.why && !props.group && <div id={whyId}>{props.why}</div>}
    </div>
  );
}

/** A repository Studio can set nothing up from (and not a gated one: its line says what it
 *  needs): why, and that its files can still be downloaded one at a time. */
function NoSetup(props: { repo: LocalRepo }) {
  const v = props.repo.verdict;
  const why = v === "unknown" ? VERDICT.notRecognised : "cannotRun" in v ? FILE_LABEL.cannotRun(v.cannotRun.reason) : null;
  return (
    <div className="flex items-start gap-3 rounded-xl border border-border bg-bg-subtle px-3.5 py-3">
      <CircleSlash size={18} aria-hidden className="mt-0.5 shrink-0 text-danger" />
      <div className="flex flex-col gap-1 text-sm">
        {why && <p className="font-medium">{why}</p>}
        <p className="text-fg-muted">{MODEL_DETAIL.noSetup}</p>
      </div>
    </div>
  );
}

/** Whether a repository's file is listed with its plain download: a file Studio can't run, an
 *  extra file, or one it did not read (budget, failure); never one that works (a main file or
 *  its sibling: in a set-up), nor one that needs a token or the model's terms, whose download
 *  would only fail (its gated line says what to do). */
function listed(v: RepoFileView): boolean {
  if ("cannotRun" in v.label || "extra" in v.label) return true;
  return "notRead" in v.label && (v.label.notRead.reason === "budget" || v.label.notRead.reason === "failed");
}

/** The repository's other files: the LoRAs, each with *Download LoRA* (after the note about
 *  local models, as a model's download), then the files Studio can't run or only adds, each
 *  with what it is and a quiet *Download file anyway*. Nothing when there are none. */
export function OtherFiles(props: { repo: LocalRepo; run(action: () => void): void }) {
  const { repo } = props;
  const [error, setError] = useState<ErrorInfo | null>(null);
  const loraPaths = new Set(repo.loras.map((l) => l.path));
  const loras = [...repo.loras, ...repo.fileViews.filter((v) => "lora" in v.label && !loraPaths.has(v.path))];
  const files = repo.fileViews.filter((v) => !("lora" in v.label) && !loraPaths.has(v.path) && listed(v));
  if (loras.length + files.length === 0) return null;
  return (
    <Section title={MODEL_DETAIL.otherFiles}>
      <ul className="flex flex-col divide-y divide-border overflow-hidden rounded-xl border border-border">
        {loras.map((v) => (
          <LoraRow key={v.path} repo={repo} file={v} run={props.run} onError={setError} />
        ))}
        {files.map((v) => (
          <FileRow key={v.path} repo={repo} file={v} onError={setError} />
        ))}
      </ul>
      {error && <ErrorBanner error={error} onDismiss={() => setError(null)} />}
    </Section>
  );
}

/** Every file of the repository, at the commit it was read at, on Hugging Face: 0.1.2's plain
 *  download stays within reach for any file the panel does not list (design 7.3). */
function FilesLink(props: { repo: LocalRepo }) {
  const [error, setError] = useState<ErrorInfo | null>(null);
  return (
    <span className="flex flex-col items-end gap-0.5 text-sm">
      <span className="inline-flex items-center gap-1">
        <BrowserLink href={`${modelPage(props.repo.repo)}/tree/${props.repo.commit}`} onError={setError}>
          {MODEL_DETAIL.openFiles}
        </BrowserLink>
        <ExternalLink size={12} aria-hidden className="shrink-0 text-accent" />
      </span>
      {error && (
        <span role="alert" className="text-xs text-danger">
          {error.message} {error.nextStep}
        </span>
      )}
    </span>
  );
}

/** A file row's name and what it is: "Can't run in Studio: …", "Extra file · Decoder (VAE)",
 *  "LoRA for SDXL" (a LoRA whose model type is not known says so, and is never said not to fit,
 *  ruling R6-lora-none), with its size. */
function FileWords(props: { file: RepoFileView; nameId: string }) {
  const { file: f } = props;
  return (
    <div className="min-w-0 flex-1">
      <p id={props.nameId} className="truncate text-sm font-medium" title={visiblePath(f.path)}>
        <bdi dir="ltr">{visiblePath(nameOf(f.path))}</bdi>
      </p>
      <p className="text-xs text-fg-muted">
        {fileLabelWords(f.label, f.path)} · {formatBytes(f.sizeBytes)}
      </p>
    </div>
  );
}

/** One LoRA file, with *Download LoRA*, or how far its download got. */
function LoraRow(props: { repo: LocalRepo; file: RepoFileView; run(action: () => void): void; onError(e: ErrorInfo): void }) {
  const { repo, file: f } = props;
  const nameId = useId();
  const downloadLora = useStudio((s) => s.downloadLora);
  const [starting, setStarting] = useState(false);
  const loraId = `hf:${repo.repo}:${f.path}`;
  const group = useDownloads((s) => s.groups.find((g) => g.loraId === loraId && g.status !== "cancelled") ?? null);
  return (
    <li className="flex items-center gap-3 px-3 py-2.5">
      <FileWords file={f} nameId={nameId} />
      {group && group.status !== "done" ? (
        <div className="w-52 shrink-0">
          <DownloadState group={group} name={visiblePath(nameOf(f.path))} />
        </div>
      ) : group ? (
        <span className="inline-flex shrink-0 items-center gap-1 text-sm text-success">
          <CircleCheck size={14} aria-hidden />
          {MODEL_DETAIL.installed}
        </span>
      ) : (
        <button
          type="button"
          aria-describedby={nameId}
          disabled={starting}
          className={`${CARD_BUTTON} inline-flex shrink-0 items-center gap-1.5 ${FOCUS_RING}`}
          onClick={() =>
            props.run(() => {
              setStarting(true);
              downloadLora(repo.repo, repo.commit, f.path)
                .catch((e: unknown) => props.onError(toErrorInfo(e)))
                .finally(() => setStarting(false));
            })
          }
        >
          <Layers size={14} aria-hidden className="text-fg-muted" />
          {MODEL_DETAIL.downloadLora}
        </button>
      )}
    </li>
  );
}

/** One file Studio can't run (or only adds): what it is, and a quiet *Download file anyway*
 *  (the file alone, without a set-up; one with no published checksum in the browser), or how
 *  far its download got, and where it was saved. */
function FileRow(props: { repo: LocalRepo; file: RepoFileView; onError(e: ErrorInfo): void }) {
  const { repo, file: f, onError } = props;
  const nameId = useId();
  const [starting, setStarting] = useState(false);
  const [queued, setQueued] = useState(false);
  const group = useDownloads((s) =>
    s.groups.find((g) => g.repo === repo.repo && g.savedFile?.commit === repo.commit && g.label === f.path),
  );
  const noChecksum = "cannotRun" in f.label && f.label.cannotRun.reason === "noSha";
  const browserUrl = `${modelPage(repo.repo)}/resolve/${repo.commit}/${f.path.split("/").map(encodeURIComponent).join("/")}?download=true`;
  const quiet = `shrink-0 rounded-md px-1.5 py-0.5 text-sm text-fg-muted underline-offset-2 hover:text-fg hover:underline disabled:opacity-60 ${FOCUS_RING}`;
  async function download() {
    setStarting(true);
    try {
      await api.studioLocalDownloadFile(repo.repo, repo.commit, f.path);
      setQueued(true);
    } catch (e) {
      onError(toErrorInfo(e));
    } finally {
      setStarting(false);
    }
  }
  return (
    <li className="flex flex-col gap-1.5 px-3 py-2.5">
      <div className="flex items-center gap-3">
        <FileWords file={f} nameId={nameId} />
        {noChecksum ? (
          <span className="shrink-0 text-sm">
            <BrowserLink href={browserUrl} onError={onError}>
              {MODEL_DETAIL.browserDownload}
            </BrowserLink>
          </span>
        ) : group && group.status !== "cancelled" && group.status !== "done" ? null : (
          <button type="button" className={quiet} aria-describedby={nameId} disabled={starting} onClick={() => void download()}>
            {SETUP.anyway}
          </button>
        )}
      </div>
      {group && group.status !== "cancelled" && group.status !== "done" && (
        <DownloadState group={group} name={visiblePath(nameOf(f.path))} />
      )}
      {group?.status === "done" && group.savedFile && (
        <p className="truncate text-xs text-fg-muted">
          <bdi dir="ltr">{visiblePath(group.savedFile.path)}</bdi>
        </p>
      )}
      {queued && !group && (
        <p role="status" className="text-xs text-fg-muted">
          {MODEL_DETAIL.queued}
        </p>
      )}
    </li>
  );
}
