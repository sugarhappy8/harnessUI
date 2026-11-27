// Studio's error boundary (0.2.1). What Studio shows lives in its store (Get models' search, the
// repositories read, the open model's page), which outlives the boundary: Try again draws the
// same data, so a crash that comes from that data would come back every time. This boundary
// gives a way out: *Back to Studio* on its notice, and leaving Studio (or changing its tab)
// while the notice shows, both go back to Studio's results with what was read dropped.
import type { ReactNode } from "react";
import { PanelBoundary } from "../CrashNotice";
import type { ResetCause } from "../ErrorBoundary";
import { useStudio } from "../../store/studio";
import { CRASH } from "../../lib/words";

/** Studio's results again: Get models, a model's page, the viewer and the model sheet closed,
 *  and the last search and the repositories read dropped (they are read again when asked). */
export function backToStudioHome(): void {
  useStudio.setState({
    localView: false,
    detail: null,
    detailError: null,
    localSearch: null,
    repos: {},
    viewer: null,
    sheetOpen: false,
  });
}

/** Studio inside its boundary, as App draws it. */
export function StudioBoundary(props: { children: ReactNode }) {
  const view = useStudio((s) => s.view);
  const tab = useStudio((s) => s.tab);
  const localView = useStudio((s) => s.localView);
  const detailOpen = useStudio((s) => s.detail !== null);
  return (
    <PanelBoundary
      where="studio"
      fill
      resetKeys={[view, tab, localView, detailOpen]}
      onReset={(cause: ResetCause) => {
        // Try again keeps the place; anything else the person did while the notice showed
        // (back to the chat, another tab) starts Studio from its results.
        if (cause === "keys") backToStudioHome();
      }}
      action={{ label: CRASH.backToStudio, run: backToStudioHome }}
    >
      {props.children}
    </PanelBoundary>
  );
}
