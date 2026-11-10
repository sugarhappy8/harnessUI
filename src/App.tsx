import { useEffect, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { SettingsDialog } from "./components/SettingsDialog";
import { useApp, toErrorInfo } from "./store/app";
import * as ipc from "./lib/ipc";

export default function App() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const { refreshChats, refreshModelOptions, setRuntime, newChat, stop, showError } = useApp();

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let unlisten: (() => void) | undefined;
    void ipc.onRuntimeStatus(setRuntime).then((u) => (unlisten = u));
    Promise.all([refreshChats(), refreshModelOptions(), ipc.runtimeStatus().then(setRuntime)]).catch(
      (err) => showError(toErrorInfo(err)),
    );
    return () => unlisten?.();
  }, [refreshChats, refreshModelOptions, setRuntime, showError]);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key === ",") {
        event.preventDefault();
        setSettingsOpen(true);
      } else if (mod && event.key.toLowerCase() === "n") {
        event.preventDefault();
        void newChat();
      } else if (mod && event.key.toLowerCase() === "k") {
        event.preventDefault();
        window.dispatchEvent(new Event("app:focus-search"));
      } else if (mod && event.key.toLowerCase() === "l") {
        event.preventDefault();
        window.dispatchEvent(new Event("app:focus-input"));
      } else if (event.key === "Escape" && useApp.getState().generation) {
        void stop();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [newChat, stop]);

  return (
    <div className="flex h-full">
      <Sidebar onOpenSettings={() => setSettingsOpen(true)} />
      <ChatView />
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} />
    </div>
  );
}
