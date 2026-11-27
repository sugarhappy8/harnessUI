import { useEffect, useId, useState } from "react";
import * as ipc from "../../lib/ipc";
import { listenInEffect } from "../../lib/listen";
import type { ErrorInfo, UpdateStatus } from "../../lib/types";
import { CANT_OPEN, updateLine } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { Outside } from "./Compare";

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
 * kind gets its own sentence and no update to wait for.
 */
export function CantOpenBox(props: { architecture: string | null }) {
  const kind = kindName(props.architecture);
  const headingId = useId();
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
