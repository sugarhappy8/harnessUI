// The Artifacts panel's commands (src-tauri/src/artifacts.rs), as typed calls.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ArtifactChanged, ArtifactDetail, ArtifactDto, Isolation, Problem } from "./types";

export const listArtifacts = (chatId: number) => invoke<ArtifactDto[]>("list_artifacts", { chatId });

/** One artifact with the content of `version` (the newest when null). */
export const getArtifact = (chatId: number, id: number, version: number | null) =>
  invoke<ArtifactDetail>("get_artifact", { chatId, id, version });

/** Saves what the user wrote in the panel as a new version. */
export const saveArtifactContent = (chatId: number, id: number, content: string) =>
  invoke<ArtifactDto>("save_artifact_version", { chatId, id, content, restore: null });

/** Saves the content of an earlier version as a new version. */
export const restoreArtifactVersion = (chatId: number, id: number, version: number) =>
  invoke<ArtifactDto>("save_artifact_version", { chatId, id, content: null, restore: version });

export const renameArtifact = (chatId: number, id: number, title: string) =>
  invoke<ArtifactDto>("rename_artifact", { chatId, id, title });

export const deleteArtifact = (chatId: number, id: number) => invoke<void>("delete_artifact", { chatId, id });

/** Writes one version to the file chosen in the save dialog; resolves to where it was written. */
export const exportArtifact = (chatId: number, id: number, version: number, path: string) =>
  invoke<string>("export_artifact", { chatId, id, version, path });

/**
 * Tells the tool that waits for it how a version's preview went: the problems the frame
 * posted, or why it could not be checked. Resolves to whether a tool was waiting.
 */
export const reportRender = (
  id: number,
  version: number,
  problems: Problem[],
  notShown: "scripts-off" | "cannot-draw" | null,
) => invoke<boolean>("report_artifact_render", { id, version, problems, notShown });

export const artifactIsolation = () => invoke<Isolation>("artifact_isolation");
export const beginArtifactCheck = () => invoke<{ nonce: string; url: string }>("begin_artifact_check");
export const finishArtifactCheck = (nonce: string, report: unknown) =>
  invoke<Isolation>("finish_artifact_check", { nonce, report });

export function onArtifactChanged(handler: (c: ArtifactChanged) => void): Promise<UnlistenFn> {
  return listen<ArtifactChanged>("artifact-changed", (e) => handler(e.payload));
}

export function onArtifactIsolation(handler: (i: Isolation) => void): Promise<UnlistenFn> {
  return listen<Isolation>("artifact-isolation", (e) => handler(e.payload));
}
