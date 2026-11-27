// 0.2.1: a workbench tab that fails to draw shows a notice inside the side panel. The panel's
// tab list and Close stay usable, and choosing another tab (or another chat) draws it again.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";

const broken = vi.hoisted(() => ({ sources: false }));
vi.mock("./workbench/SourcesTab", () => ({
  SourcesTab: (props: { chatId: number }) => {
    if (broken.sources) throw new TypeError("Cannot read properties of undefined (reading 'url')");
    return <p>sources of {props.chatId}</p>;
  },
}));
vi.mock("./workbench/ArtifactsTab", () => ({ ArtifactsTab: () => <p>artifacts</p> }));
vi.mock("./workbench/TaskTab", () => ({ TaskTab: () => <p>task</p> }));
vi.mock("./workbench/ContextTab", () => ({ ContextTab: () => <p>context</p> }));

const { Workbench } = await import("./Workbench");
const { useApp } = await import("../store/app");

let quiet: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  broken.sources = true;
  useApp.setState({ activeChatId: 1, workbench: { open: true, tab: "sources" } });
  quiet = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  quiet.mockRestore();
});

it("a tab that throws shows a notice in the panel; its tab list and Close stay, and another tab draws", () => {
  render(<Workbench />);
  const aside = screen.getByRole("complementary", { name: "Workbench" });
  expect(within(aside).getByRole("alert", { name: "The side panel couldn't be shown." })).toBeTruthy();
  expect(within(aside).getByRole("tablist", { name: "Workbench tabs" })).toBeTruthy();

  fireEvent.click(within(aside).getByRole("tab", { name: "Artifacts" }));
  expect(within(aside).queryByRole("alert")).toBeNull();
  expect(within(aside).getByText("artifacts")).toBeTruthy();

  // Back on the broken tab, then closed with the panel's own Close.
  fireEvent.click(within(aside).getByRole("tab", { name: "Sources" }));
  expect(within(aside).getByRole("alert")).toBeTruthy();
  fireEvent.click(within(aside).getByRole("button", { name: "Close the panel" }));
  expect(screen.queryByRole("complementary", { name: "Workbench" })).toBeNull();
});

it("another chat draws the tab again, and opening the panel elsewhere is not swallowed", () => {
  render(<Workbench />);
  expect(screen.getByRole("alert")).toBeTruthy();
  broken.sources = false;
  act(() => useApp.setState({ activeChatId: 2 }));
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByText("sources of 2")).toBeTruthy();

  broken.sources = true;
  act(() => useApp.setState({ activeChatId: 3 }));
  expect(screen.getByRole("alert")).toBeTruthy();
  // A citation card asking for another tab (openWorkbench) shows it.
  act(() => useApp.getState().openWorkbench("context"));
  expect(screen.queryByRole("alert")).toBeNull();
  expect(screen.getByText("context")).toBeTruthy();
});
