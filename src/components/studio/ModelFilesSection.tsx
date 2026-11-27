import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { Dialog, DropdownMenu, RadioGroup } from "radix-ui";
import clsx from "clsx";
import { ChevronDown, ChevronRight, Download, FileDown, FolderOpen, HardDrive, Loader2, Wrench, X } from "lucide-react";
import { formatBytes } from "../../lib/format";
import type { ErrorInfo } from "../../lib/types";
import {
  FILES,
  fileLabelWords,
  KIND_WORD,
  MODEL_DETAIL,
  SETUP,
  setupProblem,
  SLOT_LABEL,
  STUDIO_COMPOSER,
  STUDIO_PANEL,
  UI,
} from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { visiblePath } from "../FolderChip";
import { SelectField } from "../ui/SelectField";
import { FOCUS_RING, OPTION, POPUP } from "../ui/styles";
import { DownloadState, useModelDownload } from "./DownloadState";
import { GateLine } from "./ModelCard";
import { Licences, PlanParts, warningsFor } from "./PlanParts";
import { BOX, GHOST_BUTTON, ICON_BUTTON, PANEL_BUTTON, PANEL_CARD, PANEL_SECTION, PRIMARY_BUTTON } from "./styles";
import type {
  Gate,
  InstalledLocal,
  LocalModels,
  LocalRepo,
  ModelFiles,
  PartRole,
  PlannedSlot,
  RepoFileView,
  SetupChoice,
  SetupPlan,
  SlotFile,
} from "./types";

// The Files section of Studio's side panel (0.2.0 Task U8, design 7.5 "Set it up yourself"):
// the model's type (the recipes `studio_model_files` offers it) and each part of its set-up,
// its role in plain words ("Decoder (VAE)"), its file and size and whether it is required.
// *Change* gives a part a file of another installed model of the same kind, a file from disk,
// or a file from a Hugging Face repository (a small dialog: the repository's name, then one of
// the files Studio read in it). A file on this computer downloads nothing, so it goes to
// `setModelFiles` at once. A change that may download (another model type, a Hugging Face file)
// is planned first (`setupPlan` of the installed model, fix round 1, design 7.4.3): what
// it fetches shows in place of the parts, with each file's role, size, licence and whether it is
// already here, the licences, the total and one Download (off, with what to do, for a gated
// file); one that fetches nothing applies at once. Applied, the model's row is rewritten, or the
// download it starts brings the files: until it is done the set-up applied shows with the
// download's progress (Pause, Cancel), and *Change* and *Model type* wait, as the backend would
// refuse them; then the new set-up is read. The local models are read again each time, so a
// model that needed setting up can be used, Create with it, as soon as its files can run. A
// model type the detection did not pick says "untested set-up". The model's own file is the
// model itself: it has no *Change*.

/** A file another installed model has, which a part can use. */
interface InstalledFile {
  genModelId: string;
  /** The model it belongs to, as listed. */
  model: string;
  role: PartRole;
  path: string;
  sizeBytes: number;
}

/** A path's file name. */
function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** The files the other installed models of `model`'s kind have, read from their set-ups (on
 *  this computer: nothing is sent). A model whose files cannot be read offers none. */
async function installedFiles(
  installed: InstalledLocal[],
  model: InstalledLocal,
  read: (genModelId: string) => Promise<ModelFiles>,
): Promise<InstalledFile[]> {
  const others = installed.filter((m) => m.id !== model.id && m.kind === model.kind && !m.needsSetup);
  const lists = await Promise.all(
    others.map((m) =>
      read(m.id)
        .then((files) =>
          files.slots.flatMap((s) =>
            s.part ? [{ genModelId: m.id, model: m.name, role: s.role, path: s.part.path, sizeBytes: s.part.sizeBytes }] : [],
          ),
        )
        .catch(() => [] as InstalledFile[]),
    ),
  );
  return lists.flat();
}

/** A change of model type, or a file from Hugging Face, shown before it applies (design
 *  7.4.3): its plan, or (`plan` null) the gate that stops it; `gate` also once the change was
 *  refused for one. */
interface Review {
  id: string;
  choice: SetupChoice;
  plan: SetupPlan | null;
  gate: GateProblem | null;
  /** The Hugging Face repository the change fetches from, for the gate's model page. */
  repo: string | null;
}

/** What stops a download until the person acts: a token to save, or terms to accept. */
type GateProblem = "hf_needs_token" | "hf_needs_terms";

/** No gate line in the licences: a gated file says what to do in its own line. */
const NO_GATE: Gate = { mode: "none", collects: [] };

/** The gate `e` names, if it is one. */
function gateOf(e: ErrorInfo): GateProblem | null {
  const problem = setupProblem(e.code);
  return problem === "hf_needs_token" || problem === "hf_needs_terms" ? problem : null;
}

/** The Files section of installed model `model`. */
export function ModelFilesSection(p: { model: InstalledLocal }) {
  const { model } = p;
  const headingId = useId();
  const typeId = useId();
  const waitId = useId();
  const modelFiles = useStudio((s) => s.modelFiles);
  const setModelFiles = useStudio((s) => s.setModelFiles);
  const setupPlan = useStudio((s) => s.setupPlan);
  const chooseFile = useStudio((s) => s.chooseFile);
  const engine = useStudio((s) => s.local?.engine ?? null);
  // This model's own download while it runs (a set-up change's, or a repair): its files are not
  // changed meanwhile, as the backend refuses it (`studio_setup_in_progress`).
  const download = useModelDownload(model.id);
  const downloading = download !== null;
  // The files read last, and why they could not be read, for the model they are of.
  const [loaded, setLoaded] = useState<{ id: string; files: ModelFiles } | null>(null);
  const [failed, setFailed] = useState<{ id: string; error: ErrorInfo } | null>(null);
  // The set-up a change applied while the download that brings its files runs: shown in place
  // of the row's (which stays the old one until the download is done).
  const [applied, setApplied] = useState<{ id: string; files: ModelFiles } | null>(null);
  const [review, setReview] = useState<Review | null>(null);
  // Why the last change was refused.
  const [refused, setRefused] = useState<ErrorInfo | null>(null);
  const [busy, setBusy] = useState(false);
  // The other installed models' files, read once a *Change* opens.
  const [others, setOthers] = useState<InstalledFile[] | null>(null);
  const asked = useRef(false);
  // The part whose file comes from Hugging Face, while its dialog is open.
  const [hubFor, setHubFor] = useState<{ role: PartRole; opener: HTMLElement | null } | null>(null);

  // Read again as the model is read again (another model's set-up changed), and once its
  // download ends. A read made while it runs gives the old row: the set-up applied still shows;
  // one made after it ended shows the new set-up in its place.
  useEffect(() => {
    let current = true;
    const after = !downloading;
    modelFiles(model.id).then(
      (files) => {
        if (!current) return;
        setLoaded({ id: model.id, files });
        if (after) setApplied((a) => (a?.id === model.id ? null : a));
      },
      (e: unknown) => {
        if (current) setFailed({ id: model.id, error: toErrorInfo(e) });
      },
    );
    return () => {
      current = false;
    };
  }, [model, modelFiles, downloading]);

  const read = loaded?.id === model.id ? loaded.files : null;
  const files = applied?.id === model.id ? applied.files : read;
  const error = failed?.id === model.id && files === null ? failed.error : null;
  const reviewing = review?.id === model.id ? review : null;

  const wantInstalled = () => {
    if (asked.current) return;
    asked.current = true;
    installedFiles(useStudio.getState().local?.installed ?? [], model, modelFiles).then(setOthers, () => setOthers([]));
  };

  /** The change applied: the model's row rewritten at once, or (`downloadGroup`) the set-up
   *  shown while the download that brings its files runs. */
  async function apply(choice: SetupChoice) {
    setBusy(true);
    setRefused(null);
    try {
      const result = await setModelFiles(model.id, choice);
      if (result.downloadGroup === null) setLoaded({ id: model.id, files: result.files });
      else setApplied({ id: model.id, files: result.files });
      setReview(null);
    } catch (e) {
      const err = toErrorInfo(e);
      const gate = gateOf(err);
      // A gate turns the review's Download off, with what to do; anything else is said.
      if (gate !== null) setReview((r) => (r?.id === model.id ? { ...r, gate } : r));
      else setRefused(err);
    } finally {
      setBusy(false);
    }
  }

  /** A change that may download: its plan first, applied at once only when it fetches
   *  nothing; otherwise shown, with one Download (design 7.4.3). */
  async function propose(choice: SetupChoice, repo: string | null) {
    setBusy(true);
    setRefused(null);
    let plan: SetupPlan;
    try {
      plan = await setupPlan({ installed: { genModelId: model.id } }, choice);
    } catch (e) {
      const err = toErrorInfo(e);
      const gate = gateOf(err);
      if (gate !== null) setReview({ id: model.id, choice, plan: null, gate, repo });
      else setRefused(err);
      setBusy(false);
      return;
    }
    if (plan.canDownload && plan.toFetchBytes === 0 && !plan.needsEngine) {
      await apply(choice);
      return;
    }
    setReview({ id: model.id, choice, plan, gate: null, repo });
    setBusy(false);
  }

  async function fromDisk(role: PartRole) {
    let path: string | null;
    try {
      path = await chooseFile("model");
    } catch (e) {
      setRefused(toErrorInfo(e));
      return;
    }
    // A file on this computer downloads nothing: applied at once.
    if (path !== null) await apply({ recipe: null, overrides: [{ role, file: { disk: { path } } }] });
  }

  // A refusal with a set-up problem's code is said in its plain words.
  const problem = setupProblem(refused?.code);
  const shownRefusal = refused && problem ? { ...refused, message: SETUP.problem[problem], nextStep: "" } : refused;
  // Without a type there is nothing to give a part a file for yet; nothing changes while the
  // model's download runs, or while a change is reviewed.
  const typed = files?.recipe != null;
  const locked = busy || downloading || reviewing !== null;
  // The type shown: the one a reviewed change picks, else the set-up's.
  const shownRecipe = reviewing?.plan?.recipe ?? files?.recipe ?? null;
  // The types offered, with the one shown first should the list not have it.
  const recipes =
    shownRecipe && !(files?.recipes ?? []).some((r) => r.id === shownRecipe.id)
      ? [shownRecipe, ...(files?.recipes ?? [])]
      : (files?.recipes ?? []);
  const untested = reviewing?.plan ? reviewing.plan.untested : (files?.untested ?? false);

  return (
    <section aria-labelledby={headingId} data-settings-section="files" className={PANEL_SECTION}>
      <h3 id={headingId} className="text-section">
        {STUDIO_PANEL.files}
      </h3>
      {model.needsSetup && (
        <div className="flex items-start gap-2 rounded-lg border border-warning/30 bg-warning/10 px-3 py-2 text-sm">
          <Wrench size={15} aria-hidden className="mt-0.5 shrink-0 text-warning" />
          <p>{FILES.needsSetup}</p>
        </div>
      )}
      {files ? (
        <>
          <div className="flex flex-col gap-2">
            <p className="flex min-h-7 items-center gap-2">
              <span id={typeId} className="text-sm font-medium">
                {SETUP.modelType}
              </span>
              {untested && (
                <span className="rounded-full bg-warning/10 px-2 py-0.5 text-xs font-medium text-warning">
                  {SETUP.untested}
                </span>
              )}
            </p>
            <SelectField
              label={SETUP.modelType}
              labelledBy={typeId}
              value={shownRecipe?.id ?? null}
              placeholder={FILES.chooseType}
              options={recipes.map((r) => ({ value: r.id, label: r.name, hint: KIND_WORD[r.kind] }))}
              onChange={(id) => void propose({ recipe: id, overrides: [] }, null)}
              disabled={locked}
            />
          </div>
          {reviewing ? (
            <PlanReview
              review={reviewing}
              kind={model.kind}
              engine={engine}
              busy={busy}
              onApply={() => void apply(reviewing.choice)}
              onCancel={() => setReview(null)}
            />
          ) : (
            files.slots.length > 0 && (
              <ul className={`${PANEL_CARD} divide-y divide-border`}>
                {files.slots.map((slot) => (
                  <SlotRow
                    key={slot.role}
                    slot={slot}
                    // The model file is the model itself.
                    change={
                      slot.source === "main"
                        ? null
                        : {
                            disabled: locked || !typed,
                            reasonId: downloading ? waitId : null,
                            installed: others?.filter((f) => f.role === slot.role && f.path !== slot.part?.path) ?? null,
                            onOpen: wantInstalled,
                            // An installed file downloads nothing: applied at once.
                            onInstalled: (f) =>
                              void apply({
                                recipe: null,
                                overrides: [{ role: slot.role, file: { installed: { genModelId: f.genModelId, role: f.role } } }],
                              }),
                            onDisk: () => void fromDisk(slot.role),
                            onHub: (opener) => setHubFor({ role: slot.role, opener }),
                          }
                    }
                  />
                ))}
              </ul>
            )
          )}
        </>
      ) : (
        error === null && (
          <p role="status" className="flex items-center gap-2 text-sm text-fg-muted">
            <Loader2 size={14} aria-hidden className="animate-spin" />
            {FILES.reading}
          </p>
        )
      )}
      {download && (
        <div data-files-download className="flex flex-col gap-2">
          <p id={waitId} className="text-sm text-fg-muted">
            {FILES.downloading}
          </p>
          <DownloadState group={download} name={model.name} />
        </div>
      )}
      {error && <ErrorBanner error={error} />}
      {shownRefusal && <ErrorBanner error={shownRefusal} onDismiss={() => setRefused(null)} />}
      <HubFileDialog
        role={hubFor?.role ?? null}
        opener={hubFor?.opener ?? null}
        onClose={() => setHubFor(null)}
        onUse={(role, file) =>
          void propose({ recipe: null, overrides: [{ role, file }] }, typeof file === "object" && "hub" in file ? file.hub.repo : null)
        }
      />
    </section>
  );
}

/** A change shown before it applies: the files its plan fetches or keeps, each with its role in
 *  words, size, licence and whether it is already on this computer (PlanParts); the licences
 *  in plain words and what you make may be used for (Licences); warnings; the total to
 *  download; Cancel, back to the set-up as it is, and one Download, off while a gate stops it
 *  (with what to do: a token, the model's terms) or a part has no file. */
function PlanReview(props: {
  review: Review;
  kind: InstalledLocal["kind"];
  engine: LocalModels["engine"] | null;
  busy: boolean;
  onApply(): void;
  onCancel(): void;
}) {
  const { review, engine } = props;
  const plan = review.plan;
  const engineBytes = plan?.needsEngine && engine ? engine.downloadBytes : null;
  const total = plan ? plan.toFetchBytes + (engineBytes ?? 0) : null;
  // A set-up with a part still lacking a file has no firm total: what is known is a floor.
  const lacking = plan ? plan.slots.filter((s) => s.required && s.source === "missing") : [];
  const off = props.busy || review.gate !== null || plan === null || !plan.canDownload;
  return (
    <div data-files-review className="flex flex-col gap-4 rounded-xl border border-accent/30 bg-accent/5 p-3">
      {plan && (
        <>
          <PlanParts slots={plan.slots} licences={plan.licences} />
          <Licences
            licences={plan.licences}
            kind={props.kind}
            gate={NO_GATE}
            pageRepo={null}
            engineBytes={engineBytes}
            engineLicence={engine?.licenseName ?? ""}
          />
          {warningsFor(plan.fit, plan.lowDisk).map((w) => (
            <p key={w} className="text-sm text-warning">
              {w}
            </p>
          ))}
          {!plan.canDownload && (
            <p className="text-sm font-medium text-warning">{MODEL_DETAIL.missingWhy(lacking.map((s) => s.role))}</p>
          )}
        </>
      )}
      {review.gate !== null && <GateLine problem={review.gate} repo={review.repo} size="sm" />}
      <div className="flex flex-wrap items-center justify-between gap-x-3 gap-y-2">
        {total !== null && (
          <p className="text-sm font-semibold tabular-nums">
            {lacking.length > 0
              ? MODEL_DETAIL.totalSoFar(formatBytes(total), lacking.length)
              : MODEL_DETAIL.total(formatBytes(total))}
          </p>
        )}
        <div className="ml-auto flex gap-2">
          <button type="button" onClick={props.onCancel} disabled={props.busy} className={GHOST_BUTTON}>
            {FILES.cancel}
          </button>
          <button type="button" onClick={props.onApply} disabled={off} className={`${PRIMARY_BUTTON} inline-flex items-center gap-1.5 py-1.5 disabled:opacity-60`}>
            <Download size={15} aria-hidden />
            {MODEL_DETAIL.download}
          </button>
        </div>
      </div>
    </div>
  );
}

/** What *Change* on a part does: the installed files that part can use (`null` until they are
 *  read, which `onOpen` asks for), and each way of choosing a file. */
interface ChangeActions {
  disabled: boolean;
  /** Why it is off, when something outside says it (the model's download). */
  reasonId: string | null;
  installed: InstalledFile[] | null;
  onOpen(): void;
  onInstalled(file: InstalledFile): void;
  onDisk(): void;
  onHub(opener: HTMLElement | null): void;
}

/** One part of the set-up: its role in words, its file (or where it comes from: "inside the
 *  model file", "no file yet") and size, "required", and *Change* unless it is the model's own
 *  file (`change` null). */
export function SlotRow(props: { slot: PlannedSlot; change: ChangeActions | null }) {
  const { slot, change } = props;
  const labelId = useId();
  const part = slot.part;
  return (
    <li aria-labelledby={labelId} data-slot={slot.role} className="flex items-center gap-3 px-3 py-2.5">
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <p className="flex items-center gap-2">
          <span id={labelId} className="truncate text-sm font-medium">
            {SLOT_LABEL[slot.role]}
          </span>
          {slot.required && (
            <span className="shrink-0 rounded-full bg-bg-muted px-1.5 py-px text-xs text-fg-muted">{STUDIO_COMPOSER.required}</span>
          )}
        </p>
        {part ? (
          <p className="flex min-w-0 items-center gap-1.5 text-xs text-fg-muted">
            <span className="truncate">
              <bdi dir="ltr">{visiblePath(fileName(part.path))}</bdi>
            </span>
            <span aria-hidden>·</span>
            <span className="shrink-0 tabular-nums">{formatBytes(part.sizeBytes)}</span>
          </p>
        ) : (
          <p className={clsx("text-xs", slot.source === "missing" ? "text-warning" : "text-fg-muted")}>{SETUP.source[slot.source]}</p>
        )}
      </div>
      {change && <ChangeFileMenu describedBy={labelId} actions={change} />}
    </li>
  );
}

/** *Change* on a part: *A file on this computer* (the other installed models' files for the
 *  part, in a submenu), *A file from disk…* (the file chooser) and *A file from Hugging Face…*
 *  (a small dialog). Not modal, so the dialog it opens takes over cleanly. */
export function ChangeFileMenu(props: { describedBy: string; actions: ChangeActions }) {
  const { actions } = props;
  const trigger = useRef<HTMLButtonElement>(null);
  return (
    <DropdownMenu.Root modal={false} onOpenChange={(open) => open && actions.onOpen()}>
      <DropdownMenu.Trigger
        ref={trigger}
        aria-describedby={actions.reasonId ? `${props.describedBy} ${actions.reasonId}` : props.describedBy}
        disabled={actions.disabled}
        className={`${PANEL_BUTTON} shrink-0 py-1`}
      >
        {SETUP.change}
        <ChevronDown size={14} aria-hidden className="text-fg-muted" />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content align="end" sideOffset={4} collisionPadding={8} className={`${POPUP} min-w-56`}>
          <DropdownMenu.Sub>
            <DropdownMenu.SubTrigger className={OPTION}>
              <HardDrive size={15} aria-hidden className="text-fg-muted" />
              {FILES.installedFile}
              <ChevronRight size={14} aria-hidden className="ml-auto text-fg-muted" />
            </DropdownMenu.SubTrigger>
            <DropdownMenu.Portal>
              <DropdownMenu.SubContent sideOffset={6} collisionPadding={8} className={`${POPUP} max-w-80 min-w-56`}>
                {actions.installed === null ? (
                  <DropdownMenu.Item disabled className={OPTION}>
                    <Loader2 size={14} aria-hidden className="animate-spin" />
                    {FILES.reading}
                  </DropdownMenu.Item>
                ) : actions.installed.length === 0 ? (
                  <DropdownMenu.Item disabled className={OPTION}>
                    {FILES.noInstalled}
                  </DropdownMenu.Item>
                ) : (
                  actions.installed.map((f) => (
                    <DropdownMenu.Item key={`${f.genModelId} ${f.path}`} className={OPTION} onSelect={() => actions.onInstalled(f)}>
                      <span className="flex min-w-0 flex-col">
                        <span className="truncate">
                          <bdi dir="ltr">{visiblePath(fileName(f.path))}</bdi>
                        </span>
                        <span className="truncate text-xs text-fg-muted">{FILES.fromModel(f.model)}</span>
                      </span>
                      <span className="ml-auto shrink-0 pl-2 text-xs text-fg-muted tabular-nums">{formatBytes(f.sizeBytes)}</span>
                    </DropdownMenu.Item>
                  ))
                )}
              </DropdownMenu.SubContent>
            </DropdownMenu.Portal>
          </DropdownMenu.Sub>
          <DropdownMenu.Item className={OPTION} onSelect={actions.onDisk}>
            <FolderOpen size={15} aria-hidden className="text-fg-muted" />
            {FILES.diskFile}
          </DropdownMenu.Item>
          <DropdownMenu.Item className={OPTION} onSelect={() => actions.onHub(trigger.current)}>
            <FileDown size={15} aria-hidden className="text-fg-muted" />
            {FILES.hubFile}
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** The files of `repo` a part can use: not a LoRA, not one Studio can't run; those labelled
 *  as the part's own role first. */
function usableFiles(repo: LocalRepo, role: PartRole): RepoFileView[] {
  const usable = repo.fileViews.filter((f) => !("lora" in f.label) && !("cannotRun" in f.label));
  const own = (f: RepoFileView) => "extra" in f.label && f.label.extra.role === role;
  return [...usable.filter(own), ...usable.filter((f) => !own(f))];
}

/**
 * *A file from Hugging Face…* for part `role` (open while it is not `null`): the repository's
 * name, *Show its files* (Studio reads them, as Get models does), then one of the files it can
 * use, and *Use this file*. Esc or Cancel closes it, and focus goes back to the part's *Change*
 * (`opener`).
 */
export function HubFileDialog(props: {
  role: PartRole | null;
  opener: HTMLElement | null;
  onClose(): void;
  onUse(role: PartRole, file: SlotFile): void;
}) {
  const { role } = props;
  return (
    <Dialog.Root open={role !== null} onOpenChange={(open) => !open && props.onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-fg/30" />
        <Dialog.Content
          aria-describedby={undefined}
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            props.opener?.focus();
          }}
          className={`fixed top-1/2 left-1/2 z-50 flex max-h-[min(36rem,calc(100vh-2rem))] w-[min(30rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface shadow-pop ${FOCUS_RING}`}
        >
          {role !== null && <HubFileBody role={role} onClose={props.onClose} onUse={props.onUse} />}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** The dialog's title, form, files and buttons, afresh each time it opens. */
function HubFileBody(props: { role: PartRole; onClose(): void; onUse(role: PartRole, file: SlotFile): void }) {
  const { role } = props;
  const openRepo = useStudio((s) => s.openRepo);
  const inputId = useId();
  const [text, setText] = useState("");
  const [repo, setRepo] = useState<LocalRepo | null>(null);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const files = repo ? usableFiles(repo, role) : [];

  const read = (e: FormEvent) => {
    e.preventDefault();
    const name = text.trim();
    if (name === "" || reading) return;
    setReading(true);
    setError(null);
    setPicked(null);
    openRepo(name).then(
      (r) => {
        setRepo(r);
        setReading(false);
      },
      (err: unknown) => {
        setRepo(null);
        setError(toErrorInfo(err));
        setReading(false);
      },
    );
  };
  const use = () => {
    if (!repo || picked === null) return;
    props.onUse(role, { hub: { repo: repo.repo, commit: repo.commit, path: picked } });
    props.onClose();
  };
  const problem = setupProblem(error?.code);

  return (
    <>
      <div className="flex items-center justify-between gap-3 border-b border-border px-5 py-4">
        <Dialog.Title className="text-title">{FILES.hubTitle(SLOT_LABEL[role])}</Dialog.Title>
        <Dialog.Close aria-label={UI.close} className={ICON_BUTTON}>
          <X size={16} aria-hidden />
        </Dialog.Close>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 py-4">
        <form onSubmit={read} className="flex flex-col gap-1.5">
          <label htmlFor={inputId} className="text-sm font-medium">
            {FILES.repoLabel}
          </label>
          <div className="flex gap-2">
            <input
              id={inputId}
              value={text}
              autoFocus
              spellCheck={false}
              placeholder={FILES.repoHint}
              onChange={(e) => setText(e.target.value)}
              className={`${BOX} min-w-0 flex-1 py-1.5`}
            />
            <button type="submit" disabled={reading || text.trim() === ""} className={PANEL_BUTTON}>
              {reading && <Loader2 size={14} aria-hidden className="animate-spin" />}
              {FILES.showFiles}
            </button>
          </div>
        </form>
        {reading && (
          <p role="status" className="text-sm text-fg-muted">
            {FILES.readingRepo}
          </p>
        )}
        {error && <ErrorBanner error={problem ? { ...error, message: SETUP.problem[problem], nextStep: "" } : error} />}
        {repo && !reading && files.length === 0 && <p className="text-sm text-fg-muted">{FILES.noFiles}</p>}
        {repo && !reading && files.length > 0 && (
          <RadioGroup.Root
            aria-label={visiblePath(repo.repo)}
            value={picked ?? ""}
            onValueChange={setPicked}
            className="flex flex-col gap-1.5"
          >
            {files.map((f) => (
              <RadioGroup.Item
                key={f.path}
                value={f.path}
                className={`group flex items-center gap-3 rounded-xl border border-border bg-surface px-3 py-2.5 text-left hover:border-border-strong data-[state=checked]:border-accent data-[state=checked]:bg-accent/5 ${FOCUS_RING}`}
              >
                <span
                  aria-hidden
                  className="grid size-4 shrink-0 place-items-center rounded-full border border-border-strong group-data-[state=checked]:border-accent"
                >
                  <span className="size-2 scale-0 rounded-full bg-accent transition-transform group-data-[state=checked]:scale-100" />
                </span>
                <span className="flex min-w-0 flex-1 flex-col">
                  <span className="truncate text-sm font-medium">
                    <bdi dir="ltr">{visiblePath(f.path)}</bdi>
                  </span>
                  <span className="truncate text-xs text-fg-muted">{fileLabelWords(f.label, f.path)}</span>
                </span>
                <span className="shrink-0 text-sm text-fg-muted tabular-nums">{formatBytes(f.sizeBytes)}</span>
              </RadioGroup.Item>
            ))}
          </RadioGroup.Root>
        )}
      </div>
      <div className="flex justify-end gap-2 border-t border-border px-5 py-3">
        <button type="button" onClick={props.onClose} className={GHOST_BUTTON}>
          {FILES.cancel}
        </button>
        <button type="button" disabled={picked === null} onClick={use} className={`${PRIMARY_BUTTON} py-1.5 disabled:opacity-60`}>
          {FILES.useFile}
        </button>
      </div>
    </>
  );
}
