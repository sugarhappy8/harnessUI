import "./lib/freeze";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { UiProviders } from "./components/ui/UiProviders";
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
      <UiProviders>
        <App />
      </UiProviders>
    </StrictMode>,
  );
}
