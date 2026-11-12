import { useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { Boxes, ChevronDown, FolderOpen, HardDrive, Globe, Settings2 } from "lucide-react";
import { useApp } from "../store/app";
import { openModelSettings } from "./ModelSettings";

function labelFor(ref: string | null, options: { ref: string; label: string }[]): string {
  if (!ref) return "Choose a model";
  const found = options.find((o) => o.ref === ref);
  if (found) return found.label;
  if (ref.startsWith("local:")) return ref.slice(6).split(/[\\/]/).pop() ?? ref;
  return ref;
}

export function ModelPicker() {
  const { detail, modelOptions, refreshModelOptions, pickModel, openModelFile } = useApp();
  const current = detail?.chat.modelRef ?? null;
  const openFile = openModelFile;
  const choose = pickModel;

  // The settings panel opens as the menu closes; the menu must not take the focus back.
  const openingSettings = useRef(false);

  const local = modelOptions.filter((o) => o.source === "local");
  const remote = modelOptions.filter((o) => o.source === "provider");

  return (
    <DropdownMenu.Root onOpenChange={(o) => o && void refreshModelOptions()}>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          className={
            "flex max-w-xs min-w-0 items-center gap-2 rounded-lg border px-2.5 py-1.5 text-sm font-medium hover:bg-bg-muted " +
            (current ? "border-transparent" : "border-border bg-surface shadow-soft")
          }
          aria-label="Model for this chat"
        >
          <span className="truncate">{labelFor(current, modelOptions)}</span>
          <ChevronDown size={14} className="shrink-0 text-fg-muted" aria-hidden />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={4}
          className="z-50 min-w-64 rounded-xl border border-border bg-surface p-1.5 shadow-pop"
          onCloseAutoFocus={(e) => {
            if (openingSettings.current) {
              openingSettings.current = false;
              e.preventDefault();
            }
          }}
        >
          {local.length > 0 && (
            <DropdownMenu.Label className="px-2 py-1 text-xs text-fg-muted">On this computer</DropdownMenu.Label>
          )}
          {local.map((o) => (
            <div key={o.ref} className="flex items-center gap-0.5">
              <DropdownMenu.Item
                onSelect={() => void choose(o.ref)}
                className="flex min-w-0 flex-1 cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
              >
                <HardDrive size={14} aria-hidden /> <span className="truncate">{o.label}</span>
              </DropdownMenu.Item>
              <DropdownMenu.Item
                onSelect={() => {
                  openingSettings.current = true;
                  openModelSettings(o.ref.slice("local:".length));
                }}
                aria-label={`Settings for this model: ${o.label}`}
                title="Settings for this model"
                className="shrink-0 cursor-default rounded-lg p-1.5 text-fg-muted outline-none data-[highlighted]:bg-bg-muted data-[highlighted]:text-fg"
              >
                <Settings2 size={14} aria-hidden />
              </DropdownMenu.Item>
            </div>
          ))}
          {remote.length > 0 && (
            <DropdownMenu.Label className="px-2 py-1 text-xs text-fg-muted">Endpoints</DropdownMenu.Label>
          )}
          {remote.map((o) => (
            <DropdownMenu.Item
              key={o.ref}
              onSelect={() => void choose(o.ref)}
              className="flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
            >
              <Globe size={14} aria-hidden /> <span className="truncate">{o.label}</span>
            </DropdownMenu.Item>
          ))}
          <DropdownMenu.Separator className="my-1 h-px bg-border" />
          <DropdownMenu.Item
            onSelect={() => void openFile()}
            className="flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
          >
            <FolderOpen size={14} aria-hidden /> Open a GGUF file…
          </DropdownMenu.Item>
          <DropdownMenu.Item
            onSelect={() => window.dispatchEvent(new Event("app:open-models"))}
            className="flex cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
          >
            <Boxes size={14} aria-hidden /> Find and manage models…
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
