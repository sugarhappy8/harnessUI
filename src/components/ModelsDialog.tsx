import { useState } from "react";
import { Dialog, Tabs } from "radix-ui";
import { X } from "lucide-react";
import { Browse } from "./models/Browse";
import { Compare, CompareTabLabel } from "./models/Compare";
import { Downloads } from "./models/Downloads";
import { Installed } from "./models/Installed";

// The Models dialog: the shell and its tabs. Each tab is its own file in ./models/ (0.0.5:
// Installed is WP2's, Browse and RepoDetail are WP4b's; a new tab adds one trigger and one
// content line here).

const tabClass =
  "rounded-md px-3 py-1.5 text-sm data-[state=active]:bg-bg-muted data-[state=active]:font-medium hover:bg-bg-muted";

export function ModelsDialog(props: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [tab, setTab] = useState("installed");

  return (
    <Dialog.Root open={props.open} onOpenChange={props.onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialog-overlay fixed inset-0" />
        <Dialog.Content className="fixed top-1/2 left-1/2 flex h-[38rem] w-[52rem] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 flex-col rounded-2xl border border-border bg-surface shadow-pop">
          <div className="flex items-center justify-between border-b border-border px-5 py-3">
            <Dialog.Title className="text-base font-semibold">Models</Dialog.Title>
            <Dialog.Close className="rounded-md p-1 text-fg-muted hover:bg-bg-muted hover:text-fg" aria-label="Close models">
              <X size={16} aria-hidden />
            </Dialog.Close>
          </div>
          <Dialog.Description className="sr-only">Installed models, model search and downloads</Dialog.Description>
          <Tabs.Root value={tab} onValueChange={setTab} className="flex min-h-0 flex-1 flex-col">
            <Tabs.List className="flex gap-1 border-b border-border px-4 py-2" aria-label="Model sections">
              <Tabs.Trigger value="installed" className={tabClass}>Installed</Tabs.Trigger>
              <Tabs.Trigger value="browse" className={tabClass}>Browse</Tabs.Trigger>
              <Tabs.Trigger value="compare" className={tabClass}><CompareTabLabel /></Tabs.Trigger>
              <Tabs.Trigger value="downloads" className={tabClass}>Downloads</Tabs.Trigger>
            </Tabs.List>
            <div className="min-h-0 flex-1 overflow-y-auto p-5">
              <Tabs.Content value="installed"><Installed onClose={() => props.onOpenChange(false)} /></Tabs.Content>
              <Tabs.Content value="browse"><Browse onStarted={() => setTab("downloads")} /></Tabs.Content>
              <Tabs.Content value="compare"><Compare onOpen={setTab} /></Tabs.Content>
              <Tabs.Content value="downloads"><Downloads /></Tabs.Content>
            </div>
          </Tabs.Root>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
