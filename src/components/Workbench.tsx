import type { ComponentType } from "react";
import { BookOpen, Layers, ListChecks, PanelsTopLeft, X } from "lucide-react";
import { useApp, type WorkbenchTab } from "../store/app";
import { ArtifactsTab } from "./workbench/ArtifactsTab";
import { TaskTab } from "./workbench/TaskTab";
import { SourcesTab } from "./workbench/SourcesTab";
import { ContextTab } from "./workbench/ContextTab";

/** The tabs, in order. Each tab is one file in `workbench/`, owned by its feature. */
const TABS: { id: WorkbenchTab; label: string; icon: ComponentType<{ size?: number }>; view: ComponentType<{ chatId: number }> }[] = [
  { id: "artifacts", label: "Artifacts", icon: PanelsTopLeft, view: ArtifactsTab },
  { id: "task", label: "Task", icon: ListChecks, view: TaskTab },
  { id: "sources", label: "Sources", icon: BookOpen, view: SourcesTab },
  { id: "context", label: "Context", icon: Layers, view: ContextTab },
];

/** The panel beside the chat: what the model made, what it is working on, what it read
 *  and what it will be sent. Shown only while a chat is open. */
export function Workbench() {
  const workbench = useApp((s) => s.workbench);
  const chatId = useApp((s) => s.activeChatId);
  const openWorkbench = useApp((s) => s.openWorkbench);
  const closeWorkbench = useApp((s) => s.closeWorkbench);
  if (!workbench.open || chatId === null) return null;
  const current = TABS.find((t) => t.id === workbench.tab) ?? TABS[0]!;
  const View = current.view;
  return (
    <aside
      className="enter-fade flex w-[min(46vw,44rem)] min-w-80 shrink-0 flex-col border-l border-border bg-bg-subtle"
      aria-label="Workbench"
    >
      <div className="flex items-center gap-1 border-b border-border px-2 py-1.5" role="tablist" aria-label="Workbench tabs">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            role="tab"
            aria-selected={t.id === current.id}
            onClick={() => openWorkbench(t.id)}
            className={
              "flex items-center gap-1.5 rounded-lg px-2.5 py-1.5 text-sm whitespace-nowrap " +
              (t.id === current.id ? "bg-bg-muted font-medium text-fg" : "text-fg-muted hover:bg-bg-muted/70 hover:text-fg")
            }
          >
            <t.icon size={14} aria-hidden />
            {t.label}
          </button>
        ))}
        <div className="flex-1" />
        <button
          type="button"
          onClick={closeWorkbench}
          aria-label="Close the panel"
          title="Close the panel"
          className="rounded-lg p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
        >
          <X size={15} aria-hidden />
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto" role="tabpanel" aria-label={current.label}>
        <View key={chatId} chatId={chatId} />
      </div>
    </aside>
  );
}
