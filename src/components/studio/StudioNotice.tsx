import { Clapperboard, X } from "lucide-react";
import { STUDIO_NOTICE } from "../../lib/words";
import { useStudio } from "../../store/studio";

// The notice over the chat when a video finishes while the chat is shown (0.1.0 Task B8; the
// store raises it, `readyNotice`): "Your video is ready", and Open goes to Studio's Video tab
// with the viewer on the video's first result. Opening Studio any other way puts it away too.
// It floats under the chat's header (App puts it in the chat's column), taking no room there.

/** "Your video is ready" with Open; nothing while no video finished in the chat's view. */
export function StudioNotice() {
  const ready = useStudio((s) => (s.readyNotice === null ? null : (s.live[s.readyNotice] ?? null)));
  const dismiss = useStudio((s) => s.dismissReadyNotice);
  if (!ready) return null;
  const open = () => {
    const s = useStudio.getState();
    s.showStudio("video");
    const first = ready.outputs[0];
    if (first) s.openViewer(first.id);
  };
  return (
    <div
      role="status"
      className="enter absolute top-16 left-1/2 z-20 flex max-w-[92%] -translate-x-1/2 items-center gap-2.5 rounded-xl border border-border bg-surface px-3 py-2 text-sm shadow-float"
    >
      <Clapperboard size={15} className="shrink-0 text-accent" aria-hidden />
      <p className="min-w-0 font-medium">{STUDIO_NOTICE.ready}</p>
      <button type="button" onClick={open} className="btn-primary shrink-0 rounded-lg px-3 py-1 text-sm font-medium">
        {STUDIO_NOTICE.open}
      </button>
      <button
        type="button"
        aria-label={STUDIO_NOTICE.putAway}
        onClick={dismiss}
        className="shrink-0 rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg"
      >
        <X size={14} aria-hidden />
      </button>
    </div>
  );
}
