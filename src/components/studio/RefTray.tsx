import { useCallback, useEffect, useEffectEvent, useRef, useState, type DragEvent, type RefObject } from "react";
import { DropdownMenu } from "radix-ui";
import { Check, ChevronDown, Image as ImageIcon, ImagePlus, X } from "lucide-react";
import * as ipc from "../../lib/ipc";
import { STUDIO_COMPOSER, STUDIO_REFS } from "../../lib/words";
import { toErrorInfo } from "../../store/app";
import { draftModel, pictureRoles, placeRef, useStudio, type StudioTab } from "../../store/studio";
import { readImage } from "../Composer";
import { FOCUS_RING } from "../ui/styles";
import * as api from "./api";
import { PICTURE_TILE } from "./styles";
import type { RefItem, RefRole } from "./types";

// Added pictures (0.1.0 Task B5; design 3, "The tray"; the composer's strip since 0.2.0 Task U3):
// the tray in front of the prompt takes a picture dropped on it, pasted anywhere in Studio, or
// chosen as a file; each is read as the chat composer reads one (`readImage`), kept in the
// gallery folder by the backend (`studio_add_ref`) and placed in the first of the model's
// picture roles with room. The strip shows the reference pictures as thumbnails, each with its
// Remove button and, when the model takes more than one role, its role chip; a picture of any
// other role shows in its own zone (InputZones.tsx, which shares the adding and dropping here).
// A picture past the model's limit is refused before it is kept. A picture dropped from the
// file manager comes through the webview's own drop where it carries files (macOS, Linux) and
// through Tauri's native drop on Windows, where the window keeps Tauri's drag-and-drop and the
// webview's drop has no files (`useNativePictureDrops`, which hands the tray or input zone under
// the drop the number of files dropped: the backend keeps them from the paths it recorded,
// `studio_add_dropped`).

/** The picture types the file chooser offers (`readImage` checks each file again). */
export const PICTURE_TYPES = "image/png,image/jpeg,image/gif,image/webp";

/** Why a picture was not added, and what to do about it when there is a next step. */
interface Refusal {
  message: string;
  nextStep: string;
}

/** Why adding failed: `readImage`'s own words, or the backend's with its next step. */
function failureWords(e: unknown): Refusal {
  if (e instanceof Error) return { message: e.message, nextStep: "" };
  const { message, nextStep } = toErrorInfo(e);
  return { message, nextStep };
}

/** A refusal without a next step (the model's limit, a role with no room). */
const refusal = (message: string): Refusal => ({ message, nextStep: "" });

/** `error`'s words, with its next step in a quieter colour. */
export function RefusalWords(props: { error: Refusal }) {
  return (
    <>
      {props.error.message}
      {props.error.nextStep && <span className="text-fg-muted"> {props.error.nextStep}</span>}
    </>
  );
}

/** A picture to add, kept by the backend once it is asked for (after the model's limit was
 *  checked): a file read here, or a file of the native drop. */
type Source = () => Promise<RefItem>;

/** `files` as pictures to add: each read as the composer reads one, then kept. */
export function fromFiles(files: File[]): Source[] {
  return files.map((file) => async () => api.studioAddRef(await readImage(file)));
}

/** The first `count` files of the native drop just made, as pictures to add. */
export function fromDrop(count: number): Source[] {
  return Array.from({ length: count }, (_, index) => () => api.studioAddDropped(index, null));
}

/**
 * Adds `sources` to `tab`'s draft one at a time, as `role` (or, without one, as the first role
 * of the model's picture fields with room). Each is checked against the model's limit before it
 * is read and kept, so a picture refused is never sent to the backend; with `replace`, a
 * picture of a full role takes the place of the one there. Stops at the first picture not
 * added and returns why, else `null`.
 */
async function addPictures(tab: StudioTab, sources: Source[], role: RefRole | null, replace: boolean): Promise<Refusal | null> {
  /** The draft's pictures that count against the limit: with `replace`, not those of `role`. */
  const counted = (refs: RefItem[]) => (replace ? refs.filter((r) => (r.role ?? "reference") !== role) : refs);
  for (const source of sources) {
    const before = useStudio.getState();
    const placed = placeRef(draftModel(before, tab), counted(before.drafts[tab].refs), role);
    if ("refused" in placed) return refusal(placed.refused);
    let kept: RefItem;
    try {
      kept = await source();
    } catch (e) {
      return failureWords(e);
    }
    const now = useStudio.getState();
    if (replace && "refused" in placeRef(draftModel(now, tab), now.drafts[tab].refs, role)) {
      const there = now.drafts[tab].refs.findIndex((r) => (r.role ?? "reference") === role);
      if (there >= 0) now.removeRef(tab, there);
    }
    const why = useStudio.getState().addRef(tab, { ...kept, role });
    if (why) return refusal(why);
  }
  return null;
}

/** Adding pictures to `tab`'s draft: `add`, and why the last attempt stopped, if it did. */
export function useAdding(tab: StudioTab) {
  const [error, setError] = useState<Refusal | null>(null);
  const add = useCallback(
    (sources: Source[], role: RefRole | null, replace = false) => {
      if (sources.length === 0) return;
      setError(null);
      void addPictures(tab, sources, role, replace).then(setError);
    },
    [tab],
  );
  return { add, error, setError };
}

/** When the webview's own drop last carried files (ms since the epoch): on macOS and Linux
 *  Tauri's native drop comes with it, and must not add the same pictures again. */
let htmlDropAt = 0;

/** The handlers that take files dropped on an element (the webview's own drop). */
export function dropTarget(take: (files: File[]) => void) {
  return {
    onDragOver: (e: DragEvent) => {
      if (e.dataTransfer.types.includes("Files")) e.preventDefault();
    },
    onDrop: (e: DragEvent) => {
      const files = [...e.dataTransfer.files];
      if (files.length === 0) return;
      e.preventDefault();
      htmlDropAt = Date.now();
      take(files);
    },
  };
}

/** The event a tray or input zone gets for a native drop on it: `detail` is how many files. */
const NATIVE_DROP = "studio:native-drop";

/** How long a native drop waits for the webview's own drop of the same files, and how close
 *  together the two count as one drop. */
export const NATIVE_DROP_SETTLE_MS = 300;
const SAME_DROP_MS = 1500;

/** Calls `take` with the number of files of each native drop on `zone`. */
export function useNativeDropOn(zone: RefObject<HTMLElement | null>, take: (count: number) => void) {
  const onDrop = useEffectEvent((count: number) => take(count));
  useEffect(() => {
    const el = zone.current;
    if (!el) return;
    const listener = (e: Event) => onDrop((e as CustomEvent<number>).detail);
    el.addEventListener(NATIVE_DROP, listener);
    return () => el.removeEventListener(NATIVE_DROP, listener);
  });
}

/** The tray or input zone at `x`, `y` (CSS pixels): an input zone, the tray, or the tray of the
 *  composer dropped on (the prompt box takes a picture as the chat's message box does). */
function zoneAt(x: number, y: number): HTMLElement | null {
  const hit = document.elementFromPoint?.(x, y)?.closest<HTMLElement>("[data-input-zone], [data-ref-tray], [data-composer]");
  if (!hit || hit.matches("[data-input-zone], [data-ref-tray]")) return hit ?? null;
  return hit.querySelector<HTMLElement>("[data-ref-tray]");
}

/**
 * While `active` (Studio is shown): each native drop of files on the tray, an input zone or the
 * composer goes to that zone, which adds the files as it adds a picture dropped on it. A
 * moment later, so a drop the webview's own drop already took (macOS, Linux) is not added
 * twice. Outside the app it does nothing.
 */
export function useNativePictureDrops(active: boolean) {
  useEffect(() => {
    if (!active || !ipc.isTauri()) return;
    let alive = true;
    const onDrop = (count: number, at: { x: number; y: number }) => {
      const zone = zoneAt(at.x, at.y);
      if (!zone || count === 0) return;
      const dropped = Date.now();
      setTimeout(() => {
        if (!alive || htmlDropAt > dropped - SAME_DROP_MS) return;
        zone.dispatchEvent(new CustomEvent(NATIVE_DROP, { detail: count }));
      }, NATIVE_DROP_SETTLE_MS);
    };
    const unlisten = Promise.resolve()
      .then(() => api.onNativeDrop(onDrop))
      .catch((e: unknown) => {
        console.warn("Studio: dropped pictures cannot be followed", e);
        return undefined;
      });
    return () => {
      alive = false;
      void unlisten.then((u) => u?.());
    };
  }, [active]);
}

/** An added picture from its bytes in the gallery folder, as a `blob:` URL let go with it; a
 *  picture icon until then (and if its bytes cannot be read). A video's tile shows its start
 *  frame this way until its poster frame is saved (Task B9). */
export function RefThumb(props: { sha256: string; mime: string; alt: string; className: string }) {
  const { sha256, mime } = props;
  const [src, setSrc] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    let url: string | null = null;
    Promise.resolve()
      .then(() => api.studioRefBytes(sha256))
      .then((bytes) => {
        if (!alive) return;
        url = URL.createObjectURL(new Blob([bytes], { type: mime }));
        setSrc(url);
      })
      // The icon stays: the picture is still added, and the run reads it from the folder.
      .catch(() => undefined);
    return () => {
      alive = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [sha256, mime]);
  if (!src) {
    // Named as the picture will be, so a screen reader meets it before its bytes are read.
    const named = props.alt ? { role: "img", "aria-label": props.alt } : { "aria-hidden": true };
    return (
      <span {...named} className={`flex items-center justify-center bg-bg-muted text-fg-muted ${props.className}`}>
        <ImageIcon size={16} aria-hidden />
      </span>
    );
  }
  return <img src={src} alt={props.alt} className={`object-cover ${props.className}`} />;
}

/** What added picture `index` is for, and the menu that moves it to another role the model
 *  takes (a role with no room left is greyed out). */
function RoleChip(props: {
  tab: StudioTab;
  index: number;
  item: RefItem;
  roles: { role: RefRole; max: number }[];
  refs: RefItem[];
  onRefused(why: string): void;
}) {
  const setRefRole = useStudio((s) => s.setRefRole);
  const role = props.item.role ?? "reference";
  const count = (r: RefRole) => props.refs.filter((ref) => (ref.role ?? "reference") === r).length;
  return (
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button
          type="button"
          title={STUDIO_REFS.roleMenu}
          className={`flex items-center gap-0.5 rounded-full border border-border px-1.5 text-xs hover:border-border-strong ${FOCUS_RING}`}
        >
          {STUDIO_REFS.role[role]}
          <ChevronDown size={12} aria-hidden />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          align="start"
          sideOffset={4}
          collisionPadding={8}
          aria-label={STUDIO_REFS.roleMenu}
          className="z-50 min-w-32 rounded-xl border border-border bg-surface p-1 text-sm shadow-pop"
        >
          <DropdownMenu.RadioGroup
            value={role}
            onValueChange={(to) => {
              const why = setRefRole(props.tab, props.index, to as RefRole);
              if (why) props.onRefused(why);
            }}
          >
            {props.roles.map((r) => (
              <DropdownMenu.RadioItem
                key={r.role}
                value={r.role}
                disabled={r.role !== role && count(r.role) >= r.max}
                className="flex cursor-default items-center gap-2 rounded-lg px-2 py-1 outline-none data-[disabled]:text-fg-muted data-[highlighted]:bg-bg-muted"
              >
                <span className="w-3.5">
                  <DropdownMenu.ItemIndicator>
                    <Check size={14} aria-hidden className="text-accent" />
                  </DropdownMenu.ItemIndicator>
                </span>
                {STUDIO_REFS.role[r.role]}
              </DropdownMenu.RadioItem>
            ))}
          </DropdownMenu.RadioGroup>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

/** A stable key for added picture `index`: its bytes' hash and which of the pictures with that
 *  hash it is, so removing one keeps every other thumbnail (and its bytes read) as it was. */
function refKey(refs: RefItem[], index: number): string {
  const sha256 = refs[index]?.sha256 ?? "";
  const same = refs.slice(0, index).filter((r) => r.sha256 === sha256).length;
  return `${sha256} ${same}`;
}

/** The reference pictures of `tab`'s draft as thumbnails, each with its Remove button (and its
 *  role chip while the model takes more than one role), then *Add picture* while the model
 *  takes reference pictures. Its children sit in the composer's row in front of the prompt. A
 *  picture pasted anywhere in Studio's view comes here, and so does one dropped on the composer
 *  (to the first of the model's roles with room). */
export function RefTray(props: { tab: StudioTab }) {
  const { tab } = props;
  const refs = useStudio((s) => s.drafts[tab].refs);
  const model = useStudio((s) => draftModel(s, tab));
  const removeRef = useStudio((s) => s.removeRef);
  const { add, error, setError } = useAdding(tab);
  const fileInput = useRef<HTMLInputElement>(null);
  const tray = useRef<HTMLDivElement>(null);
  const roles = pictureRoles(model);
  const takesReferences = roles.some((r) => r.role === "reference");
  // The strip's pictures, with where each is in the draft; the other roles have their zones.
  const strip = refs.flatMap((item, index) => ((item.role ?? "reference") === "reference" ? [{ item, index }] : []));
  useNativeDropOn(tray, (count) => add(fromDrop(count), null));

  // A paste goes to the focused element, so the tray listens on the document for one in
  // Studio's view (the prompt box, mostly) that carries pictures while the model takes any
  // (read as the paste comes); anything else is left alone, to go where it was pasted.
  useEffect(() => {
    const onPaste = (e: ClipboardEvent) => {
      if (!(e.target instanceof Element) || !e.target.closest("[data-studio-view]")) return;
      if (pictureRoles(draftModel(useStudio.getState(), tab)).length === 0) return;
      const files = [...(e.clipboardData?.files ?? [])].filter((f) => f.type.startsWith("image/"));
      if (files.length === 0) return;
      e.preventDefault();
      add(fromFiles(files), null);
    };
    document.addEventListener("paste", onPaste);
    return () => document.removeEventListener("paste", onPaste);
  }, [add, tab]);

  if (roles.length === 0 && strip.length === 0 && !error) return null;
  return (
    // `contents`: its tiles are items of the composer's row; it stays the drop target.
    <div ref={tray} data-ref-tray className="contents" {...dropTarget((files) => add(fromFiles(files), null))}>
      {strip.map(({ item, index }, n) => {
        const name = STUDIO_REFS.picture(n + 1);
        return (
          <div key={refKey(refs, index)} className={`${PICTURE_TILE} border-border bg-surface p-1 pr-1.5`}>
            <RefThumb sha256={item.sha256} mime={item.mime} alt={name} className="size-10 rounded-lg" />
            {roles.length > 1 && (
              <RoleChip tab={tab} index={index} item={item} roles={roles} refs={refs} onRefused={(why) => setError(refusal(why))} />
            )}
            <button
              type="button"
              aria-label={STUDIO_COMPOSER.remove(name)}
              onClick={() => removeRef(tab, index)}
              className={`rounded-full p-1 text-fg-muted hover:bg-bg-muted hover:text-fg ${FOCUS_RING}`}
            >
              <X size={14} aria-hidden />
            </button>
          </div>
        );
      })}
      {takesReferences && (
        <button
          type="button"
          onClick={() => fileInput.current?.click()}
          className={`${PICTURE_TILE} border-dashed border-border-strong px-3 text-fg-muted hover:border-accent hover:text-fg ${FOCUS_RING}`}
        >
          <ImagePlus size={16} aria-hidden />
          {STUDIO_COMPOSER.addPicture}
        </button>
      )}
      <input
        ref={fileInput}
        type="file"
        accept={PICTURE_TYPES}
        multiple
        hidden
        aria-label={STUDIO_REFS.choose}
        onChange={(e) => {
          if (e.target.files) add(fromFiles([...e.target.files]), null);
          e.target.value = "";
        }}
      />
      {error && (
        <p role="alert" className="basis-full text-xs text-danger">
          <RefusalWords error={error} />
        </p>
      )}
    </div>
  );
}
