// The preview page's entry: the app's own main.tsx, with Tauri's IPC mocked first.
// `?view=studio` opens Studio at start (as Ctrl+G does); the theme follows the browser's
// prefers-color-scheme (Playwright's `colorScheme`), as the app's "System" theme does.
// `?crash=root` draws a part that throws at once, to show the whole window's error notice.
import "../../src/lib/freeze";
import { installMocks } from "./mock/ipc";

installMocks();

const [{ StrictMode, createElement }, { createRoot }, { default: App }, { UiProviders }, { AppRoot }, trusted, theme, app, studioStore, appStore, crash] =
  await Promise.all([
    import("react"),
    import("react-dom/client"),
    import("../../src/App"),
    import("../../src/components/ui/UiProviders"),
    import("../../src/components/CrashNotice"),
    import("../../src/lib/trustedTypes"),
    import("../../src/lib/theme"),
    import("../../src/lib/app"),
    import("../../src/store/studio"),
    import("../../src/store/app"),
    import("../../src/lib/crash"),
  ]);
await import("../../src/styles.css");

trusted.installTrustedTypes();
theme.applyTheme("system");
document.title = app.APP_NAME;

const preview = (window as unknown as { __preview: Record<string, unknown> }).__preview;
preview.useStudio = studioStore.useStudio;
preview.useApp = appStore.useApp;

const params = new URLSearchParams(location.search);
if (params.get("view") === "studio") studioStore.useStudio.getState().showStudio("image");

function Crash(): never {
  throw new TypeError("A deliberate error from the preview's ?crash=root");
}
const drawn = params.get("crash") === "root" ? createElement(Crash) : createElement(App);
const root = document.getElementById("root");
if (root) createRoot(root, crash.rootErrorHandlers(root)).render(createElement(StrictMode, null, createElement(AppRoot, null, createElement(UiProviders, null, drawn))));
