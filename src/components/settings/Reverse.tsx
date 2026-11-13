import { useCallback, useEffect, useRef, useState } from "react";
import { AlertDialog } from "radix-ui";
import { Download, RefreshCw, Trash2, X } from "lucide-react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import * as ipc from "../../lib/ipc";
import type {
  ErrorInfo,
  ReverseComponent,
  ReverseComponentStatus,
  ReverseDecompiler,
  ReverseDownloadStage,
  ReverseProgress,
  ReverseStatus,
} from "../../lib/types";
import { toErrorInfo } from "../../store/app";
import { ErrorBanner } from "../ErrorBanner";
import { SafeLink } from "../Markdown";

// Settings → Reverse engineering (0.0.6 item 5; docs/dev/0.0.6/item5-spec.md WP9d): rea, a
// free tool, with a decompiler (Kuna or Ghidra), as optional downloads like the ROCm pack.
// Nothing is downloaded until the owner clicks Download and accepts the licences here.
//
// - Every text depends on what this computer offers (`reverse_status`: the rows, whether
//   each is available and why not, the system, Apple's command line tools on a Mac).
// - The page asks again every POLL_MS while it is open and visible, and when the window
//   regains focus, so Apple's tools turn to "Installed" without a restart.
// - A change that would stop rea while it works on a file answers `busy`; the page asks
//   "rea is working on a file in a chat. Stop it and continue?" and repeats it with force.
// - Nothing here is reachable by a tool or the model: these are main-window commands.

/** How often the page asks for the status while it is open. */
export const POLL_MS = 5000;

const INTRO =
  "Lets the model look inside programs and other files you point it at: what a program is made of, the text and functions inside it, and readable code worked out from its machine code (decompiling). It uses rea, a free tool, with a decompiler. Nothing is downloaded until you choose it here. After that it works without the internet.";
const OWN_FILES =
  "Use it only on files you own or are allowed to examine. Some licences and some countries restrict reverse engineering.";
const RESPONSIBLE_USE_URL = "https://github.com/morluto/rea/blob/main/DISCLAIMER.md";
const CRAFTED =
  "A file made to attack decompilers could still harm this computer. Open programs from people you don't trust only on a computer you can wipe.";
const REACH =
  "In Ask first and Allow all, rea reaches only the chat's approved folders. In Full access it reaches any file except harnessUI's own. rea never runs the files it looks at, and it is never connected to programs that are running.";
const APPLE_INTRO =
  "On a Mac, rea needs Apple's command line tools before it can be used. They are free and come from Apple (a large download).";
const APPLE_ASKED = "Follow Apple's window to install them. This page updates when they are ready.";
const APPLE_FIRST = "Can't be used until Apple's command line tools are installed (step 1 above).";
const NO_DECOMPILER =
  "Install a decompiler to open programs. Checks that need none (for example, which functions a Windows program uses from other libraries) work with rea alone.";
// The owner's answer Q4 = B: the group is off in new chats until turned on in a chat's Tools
// panel; after that, new chats follow the last choice. So the first step is turning it on.
const HOW_TO_USE =
  "To use it, turn on Reverse engineering in a chat's Tools panel. New chats then follow your last choice. Ask about a program file in one of the chat's folders, or drop the file on the message box.";
const BUSY_QUESTION = "rea is working on a file in a chat. Stop it and continue?";
const KUNA_INTRO = "Small and fast; its makers call it an experiment.";
const KUNA_MAC =
  "It can't open most Mac apps, because they are built for both Apple and Intel chips, or Windows programs.";
const KUNA_LINUX = "Opens Linux programs and Mac programs built for one kind of chip, not Windows programs.";
const GHIDRA_ABOUT =
  "Made by the NSA and widely used. Slower: each file can take a few minutes to open, and it uses up to about 3 GB of memory while it works.";
const GHIDRA_16GB = "With a large model loaded, your computer may slow down while Ghidra works.";
const GHIDRA_WINDOWS =
  "On Windows, rea's Ghidra support is new and opens Windows programs and libraries (.exe and .dll) on this PC's own drives only.";
const DECOMPILER_ABOUT = "It turns a program's machine code back into code people can read.";
const DECOMPILER_BOTH =
  "A decompiler turns a program's machine code back into code people can read. Install one, or both and switch between them.";
const ACCEPT = "Downloading means you accept these terms.";
const REA_TERMS =
  "rea is under the MIT License and includes other open-source software listed in its notices. Node.js is under the MIT License.";
const KUNA_TERMS = "Kuna is under the Apache License 2.0.";
const GHIDRA_TERMS =
  "Ghidra is under the Apache License 2.0, with a few parts under the GPL 3. Eclipse Temurin is under the GPL 2 with the Classpath Exception.";

const ROW_LABEL: Record<ReverseComponent, string> = { rea: "rea", kuna: "Kuna", ghidra: "Ghidra" };
const PART_LABEL: Record<string, string> = { rea: "rea", node: "Node.js", kuna: "Kuna", ghidra: "Ghidra", java: "Java" };
const DECOMPILERS: ReverseDecompiler[] = ["kuna", "ghidra"];
const STAGE_ORDER: Record<ReverseDownloadStage, number> = { downloading: 0, checking: 1, unpacking: 2 };

/** Whole megabytes, as Hugging Face counts them ("80"). */
export function mb(bytes: number): string {
  return Math.round(bytes / 1e6).toLocaleString("en-US");
}

/** Gigabytes with one decimal ("1.1"). */
export function gb(bytes: number): string {
  return (bytes / 1e9).toFixed(1);
}

function partLabel(part: string): string {
  return PART_LABEL[part] ?? part;
}

/** A row is on this computer, usable or not. */
function onDisk(row: ReverseComponentStatus | undefined): boolean {
  const k = row?.state.kind;
  return k === "installed" || k === "broken" || k === "needsUpdate";
}

function version(row: ReverseComponentStatus | undefined, part: string): string {
  return row?.parts.find((p) => p.name === part)?.version ?? "";
}

/** "21.0.13+9" → "21". */
function major(v: string): string {
  return v.split(/[.+]/)[0] ?? v;
}

/** Where a system is named in a sentence ("On Windows", "On a Mac"). */
function onSystem(os: ReverseStatus["os"]): string {
  switch (os) {
    case "windows":
      return "On Windows";
    case "macos":
      return "On a Mac";
    case "linux":
      return "On Linux";
    default:
      return "On this computer";
  }
}

/** The step a download is in, as one line. */
function stageLine(stage: ReverseDownloadStage, done: number, total: number): string {
  if (stage === "checking") return "Checking…";
  if (stage === "unpacking") return "Unpacking…";
  return `Downloading… ${mb(done)} MB of ${mb(total)} MB`;
}

/** One download the owner started here: its last event and each part seen so far. */
interface Op {
  total: number;
  last: ReverseProgress | null;
  parts: { name: string; stage: ReverseDownloadStage; done: number; total: number }[];
}

/** Steps run one part at a time, so every other part seen before has finished its step. */
export function nextOp(op: Op | undefined, e: ReverseProgress, total: number): Op {
  const parts = (op?.parts ?? []).map((p) =>
    p.name !== e.part && STAGE_ORDER[p.stage] <= STAGE_ORDER[e.stage] ? { ...p, done: p.total } : p,
  );
  const mine = { name: e.part, stage: e.stage, done: e.partDone, total: e.partTotal };
  const i = parts.findIndex((p) => p.name === e.part);
  if (i >= 0) parts[i] = mine;
  else parts.push(mine);
  return { total: op?.total ?? total, last: e, parts };
}

interface DownloadAsk {
  component: ReverseComponent;
  title: string;
  text: string;
  licenses: { part: string; name: string; url: string }[];
}

/** Whether a download of `component` brings rea along (a decompiler's, while rea is missing). */
function bringsRea(status: ReverseStatus, component: ReverseComponent): boolean {
  const k = status.components.find((r) => r.component === "rea")?.state.kind;
  return component !== "rea" && (k === "notInstalled" || k === "partial");
}

/** Bytes a download of `component` fetches, rea's included when it comes along. */
export function downloadBytes(status: ReverseStatus, component: ReverseComponent): number {
  const own = status.components.find((r) => r.component === component)?.downloadBytes ?? 0;
  const rea = status.components.find((r) => r.component === "rea")?.downloadBytes ?? 0;
  return own + (bringsRea(status, component) ? rea : 0);
}

/** The download dialog for a row: rea's parts come first when a decompiler brings rea along. */
export function downloadAsk(status: ReverseStatus, component: ReverseComponent): DownloadAsk {
  const row = (c: ReverseComponent) => status.components.find((r) => r.component === c);
  const rea = row("rea");
  const reaMissing = bringsRea(status, component);
  const own = row(component);
  const bytes = downloadBytes(status, component);
  const licenses = [...(reaMissing ? (rea?.licenses ?? []) : []), ...(own?.licenses ?? [])];
  const about = `About ${mb(bytes)} MB`;
  if (component === "rea") {
    return {
      component,
      title: "Download rea?",
      text: `${about} from npmjs.com (rea) and nodejs.org (Node.js). ${REA_TERMS} ${ACCEPT}`,
      licenses,
    };
  }
  const from =
    component === "kuna"
      ? "Kuna from Kuna's releases on GitHub"
      : "Ghidra from the NSA's releases on GitHub and Java from Eclipse Adoptium's";
  const terms = component === "kuna" ? KUNA_TERMS : GHIDRA_TERMS;
  if (reaMissing) {
    return {
      component,
      title: `Download rea and ${ROW_LABEL[component]}?`,
      text: `${about}: rea from npmjs.com and Node.js from nodejs.org, then ${from}. ${REA_TERMS} ${terms} ${ACCEPT}`,
      licenses,
    };
  }
  if (component === "kuna") {
    return {
      component,
      title: "Download Kuna?",
      text: `${about} from Kuna's releases on GitHub. ${KUNA_TERMS} ${ACCEPT}`,
      licenses,
    };
  }
  return {
    component,
    title: "Download Ghidra and Java?",
    text: `${about}: Ghidra from the NSA's releases on GitHub and Java from Eclipse Adoptium's. ${GHIDRA_TERMS} ${ACCEPT}`,
    licenses,
  };
}

/** The remove dialog's question and what it frees. */
export function removeAsk(status: ReverseStatus, component: ReverseComponent): { title: string; text: string } {
  const row = (c: ReverseComponent) => status.components.find((r) => r.component === c);
  if (component === "kuna") return { title: "Remove Kuna?", text: `This frees ${mb(row("kuna")?.diskBytes ?? 0)} MB.` };
  if (component === "ghidra") return { title: "Remove Ghidra?", text: `This frees ${gb(row("ghidra")?.diskBytes ?? 0)} GB.` };
  const with_ = DECOMPILERS.filter((d) => onDisk(row(d))).map((d) => ROW_LABEL[d]);
  const frees = gb(
    status.components.filter((r) => r.component === "rea" || onDisk(r)).reduce((n, r) => n + r.diskBytes, 0),
  );
  if (with_.length === 0) return { title: "Remove rea?", text: `This frees ${frees} GB. You can download it again later.` };
  return {
    title: "Remove rea?",
    text: `This also removes ${with_.join(" and ")} and frees ${frees} GB. You can download them again later.`,
  };
}

/** The line under an installed row that cannot be used, with its one action. */
function problemLine(row: ReverseComponentStatus): { text: string; action: "repair" | "update" } | null {
  const s = row.state;
  if (s.kind === "broken" && s.why === "missing")
    return { text: `Part of ${ROW_LABEL[row.component]} is missing, perhaps removed by antivirus software.`, action: "repair" };
  if (s.kind === "broken")
    return { text: `Your antivirus blocked part of ${ROW_LABEL[row.component]}. Allow harnessUI in it, then choose Repair.`, action: "repair" };
  if (s.kind === "needsUpdate" && s.why.kind === "securityFix")
    return { text: `This version of ${partLabel(s.why.part)} has a known problem. Update it to keep using it.`, action: "update" };
  if (s.kind === "needsUpdate" && s.why.kind === "notAccepted") {
    const bytes = row.update?.downloadBytes ?? row.downloadBytes;
    return {
      text: `This version of harnessUI needs rea ${s.why.need}. Until you update (${mb(bytes)} MB), reverse engineering is off.`,
      action: "update",
    };
  }
  return null;
}

type Pending = { kind: "remove" | "discard" | "switch" | "clear"; component?: ReverseComponent; decompiler?: ReverseDecompiler };

export function Reverse() {
  const [status, setStatus] = useState<ReverseStatus | null>(null);
  const [ops, setOps] = useState<Partial<Record<ReverseComponent, Op>>>({});
  const [error, setError] = useState<{ error: ErrorInfo; component: ReverseComponent | null } | null>(null);
  const [asking, setAsking] = useState<DownloadAsk | null>(null);
  const [removing, setRemoving] = useState<ReverseComponent | null>(null);
  const [busy, setBusy] = useState<Pending | null>(null);
  const [appleAsked, setAppleAsked] = useState(false);
  const [details, setDetails] = useState(false);
  const [reported, setReported] = useState(false);
  const [working, setWorking] = useState(false);
  const seq = useRef(0);

  /** A late answer never overwrites a newer one. */
  const show = useCallback((s: ReverseStatus, n: number) => {
    if (n === seq.current) setStatus(s);
  }, []);

  const refresh = useCallback(() => {
    const n = ++seq.current;
    return ipc
      .reverseStatus()
      .then((s) => show(s, n))
      .catch((err) => setError({ error: toErrorInfo(err), component: null }));
  }, [show]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void refresh();
    }, POLL_MS);
    const onFocus = () => void refresh();
    window.addEventListener("focus", onFocus);
    let alive = true;
    let unlisten: (() => void) | undefined;
    void ipc
      .onReverseProgress((p) => {
        if (p.finished) {
          setOps((old) => {
            const next = { ...old };
            delete next[p.component];
            return next;
          });
          if (p.error && p.error.code !== "cancelled") setError({ error: p.error, component: p.component });
          void refresh();
          return;
        }
        setOps((old) => ({ ...old, [p.component]: nextOp(old[p.component], p, p.total) }));
      })
      .then((u) => {
        if (alive) unlisten = u;
        else u();
      });
    return () => {
      alive = false;
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
      unlisten?.();
    };
  }, [refresh]);

  const fail = useCallback((err: unknown, component: ReverseComponent | null = null) => {
    setError({ error: toErrorInfo(err), component });
  }, []);

  async function download(component: ReverseComponent) {
    setAsking(null);
    setError(null);
    setReported(false);
    const total = status ? downloadBytes(status, component) : 0;
    setOps((old) => ({ ...old, [component]: old[component] ?? { total, last: null, parts: [] } }));
    try {
      await ipc.reverseDownload(component, true);
    } catch (err) {
      setOps((old) => {
        const next = { ...old };
        delete next[component];
        return next;
      });
      fail(err, component);
    }
    void refresh();
  }

  /** Runs a change; `busy` turns into the question, and Stop and continue repeats it forced. */
  async function change(p: Pending, force = false) {
    setBusy(null);
    setRemoving(null);
    setError(null);
    setWorking(true);
    try {
      let s: ReverseStatus;
      if (p.kind === "remove" && p.component) s = await ipc.reverseRemove(p.component, force);
      else if (p.kind === "discard" && p.component) s = await ipc.reverseDiscardPartial(p.component);
      else if (p.kind === "switch" && p.decompiler) s = await ipc.reverseSetDecompiler(p.decompiler, force);
      else s = await ipc.reverseClearCache(force);
      show(s, ++seq.current);
    } catch (err) {
      const e = toErrorInfo(err);
      if (e.code === "busy" && !force) setBusy(p);
      else fail(err, p.component ?? null);
    } finally {
      setWorking(false);
    }
  }

  async function installAppleTools() {
    setError(null);
    try {
      await ipc.reverseInstallAppleTools();
      setAppleAsked(true);
    } catch (err) {
      fail(err);
    }
    void refresh();
  }

  async function report(e: ErrorInfo) {
    const text = `${e.message}\n${e.nextStep}\ncode: ${e.code}\n\n${e.details}`;
    try {
      if (ipc.isTauri()) await writeText(text);
      else await navigator.clipboard.writeText(text);
      setReported(true);
    } catch {
      setReported(false);
    }
  }

  const rows = status?.components ?? [];
  const row = (c: ReverseComponent) => rows.find((r) => r.component === c);
  const os = status?.os ?? "other";
  const rea = row("rea");
  const reaOnDisk = onDisk(rea);
  const appleMissing = status?.appleTools === "missing";
  const offered = DECOMPILERS.filter((d) => row(d)?.available);
  const onlyOffered = offered.length === 1 ? offered[0] : undefined;
  // The decompilers that can be the choice: offered here and on this computer (reverse_cmd.rs
  // decides the same way, so files of one this build does not offer are never used).
  const decompilersOnDisk = DECOMPILERS.filter((d) => row(d)?.available && onDisk(row(d)));
  const used = status?.chosen && decompilersOnDisk.includes(status.chosen) ? status.chosen : decompilersOnDisk[0];
  const label = (d: ReverseDecompiler) => ROW_LABEL[d];

  function errorBanner() {
    if (!error) return null;
    const e = error.error;
    let action: { label: string; onClick: () => void } | undefined;
    if ((e.code === "missing" || e.code === "blocked") && error.component) {
      const c = error.component;
      action = { label: "Repair", onClick: () => void download(c) };
    } else if (e.code === "path_too_long" || e.code === "blocked_host" || e.code === "not_local_drive") {
      action = { label: reported ? "Copied" : "Report this", onClick: () => void report(e) };
    }
    return <ErrorBanner error={e} action={action} onDismiss={() => setError(null)} />;
  }

  function progress(c: ReverseComponent, r: ReverseComponentStatus) {
    const op = ops[c];
    const carrier = c === "rea" ? DECOMPILERS.find((d) => ops[d]) : undefined;
    if (!op && carrier) {
      return <p className="text-xs text-fg-muted">{`Comes with the ${ROW_LABEL[carrier]} download.`}</p>;
    }
    let line: string;
    let fraction: number;
    if (op?.last) {
      const l = op.last;
      line = stageLine(l.stage, l.done, l.total);
      fraction = l.stage === "unpacking" ? (l.partTotal > 0 ? l.partDone / l.partTotal : 0) : l.total > 0 ? l.done / l.total : 0;
    } else if (r.state.kind === "downloading") {
      line = stageLine(r.state.stage, r.state.done, r.state.total);
      fraction = r.state.total > 0 ? r.state.done / r.state.total : 0;
    } else {
      line = stageLine("downloading", 0, op?.total ?? r.downloadBytes);
      fraction = 0;
    }
    const pct = Math.max(0, Math.min(100, fraction * 100));
    return (
      <div className="mt-2 space-y-1">
        <div
          className="h-1.5 overflow-hidden rounded bg-bg-muted"
          role="progressbar"
          aria-label={`${ROW_LABEL[c]} download`}
          aria-valuenow={Math.round(pct)}
          aria-valuemin={0}
          aria-valuemax={100}
        >
          <div className="h-full bg-accent" style={{ width: `${pct}%` }} />
        </div>
        <p className="text-xs text-fg-muted">{line}</p>
        {op && op.parts.length > 0 && (
          <ul className="space-y-1" aria-label="Parts">
            {op.parts.map((p) => {
              const pp = p.total > 0 ? Math.min(100, (p.done / p.total) * 100) : 0;
              const text =
                p.stage === "downloading"
                  ? `${mb(p.done)} MB of ${mb(p.total)} MB`
                  : p.stage === "checking"
                    ? // nextOp marks a part whose check is over as done; the one being checked has 0.
                      p.done >= p.total && p.total > 0
                      ? "Checked"
                      : "Checking…"
                    : p.done >= p.total && p.total > 0
                      ? "Unpacked"
                      : "Unpacking…";
              return (
                <li key={p.name} className="flex items-center gap-2 text-xs text-fg-muted">
                  <span className="w-16 shrink-0">{partLabel(p.name)}</span>
                  <span
                    className="h-1 flex-1 overflow-hidden rounded bg-bg-muted"
                    role="progressbar"
                    aria-label={partLabel(p.name)}
                    aria-valuenow={Math.round(pp)}
                    aria-valuemin={0}
                    aria-valuemax={100}
                  >
                    <span className="block h-full bg-accent" style={{ width: `${pp}%` }} />
                  </span>
                  <span className="w-32 shrink-0 text-right">{text}</span>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    );
  }

  function describe(c: ReverseComponent, r: ReverseComponentStatus): string[] {
    if (c === "rea") return [`rea ${version(r, "rea")}, with Node.js ${version(r, "node")} to run it`];
    if (c === "kuna") {
      const start = `Kuna ${version(r, "kuna")}. ${KUNA_INTRO}`;
      if (os === "macos") return [`${start} ${KUNA_MAC}${row("ghidra")?.available ? " Ghidra can open Mac apps." : ""}`];
      return [`${start} ${KUNA_LINUX}`];
    }
    const lines = [`Ghidra ${version(r, "ghidra")}, with Java ${major(version(r, "java"))} (Eclipse Temurin). ${GHIDRA_ABOUT}`];
    if (status?.memoryGb != null && status.memoryGb <= 16) lines.push(GHIDRA_16GB);
    if (os === "windows") lines.push(GHIDRA_WINDOWS);
    return lines;
  }

  function rowView(c: ReverseComponent) {
    const r = row(c);
    if (!r) return null;
    if (!r.available) {
      // Not offered by this build here. Files left on this computer (by a debug build, or a
      // part a later version withdrew) are never used, but can be removed to free the space.
      const leftover = onDisk(r) ? "remove" : r.state.kind === "partial" ? "discard" : null;
      return (
        <li key={c} className="rounded-md border border-border px-3 py-2" aria-label={ROW_LABEL[c]}>
          <div className="flex items-start gap-2">
            <div className="min-w-0 flex-1 space-y-0.5 opacity-60">
              <p className="font-medium">{ROW_LABEL[c]}</p>
              <p className="text-xs text-fg-muted">{r.reason}</p>
            </div>
            {leftover && (
              <button
                type="button"
                className="flex shrink-0 items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-bg-muted disabled:opacity-40"
                disabled={working}
                onClick={() => (leftover === "remove" ? setRemoving(c) : void change({ kind: "discard", component: c }))}
              >
                <Trash2 size={12} aria-hidden /> Remove
              </button>
            )}
          </div>
        </li>
      );
    }
    const s = r.state;
    // rea's row also shows a decompiler's download that brings rea along.
    const running =
      !!ops[c] || s.kind === "downloading" || (c === "rea" && !onDisk(r) && DECOMPILERS.some((d) => ops[d]));
    const problem = problemLine(r);
    const button = "flex items-center gap-1 rounded-md border border-border px-2 py-1 text-xs hover:bg-bg-muted disabled:opacity-40";
    const primary = "flex items-center gap-1 rounded-md bg-accent px-3 py-1 text-xs text-accent-fg disabled:opacity-40";
    return (
      <li key={c} className="rounded-md border border-border px-3 py-2" aria-label={ROW_LABEL[c]}>
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1 space-y-0.5">
            <p className="font-medium">{ROW_LABEL[c]}</p>
            {describe(c, r).map((line) => (
              <p key={line} className="text-xs text-fg-muted">
                {line}
              </p>
            ))}
            <p className="text-xs text-fg-muted">{`${mb(r.downloadBytes)} MB download · ${mb(r.diskBytes)} MB on disk`}</p>
          </div>
          <div className="flex shrink-0 flex-wrap items-center justify-end gap-1">
            {running ? (
              <button type="button" className={button} onClick={() => void ipc.reverseCancel(c)}>
                <X size={12} aria-hidden /> Cancel
              </button>
            ) : s.kind === "notInstalled" ? (
              <button
                type="button"
                className={primary}
                onClick={() => status && setAsking(downloadAsk(status, c))}
              >
                <Download size={12} aria-hidden /> Download
              </button>
            ) : s.kind === "partial" ? (
              <>
                <button type="button" className={primary} onClick={() => void download(c)}>
                  <Download size={12} aria-hidden /> Resume
                </button>
                <button type="button" className={button} disabled={working} onClick={() => void change({ kind: "discard", component: c })}>
                  <Trash2 size={12} aria-hidden /> Remove
                </button>
              </>
            ) : (
              <>
                {problem?.action === "repair" && (
                  <button type="button" className={primary} onClick={() => void download(c)}>
                    <RefreshCw size={12} aria-hidden /> Repair
                  </button>
                )}
                {(problem?.action === "update" || (s.kind === "installed" && r.update)) && (
                  <button type="button" className={primary} onClick={() => void download(c)}>
                    <Download size={12} aria-hidden /> Update
                  </button>
                )}
                <button type="button" className={button} disabled={working} onClick={() => setRemoving(c)}>
                  <Trash2 size={12} aria-hidden /> Remove
                </button>
              </>
            )}
          </div>
        </div>
        {running && progress(c, r)}
        {!running && s.kind === "partial" && (
          <p className="mt-1 text-xs text-fg-muted">{`Paused at ${mb(s.bytes)} MB of ${mb(r.downloadBytes)} MB. Remove frees that space.`}</p>
        )}
        {!running && s.kind === "notInstalled" && <p className="mt-1 text-xs text-fg-muted">Not installed</p>}
        {!running && s.kind === "installed" && <p className="mt-1 text-xs">Installed</p>}
        {!running && s.kind === "installed" && r.update && (
          <p className="mt-1 text-xs">
            {`Update available: ${partLabel(r.update.part)} ${r.update.from} → ${r.update.to}. ${mb(r.update.downloadBytes)} MB. The version you have keeps working until you update.`}
          </p>
        )}
        {!running && problem && (
          <p className="mt-1 text-xs text-danger" role="status">
            {problem.text}
          </p>
        )}
        {!running && c === "rea" && appleMissing && onDisk(r) && <p className="mt-1 text-xs text-warning">{APPLE_FIRST}</p>}
      </li>
    );
  }

  function runLine(): { text: string; failed: string | null } | null {
    if (!status || !reaOnDisk) return null;
    const r = status.running;
    switch (r.kind) {
      case "stopped":
        return { text: "rea is not running.", failed: null };
      case "starting":
        return { text: "rea is starting…", failed: null };
      case "ready":
        return { text: status.decompiler ? `rea is running (${label(status.decompiler)}).` : "rea is running.", failed: null };
      case "busy":
        return { text: "rea is working on a file…", failed: null };
      case "failed":
        return { text: "rea stopped with a problem. It starts again the next time it is used.", failed: r.reason };
    }
  }

  const run = runLine();
  const ask = removing && status ? removeAsk(status, removing) : null;

  return (
    <div className="space-y-4 text-sm">
      <h2 className="text-lg font-medium">Reverse engineering</h2>
      <p className="text-fg-muted">{INTRO}</p>
      <p className="text-fg-muted">
        {OWN_FILES} <SafeLink href={RESPONSIBLE_USE_URL}>rea's notes on responsible use</SafeLink>
      </p>
      <p className="text-fg-muted">{CRAFTED}</p>
      <p className="text-fg-muted">{REACH}</p>
      {errorBanner()}

      {os === "macos" && status && (
        <section className="space-y-1 rounded-md border border-border px-3 py-2" aria-label="Apple's command line tools">
          <h3 className="font-medium">Step 1: Apple's command line tools</h3>
          <p className="text-xs text-fg-muted">{APPLE_INTRO}</p>
          <div className="flex items-center gap-2">
            <span className={appleMissing ? "text-warning" : ""}>{appleMissing ? "Not installed" : "Installed"}</span>
            {appleMissing && (
              <button
                type="button"
                onClick={() => void installAppleTools()}
                className="rounded-md bg-accent px-3 py-1 text-xs text-accent-fg"
              >
                Install
              </button>
            )}
          </div>
          {appleMissing && appleAsked && <p className="text-xs text-fg-muted">{APPLE_ASKED}</p>}
        </section>
      )}

      {status && (
        <ul className="space-y-2" aria-label="rea">
          {rowView("rea")}
        </ul>
      )}

      {status && (
        <section className="space-y-2" aria-label="Decompiler">
          <h3 className="font-medium">Decompiler</h3>
          <p className="text-xs text-fg-muted">
            {offered.length >= 2
              ? DECOMPILER_BOTH
              : onlyOffered
                ? `${onSystem(os)}, ${label(onlyOffered)} is the decompiler. ${DECOMPILER_ABOUT}`
                : "No decompiler is offered for this computer."}
          </p>
          <ul className="space-y-2">
            {rowView("kuna")}
            {rowView("ghidra")}
          </ul>
          {decompilersOnDisk.length >= 2 ? (
            <div className="flex items-center gap-2" role="radiogroup" aria-label="Use for decompiling:">
              <span>Use for decompiling:</span>
              {DECOMPILERS.map((d) => (
                <button
                  key={d}
                  type="button"
                  role="radio"
                  aria-checked={status.chosen === d}
                  disabled={working}
                  onClick={() => status.chosen !== d && void change({ kind: "switch", decompiler: d })}
                  className={
                    "rounded-md border px-3 py-1 text-xs " +
                    (status.chosen === d ? "border-accent bg-accent text-accent-fg" : "border-border hover:bg-bg-muted")
                  }
                >
                  {label(d)}
                </button>
              ))}
            </div>
          ) : decompilersOnDisk.length === 1 ? (
            // The owner's choice, also while it is broken or needs an update (its row says so).
            <p>{`Decompiling uses ${label(used ?? "kuna")}.`}</p>
          ) : offered.length > 0 ? (
            <p className="text-fg-muted">{NO_DECOMPILER}</p>
          ) : null}
          {onDisk(row("kuna")) && (
            <div className="flex items-center gap-2">
              <p className="flex-1 text-xs text-fg-muted">
                {`Saved analysis: ${mb(status.cacheBytes)} MB. Kuna keeps what it worked out so a file opens faster next time.`}
              </p>
              <button
                type="button"
                disabled={working}
                onClick={() => void change({ kind: "clear" })}
                className="rounded-md border border-border px-2 py-1 text-xs hover:bg-bg-muted disabled:opacity-40"
              >
                Clear
              </button>
            </div>
          )}
        </section>
      )}

      {run && (
        <section className="space-y-1" aria-label="rea status">
          <p role="status">
            {run.text}{" "}
            {run.failed !== null && (
              <button type="button" className="text-xs underline" onClick={() => setDetails((d) => !d)}>
                {details ? "Hide details" : "Show details"}
              </button>
            )}
          </p>
          {run.failed !== null && details && (
            <pre className="max-h-48 overflow-auto rounded-md border border-border bg-bg-subtle p-2 text-xs whitespace-pre-wrap">
              {run.failed}
            </pre>
          )}
        </section>
      )}
      {reaOnDisk && <p className="text-xs text-fg-muted">{HOW_TO_USE}</p>}

      <AlertDialog.Root open={asking !== null} onOpenChange={(o) => !o && setAsking(null)}>
        <AlertDialog.Portal>
          <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
          <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[32rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 shadow-pop">
            <AlertDialog.Title className="text-base font-semibold">{asking?.title}</AlertDialog.Title>
            <AlertDialog.Description className="mt-2 text-sm text-fg-muted">{asking?.text}</AlertDialog.Description>
            {asking && asking.licenses.length > 0 && (
              <ul className="mt-2 space-y-0.5 text-xs" aria-label="Licenses">
                {asking.licenses.map((l) => (
                  <li key={l.part}>
                    <SafeLink href={l.url}>{`${partLabel(l.part)} license`}</SafeLink>
                  </li>
                ))}
              </ul>
            )}
            <div className="mt-4 flex justify-end gap-2 text-sm">
              <AlertDialog.Cancel className="rounded-md px-3 py-1.5 hover:bg-bg-muted">Cancel</AlertDialog.Cancel>
              <AlertDialog.Action
                className="rounded-md bg-accent px-3 py-1.5 text-accent-fg"
                onClick={() => asking && void download(asking.component)}
              >
                Accept and download
              </AlertDialog.Action>
            </div>
          </AlertDialog.Content>
        </AlertDialog.Portal>
      </AlertDialog.Root>

      <AlertDialog.Root open={ask !== null} onOpenChange={(o) => !o && setRemoving(null)}>
        <AlertDialog.Portal>
          <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
          <AlertDialog.Content className="fixed top-1/2 left-1/2 w-[30rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 shadow-pop">
            <AlertDialog.Title className="text-base font-semibold">{ask?.title}</AlertDialog.Title>
            <AlertDialog.Description className="mt-2 text-sm text-fg-muted">{ask?.text}</AlertDialog.Description>
            <div className="mt-4 flex justify-end gap-2 text-sm">
              <AlertDialog.Cancel className="rounded-md px-3 py-1.5 hover:bg-bg-muted">Cancel</AlertDialog.Cancel>
              <AlertDialog.Action
                className="rounded-md bg-danger px-3 py-1.5 text-white"
                onClick={() => removing && void change({ kind: "remove", component: removing })}
              >
                Remove
              </AlertDialog.Action>
            </div>
          </AlertDialog.Content>
        </AlertDialog.Portal>
      </AlertDialog.Root>

      <AlertDialog.Root open={busy !== null} onOpenChange={(o) => !o && setBusy(null)}>
        <AlertDialog.Portal>
          <AlertDialog.Overlay className="dialog-overlay fixed inset-0" />
          <AlertDialog.Content
            aria-describedby={undefined}
            className="fixed top-1/2 left-1/2 w-[28rem] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 rounded-2xl border border-border bg-surface p-5 shadow-pop"
          >
            <AlertDialog.Title className="text-base font-semibold">{BUSY_QUESTION}</AlertDialog.Title>
            <div className="mt-4 flex justify-end gap-2 text-sm">
              <AlertDialog.Cancel className="rounded-md px-3 py-1.5 hover:bg-bg-muted">Cancel</AlertDialog.Cancel>
              <AlertDialog.Action
                className="rounded-md bg-accent px-3 py-1.5 text-accent-fg"
                onClick={() => busy && void change(busy, true)}
              >
                Stop and continue
              </AlertDialog.Action>
            </div>
          </AlertDialog.Content>
        </AlertDialog.Portal>
      </AlertDialog.Root>
    </div>
  );
}
