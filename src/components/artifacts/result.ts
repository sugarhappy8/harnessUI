import type { ArtifactKind } from "./types";

/** What the first line of a `create_artifact` or `update_artifact` result says. */
export interface SavedArtifact {
  id: number;
  title: string;
  kind: ArtifactKind;
  version: number;
}

// The line is written by src-tauri/src/artifacts/server.rs (`saved_line`). A title may
// contain quotes and brackets, so the pattern is anchored at both ends and the title takes
// whatever is between the first and the last quote.
const SAVED = /^Artifact #(\d{1,15}) "(.*)" \((markdown|html|svg|mermaid)\), version (\d{1,9}) saved\.$/;

/**
 * Reads the artifact a tool result is about from its first line. Null for anything else:
 * an error, a result that is still being written, text that only looks similar.
 */
export function parseSavedLine(text: string): SavedArtifact | null {
  const first = text.split("\n", 1)[0] ?? "";
  const found = SAVED.exec(first.trimEnd());
  if (!found) return null;
  const id = Number(found[1]);
  const version = Number(found[4]);
  if (!Number.isSafeInteger(id) || id < 1 || !Number.isSafeInteger(version) || version < 1) return null;
  return { id, title: found[2] ?? "", kind: found[3] as ArtifactKind, version };
}
