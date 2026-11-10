import { useEffect, useState } from "react";
import { MessageSquarePlus, Settings } from "lucide-react";
import { APP_NAME } from "./lib/app";
import { SettingsDialog } from "./components/SettingsDialog";

export default function App() {
  const [settingsOpen, setSettingsOpen] = useState(false);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key === ",") {
        event.preventDefault();
        setSettingsOpen(true);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div className="flex h-full">
      <aside
        className="flex w-64 shrink-0 flex-col border-r border-border bg-bg-subtle"
        aria-label="Chats"
      >
        <div className="flex items-center justify-between px-3 py-3">
          <span className="font-semibold">{APP_NAME}</span>
          <button
            type="button"
            className="rounded-md p-1.5 text-fg-muted hover:bg-bg-muted hover:text-fg"
            aria-label="Settings"
            title="Settings (Ctrl+,)"
            onClick={() => setSettingsOpen(true)}
          >
            <Settings size={16} aria-hidden />
          </button>
        </div>
        <div className="px-3">
          <button
            type="button"
            disabled
            className="flex w-full items-center gap-2 rounded-md border border-border px-3 py-2 text-left text-fg-muted disabled:cursor-not-allowed"
            title="Chat is not available in this build yet"
          >
            <MessageSquarePlus size={16} aria-hidden />
            New chat
          </button>
        </div>
        <p className="px-3 py-4 text-xs text-fg-muted">No chats yet.</p>
      </aside>
      <main className="flex flex-1 items-center justify-center p-8">
        <div className="max-w-md text-center">
          <h1 className="mb-2 text-xl font-semibold">{APP_NAME}</h1>
          <p className="text-fg-muted">
            This build contains the app shell only. Loading a model and chatting are not
            available yet.
          </p>
        </div>
      </main>
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
    </div>
  );
}
