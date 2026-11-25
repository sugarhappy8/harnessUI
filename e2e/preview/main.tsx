// The preview page's entry: the app's own main.tsx, with Tauri's IPC mocked first.
// `?view=studio` opens Studio at start (as Ctrl+G does); the theme follows the browser's
// prefers-color-scheme (Playwright's `colorScheme`), as the app's "System" theme does.
import "../../src/lib/freeze";
import { installMocks } from "./mock/ipc";

installMocks();

const [{ StrictMode, createElement }, { createRoot }, { default: App }, { UiProviders }, theme, app, studioStore, appStore] =
  await Promise.all([
    import("react"),
    import("react-dom/client"),
    import("../../src/App"),
    import("../../src/components/ui/UiProviders"),
    import("../../src/lib/theme"),
    import("../../src/lib/app"),
    import("../../src/store/studio"),
    import("../../src/store/app"),
  ]);
await import("../../src/styles.css");

theme.applyTheme("system");
document.title = app.APP_NAME;

const preview = (window as unknown as { __preview: Record<string, unknown> }).__preview;
preview.useStudio = studioStore.useStudio;
preview.useApp = appStore.useApp;

const params = new URLSearchParams(location.search);
if (params.get("view") === "studio") studioStore.useStudio.getState().showStudio("image");

const root = document.getElementById("root");
if (root) createRoot(root).render(createElement(StrictMode, null, createElement(UiProviders, null, createElement(App))));
