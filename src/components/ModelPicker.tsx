import { DropdownMenu } from "radix-ui";
import { ChevronDown, FolderOpen, HardDrive, Globe } from "lucide-react";
import { open } from "@tauri-apps/plugin-dialog";
import { useApp, toErrorInfo } from "../store/app";
import * as ipc from "../lib/ipc";

function labelFor(ref: string | null, options: { ref: string; label: string }[]): string {
  if (!ref) return "Choose a model";
  const found = options.find((o) => o.ref === ref);
  if (found) return found.label;
  if (ref.startsWith("local:")) return ref.slice(6).split(/[\\/]/).pop() ?? ref;
  return ref;
}

export function ModelPicker() {
  const { detail, modelOptions, setChatModel, refreshModelOptions, showError } = useApp();
  const current = detail?.chat.modelRef ?? null;

  async function openFile() {
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        filters: [{ name: "GGUF model", extensions: ["gguf"] }],
      });
      if (typeof picked !== "string") return;
      const ref = `local:${picked}`;
      const settings = await ipc.getLocalModelSettings();
      await setChatModel(ref);
      await ipc.loadLocalModel(picked, settings);
      await refreshModelOptions();
    } catch (err) {
      showError(toErrorInfo(err));
    }
  }

  async function choose(ref: string) {
    try {
      await setChatModel(ref);
      if (ref.startsWith("local:")) {
        const settings = await ipc.getLocalModelSettings();
        await ipc.loadLocalModel(ref.slice(6), settings);
      }
    } catch (err) {
      showError(toErrorInfo(err));
    }
  }

  const local = modelOptions.filter((o) => o.source === "local");
  const remote = modelOptions.filter((o) => o.source === "provider");

  return (
    <DropdownMenu.Root onOpenChange={(o) => o && void refreshModelOptions()}>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          className="flex max-w-xs items-center gap-1 rounded-md px-2 py-1 text-sm hover:bg-bg-muted"
          aria-label="Model for this chat"
        >
          <span className="truncate">{labelFor(current, modelOptions)}</span>
          <ChevronDown size={14} aria-hidden />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={4}
          className="z-50 min-w-64 rounded-md border border-border bg-bg p-1 shadow-lg"
        >
          {local.length > 0 && (
            <DropdownMenu.Label className="px-2 py-1 text-xs text-fg-muted">On this computer</DropdownMenu.Label>
          )}
          {local.map((o) => (
            <DropdownMenu.Item
              key={o.ref}
              onSelect={() => void choose(o.ref)}
              className="flex cursor-default items-center gap-2 rounded px-2 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
            >
              <HardDrive size={14} aria-hidden /> <span className="truncate">{o.label}</span>
            </DropdownMenu.Item>
          ))}
          {remote.length > 0 && (
            <DropdownMenu.Label className="px-2 py-1 text-xs text-fg-muted">Endpoints</DropdownMenu.Label>
          )}
          {remote.map((o) => (
            <DropdownMenu.Item
              key={o.ref}
              onSelect={() => void choose(o.ref)}
              className="flex cursor-default items-center gap-2 rounded px-2 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
            >
              <Globe size={14} aria-hidden /> <span className="truncate">{o.label}</span>
            </DropdownMenu.Item>
          ))}
          <DropdownMenu.Separator className="my-1 h-px bg-border" />
          <DropdownMenu.Item
            onSelect={() => void openFile()}
            className="flex cursor-default items-center gap-2 rounded px-2 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
          >
            <FolderOpen size={14} aria-hidden /> Open a GGUF file…
          </DropdownMenu.Item>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
