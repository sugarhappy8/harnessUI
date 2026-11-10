import { StrictMode, useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "../styles.css";

/**
 * The computer-use indicator's Stop button (ARCHITECTURE.md 9.3, "Indicator"). This window
 * renders no model output and may call one command: stop. Input injection never targets
 * it (it belongs to this app).
 */
function Indicator() {
  const [hotkey, setHotkey] = useState<string>("the stop hotkey");
  useEffect(() => {
    const off = listen<string>("indicator-hotkey", (e) => setHotkey(e.payload.replace(/\+/g, " ")));
    return () => {
      void off.then((f) => f());
    };
  }, []);
  return (
    <div className="flex h-screen items-center justify-between gap-3 rounded-full border-2 border-danger bg-bg/95 px-4 text-sm shadow-lg">
      <span>
        AI is controlling this computer. Press <span className="font-mono">{hotkey}</span> to stop.
      </span>
      <button
        type="button"
        onClick={() => void invoke("overlay_stop")}
        className="rounded-full bg-danger px-3 py-1 font-medium text-white"
      >
        Stop
      </button>
    </div>
  );
}

const root = document.getElementById("indicator-root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <Indicator />
    </StrictMode>,
  );
}
