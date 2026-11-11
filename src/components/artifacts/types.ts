// What the Artifacts panel exchanges with Rust (src-tauri/src/artifacts.rs).

export type ArtifactKind = "markdown" | "html" | "svg" | "mermaid";
export type ArtifactAuthor = "model" | "user";

export interface ArtifactDto {
  id: number;
  chatId: number;
  title: string;
  kind: ArtifactKind;
  /** Versions kept. */
  versions: number;
  /** The number of the newest version. */
  latestVersion: number;
  createdAt: number;
  updatedAt: number;
}

export interface VersionDto {
  version: number;
  author: ArtifactAuthor;
  note: string | null;
  createdAt: number;
  bytes: number;
}

/** One artifact with its versions and the content of one of them. */
export interface ArtifactDetail {
  artifact: ArtifactDto;
  /** The kept versions, oldest first. */
  versions: VersionDto[];
  /** The version whose content this is. */
  version: number;
  content: string;
  /** What the preview frame loads; null for a document, which the app shows itself. */
  frameUrl: string | null;
}

/** Sent by Rust when a tool created an artifact or saved a new version of one. */
export interface ArtifactChanged {
  chatId: number;
  id: number;
  version: number;
  kind: ArtifactKind;
  title: string;
  /** The tool is waiting for this window to show the version and report its problems. */
  wantsReport: boolean;
}

/** Whether previews may run scripts on this system (src-tauri/src/artifacts/isolation.rs). */
export type Isolation =
  | { state: "unchecked" }
  | { state: "checking" }
  | { state: "proven" }
  | { state: "broken"; reason: string };

/** One thing that went wrong in a preview. Every field comes from a page the model wrote. */
export interface Problem {
  type: ProblemType;
  message: string;
  line: number | null;
  col: number | null;
}

export type ProblemType = "error" | "rejection" | "console" | "blocked" | "diagram" | "svg";

/** The file extension an artifact of each kind is saved with. */
export const EXTENSION: Record<ArtifactKind, string> = {
  markdown: "md",
  html: "html",
  svg: "svg",
  mermaid: "mmd",
};

export const KIND_LABEL: Record<ArtifactKind, string> = {
  markdown: "Document",
  html: "Page",
  svg: "Picture",
  mermaid: "Diagram",
};
