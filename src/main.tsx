import "./lib/freeze";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { applyTheme, loadTheme } from "./lib/theme";
import { APP_NAME } from "./lib/app";
import "./styles.css";

applyTheme(loadTheme());
document.title = APP_NAME;

const root = document.getElementById("root");
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}
