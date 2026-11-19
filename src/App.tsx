import { useEffect, useState } from "react";
import { Sidebar } from "./components/Sidebar";
import { ChatView } from "./components/ChatView";
import { Workbench } from "./components/Workbench";
import { SettingsDialog } from "./components/SettingsDialog";
import { ModelsDialog } from "./components/ModelsDialog";
import { FirstRun } from "./components/FirstRun";
import { ShortcutsDialog } from "./components/ShortcutsDialog";
import { AppNotices } from "./components/AppNotices";
import type { UpdatePlacement } from "./components/UpdateBox";
import { FullAccessStartCheck } from "./components/AllowAllButton";
import { StudioView } from "./components/studio/StudioView";
import { StudioNotice } from "./components/studio/StudioNotice";
import { QuitWhileMaking } from "./components/studio/QuitWhileMaking";
import { followApprovals, followRunningTurn, turnChatId, useApp, toErrorInfo } from "./store/app";
import { followStudio, useStudio } from "./store/studio";
import * as ipc from "./lib/ipc";
import { listenInEffect } from "./lib/listen";

export default function App() {
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsTab, setSettingsTab] = useState<string | undefined>(undefined);
  const [modelsOpen, setModelsOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  // null while loading; true once first run was completed or skipped.
  const [firstRunDone, setFirstRunDone] = useState<boolean | null>(ipc.isTauri() ? null : true);
  const { refreshChats, refreshModelOptions, setRuntime, newChat, stop, showError } = useApp();
  // 0.1.0 Studio: shown in place of the chat, which stays mounted (hidden) so its reply keeps
  // running and its draft stays. Studio is mounted when first opened, then kept.
  const studioShown = useStudio((s) => s.view === "studio");
  const [studioMounted, setStudioMounted] = useState(studioShown);
  if (studioShown && !studioMounted) setStudioMounted(true);

  // Approval cards: Rust sends each request only to this window with a random id.
  // [0.0.7 P6] Each listener is stopped even when it starts after the cleanup ran, and the
  // cards waiting when this window (re)loaded are read once.
  useEffect(() => {
    if (!ipc.isTauri()) return;
    return followApprovals();
  }, []);

  // [0.0.7 P6] Which chat's reply the app is writing, also one that went on while this
  // window reloaded.
  useEffect(() => {
    if (!ipc.isTauri()) return;
    return followRunningTurn();
  }, []);

  // Studio's runs: their progress, also of runs that went on while this window reloaded.
  useEffect(() => {
    if (!ipc.isTauri()) return;
    return followStudio();
  }, []);

  useEffect(() => {
    if (!ipc.isTauri()) return;
    const unlisten = listenInEffect(() => ipc.onRuntimeStatus(setRuntime));
    Promise.all([refreshChats(), refreshModelOptions(), ipc.runtimeStatus().then(setRuntime)]).catch(
      (err) => showError(toErrorInfo(err)),
    );
    ipc
      .firstRunInfo()
      .then((i) => setFirstRunDone(i.done))
      .catch(() => setFirstRunDone(true));
    return unlisten;
  }, [refreshChats, refreshModelOptions, setRuntime, showError]);

  useEffect(() => {
    const onOpenModels = () => setModelsOpen(true);
    // A component deep in the tree asks for Settings on one page (detail: the tab name).
    const onOpenSettings = (event: Event) => {
      const tab: unknown = (event as CustomEvent<unknown>).detail;
      setSettingsTab(typeof tab === "string" ? tab : undefined);
      setSettingsOpen(true);
    };
    // Studio, on the tab the detail names ("image" or "video"), else on the one shown last.
    const onOpenStudio = (event: Event) => {
      const tab: unknown = (event as CustomEvent<unknown>).detail;
      useStudio.getState().showStudio(tab === "image" || tab === "video" ? tab : undefined);
    };
    window.addEventListener("app:open-models", onOpenModels);
    window.addEventListener("app:open-settings", onOpenSettings);
    window.addEventListener("app:open-studio", onOpenStudio);
    return () => {
      window.removeEventListener("app:open-models", onOpenModels);
      window.removeEventListener("app:open-settings", onOpenSettings);
      window.removeEventListener("app:open-studio", onOpenStudio);
    };
  }, []);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      // A key something on screen already took: Esc that closed a dialog (Radix prevents its
      // default but lets it reach the window) or a popup, never also the reply being written.
      if (event.defaultPrevented) return;
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
      } else if (mod && event.key.toLowerCase() === "g") {
        event.preventDefault();
        useStudio.getState().showStudio();
      } else if (mod && event.key.toLowerCase() === "n") {
        event.preventDefault();
        useStudio.getState().showChat();
        void newChat();
      } else if (mod && event.key.toLowerCase() === "k") {
        event.preventDefault();
        window.dispatchEvent(new Event("app:focus-search"));
      } else if (mod && event.key.toLowerCase() === "l") {
        event.preventDefault();
        useStudio.getState().showChat();
        window.dispatchEvent(new Event("app:focus-input"));
      } else if (event.key === "Escape") {
        // Esc never cancels a Studio job, nor the chat's reply while Studio is shown: there it
        // only closes the viewer or a popup.
        if (useStudio.getState().view === "studio") return;
        // [0.0.7 P6] The open chat's reply only: never one running in another chat.
        const s = useApp.getState();
        const turn = turnChatId(s);
        if (turn !== null && turn === s.activeChatId) void stop(turn);
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [newChat, stop]);

  const onSettingsOpenChange = (open: boolean) => {
    setSettingsOpen(open);
    // Studio reads again what Settings may have changed (an endpoint added, a preference).
    if (!open) window.dispatchEvent(new Event("app:settings-closed"));
  };
  const dialogs = (
    <>
      <SettingsDialog open={settingsOpen} onOpenChange={onSettingsOpenChange} initialTab={settingsTab} />
      <ModelsDialog open={modelsOpen} onOpenChange={setModelsOpen} />
      <ShortcutsDialog open={shortcutsOpen} onOpenChange={setShortcutsOpen} />
      {/* Asked when the window is closed, or the app quit, while a local Studio job works. */}
      <QuitWhileMaking />
    </>
  );
  // The update box floats where it covers neither the message box nor the header's controls
  // (UpdateBox.tsx): over the foot of the sidebar, or under the header when the sidebar is
  // hidden, or in the corner of the first-run screen.
  const notices = (updatePlacement: UpdatePlacement) => (
    <AppNotices
      updatePlacement={updatePlacement}
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
        {notices("corner")}
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
      {notices(sidebarOpen ? "sidebar" : "below-header")}
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
        <div className="relative flex min-w-0 flex-1" hidden={studioShown}>
          <ChatView />
          <Workbench />
          {/* A video that finished while the chat was shown. */}
          <StudioNotice />
        </div>
        {studioMounted && (
          <div className="flex min-w-0 flex-1" hidden={!studioShown}>
            <StudioView />
          </div>
        )}
      </div>
      {dialogs}
      {/* Only with the main window on screen, after first run: it asks about chats. */}
      {ipc.isTauri() && <FullAccessStartCheck />}
    </div>
  );
}
