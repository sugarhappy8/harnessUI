// What the error boundaries show in place of what could not be drawn (0.2.1). The whole
// window's notice (AppCrash, around the app in main.tsx), and a panel's smaller one
// (PanelBoundary, around Studio, the chat, the workbench, a model's page in Get models, the
// model sheet and each page of Settings), which keeps the sidebar and the rest working.
import { useEffect, useId, useRef, type ReactNode } from "react";
import { RotateCcw, TriangleAlert } from "lucide-react";
import clsx from "clsx";
import { ErrorBoundary, type FallbackProps } from "./ErrorBoundary";
import { crashSummary } from "../lib/crash";
import { CRASH } from "../lib/words";
import { FOCUS_RING } from "./ui/styles";

const SECONDARY =
  "inline-flex items-center gap-1.5 rounded-lg border border-border bg-bg px-3 py-1.5 text-sm hover:border-border-strong hover:bg-bg-muted " +
  FOCUS_RING;
const PRIMARY = "btn-primary inline-flex items-center gap-1.5 rounded-lg px-3 py-1.5 text-sm font-medium " + FOCUS_RING;

/** The collapsed *Details*: the error's name and its cleaned message (no path, no key). */
function Details(props: { error: Error }) {
  const { name, message } = crashSummary(props.error);
  return (
    <details className="group text-xs text-fg-muted">
      <summary className={clsx("w-fit cursor-pointer rounded select-none hover:text-fg", FOCUS_RING)}>
        {CRASH.details}
      </summary>
      <p
        data-crash-details
        className="mt-2 rounded-lg border border-border bg-bg-subtle px-3 py-2 font-mono break-words whitespace-pre-wrap text-fg"
      >
        {message ? `${name}: ${message}` : name}
      </p>
    </details>
  );
}

/** The whole window's notice: what happened, that nothing was lost, *Try again* (draws the app
 *  again) and *Reload harnessUI* (loads the window again), and the details, collapsed. It needs
 *  nothing the app sets up (no provider, no store), so it draws whatever failed. */
export function AppCrash(props: FallbackProps) {
  const titleId = useId();
  const again = useRef<HTMLButtonElement>(null);
  // Keyboard users land on the way out, not on the page that went away.
  useEffect(() => again.current?.focus(), []);
  return (
    <div data-app-crash className="flex h-full min-h-0 w-full overflow-y-auto bg-bg text-fg">
      <div
        role="alert"
        aria-labelledby={titleId}
        className="m-auto flex w-full max-w-md flex-col gap-4 rounded-2xl border border-border bg-surface p-6 shadow-pop max-sm:mx-4 max-sm:p-5"
      >
        <div className="flex items-start gap-3">
          <span className="grid size-9 shrink-0 place-items-center rounded-full bg-danger/12 text-danger" aria-hidden>
            <TriangleAlert className="size-5" />
          </span>
          <div className="flex min-w-0 flex-col gap-1.5">
            <h1 id={titleId} className="text-base font-semibold text-balance">
              {CRASH.title}
            </h1>
            <p className="text-sm text-fg-muted">
              {CRASH.safe} {CRASH.next}
            </p>
          </div>
        </div>
        <div className="flex flex-wrap gap-2">
          <button ref={again} type="button" className={PRIMARY} onClick={props.reset}>
            <RotateCcw className="size-4" aria-hidden />
            {CRASH.tryAgain}
          </button>
          <button type="button" className={SECONDARY} onClick={() => window.location.reload()}>
            {CRASH.reload}
          </button>
        </div>
        <Details error={props.error} />
      </div>
    </div>
  );
}

/** Where a panel's notice is shown, with its words and the name the console report gives. */
export type CrashWhere = keyof typeof CRASH.where;

const COMPONENT: Record<CrashWhere, string> = {
  studio: "StudioView",
  chat: "ChatView",
  workbench: "Workbench",
  detail: "ModelDetail",
  sheet: "ModelSheet",
  settings: "SettingsDialog",
};

/** A panel's notice: "<panel> couldn't be shown.", that the rest still works, *Try again* and
 *  the details. `fill` centres it in a large pane (Studio, the chat); otherwise it sits at the
 *  top of the panel it replaces (a dialog's content). */
export function PanelCrash(props: FallbackProps & { where: CrashWhere; fill?: boolean }) {
  const titleId = useId();
  return (
    <div className={clsx(props.fill && "flex min-h-0 min-w-0 flex-1 items-center justify-center overflow-y-auto p-6 max-sm:p-4")}>
      <div
        role="alert"
        aria-labelledby={titleId}
        data-panel-crash={props.where}
        className="flex w-full max-w-md flex-col gap-3 rounded-xl border border-danger/40 bg-danger/10 px-4 py-3.5 text-sm"
      >
        <div className="flex items-start gap-2.5">
          <TriangleAlert className="mt-0.5 size-4 shrink-0 text-danger" aria-hidden />
          <div className="flex min-w-0 flex-col gap-0.5">
            <p id={titleId} className="font-medium">
              {CRASH.panel(CRASH.where[props.where])}
            </p>
            <p className="text-fg-muted">{CRASH.panelNext}</p>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-2 pl-6.5">
          <button type="button" className={SECONDARY} onClick={props.reset}>
            <RotateCcw className="size-4" aria-hidden />
            {CRASH.tryAgain}
          </button>
        </div>
        <div className="pl-6.5">
          <Details error={props.error} />
        </div>
      </div>
    </div>
  );
}

/** An error boundary around one independent panel: an error drawing it shows PanelCrash there
 *  and leaves the rest of the window as it was. `resetKeys` draw it again when what it shows
 *  changes (another model, another chat). */
export function PanelBoundary(props: {
  where: CrashWhere;
  fill?: boolean;
  resetKeys?: readonly unknown[];
  children: ReactNode;
}) {
  return (
    <ErrorBoundary
      name={COMPONENT[props.where]}
      resetKeys={props.resetKeys}
      fallback={(f) => <PanelCrash {...f} where={props.where} fill={props.fill} />}
    >
      {props.children}
    </ErrorBoundary>
  );
}

/** The app's root: an error boundary around everything, with AppCrash for the whole window.
 *  main.tsx and the preview harness draw the app inside it. */
export function AppRoot(props: { children: ReactNode }) {
  return (
    <ErrorBoundary name="App" fallback={(f) => <AppCrash {...f} />}>
      {props.children}
    </ErrorBoundary>
  );
}
