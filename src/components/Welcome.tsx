import { FileText, FolderOpen, FolderSearch, Globe, HardDrive, ShieldCheck, TerminalSquare, WifiOff } from "lucide-react";
import { useApp } from "../store/app";
import { APP_NAME } from "../lib/app";
import { BrandMark } from "./Brand";

/** Things to try. Choosing one writes it into the message box; nothing is sent. The
 *  wording avoids the labels of buttons elsewhere in the app. */
const IDEAS = [
  {
    icon: FolderSearch,
    title: "Understand a project",
    text: "Look through my project folder and explain what it does and how it is organised.",
  },
  {
    icon: TerminalSquare,
    title: "Run and fix",
    text: "Run the tests in my project folder and fix whatever fails.",
  },
  {
    icon: Globe,
    title: "Research on the web",
    text: "Search the web for the latest on this topic and summarise it with sources: ",
  },
  {
    icon: FileText,
    title: "Summarise a document",
    text: "Summarise the document I attach in ten short points.",
  },
];

/** Puts `text` into the message box (the composer listens for this). */
function compose(text: string) {
  window.dispatchEvent(new CustomEvent("app:compose", { detail: text }));
}

function Ideas() {
  return (
    <div className="grid w-full gap-3 sm:grid-cols-2">
      {IDEAS.map((idea, i) => (
        <button
          key={idea.title}
          type="button"
          onClick={() => compose(idea.text)}
          className="card-interactive enter flex items-start gap-3 rounded-xl p-3.5 text-left"
          style={{ animationDelay: `${80 + i * 50}ms` }}
        >
          <span className="mt-0.5 rounded-lg bg-bg-muted p-2 text-accent">
            <idea.icon size={16} aria-hidden />
          </span>
          <span className="min-w-0">
            <span className="block text-sm font-medium">{idea.title}</span>
            <span className="block text-xs leading-relaxed text-fg-muted">{idea.text}</span>
          </span>
        </button>
      ))}
    </div>
  );
}

/** The model that is loaded, or the ones that can be loaded with one click. */
function ModelCard(props: { onOpenModels: () => void }) {
  const runtime = useApp((s) => s.runtime);
  const options = useApp((s) => s.modelOptions);
  const pickModel = useApp((s) => s.pickModel);
  const openModelFile = useApp((s) => s.openModelFile);

  if (runtime?.state === "ready" || runtime?.state === "starting") {
    const ready = runtime.state === "ready";
    const where = [runtime.backend, runtime.device].filter(Boolean).join(" · ");
    return (
      <div className="enter flex w-full items-center gap-3 rounded-xl border border-border bg-surface p-3.5 shadow-soft">
        <span className={"h-2.5 w-2.5 shrink-0 rounded-full " + (ready ? "bg-success" : "pulse bg-warning")} aria-hidden />
        <div className="min-w-0 flex-1">
          <p className="truncate text-sm font-medium">{runtime.modelName ?? "Local model"}</p>
          <p className="truncate text-xs text-fg-muted">
            {ready
              ? `Ready${where ? ` on ${where}` : ""}${runtime.nCtx ? ` · ${runtime.nCtx.toLocaleString()} context` : ""}`
              : "Loading…"}
          </p>
        </div>
        {ready && runtime.modelPath && (
          <button
            type="button"
            onClick={() => void pickModel(`local:${runtime.modelPath}`)}
            className="btn-primary rounded-lg px-3 py-1.5 text-sm font-medium"
          >
            Chat with it
          </button>
        )}
      </div>
    );
  }

  const recent = options.slice(0, 3);
  return (
    <div className="enter w-full rounded-xl border border-border bg-surface p-3.5 shadow-soft">
      <p className="mb-2 text-sm font-medium">{recent.length > 0 ? "Pick up where you left off" : "Start with a model"}</p>
      <div className="flex flex-wrap gap-2">
        {recent.map((o) => (
          <button
            key={o.ref}
            type="button"
            onClick={() => void pickModel(o.ref)}
            title={o.label}
            className="flex max-w-full items-center gap-1.5 rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm hover:border-border-strong hover:bg-bg-muted"
          >
            {o.source === "local" ? <HardDrive size={14} aria-hidden /> : <Globe size={14} aria-hidden />}
            <span className="truncate">{o.label}</span>
          </button>
        ))}
        <button
          type="button"
          onClick={() => void openModelFile()}
          className="flex items-center gap-1.5 rounded-lg border border-border bg-bg px-2.5 py-1.5 text-sm hover:border-border-strong hover:bg-bg-muted"
        >
          <FolderOpen size={14} aria-hidden /> Open a GGUF file…
        </button>
        <button
          type="button"
          onClick={props.onOpenModels}
          className="rounded-lg px-2.5 py-1.5 text-sm text-fg-muted hover:bg-bg-muted hover:text-fg"
        >
          Find a model to download
        </button>
      </div>
    </div>
  );
}

/** The first thing on screen when no chat is open. */
export function Welcome() {
  const openModels = () => window.dispatchEvent(new Event("app:open-models"));
  return (
    <div className="mx-auto flex min-h-full max-w-2xl flex-col items-center justify-center gap-6 px-4 py-10">
      <div className="enter flex flex-col items-center gap-3 text-center">
        <BrandMark size={52} glow />
        <h1 className="text-2xl font-semibold tracking-tight">
          What should <span className="brand-text">{APP_NAME}</span> work on?
        </h1>
        <p className="max-w-md text-sm text-fg-muted">
          Start a new chat or pick one from the list. The model runs on this computer. It asks before it changes
          anything, unless you switch a chat to Allow all.
        </p>
      </div>
      <ModelCard onOpenModels={openModels} />
      <Ideas />
      <ul className="enter-fade flex flex-wrap items-center justify-center gap-x-5 gap-y-1 text-xs text-fg-muted">
        <li className="flex items-center gap-1.5">
          <ShieldCheck size={13} aria-hidden /> Every action needs your approval
        </li>
        <li className="flex items-center gap-1.5">
          <WifiOff size={13} aria-hidden /> Works offline, no account
        </li>
      </ul>
    </div>
  );
}

/** An open chat with nothing in it yet. */
export function EmptyChat() {
  return (
    <div className="mx-auto flex max-w-2xl flex-col items-center gap-5 px-4 py-12">
      <div className="enter flex flex-col items-center gap-2 text-center">
        <BrandMark size={40} glow />
        <p className="text-lg font-semibold tracking-tight">Send a message to start.</p>
        <p className="text-sm text-fg-muted">Or begin with one of these.</p>
      </div>
      <Ideas />
    </div>
  );
}
