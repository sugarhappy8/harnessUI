import "./lib/freeze";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { UiProviders } from "./components/ui/UiProviders";
import { AppRoot } from "./components/CrashNotice";
import { applyTheme, loadTheme } from "./lib/theme";
import { APP_NAME } from "./lib/app";
import { installTrustedTypes } from "./lib/trustedTypes";
import "./styles.css";

// Before anything is drawn: the window's Trusted Types rule refuses HTML strings, and the one a
// library writes (Radix Select's style) needs the app's policy (src/lib/trustedTypes.ts).
installTrustedTypes();
applyTheme(loadTheme());
document.title = APP_NAME;

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      {/* An error drawing any part of the app shows a notice with Try again, never a blank
          window (React removes everything up to the nearest error boundary). */}
      <AppRoot>
        <UiProviders>
          <App />
        </UiProviders>
      </AppRoot>
    </StrictMode>,
  );
}
