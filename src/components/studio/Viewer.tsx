import { useEffect, useEffectEvent, useRef, useState, type ReactNode, type RefObject } from "react";
import { Dialog } from "radix-ui";
import { save as saveDialog } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { ChevronLeft, ChevronRight, Cloud, Copy, Star, X } from "lucide-react";
import * as ipc from "../../lib/ipc";
import type { ErrorInfo } from "../../lib/types";
import {
  cloudChipWords,
  cloudGoesTo,
  madeWords,
  SETTING_WORDS,
  STUDIO_ACTIONS,
  STUDIO_ADVANCED,
  STUDIO_CARD,
  STUDIO_CHAT_LINK,
  STUDIO_COMPANY,
  STUDIO_OFFER,
  STUDIO_PRICE,
  STUDIO_TIME,
  STUDIO_VIEWER,
} from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import {
  fileNameOf,
  listedModel,
  requestFromRun,
  runOfOutput,
  seedKey,
  targetOf,
  useStudio,
  type ModelOffer,
  type StudioState,
} from "../../store/studio";
import { ErrorBanner } from "../ErrorBanner";
import { PlaceMark } from "../PlaceMark";
import { ExactCommand, ExactRequest } from "./AdvancedDrawer";
import * as api from "./api";
import { valueWords } from "./canonical";
import { COST_TONE_CLASS, costTone, formatUsd } from "./cost";
import { tookWords } from "./eta";
import { ConfirmDelete, resultsOf, useOutputUrl } from "./results";
import { LocalMark } from "./LocalMark";
import { CARD_BUTTON } from "./styles";
import type { Company, MediaModel, ModelChoice, Output, RequestPreview, Run, StudioEndpoint } from "./types";

// Studio's viewer (0.1.0 Task B9; design Wireframe 4, local addendum): one result full size,
// over the whole view, moved through among its run's results with the arrows (← and → too).
// Beside it: the prompt; the model and where it ran (the company, and the one OpenRouter ran it
// at when known, with the Cloud mark; for a local run "On this computer"); the size and seed;
// every setting; the cost and the time it took (a local run's "Cost: free · took … · on
// <device> · engine build <n>"); when it was made; the file; the model's notes and, for a local
// run, the AI label in the file. The actions: Star, Again, Edit, Animate, Use as reference, Show
// in folder, Delete (after the question that says what the audit log keeps), Reuse all and
// Reuse without seed (which fill the prompt box and never start a run), Save as (the settings
// file only when asked), Copy (a picture, as a PNG; Save as when the clipboard refuses), Send to
// chat (a picture, into the chat's message box: Task B11) and the exact request or command.
// Keys: ←/→, F, R, Shift R, A, S, C, Del, Space and Esc, which closes the viewer and never
// cancels a run. A picture is shown with an `img` element only, so an SVG's scripts never run;
// a video plays from a `blob:` URL (the CSP's `media-src blob:`; Task B12), and the first time
// one without a thumbnail plays, the frame shown is kept as its poster (`capturePoster`). A
// video over 256 MiB is not played here: the viewer says so, and Show in folder opens it.

/** Why an action said something: what it did, or why it could not. */
type Said = { text: string; warn: boolean } | { error: ErrorInfo };

/** The company a run went to, by name: its endpoint's, else known by its host; `null` when
 *  neither tells. */
function companyOf(run: Run, endpoints: StudioEndpoint[]): Company | null {
  const endpoint = run.providerId === null ? null : endpoints.find((e) => e.providerId === run.providerId);
  if (endpoint) return endpoint.company;
  if (run.providerHost === "openrouter.ai") return "openrouter";
  if (run.providerHost === "api.openai.com") return "openai";
  return null;
}

/** A setting's value in words: as its field lists it, a switch as on or off. */
function settingValue(value: unknown, model: MediaModel | null, key: string): string {
  if (typeof value === "boolean") return value ? STUDIO_ADVANCED.on : STUDIO_ADVANCED.off;
  return SETTING_WORDS.midSentence(valueWords(value, model?.params.find((p) => p.key === key)));
}

/** Every setting the run was made with, in words, the seed aside (it is on the size row): its
 *  model's fields in the model's order with their values ("Shape 1:1 · How many 4 · Quality
 *  low"), any other it sent by its key, its Extra fields, and which company may run it. */
function settingsWords(run: Run, model: MediaModel | null): string {
  const seed = seedKey(model);
  const sent = Object.keys(run.params).filter((k) => k !== seed && run.params[k] != null && run.params[k] !== "");
  const listed = model ? model.params.map((p) => p.key).filter((k) => sent.includes(k)) : [];
  const parts = [...listed, ...sent.filter((k) => !listed.includes(k))].map((key) => {
    const label = model?.params.find((p) => p.key === key)?.label ?? SETTING_WORDS.keyName(key);
    return STUDIO_VIEWER.setting(label, settingValue(run.params[key], model, key));
  });
  for (const [key, value] of Object.entries(run.extra)) parts.push(STUDIO_VIEWER.setting(key, JSON.stringify(value)));
  const routing = run.routing;
  if (routing && routing.order.length > 0) {
    const names = routing.order.map((tag) => model?.companies.find((c) => c.tag === tag)?.name ?? tag);
    parts.push(STUDIO_VIEWER.setting(STUDIO_ADVANCED.company, names.join(", ")));
  }
  if (routing?.zdr) parts.push(STUDIO_ADVANCED.zdr);
  return parts.join(STUDIO_CARD.joiner);
}

/** The size row: the result's width × height, a video's length, and the seed it was made with. */
function sizeWords(run: Run, output: Output, model: MediaModel | null): string {
  const seed = output.seed ?? run.params[seedKey(model)] ?? null;
  return [
    output.width !== null && output.height !== null ? STUDIO_CARD.size(String(output.width), String(output.height)) : null,
    output.durationMs !== null ? STUDIO_TIME.seconds(Math.round(output.durationMs / 100) / 10) : null,
    typeof seed === "number" || typeof seed === "string" ? STUDIO_CARD.seed(String(seed)) : null,
  ]
    .filter((p): p is string => p !== null)
    .join(STUDIO_CARD.joiner);
}

/** How long a run took: a cloud run from start to end, a local run as the engine timed it. */
function tookMs(run: Run): number | null {
  if (run.runsOn === "local") return run.local?.elapsedMs ?? null;
  return run.startedAt !== null && run.finishedAt !== null ? run.finishedAt - run.startedAt : null;
}

/** A picture's bytes as a PNG: as they are for a PNG, else drawn onto a canvas. */
async function pngOf(output: Output): Promise<Blob> {
  const blob = new Blob([await api.studioOutputBytes(output.id, false)], { type: output.mime });
  if (output.mime === "image/png") return blob;
  const url = URL.createObjectURL(blob);
  try {
    const picture = new Image();
    picture.src = url;
    await picture.decode();
    const canvas = document.createElement("canvas");
    canvas.width = picture.naturalWidth;
    canvas.height = picture.naturalHeight;
    const context = canvas.getContext("2d");
    if (!context) throw new Error("no canvas");
    context.drawImage(picture, 0, 0);
    return await new Promise<Blob>((resolve, reject) =>
      canvas.toBlob((png) => (png ? resolve(png) : reject(new Error("no PNG"))), "image/png"),
    );
  } finally {
    URL.revokeObjectURL(url);
  }
}

/** Puts picture `output` on the clipboard as a PNG; rejects when the clipboard refuses, or
 *  cannot take pictures here. The PNG is handed over as a promise, so the write starts while the
 *  press still counts as the user's. */
async function copyPicture(output: Output): Promise<void> {
  if (!navigator.clipboard?.write || typeof ClipboardItem === "undefined") throw new Error("no clipboard");
  const png = pngOf(output);
  // Whoever reads it hears of a failure; this keeps it from being reported as unhandled, and
  // leaves its cause in the log (the clipboard's own refusal may not name it).
  png.catch((e: unknown) => console.warn("Studio: the picture could not be made a PNG for the clipboard", e));
  await navigator.clipboard.write([new ClipboardItem({ "image/png": png })]);
}

/** Run `runId` at its newest state (the live runs first, then the gallery's and the newest
 *  runs); `null` when the store holds it no more. */
function runById(s: Pick<StudioState, "live" | "runs" | "recentRuns">, runId: string): Run | null {
  return s.live[runId] ?? s.runs.find((r) => r.id === runId) ?? s.recentRuns.find((r) => r.id === runId) ?? null;
}

/** Whether a key goes to what has the focus: text being typed, or a list being chosen from. */
function typing(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable || target.tagName === "TEXTAREA" || target.tagName === "SELECT") return true;
  return target instanceof HTMLInputElement && !["checkbox", "radio", "button"].includes(target.type);
}

/** The longest side of a video's poster frame: a picture's thumbnail's (`THUMB_SIDE`). */
const POSTER_SIDE = 512;
/** How `toDataURL` starts a JPEG. */
const JPEG_DATA = "data:image/jpeg;base64,";

/**
 * The poster frame of `video`: the frame it shows now, drawn on a canvas at most 512 px on its
 * longest side (never enlarged), as base64 JPEG without its `data:` prefix. `null` while the
 * video has no frame size yet, or when the canvas gives no JPEG.
 */
export function capturePoster(video: HTMLVideoElement): string | null {
  const { videoWidth, videoHeight } = video;
  if (videoWidth === 0 || videoHeight === 0) return null;
  const scale = Math.min(1, POSTER_SIDE / Math.max(videoWidth, videoHeight));
  const canvas = document.createElement("canvas");
  canvas.width = Math.max(1, Math.round(videoWidth * scale));
  canvas.height = Math.max(1, Math.round(videoHeight * scale));
  const context = canvas.getContext("2d");
  if (!context) return null;
  context.drawImage(video, 0, 0, canvas.width, canvas.height);
  let data: string;
  try {
    data = canvas.toDataURL("image/jpeg", 0.8);
  } catch {
    // A canvas the webview will not read back.
    return null;
  }
  return data.startsWith(JPEG_DATA) ? data.slice(data.indexOf(",") + 1) : null;
}

/** The result itself: a picture in an `img` element, a video from its `blob:` URL, whose poster
 *  frame is kept as its thumbnail the first time it plays when it has none; why it cannot be
 *  shown when its bytes cannot be read (a video too big to play here, as a notice: Show in
 *  folder, among the actions, opens it). */
function Media(props: { run: Run; output: Output; videoRef: RefObject<HTMLVideoElement | null> }) {
  const { run, output, videoRef } = props;
  const { url, error } = useOutputUrl(output.id, false, output.mime);
  // Whether its poster frame has been sent to be kept, while this result is shown.
  const posterSent = useRef(false);
  const keepPoster = (video: HTMLVideoElement) => {
    if (output.hasThumb || posterSent.current) return;
    const poster = capturePoster(video);
    if (poster === null) return;
    posterSent.current = true;
    // A poster that is not kept costs nothing: the tile keeps showing the start frame. Nothing
    // is said on screen; the refusal leaves a trace.
    useStudio
      .getState()
      .savePoster(output.id, poster)
      .catch((e: unknown) => console.warn("Studio: a video's poster frame could not be kept", e));
  };
  if (error) return <ErrorBanner error={error} tone={error.code === "studio_too_big_to_show" ? "notice" : "error"} />;
  if (!url) return null;
  return run.kind === "video" ? (
    <video
      data-testid="studio-video"
      ref={videoRef}
      src={url}
      controls
      onPlaying={(e) => keepPoster(e.currentTarget)}
      className="max-h-full max-w-full"
    />
  ) : (
    <img src={url} alt={run.prompt} className="max-h-full max-w-full object-contain" />
  );
}

/** A row of the panel: its name, then its value. */
function Row(props: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-fg-muted">{props.label}</dt>
      <dd className="min-w-0 break-words">{props.children}</dd>
    </>
  );
}

const ACTION = "rounded-lg px-2.5 py-1 text-sm hover:bg-bg-muted disabled:opacity-60 aria-pressed:text-accent";

/** The viewer, open on result `outputId`; `onClose` closes it. */
export function Viewer(props: { outputId: string; onClose(): void }) {
  const { onClose } = props;
  // The result shown: the one it was opened on, until the arrows move it.
  const [shownId, setShownId] = useState(props.outputId);
  const [said, setSaid] = useState<Said | null>(null);
  const [offer, setOffer] = useState<(ModelOffer & { action: "animate" | "edit" }) | null>(null);
  const [preview, setPreview] = useState<{ runId: string; answer: { preview: RequestPreview } | { error: ErrorInfo } } | null>(
    null,
  );
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [withSettings, setWithSettings] = useState(false);
  const [opened, setOpened] = useState(props.outputId);
  if (props.outputId !== opened) {
    setOpened(props.outputId);
    setShownId(props.outputId);
    setSaid(null);
    setOffer(null);
  }
  // Set by an action that leaves the cursor in the prompt box once the viewer has closed.
  const promptNext = useRef(false);
  const video = useRef<HTMLVideoElement>(null);
  const content = useRef<HTMLDivElement>(null);

  const run = useStudio((s) => runOfOutput(s, shownId));
  const target = run ? targetOf(run) : null;
  const model = useStudio((s) => (run && target ? listedModel(target, run.model, s.local, s.endpoints) : null));
  const company = useStudio((s) => (run ? companyOf(run, s.endpoints) : null));
  const outputs = run ? resultsOf(run) : [];
  const index = outputs.findIndex((o) => o.id === shownId);
  const output = outputs[index] ?? null;
  const picture = run?.kind === "image";
  const local = run?.runsOn === "local";

  // Where the result shown sits, its run and its place there: when it leaves the run (deleted
  // meanwhile, say after ← went back to it during a Delete), the result nearest its place takes
  // its place, and the viewer closes once the run has none left.
  const [place, setPlace] = useState<{ runId: string; index: number } | null>(null);
  if (run && output && (place?.runId !== run.id || place.index !== index)) setPlace({ runId: run.id, index });
  const left = useStudio((s) => (run === null && place !== null ? runById(s, place.runId) : null));
  const remaining = left ? resultsOf(left) : [];
  const nearest = place ? remaining[Math.min(place.index, remaining.length - 1)] : undefined;
  if (!run && nearest) {
    setShownId(nearest.id);
    setSaid(null);
    setOffer(null);
  }
  const gone = !run && !nearest;
  const closeGone = useEffectEvent(() => onClose());
  useEffect(() => {
    if (gone) closeGone();
  }, [gone]);

  const store = useStudio.getState;
  const show = (id: string) => {
    setShownId(id);
    setSaid(null);
    setOffer(null);
  };
  const perform = (action: () => Promise<void>) => {
    setBusy(true);
    setSaid(null);
    action().then(
      () => setBusy(false),
      (e: unknown) => {
        setBusy(false);
        setSaid({ error: toErrorInfo(e) });
      },
    );
  };
  /** Closes the viewer; with `prompt`, the prompt box gets the cursor once it has. */
  const closeTo = (prompt: boolean) => {
    promptNext.current = prompt;
    onClose();
  };

  const move = (by: number) => {
    const to = outputs[index + by];
    if (to) show(to.id);
  };
  const star = () => {
    if (output) perform(() => store().star(output.id, !output.starred));
  };
  const again = () => {
    if (!run) return;
    // Started from the run's own settings (a second press meanwhile does nothing); its card,
    // or why it was refused (under the prompt of its tab), shows once the viewer is closed:
    // Studio goes to the run's tab first, as it may have been opened from the grid's All.
    store().setTab(run.kind);
    void store().again(run);
    closeTo(false);
  };
  const reuse = (withSeed: boolean) => {
    if (!run) return;
    setOffer(null);
    const refused = store().reuse(run, withSeed);
    setSaid(refused ? { text: refused, warn: true } : { text: STUDIO_VIEWER.reused, warn: false });
  };
  const putPicture = (action: "animate" | "edit", use?: ModelChoice) => {
    if (!output || !picture) return;
    setOffer(null);
    perform(async () => {
      const answer =
        action === "animate" ? await store().animate(output, use) : await store().editWithWords(output, use);
      if (answer) setOffer({ ...answer, action });
      else closeTo(true);
    });
  };
  const useAsReference = () => {
    if (!output || !picture) return;
    perform(async () => {
      const refused = await store().useAsReference(output);
      setSaid(refused ? { text: refused, warn: true } : { text: STUDIO_VIEWER.addedAsReference, warn: false });
    });
  };
  const saveAs = () => {
    if (!run || !output) return;
    const name = fileNameOf(output.path);
    const ext = name.includes(".") ? (name.split(".").pop() ?? "") : "";
    perform(async () => {
      const path = await saveDialog({
        defaultPath: name,
        filters: ext ? [{ name: STUDIO_VIEWER.fileType(ext, run.kind), extensions: [ext] }] : undefined,
      });
      if (typeof path !== "string") return;
      const written = await api.studioExport(output.id, path, withSettings);
      setSaid({ text: STUDIO_VIEWER.savedTo(written), warn: false });
    });
  };
  const copy = () => {
    if (!output || !picture) return;
    setBusy(true);
    setSaid(null);
    copyPicture(output).then(
      () => {
        setBusy(false);
        setSaid({ text: STUDIO_VIEWER.copied, warn: false });
      },
      () => {
        setBusy(false);
        setSaid({ text: STUDIO_VIEWER.noCopy, warn: true });
      },
    );
  };
  const sendToChat = () => {
    if (!output || !picture) return;
    // The chat is shown once it is in the message box, and the viewer closes with that; why it
    // cannot go is said here, with Studio left as it was.
    perform(() => store().sendToChat(output));
  };
  const copyPrompt = () => {
    if (!run) return;
    perform(async () => {
      if (ipc.isTauri()) await writeText(run.prompt);
      else await navigator.clipboard.writeText(run.prompt);
      setSaid({ text: STUDIO_VIEWER.copied, warn: false });
    });
  };
  const remove = () => {
    if (!output) return;
    setAsking(false);
    const deleted = output;
    // The next result of the run takes its place at once (else the one before); the last one
    // deleted closes the viewer, once it has left the run.
    const neighbor = outputs[index + 1] ?? outputs[index - 1] ?? null;
    if (neighbor) show(neighbor.id);
    perform(async () => {
      try {
        await store().remove([deleted.id]);
      } catch (e) {
        if (neighbor) show(deleted.id);
        throw e;
      }
    });
  };
  const seeRequest = () => {
    if (!run) return;
    if (preview?.runId === run.id) {
      setPreview(null);
      return;
    }
    const req = requestFromRun(run, "preview", model, true);
    if (!req) return;
    const runId = run.id;
    api.studioPreview(req).then(
      (answer) => setPreview({ runId, answer: { preview: answer } }),
      (e: unknown) => setPreview({ runId, answer: { error: toErrorInfo(e) } }),
    );
  };
  const playPause = () => {
    const v = video.current;
    if (!v) return;
    // A refused play (no sound allowed yet, say) leaves the video as it was, and a trace.
    if (v.paused) v.play().catch((e: unknown) => console.warn("Studio: the video could not be played", e));
    else v.pause();
  };

  // The viewer's keys, wherever the focus is but in text being typed, and not while a question
  // over it is open (its own keys, Esc too, are its). A key another layer took (a popup that
  // closed on Esc) is left alone.
  const onKey = useEffectEvent((e: KeyboardEvent) => {
    if (e.defaultPrevented || asking || e.ctrlKey || e.metaKey || e.altKey || typing(e.target)) return;
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    // The video's own controls take the arrows (seeking) and Space while it has the focus.
    if (e.target instanceof HTMLVideoElement && (key === " " || key === "ArrowLeft" || key === "ArrowRight")) return;
    // While an action is on its way (its buttons wait), only moving and closing are taken.
    if (busy && key !== "Escape" && key !== "ArrowLeft" && key !== "ArrowRight") return;
    const actions: Record<string, () => void> = {
      Escape: onClose,
      ArrowRight: () => move(1),
      ArrowLeft: () => move(-1),
      f: star,
      r: () => reuse(!e.shiftKey),
      a: () => putPicture("animate"),
      s: saveAs,
      c: copy,
      Delete: () => setAsking(output !== null),
      Backspace: () => setAsking(output !== null),
    };
    if (key === " ") {
      // A focused button, or checkbox, takes Space itself (a checkbox ticks on the keyup only
      // when its keydown was left alone).
      if (e.target instanceof HTMLButtonElement || e.target instanceof HTMLInputElement) return;
      e.preventDefault();
      playPause();
      return;
    }
    const action = actions[key];
    if (!action) return;
    e.preventDefault();
    action();
  });
  useEffect(() => {
    const listener = (e: KeyboardEvent) => onKey(e);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  const cost = run && !local ? (run.costUsd === null ? STUDIO_PRICE.unknown : formatUsd(run.costUsd)) : null;
  const took = run ? tookMs(run) : null;
  const tookText = took === null ? null : STUDIO_CARD.took(tookWords(took));
  const localCost =
    run && local
      ? [
          STUDIO_PRICE.free,
          tookText,
          run.local?.device ? STUDIO_VIEWER.on(run.local.device) : null,
          run.local?.engineBuild != null ? STUDIO_VIEWER.engineBuild(run.local.engineBuild) : null,
        ]
          .filter((p): p is string => p !== null)
          .join(STUDIO_CARD.joiner)
      : null;
  const settings = run ? settingsWords(run, model) : "";
  const notes = [...(model?.notes ?? []), ...(run?.local?.labelled ? [STUDIO_VIEWER.labelled] : [])];
  const shownPreview = run && preview?.runId === run.id ? preview.answer : null;

  return (
    <Dialog.Root
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay fixed inset-0" />
        <Dialog.Content
          aria-describedby={undefined}
          data-studio-viewer
          ref={content}
          // The viewer itself takes the focus, not its first button, so Space plays a video
          // rather than pressing that button.
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            content.current?.focus();
          }}
          onCloseAutoFocus={(e) => {
            if (!promptNext.current) return;
            e.preventDefault();
            window.dispatchEvent(new Event("app:focus-studio-prompt"));
          }}
          // Side by side from 700 px (the window is never narrower than 720 px); stacked below
          // that, the details take at most half the height and scroll, so the result keeps the rest.
          className="fixed inset-4 flex flex-col overflow-hidden rounded-2xl border border-border bg-surface shadow-pop min-[700px]:flex-row"
        >
          <Dialog.Title className="sr-only">{STUDIO_VIEWER.title(run?.kind ?? "image")}</Dialog.Title>
          <section className="flex min-h-0 min-w-0 flex-1 flex-col">
            <div className="flex min-h-0 flex-1 items-center justify-center bg-black/85 p-4">
              {run && output && <Media key={output.id} run={run} output={output} videoRef={video} />}
            </div>
            {outputs.length > 0 && (
              <div className="flex items-center justify-center gap-3 border-t border-border py-1.5 text-sm">
                <button
                  type="button"
                  aria-label={STUDIO_VIEWER.previous}
                  disabled={index <= 0}
                  onClick={() => move(-1)}
                  className="rounded-md p-1 hover:bg-bg-muted disabled:opacity-40"
                >
                  <ChevronLeft size={16} aria-hidden />
                </button>
                <span className="tabular-nums">{STUDIO_VIEWER.position(index + 1, outputs.length)}</span>
                <button
                  type="button"
                  aria-label={STUDIO_VIEWER.next}
                  disabled={index >= outputs.length - 1}
                  onClick={() => move(1)}
                  className="rounded-md p-1 hover:bg-bg-muted disabled:opacity-40"
                >
                  <ChevronRight size={16} aria-hidden />
                </button>
              </div>
            )}
            {run && output && (
              <div className="flex flex-wrap items-center gap-1 border-t border-border px-3 py-2">
                <button type="button" aria-pressed={output.starred} disabled={busy} onClick={star} className={ACTION}>
                  <Star size={14} aria-hidden fill={output.starred ? "currentColor" : "none"} className="mr-1 inline" />
                  {STUDIO_VIEWER.star}
                </button>
                <button type="button" onClick={again} className={ACTION}>
                  {STUDIO_CARD.again}
                </button>
                {picture && (
                  <>
                    <button type="button" disabled={busy} onClick={() => putPicture("edit")} className={ACTION}>
                      {STUDIO_VIEWER.edit}
                    </button>
                    <button type="button" disabled={busy} onClick={() => putPicture("animate")} className={ACTION}>
                      {STUDIO_VIEWER.animate}
                    </button>
                    <button type="button" disabled={busy} onClick={useAsReference} className={ACTION}>
                      {STUDIO_VIEWER.useAsReference}
                    </button>
                  </>
                )}
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => perform(() => api.studioShowInFolder(output.id))}
                  className={ACTION}
                >
                  {STUDIO_VIEWER.showInFolder}
                </button>
                <button type="button" disabled={busy} onClick={() => setAsking(true)} className={`${ACTION} text-danger`}>
                  {STUDIO_VIEWER.delete}
                </button>
              </div>
            )}
          </section>
          <aside
            data-viewer-details
            className="flex max-h-[50%] min-h-0 w-full flex-col gap-3 overflow-y-auto border-t border-border p-4 text-sm min-[700px]:max-h-none min-[700px]:w-72 min-[700px]:shrink-0 min-[700px]:border-t-0 min-[700px]:border-l md:w-80"
          >
            <div className="flex justify-end">
              <Dialog.Close aria-label={STUDIO_VIEWER.close} className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg">
                <X size={16} aria-hidden />
              </Dialog.Close>
            </div>
            {run && output && (
              <>
                <div className="flex flex-col gap-1">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-xs text-fg-muted">{STUDIO_VIEWER.prompt}</span>
                    <button
                      type="button"
                      aria-label={STUDIO_VIEWER.copyPrompt}
                      onClick={copyPrompt}
                      className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg"
                    >
                      <Copy size={14} aria-hidden />
                    </button>
                  </div>
                  <p className="break-words whitespace-pre-wrap">{run.prompt}</p>
                </div>
                <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5">
                  <Row label={STUDIO_VIEWER.model}>
                    <p>{run.modelLabel}</p>
                    {local ? (
                      <LocalMark />
                    ) : (
                      <div className="flex flex-wrap items-center gap-1.5">
                        <p className="text-fg-muted">
                          {STUDIO_VIEWER.runsAt(company ? STUDIO_COMPANY[company] : run.providerName, run.upstream)}
                        </p>
                        <PlaceMark
                          mark="cloud"
                          icon={Cloud}
                          label={cloudChipWords(run.providerHost)}
                          title={run.providerHost ? cloudGoesTo(run.providerHost) : undefined}
                        />
                      </div>
                    )}
                  </Row>
                  <Row label={STUDIO_VIEWER.size}>{sizeWords(run, output, model)}</Row>
                  {settings && <Row label={STUDIO_VIEWER.settings}>{settings}</Row>}
                  <Row label={STUDIO_VIEWER.made}>{madeWords(run.createdAt)}</Row>
                  <Row label={STUDIO_VIEWER.file}>
                    <span title={output.path} className="font-mono text-xs break-all">
                      {output.path}
                    </span>
                  </Row>
                </dl>
                {local ? (
                  <p>
                    {STUDIO_VIEWER.row(STUDIO_VIEWER.cost)}
                    {localCost}
                  </p>
                ) : (
                  <p>
                    {STUDIO_VIEWER.row(STUDIO_VIEWER.cost)}
                    <span className={`tabular-nums ${COST_TONE_CLASS[costTone(run.costUsd)]}`}>
                      {[cost, tookText].filter((p) => p !== null).join(STUDIO_CARD.joiner)}
                    </span>
                  </p>
                )}
                {notes.length > 0 && (
                  <ul className="flex list-disc flex-col gap-0.5 pl-4 text-xs text-fg-muted" aria-label={STUDIO_VIEWER.notes}>
                    {notes.map((note) => (
                      <li key={note}>{note}</li>
                    ))}
                  </ul>
                )}
                <div className="flex flex-wrap gap-2">
                  <button type="button" onClick={() => reuse(true)} className={CARD_BUTTON}>
                    {STUDIO_VIEWER.reuseAll}
                  </button>
                  <button type="button" onClick={() => reuse(false)} className={CARD_BUTTON}>
                    {STUDIO_VIEWER.reuseWithoutSeed}
                  </button>
                </div>
                <div className="flex flex-col gap-1.5">
                  <div className="flex flex-wrap gap-2">
                    {picture && (
                      <button type="button" disabled={busy} onClick={sendToChat} className={CARD_BUTTON}>
                        {STUDIO_CHAT_LINK.sendToChat}
                      </button>
                    )}
                    <button type="button" disabled={busy} onClick={saveAs} className={CARD_BUTTON}>
                      {STUDIO_VIEWER.saveAs}
                    </button>
                    {picture && (
                      <button type="button" disabled={busy} onClick={copy} className={CARD_BUTTON}>
                        {STUDIO_VIEWER.copy}
                      </button>
                    )}
                  </div>
                  <label className="flex items-center gap-2 text-xs">
                    <input type="checkbox" checked={withSettings} onChange={(e) => setWithSettings(e.target.checked)} />
                    {STUDIO_VIEWER.withSettings}
                  </label>
                </div>
                {offer && (
                  <div role="status" className="flex flex-col gap-1.5 rounded-lg border border-border bg-bg-subtle p-2.5">
                    <p>{offer.text}</p>
                    <div className="flex flex-wrap gap-2">
                      {offer.choice && offer.name && (
                        <button
                          type="button"
                          disabled={busy}
                          onClick={() => offer.choice && putPicture(offer.action, offer.choice)}
                          className={CARD_BUTTON}
                        >
                          {STUDIO_OFFER.choose(offer.name)}
                        </button>
                      )}
                      <button type="button" onClick={() => setOffer(null)} className={CARD_BUTTON}>
                        {STUDIO_ACTIONS.notNow}
                      </button>
                    </div>
                  </div>
                )}
                {said &&
                  ("error" in said ? (
                    <ErrorBanner error={said.error} onDismiss={() => setSaid(null)} />
                  ) : (
                    <p role="status" className={said.warn ? "text-warning" : "text-fg-muted"}>
                      {said.text}
                    </p>
                  ))}
                <div className="flex flex-col gap-2 border-t border-border pt-3">
                  <button
                    type="button"
                    aria-expanded={shownPreview !== null}
                    onClick={seeRequest}
                    className="self-start rounded-lg border border-border px-2.5 py-1 text-xs hover:border-border-strong"
                  >
                    {local ? STUDIO_ADVANCED.seeCommand : STUDIO_ADVANCED.seeRequest}
                  </button>
                  {shownPreview &&
                    ("error" in shownPreview ? (
                      <ErrorBanner error={shownPreview.error} />
                    ) : shownPreview.preview.method === "run" ? (
                      <ExactCommand preview={shownPreview.preview} />
                    ) : (
                      <ExactRequest preview={shownPreview.preview} />
                    ))}
                </div>
              </>
            )}
          </aside>
          <ConfirmDelete
            results={asking && run && output ? [{ run, output }] : null}
            onCancel={() => setAsking(false)}
            onConfirm={remove}
          />
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
