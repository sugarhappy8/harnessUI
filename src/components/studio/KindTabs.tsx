import { Tabs } from "radix-ui";
import { STUDIO_BAR, STUDIO_TABS } from "../../lib/words";
import type { StudioTab } from "../../store/studio";

// Studio's [Pictures | Video] (0.1.0 Tasks B2 and B13): the top bar's, bound to the tab, and
// Get local models' own, bound to its tab, look and behave alike. Each sits in the Radix
// `Tabs.Root` that holds its value.

const TABS: readonly StudioTab[] = ["image", "video"];

const TAB_CLASS =
  "rounded-md px-3 py-1 text-sm data-[state=active]:bg-surface data-[state=active]:font-medium data-[state=active]:shadow-soft hover:text-fg";

/** [Pictures | Video]; `onSelect` is told the tab picked. */
export function KindTabs(props: { onSelect: (tab: StudioTab) => void }) {
  const { onSelect } = props;
  return (
    <Tabs.List aria-label={STUDIO_BAR.tabs} className="flex gap-0.5 rounded-lg bg-bg-muted p-0.5 text-fg-muted">
      {TABS.map((t) => (
        // A click selects too: assistive technology may send one without a mouse press.
        <Tabs.Trigger key={t} value={t} className={TAB_CLASS} onClick={() => onSelect(t)}>
          {STUDIO_TABS[t]}
        </Tabs.Trigger>
      ))}
    </Tabs.List>
  );
}
