import type { ReactNode } from "react";
import { PanelsTopLeft } from "lucide-react";
import { useApp } from "../../store/app";
import type { ToolCallView } from "../ToolCallCard";
import { parseSavedLine, type SavedArtifact } from "./result";
import { useArtifacts } from "./state";
import { startArtifacts } from "./startup";
import { KIND_LABEL } from "./types";

// The tool cards are part of the app from its first render, so this is where the
// artifact events start being listened to (once, and only inside the app).
startArtifacts();

/** Under the card of `create_artifact` and `update_artifact`: what was saved, and a
 *  button that shows it in the panel beside the chat. */
function ArtifactCardExtra(props: { saved: SavedArtifact }) {
  const chatId = useApp((s) => s.activeChatId);
  const saved = props.saved;
  return (
    <div className="flex items-center gap-2">
      <span className="min-w-0 flex-1 truncate text-xs text-fg-muted">
        {KIND_LABEL[saved.kind]} · {saved.title} · version {saved.version}
      </span>
      <button
        type="button"
        disabled={chatId === null}
        onClick={() => {
          if (chatId === null) return;
          useArtifacts.getState().requestFocus(chatId, saved.id, saved.version);
          useApp.getState().openWorkbench("artifacts");
        }}
        className="flex shrink-0 items-center gap-1.5 rounded-lg border border-border px-2.5 py-1 text-xs font-medium hover:border-border-strong hover:bg-bg-muted"
      >
        <PanelsTopLeft size={13} aria-hidden />
        Open artifact
      </button>
    </div>
  );
}

/** For the registry in `toolcards.tsx`: nothing until the call has saved something. */
export function artifactCardExtra(call: ToolCallView): ReactNode {
  const saved = parseSavedLine(call.text);
  return saved ? <ArtifactCardExtra saved={saved} /> : null;
}
