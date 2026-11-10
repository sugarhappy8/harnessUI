import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "../styles.css";

// The computer-use indicator. M7 adds the stop button and live state; until then
// the window is never shown.
function Overlay() {
  return (
    <div className="pointer-events-none fixed inset-0 border-4 border-danger" aria-hidden />
  );
}

const root = document.getElementById("overlay-root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <Overlay />
    </StrictMode>,
  );
}
