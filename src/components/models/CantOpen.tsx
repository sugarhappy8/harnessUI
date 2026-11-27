import { useEffect, useId, useState } from "react";
import { CircleSlash } from "lucide-react";
import * as ipc from "../../lib/ipc";
import { listenInEffect } from "../../lib/listen";
import type { ErrorInfo, UpdateStatus } from "../../lib/types";
import { CANT_OPEN, cantOpenCardTip, updateLine } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { Outside, shown } from "./Compare";

// A model whose kind this version of the app cannot open yet (the model runner it comes with
// does not know it). The words are words.ts's CANT_OPEN; this file is the pieces the
// repository page and the Installed row share: what the kind is called, one box, and the
// update check that answers it.

/**
 * The kind's name as the screens say it: the file's own text, or null when there is none to
 * say (no `general.architecture` in the file reads "unknown"). Show it through `Outside`.
 */
export function kindName(architecture: string | null | undefined): string | null {
  const kind = architecture?.trim();
  return kind && kind !== "unknown" ? kind : null;
}

/** What an update check found, for a model that still cannot open: one line, or null. */
function checkLine(s: UpdateStatus): string | null {
  if (!s.configured) return CANT_OPEN.noUpdates;
  if (s.ready) return `${updateLine(s)} ${CANT_OPEN.readyHint}`;
  // Nothing newer: say what that means for this model, not only "latest".
  if (!s.downloading && !s.checking && !s.error && !s.available && s.checked) return CANT_OPEN.latest(s.currentVersion);
  return updateLine(s, CANT_OPEN.tryAgain);
}

/**
 * "Check for updates", with its answer in a line beside it: the same check as Settings →
 * General → Updates, started where the person is stuck. A newer version is downloaded and
 * checked by the updater itself and then offered in the update box; nothing is installed here.
 * `look`: a button (the page's box) or an underlined link (an Installed row).
 */
export function UpdateCheck(props: { look: "button" | "link" }) {
  const [status, setStatus] = useState<UpdateStatus | null>(null);
  const [asked, setAsked] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ErrorInfo | null>(null);

  // Once asked, the updater's own changes (downloading, ready) change the line too.
  useEffect(() => {
    if (!asked) return;
    return listenInEffect(() =>
      ipc.onUpdateState((state) => setStatus((cur) => (cur ? { ...cur, ...state } : cur))),
    );
  }, [asked]);

  async function check() {
    setBusy(true);
    setAsked(true);
    setError(null);
    try {
      const before = await ipc.updateStatus();
      if (before.configured) {
        try {
          await ipc.checkForUpdate();
        } catch (e) {
          // A check that did not work is in the status line; say it only once.
          if (toErrorInfo(e).code !== "update_check") throw e;
        }
      }
      setStatus(await ipc.updateStatus());
    } catch (e) {
      setError(toErrorInfo(e));
    } finally {
      setBusy(false);
    }
  }

  const line = error ? null : busy ? CANT_OPEN.checking : status ? checkLine(status) : null;
  const className =
    props.look === "button"
      ? "btn-primary rounded-lg px-3 py-1 text-sm font-medium"
      : "underline hover:text-fg disabled:no-underline disabled:opacity-50";
  return (
    <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
      <button type="button" disabled={busy} onClick={() => void check()} className={className}>
        {CANT_OPEN.check}
      </button>
      <span role="status" className="text-xs text-fg-muted">
        {line}
      </span>
      {error && (
        <span role="alert" className="text-xs text-danger">
          {error.message} {error.nextStep}
        </span>
      )}
    </span>
  );
}

/**
 * The one box on the page of a model this version cannot open: what is wrong, that nobody
 * is at fault, and what to do next. The same shape as the app's other notices (nothing is
 * broken: the app stopped on purpose), with the update check beside it. A file that names no
 * kind gets its own sentence and no update to wait for, and a picture or video model says what
 * it is (an update would not change that).
 */
export function CantOpenBox(props: { architecture: string | null; picture?: boolean }) {
  const kind = kindName(props.architecture);
  const headingId = useId();
  if (props.picture) {
    return (
      <section
        aria-labelledby={headingId}
        className="enter space-y-1 rounded-xl border border-warning/40 bg-warning/10 px-3.5 py-2.5 text-sm shadow-soft"
      >
        <h4 id={headingId} className="font-medium">
          {CANT_OPEN.picture.title}
        </h4>
        <p>{CANT_OPEN.picture.body}</p>
      </section>
    );
  }
  return (
    <section
      aria-labelledby={headingId}
      className="enter space-y-1 rounded-xl border border-warning/40 bg-warning/10 px-3.5 py-2.5 text-sm shadow-soft"
    >
      <h4 id={headingId} className="font-medium">
        {CANT_OPEN.title}
      </h4>
      {kind === null ? (
        <p>{CANT_OPEN.noKind}</p>
      ) : (
        <>
          <p>
            {CANT_OPEN.body.before}
            <Outside text={kind} cap={60} />
            {CANT_OPEN.body.after}
          </p>
          <p className="text-fg-muted">{CANT_OPEN.next}</p>
          <div className="pt-1">
            <UpdateCheck look="button" />
          </div>
        </>
      )}
    </section>
  );
}

/**
 * An installed model's row: the same news in one line, with the update check beside it as a
 * link. `id` is the line's, for Use's `aria-describedby`: Use is off, and this says why.
 */
export function CantOpenLine(props: { id: string; architecture: string | null | undefined }) {
  const kind = kindName(props.architecture);
  return (
    <div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs">
      <p id={props.id} className="flex items-start gap-1.5">
        <CircleSlash size={12} className="mt-0.5 shrink-0 text-warning" aria-hidden />
        <span>
          {kind === null ? (
            CANT_OPEN.noKind
          ) : (
            <>
              {CANT_OPEN.row.before}
              <Outside text={kind} cap={60} />
              {CANT_OPEN.row.after}
            </>
          )}
        </span>
      </p>
      {kind !== null && <UpdateCheck look="link" />}
    </div>
  );
}

/**
 * A search card's hint that this version may not know the model's kind. Dashed and without a
 * fill, like Studio's hints from a name: it is a guess from Hugging Face's listing, and the
 * page checks the files. The tooltip says so, and so does the text a screen reader gets (a
 * tooltip is not read to it). It sits inside the card's button, so it is text, not a control.
 */
export function CantOpenHint(props: { kind: string }) {
  const tip = cantOpenCardTip(shown(props.kind, 60));
  return (
    <span
      data-hint="listing"
      title={tip}
      className="inline-flex shrink-0 items-center gap-1 rounded-full border border-dashed border-danger/40 px-2 py-0.5 text-[11px] text-danger"
    >
      <CircleSlash size={11} aria-hidden />
      {CANT_OPEN.short}
      <span className="sr-only"> ({tip})</span>
    </span>
  );
}
