// 0.2.1: a model's page in Get models that fails to draw shows a notice in its panel; Get
// models' list behind it stays, and Try again or another model draws the page again. The data
// is real (the owner's repository); the throw is put in the page's parts list (PlanParts), which
// only the page draws.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import type { DownloadGroup } from "../../lib/types";
import type { LocalRepo, LocalSearchPage, SetupPlan } from "./types";
import searchJson from "./fixtures/real/search.json";
import bugRepoJson from "./fixtures/real/chatrlabs__Qwen-Image-2.1-Uncensored-Omni-Quantized/repo.json";
import bugPlanJson from "./fixtures/real/chatrlabs__Qwen-Image-2.1-Uncensored-Omni-Quantized/plan.json";
import controlRepoJson from "./fixtures/real/AtomicChat__Qwen-Image-2.1-Turbo-Uncensored-GGUF/repo.json";

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
  hubTokenStatus: vi.fn<() => Promise<{ saved: boolean; account: string | null }>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");
const broken = vi.hoisted(() => ({ parts: false }));
vi.mock("./PlanParts", async (importOriginal) => {
  const real = await importOriginal<typeof import("./PlanParts")>();
  return {
    ...real,
    PlanParts: (props: Parameters<typeof real.PlanParts>[0]) => {
      if (broken.parts) throw new TypeError("Cannot read properties of undefined (reading 'role')");
      return real.PlanParts(props);
    },
  };
});

const api = await import("./api");
const { LOCAL } = await import("./fixtures/models");
const { flush, showLocal } = await import("./testing/local");
const { useStudio } = await import("../../store/studio");
const { useDownloads } = await import("../../store/downloads");

Element.prototype.hasPointerCapture ??= () => false;
Element.prototype.setPointerCapture ??= () => {};
Element.prototype.releasePointerCapture ??= () => {};
Element.prototype.scrollIntoView ??= () => {};

const INITIAL = useStudio.getState();
const SEARCH = searchJson as unknown as LocalSearchPage;
const REPO = bugRepoJson as unknown as LocalRepo;
const PLAN = (bugPlanJson as unknown as { plan: SetupPlan }).plan;
const CONTROL = controlRepoJson as unknown as LocalRepo;

let quiet: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  vi.clearAllMocks();
  broken.parts = true;
  useStudio.setState(INITIAL, true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.hubTokenStatus.mockResolvedValue({ saved: false, account: null });
  ipc.onDownloadProgress.mockResolvedValue(() => undefined);
  quiet = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  cleanup();
  quiet.mockRestore();
});

async function search() {
  await showLocal(LOCAL, { popular: [] });
  vi.mocked(api.studioLocalSearch).mockResolvedValue(SEARCH);
  const box = screen.getByRole("searchbox", { name: "Search Hugging Face" });
  await act(async () => {
    fireEvent.change(box, { target: { value: "qwen image uncensored" } });
    fireEvent.submit(box.closest("form")!);
  });
  await flush();
}

async function open(repo: string) {
  const card = screen.getByRole("article", { name: repo });
  await act(async () => {
    fireEvent.click(within(card).getByRole("button", { name: repo }));
  });
  await flush();
  await flush();
}

it("a model's page that throws shows a notice in its panel, and leaves Get models' list and the app drawn", async () => {
  vi.mocked(api.studioLocalRepo).mockResolvedValue(REPO);
  vi.mocked(api.studioSetupPlan).mockResolvedValue(PLAN);
  await search();
  await open(REPO.repo);

  const dialog = screen.getByRole("dialog", { name: REPO.repo });
  expect(within(dialog).getByRole("alert", { name: "This model's page couldn't be shown." })).toBeTruthy();
  expect(within(dialog).getByRole("button", { name: "Try again" })).toBeTruthy();
  // Get models is still there behind the panel: its search and every card of the search.
  expect(screen.getByRole("searchbox", { name: "Search Hugging Face", hidden: true })).toBeTruthy();
  expect(screen.getAllByRole("article", { hidden: true })).toHaveLength(SEARCH.items.length);
  expect(quiet.mock.calls.map((c: unknown[]) => String(c[0])).filter((t: string) => t.startsWith("[crash]"))).toEqual([
    "[crash] ModelDetail could not be drawn: TypeError: Cannot read properties of undefined (reading 'role')",
  ]);

  // Try again, once the page can be drawn: the page is back, with its Model type menu.
  broken.parts = false;
  await act(async () => fireEvent.click(within(dialog).getByRole("button", { name: "Try again" })));
  expect(within(dialog).queryByRole("alert")).toBeNull();
  expect(within(dialog).getByRole("combobox")).toBeTruthy();
});

it("another model opened after a page that threw draws its own page", async () => {
  vi.mocked(api.studioLocalRepo).mockResolvedValue(REPO);
  vi.mocked(api.studioSetupPlan).mockResolvedValue(PLAN);
  await search();
  await open(REPO.repo);
  expect(screen.getByRole("alert", { name: "This model's page couldn't be shown." })).toBeTruthy();

  // Closed, then another card opened.
  broken.parts = false;
  await act(async () => useStudio.getState().closeDetail());
  vi.mocked(api.studioLocalRepo).mockResolvedValue(CONTROL);
  await open(CONTROL.repo);
  const dialog = screen.getByRole("dialog", { name: CONTROL.repo });
  expect(within(dialog).queryByRole("alert")).toBeNull();
  expect(dialog.textContent).toContain(CONTROL.repo.split("/")[1]);
});
