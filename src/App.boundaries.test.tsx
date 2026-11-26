// 0.2.1: one pane that fails to draw never blanks the window. The app as main.tsx draws it
// (AppRoot around UiProviders and App), with a pane that throws: the chat, Studio or the sidebar.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { StrictMode } from "react";

const broken = vi.hoisted(() => ({ chat: false, studio: false, sidebar: false }));

vi.mock("./lib/ipc", () => ({
  isTauri: () => false,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./components/Sidebar", () => ({
  Sidebar: () => {
    if (broken.sidebar) throw new TypeError("the sidebar could not be drawn");
    return <nav data-testid="chat-list" />;
  },
}));
vi.mock("./components/ChatView", () => ({
  ChatView: () => {
    if (broken.chat) throw new TypeError("Cannot read properties of undefined (reading 'map')");
    return <div data-testid="chat-view" />;
  },
}));
vi.mock("./components/Workbench", () => ({ Workbench: () => null }));
vi.mock("./components/studio/StudioView", () => ({
  StudioView: () => {
    if (broken.studio) throw new TypeError("Failed to set the 'innerHTML' property on 'Element'");
    return <div data-testid="studio-view" />;
  },
}));
vi.mock("./components/studio/api");
vi.mock("./components/SettingsDialog", () => ({ SettingsDialog: () => null }));
vi.mock("./components/ModelsDialog", () => ({ ModelsDialog: () => null }));
vi.mock("./components/FirstRun", () => ({ FirstRun: () => null }));
vi.mock("./components/ShortcutsDialog", () => ({ ShortcutsDialog: () => null }));
vi.mock("./components/AppNotices", () => ({ AppNotices: () => null }));

const { default: App } = await import("./App");
const { AppRoot } = await import("./components/CrashNotice");
const { UiProviders } = await import("./components/ui/UiProviders");
const { useApp } = await import("./store/app");
const { useStudio } = await import("./store/studio");
const studioApi = await import("./components/studio/api");

let quiet: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  Object.assign(broken, { chat: false, studio: false, sidebar: false });
  useApp.setState({ activeChatId: null });
  useStudio.setState({ view: "chat", tab: "image", live: {}, readyNotice: null });
  vi.mocked(studioApi.studioLive).mockResolvedValue([]);
  vi.mocked(studioApi.onQuitRequested).mockResolvedValue(() => {});
  quiet = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  quiet.mockRestore();
});

function drawApp() {
  return render(
    <StrictMode>
      <AppRoot>
        <UiProviders>
          <App />
        </UiProviders>
      </AppRoot>
    </StrictMode>,
  );
}

it("a chat that fails to draw shows a notice in its pane; the sidebar stays, and another chat draws again", () => {
  broken.chat = true;
  drawApp();
  expect(screen.getByTestId("chat-list")).toBeTruthy();
  expect(screen.getByRole("alert", { name: "This chat couldn't be shown." })).toBeTruthy();
  broken.chat = false;
  act(() => useApp.setState({ activeChatId: 7 }));
  expect(screen.getByTestId("chat-view")).toBeTruthy();
  expect(screen.queryByRole("alert")).toBeNull();
});

it("Studio that fails to draw shows a notice in its place; the sidebar stays, and Try again draws it", () => {
  broken.studio = true;
  useStudio.setState({ view: "studio" });
  drawApp();
  expect(screen.getByTestId("chat-list")).toBeTruthy();
  expect(screen.getByRole("alert", { name: "Studio couldn't be shown." })).toBeTruthy();
  broken.studio = false;
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  expect(screen.getByTestId("studio-view")).toBeTruthy();
});

it("an error no pane catches shows the whole window's notice, never a blank window, and Try again recovers", () => {
  broken.sidebar = true;
  const { container } = drawApp();
  expect(container.textContent?.length ?? 0).toBeGreaterThan(50);
  expect(screen.getByRole("alert", { name: "Something went wrong drawing this screen" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Reload harnessUI" })).toBeTruthy();
  broken.sidebar = false;
  fireEvent.click(screen.getByRole("button", { name: "Try again" }));
  expect(screen.getByTestId("chat-list")).toBeTruthy();
  expect(screen.getByTestId("chat-view")).toBeTruthy();
});
