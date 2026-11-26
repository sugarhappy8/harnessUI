import { useCallback, useEffect, useId, useRef, useState } from "react";
import { Dialog, RadioGroup } from "radix-ui";
import { AlertTriangle, CheckCircle2, FileBox, Loader2, X } from "lucide-react";
import { clsx } from "clsx";
import { formatBytes } from "../../lib/format";
import type { ErrorInfo } from "../../lib/types";
import {
  DOWNLOAD_LOCAL,
  fileLabelWords,
  IMPORT,
  LOCAL_WARNING,
  SETUP,
  setupProblem,
  SLOT_LABEL,
  UI,
} from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { useStudio } from "../../store/studio";
import { FOCUS_RING } from "../ui/styles";
import { LocalNotice } from "./LocalNotice";
import { SECONDARY_BUTTON } from "./styles";
import type { ImportInspection, ImportMode, SetupPlan } from "./types";

// *Add a model file* and *Add a LoRA file* (0.2.0 Task U4, design 7.8), opened by the store's
// `importOpen` from the model sheet's foot, Get models or the LoRAs section. Its steps:
// 1. the file chooser (.safetensors, .sft, .gguf) opens at once; closed without a file, so does
//    the dialog;
// 2. the file is read from its header and its label said in words ("Works in Studio · SDXL
//    picture model", "LoRA for SDXL", "Can't run in Studio: …"); a file Studio cannot read says
//    why, in the backend's words (a pickle file "Studio can't read .pt files", by its own
//    extension), and offers only another file: never a set-up, a copy or a use where it is;
// 3. *Copy it into Studio's models folder* or *Use it where it is*, the last choice first;
//    not asked for a file inside the models folder, which is used where it is;
// 4. the extra files its set-up adds, and what that downloads; a problem in the set-up (a Hugging
//    Face token or terms, a part with no file) is said plainly in the total's place;
// 5. *Add* (the note about local models first, while it was never accepted), its progress
//    (`importProgress`), then Added, and the model is chosen on its tab. A LoRA file, whichever
//    button chose it, is added to the LoRAs.
// While a file is being added, the dialog stays open. Focus goes back to what opened it, else
// to the composer's model chip.

/** Where the dialog is. */
type Step =
  /** The file chooser is open. */
  | { at: "choosing" }
  /** The file's header is being read. */
  | { at: "reading"; path: string }
  /** Read: what it is, and (after a refused *Add*) why it was not added. */
  | { at: "read"; file: ImportInspection; problem: ErrorInfo | null }
  | { at: "adding"; file: ImportInspection }
  /** Added; `fetching` while the extra files its set-up needs download. */
  | { at: "done"; file: ImportInspection; fetching: boolean }
  /** The file could not be read, or Studio does not read its kind of file. */
  | { at: "failed"; path: string; error: ErrorInfo };

/** A refusal in plain words: a set-up's problem in `SETUP.problem`'s words (which say what to
 *  do), else the backend's message and next step. */
function problemWords(error: ErrorInfo): { message: string; nextStep: string | null } {
  const problem = setupProblem(error.code);
  if (problem !== null) return { message: SETUP.problem[problem], nextStep: null };
  return { message: error.message, nextStep: error.nextStep || null };
}

/** A file's name from its path, on any system. */
function nameOf(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}

/** The dialog, while `importOpen` names a kind of file; `null` otherwise. */
export function ImportDialog() {
  const kind = useStudio((s) => s.importOpen);
  const openImport = useStudio((s) => s.openImport);
  const [step, setStep] = useState<Step>({ at: "choosing" });
  const [mode, setMode] = useState<ImportMode>("copy");
  const [notice, setNotice] = useState(false);
  // Counts the files chosen, and each closing, so a late answer for one before is dropped.
  const run = useRef(0);
  const opener = useRef<Element | null>(null);

  // Each opening starts at the file chooser, once (counted, so a second run of the effect for
  // the same opening, as React's strict mode makes, opens no second chooser).
  const [seen, setSeen] = useState(kind);
  const [opening, setOpening] = useState(kind === null ? 0 : 1);
  const chosenFor = useRef(0);
  if (kind !== seen) {
    setSeen(kind);
    if (kind !== null) {
      setStep({ at: "choosing" });
      setNotice(false);
      setOpening((n) => n + 1);
    }
  }

  const read = useCallback(async (path: string, mine: number) => {
    setStep({ at: "reading", path });
    try {
      const file = await useStudio.getState().importInspect(path);
      if (mine !== run.current) return;
      setMode(file.lastMode);
      setStep({ at: "read", file, problem: null });
    } catch (e: unknown) {
      if (mine === run.current) setStep({ at: "failed", path, error: toErrorInfo(e) });
    }
  }, []);

  /** The file chooser; closed without a file, the dialog closes too the first time (`first`),
   *  and stays as it was after *Choose another file*. */
  const choose = useCallback(
    async (of: "model" | "lora", first: boolean) => {
      const mine = ++run.current;
      let path: string | null;
      try {
        path = await useStudio.getState().chooseFile(of);
      } catch (e: unknown) {
        if (mine === run.current) setStep({ at: "failed", path: "", error: toErrorInfo(e) });
        return;
      }
      if (mine !== run.current) return;
      if (path !== null) await read(path, mine);
      else if (first) useStudio.getState().openImport(null);
    },
    [read],
  );

  useEffect(() => {
    if (kind === null || chosenFor.current === opening) return;
    chosenFor.current = opening;
    opener.current = document.activeElement;
    void choose(kind, true);
  }, [kind, opening, choose]);

  if (kind === null) return null;
  const adding = step.at === "adding";

  const close = () => {
    run.current++;
    openImport(null);
  };

  /** Adds the model as it was read (as a LoRA when it is one), after the note about local
   *  models while it was never accepted; a model added at once is chosen on its tab. */
  const add = async (file: ImportInspection) => {
    const s = useStudio.getState();
    if (s.local && !s.local.noticeAccepted) {
      setNotice(true);
      return;
    }
    const mine = run.current;
    setStep({ at: "adding", file });
    try {
      if ("lora" in file.label) {
        await s.importLora(file.path);
        if (mine === run.current) setStep({ at: "done", file, fetching: false });
        return;
      }
      const result = await s.importModel(file.path, file.insideModels ? "inPlace" : mode, null);
      if (mine !== run.current) return;
      const after = useStudio.getState();
      const added = result.genModelId === null ? undefined : after.local?.installed.find((m) => m.id === result.genModelId);
      if (added) {
        after.chooseModel(added.kind, "thisComputer", added.model.id);
        if (after.tab !== added.kind) after.setTab(added.kind);
      }
      setStep({ at: "done", file, fetching: result.genModelId === null && result.downloadGroup !== null });
    } catch (e: unknown) {
      if (mine === run.current) setStep({ at: "read", file, problem: toErrorInfo(e) });
    }
  };

  const file = step.at === "read" || step.at === "adding" || step.at === "done" ? step.file : null;
  const works = file !== null && "works" in file.label;
  const isLora = file !== null && "lora" in file.label;
  const incomplete = works && file.plan !== null && !file.plan.canDownload;
  const canAdd = step.at === "read" && (works || isLora) && !incomplete;

  return (
    <Dialog.Root open onOpenChange={(o) => !o && !adding && close()}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay fixed inset-0" />
        <Dialog.Content
          aria-describedby={undefined}
          data-import-dialog
          onCloseAutoFocus={(e) => {
            e.preventDefault();
            const back =
              opener.current instanceof HTMLElement && opener.current.isConnected && opener.current !== document.body
                ? opener.current
                : document.querySelector<HTMLElement>("[data-model-chip] button");
            back?.focus();
          }}
          className="fixed top-1/2 left-1/2 flex max-h-[min(40rem,92vh)] w-[32rem] max-w-[calc(100vw-2rem)] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface text-fg shadow-pop"
        >
          <div className="flex items-center justify-between gap-3 px-5 pt-4 pb-3">
            <Dialog.Title className="text-title">{kind === "lora" ? IMPORT.addLora : IMPORT.title}</Dialog.Title>
            <Dialog.Close
              aria-label={UI.close}
              disabled={adding}
              className={`rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg disabled:opacity-40 ${FOCUS_RING}`}
            >
              <X className="size-4" aria-hidden />
            </Dialog.Close>
          </div>
          <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-5 pb-4">
            {step.at === "choosing" && <p className="text-sm text-fg-muted">{IMPORT.choose}</p>}
            {step.at === "reading" && (
              <>
                <FileHead name={nameOf(step.path)} meta={null} tone="muted" />
                <Working words={IMPORT.reading} />
              </>
            )}
            {step.at === "failed" && (
              <>
                {step.path !== "" && <FileHead name={nameOf(step.path)} meta={null} tone="muted" />}
                <Problem error={step.error} />
              </>
            )}
            {file !== null && (
              <FileHead
                name={file.name}
                meta={`${formatBytes(file.sizeBytes)} · ${fileLabelWords(file.label, file.path)}`}
                tone={works || isLora ? "works" : "cannot"}
              />
            )}
            {step.at === "read" && isLora && <p className="text-sm">{IMPORT.asLora}</p>}
            {step.at === "read" && works && (
              <>
                {!file.insideModels && <ModeQuestion mode={mode} onMode={setMode} />}
                {file.plan && <PlanPart plan={file.plan} problem={step.problem} />}
              </>
            )}
            {step.at === "read" && step.problem && !(works && file.plan && setupProblem(step.problem.code) !== null) && (
              <Problem error={step.problem} />
            )}
            {step.at === "adding" && <Adding total={file?.sizeBytes ?? 0} />}
            {step.at === "done" && (
              <div role="status" className="flex flex-col gap-1">
                <p className="flex items-center gap-2 text-sm font-medium text-success">
                  <CheckCircle2 size={16} aria-hidden />
                  {IMPORT.done}
                </p>
                {step.fetching && <p className="text-sm text-fg-muted">{IMPORT.fetching}</p>}
              </div>
            )}
          </div>
          <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border px-5 py-3">
            {(step.at === "failed" || (step.at === "read" && !canAdd)) && (
              <button type="button" onClick={() => void choose(kind, false)} className={`${SECONDARY_BUTTON} mr-auto ${FOCUS_RING}`}>
                {IMPORT.chooseAnother}
              </button>
            )}
            {step.at === "done" ? (
              <button type="button" onClick={close} className={`btn-primary h-9 rounded-full px-4 text-sm font-medium ${FOCUS_RING}`}>
                {IMPORT.finish}
              </button>
            ) : (
              <>
                <button type="button" disabled={adding} onClick={close} className={`${SECONDARY_BUTTON} ${FOCUS_RING}`}>
                  {IMPORT.cancel}
                </button>
                {(canAdd || incomplete || adding) && (
                  <button
                    type="button"
                    disabled={!canAdd}
                    onClick={() => file && void add(file)}
                    className={`btn-primary h-9 rounded-full px-4 text-sm font-medium ${FOCUS_RING}`}
                  >
                    {IMPORT.add}
                  </button>
                )}
              </>
            )}
          </div>
          {/* Before the first file is added: the note about local models, then *Add* goes on. */}
          <LocalNotice
            open={notice}
            onAccepted={() => {
              setNotice(false);
              if (file) void add(file);
            }}
            onClose={() => setNotice(false)}
          />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** The file: its name, then its size and what it is. */
function FileHead(props: { name: string; meta: string | null; tone: "muted" | "works" | "cannot" }) {
  return (
    <div className="flex items-center gap-3 rounded-xl border border-border bg-bg px-3 py-2.5">
      <span
        aria-hidden
        className={clsx(
          "grid size-10 shrink-0 place-items-center rounded-lg",
          props.tone === "cannot" ? "bg-warning/10 text-warning" : "bg-accent/10 text-accent",
        )}
      >
        <FileBox size={18} />
      </span>
      <div className="min-w-0">
        <p className="truncate text-sm font-medium">{props.name}</p>
        {props.meta && (
          <p className={clsx("text-sm", props.tone === "cannot" ? "text-warning" : "text-fg-muted")}>{props.meta}</p>
        )}
      </div>
    </div>
  );
}

/** A step under way, as a quiet spinning line. */
function Working(props: { words: string }) {
  return (
    <p role="status" className="flex items-center gap-2 text-sm text-fg-muted">
      <Loader2 size={14} aria-hidden className="animate-spin" />
      {props.words}
    </p>
  );
}

/** Why the file was not read or added, in plain words, with what to do. */
function Problem(props: { error: ErrorInfo }) {
  const { message, nextStep } = problemWords(props.error);
  return (
    <div role="alert" className="flex items-start gap-2 rounded-xl bg-warning/10 px-3 py-2.5 text-sm">
      <AlertTriangle size={15} aria-hidden className="mt-0.5 shrink-0 text-warning" />
      <div className="flex flex-col gap-0.5">
        <p className="font-medium">{message}</p>
        {nextStep && <p className="text-fg-muted">{nextStep}</p>}
      </div>
    </div>
  );
}

/** *Copy it into Studio's models folder* or *Use it where it is*. */
function ModeQuestion(props: { mode: ImportMode; onMode(mode: ImportMode): void }) {
  const id = useId();
  return (
    <div className="flex flex-col gap-2">
      <h3 id={id} className="text-section">
        {IMPORT.mode}
      </h3>
      <RadioGroup.Root
        aria-labelledby={id}
        value={props.mode}
        onValueChange={(v) => props.onMode(v as ImportMode)}
        className="flex flex-col gap-1.5"
      >
        {(["copy", "inPlace"] as const).map((m) => (
          <label
            key={m}
            className="flex cursor-pointer items-center gap-2.5 rounded-xl border border-border px-3 py-2 hover:border-border-strong has-[[data-state=checked]]:border-accent/60 has-[[data-state=checked]]:bg-accent/5"
          >
            <RadioGroup.Item
              value={m}
              className={`grid size-4 shrink-0 place-items-center rounded-full border border-border-strong bg-surface data-[state=checked]:border-accent ${FOCUS_RING}`}
            >
              <RadioGroup.Indicator className="size-2 rounded-full bg-accent" />
            </RadioGroup.Item>
            <span className="text-sm">{IMPORT[m]}</span>
          </label>
        ))}
      </RadioGroup.Root>
    </div>
  );
}

/** The extra files its set-up adds (each already here, or its size), then what that downloads;
 *  a problem in the set-up (`problem`, or a part with no file) is said in the total's place. A
 *  model that may not fit this computer says so, as a warning. */
function PlanPart(props: { plan: SetupPlan; problem: ErrorInfo | null }) {
  const { plan } = props;
  const id = useId();
  const extras = plan.slots.filter((s) => s.part !== null && s.source !== "main");
  const known = setupProblem(props.problem?.code);
  const problem =
    known !== null
      ? SETUP.problem[known]
      : !plan.canDownload
        ? SETUP.problem.studio_setup_incomplete
        : null;
  const warning = plan.fit.word === "notHere" && plan.fit.reason ? LOCAL_WARNING.mayNotFit(plan.fit.reason) : null;
  return (
    <>
      {(extras.length > 0 || problem !== null || plan.toFetchBytes > 0) && (
        <section aria-labelledby={id} className="flex flex-col gap-1.5">
          <h3 id={id} className="text-section">
            {IMPORT.extraFiles}
          </h3>
          {extras.length > 0 && (
            <ul className="flex flex-col divide-y divide-border rounded-xl border border-border">
              {extras.map((s) => (
                <li key={s.role} className="flex items-center justify-between gap-3 px-3 py-1.5 text-sm">
                  <span className="min-w-0 truncate">{SLOT_LABEL[s.role] ?? s.role}</span>
                  <span className="shrink-0 text-fg-muted tabular-nums">
                    {s.part!.alreadyHere ? DOWNLOAD_LOCAL.alreadyHere : formatBytes(s.part!.sizeBytes)}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {problem !== null ? (
            <p role="alert" className="flex items-start gap-2 text-sm font-medium text-warning">
              <AlertTriangle size={15} aria-hidden className="mt-0.5 shrink-0" />
              {problem}
            </p>
          ) : (
            plan.toFetchBytes > 0 && <p className="text-sm font-medium">{IMPORT.alsoDownloads(formatBytes(plan.toFetchBytes))}</p>
          )}
        </section>
      )}
      {plan.lowDisk && <p className="text-sm text-warning">{DOWNLOAD_LOCAL.lowDisk}</p>}
      {warning && <p className="text-sm text-warning">{warning}</p>}
    </>
  );
}

/** How far adding the file is: reading it through (or copying it), with a bar. */
function Adding(props: { total: number }) {
  const progress = useStudio((s) => s.importProgress);
  const total = progress?.totalBytes || props.total;
  const done = progress?.doneBytes ?? 0;
  const pct = total > 0 ? Math.min(100, Math.round((done / total) * 100)) : 0;
  const words = progress?.stage === "copying" ? IMPORT.copying : IMPORT.reading;
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-center justify-between gap-3 text-sm">
        <span className="flex items-center gap-2">
          <Loader2 size={14} aria-hidden className="animate-spin text-fg-muted" />
          {words}
        </span>
        <span className="text-fg-muted tabular-nums">{IMPORT.amount(formatBytes(done), formatBytes(total))}</span>
      </div>
      <div
        role="progressbar"
        aria-label={words}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={pct}
        className="h-1.5 w-full overflow-hidden rounded-full bg-bg-muted"
      >
        <div className="h-full rounded-full bg-accent transition-[width] duration-300 ease-out" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}
