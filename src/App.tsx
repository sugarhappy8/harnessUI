import { useEffect, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { SettingsDialog } from "./components/SettingsDialog";
import { ModelsDialog } from "./components/ModelsDialog";
import { FirstRun } from "./components/FirstRun";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { AppNotices } from "./components/AppNotices";
import { useApp, toErrorInfo } from "./store/app";
import * as ipc from "./lib/ipc";

export default function App() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<string | undefined>(undefined);
  const [modelsOpen, setModelsOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  // null while loading; true once first run was completed or skipped.
  const [firstRunDone, setFirstRunDone] = useState<boolean | null>(ipc.isTauri() ? null : true);
  const { refreshChats, refreshModelOptions, setRuntime, newChat, stop, showError, addApproval, removeApproval } =
    useApp();

  // Approval cards: Rust sends each request only to this window with a random id.
  useEffect(() => {
    if (!ipc.isTauri()) return;
    const offs: (() => void)[] = [];
    void ipc.onApprovalRequest(addApproval).then((u) => offs.push(u));
    void ipc.onApprovalClosed(removeApproval).then((u) => offs.push(u));
    return () => offs.forEach((f) => f());
  }, [addApproval, removeApproval]);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    let unlisten: (() => void) | undefined;
    void ipc.onRuntimeStatus(setRuntime).then((u) => (unlisten = u));
    Promise.all([refreshChats(), refreshModelOptions(), ipc.runtimeStatus().then(setRuntime)]).catch(
      (err) => showError(toErrorInfo(err)),
    );
    ipc
      .firstRunInfo()
      .then((i) => setFirstRunDone(i.done))
      .catch(() => setFirstRunDone(true));
    return () => unlisten?.();
  }, [refreshChats, refreshModelOptions, setRuntime, showError]);

  useEffect(() => {
    const onOpenModels = () => setModelsOpen(true);
    window.addEventListener("app:open-models", onOpenModels);
    return () => window.removeEventListener("app:open-models", onOpenModels);
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      const mod = event.metaKey || event.ctrlKey;
      if (mod && event.key === ",") {
        event.preventDefault();
        setSettingsTab(undefined);
        setSettingsOpen(true);
      } else if (mod && event.key.toLowerCase() === "b") {
        event.preventDefault();
        setSidebarOpen((o) => !o);
      } else if (mod && event.key === "/") {
        event.preventDefault();
        setShortcutsOpen(true);
      } else if (mod && event.key.toLowerCase() === "m") {
        event.preventDefault();
        setModelsOpen(true);
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

  const dialogs = (
    <>
      <SettingsDialog open={settingsOpen} onOpenChange={setSettingsOpen} initialTab={settingsTab} />
      <ModelsDialog open={modelsOpen} onOpenChange={setModelsOpen} />
      <ShortcutsDialog open={shortcutsOpen} onOpenChange={setShortcutsOpen} />
    </>
  );
  const notices = (
    <AppNotices
      onOpenSettings={(tab) => {
        setSettingsTab(tab);
        setSettingsOpen(true);
      }}
    />
  );

  if (firstRunDone === null) return null;
  if (!firstRunDone) {
    return (
      <div className="flex h-full flex-col">
        {notices}
        <div className="min-h-0 flex-1">
          <FirstRun
            onDone={() => {
              setFirstRunDone(true);
              void refreshModelOptions();
            }}
            onOpenEndpoints={() => {
              setSettingsTab("endpoints");
              setSettingsOpen(true);
            }}
            onOpenModels={() => setModelsOpen(true)}
          />
        </div>
        {dialogs}
      </div>
    );
  }

  return (
    <div className="flex h-full flex-col">
      {notices}
      <div className="flex min-h-0 flex-1">
        {sidebarOpen && (
          <Sidebar
            onOpenSettings={() => {
              setSettingsTab(undefined);
              setSettingsOpen(true);
            }}
            onOpenModels={() => setModelsOpen(true)}
          />
        )}
        <ChatView />
      </div>
      {dialogs}
    </div>
  );
}
