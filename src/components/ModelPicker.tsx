import { useEffect, useRef } from "react";
import { DropdownMenu } from "radix-ui";
import { Boxes, ChevronDown, FolderOpen, HardDrive, Globe, Settings2 } from "lucide-react";
import { refreshModelsAfterDownloads, useApp } from "../store/app";
import type { ModelOption } from "../lib/types";
import { visiblePath } from "./FolderChip";
import { openModelSettings } from "./ModelSettings";

function labelFor(ref: string | null, options: { ref: string; label: string }[]): string {
  if (!ref) return "Choose a model";
  const found = options.find((o) => o.ref === ref);
  if (found) return found.label;
  if (ref.startsWith("local:")) return ref.slice(6).split(/[\\/]/).pop() ?? ref;
  return ref;
}

/** [0.0.6 item 8] How a local entry reads: its file name, and the end of its folder when
 *  another file in the list has the same name, each with invisible characters as codes. */
function entryWords(o: ModelOption): string {
  return o.hint ? `${visiblePath(o.label)} · ${visiblePath(o.hint)}` : visiblePath(o.label);
}

export function ModelPicker() {
  const { detail, modelOptions, refreshModelOptions, pickModel, openModelFile } = useApp();
  const current = detail?.chat.modelRef ?? null;
  const openFile = openModelFile;
  const choose = pickModel;

  // The settings panel opens as the menu closes; the menu must not take the focus back.
  const openingSettings = useRef(false);

  // [0.0.6 item 8] The list names this chat's own model file the way the chat does (a link, a
  // mapped drive), so it is read again when the chat's model changes (another chat, a pick).
  const listedFor = useRef<string | null>(null);
  useEffect(() => {
    if (listedFor.current === current) return;
    listedFor.current = current;
    refreshModelOptions().catch(() => undefined);
  }, [current, refreshModelOptions]);

  // [0.0.6 item 8] A finished download shows up in the list without the menu being reopened.
  useEffect(() => {
    let alive = true;
    let stop: (() => void) | undefined;
    void refreshModelsAfterDownloads().then((u) => {
      if (alive) stop = u;
      else u();
    });
    return () => {
      alive = false;
      stop?.();
    };
  }, []);

  const local = modelOptions.filter((o) => o.source === "local");
  const remote = modelOptions.filter((o) => o.source === "provider");
  const shownName = visiblePath(labelFor(current, modelOptions));

  return (
    <DropdownMenu.Root onOpenChange={(o) => o && void refreshModelOptions()}>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          className={
            // [0.0.6 header-layout] Gives way to the ready line beside it only down to 12
            // characters of the name and the ellipsis (9.75rem with the padding, the gap and the
            // arrow: about 7rem for the name); the header moves its buttons to a second row before
            // that. The name ends in an ellipsis on its one line; it breaks anywhere, so the
            // header can count the name at that width and no more.
            "flex max-w-xs min-w-[9.75rem] items-center gap-2 rounded-lg border px-2.5 py-1.5 text-sm font-medium hover:bg-bg-muted " +
            (current ? "border-transparent" : "border-border bg-surface shadow-soft")
          }
          aria-label="Model for this chat"
          title={shownName}
        >
          <span data-model-name className="line-clamp-1 break-all">
            {shownName}
          </span>
          <ChevronDown size={14} className="shrink-0 text-fg-muted" aria-hidden />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={4}
          collisionPadding={8}
          // [0.0.6 item 8] Every installed model is listed, so the menu can be taller than the
          // window: it stops at the window's edge and scrolls.
          className="z-50 max-h-[var(--radix-dropdown-menu-content-available-height)] min-w-64 overflow-y-auto rounded-xl border border-border bg-surface p-1.5 shadow-pop"
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
                title={visiblePath(o.ref.slice("local:".length))}
                className="flex min-w-0 flex-1 cursor-default items-center gap-2 rounded-lg px-2.5 py-1.5 text-sm outline-none data-[highlighted]:bg-bg-muted"
              >
                <HardDrive size={14} aria-hidden /> <span className="truncate">{visiblePath(o.label)}</span>
                {o.hint && (
                  <>
                    {" "}
                    <span className="max-w-[50%] shrink-0 truncate text-xs text-fg-muted">· {visiblePath(o.hint)}</span>
                  </>
                )}
              </DropdownMenu.Item>
              <DropdownMenu.Item
                onSelect={() => {
                  openingSettings.current = true;
                  openModelSettings(o.ref.slice("local:".length));
                }}
                aria-label={`Settings for this model: ${entryWords(o)}`}
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
              <Globe size={14} aria-hidden /> <span className="truncate">{visiblePath(o.label)}</span>
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
