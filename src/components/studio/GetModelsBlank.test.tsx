// Bug report on 0.2.0 (Windows, WebView2): in Get models, clicking the card of
// chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized left the whole window blank.
//
// The app's Content-Security-Policy (src-tauri/tauri.conf.json) has
// `require-trusted-types-for 'script'`, which WebView2 (Chromium) enforces: setting `innerHTML`
// to a plain string throws a TypeError. That repository's plan offers two recipes, so the panel
// draws the Model type menu (SelectField, Radix Select). Even closed, Radix Select draws its
// Viewport, which renders `<style dangerouslySetInnerHTML>`; React sets that element's
// `innerHTML`, the browser throws, and with no error boundary React unmounted the whole app.
// The fix: the app's `default` Trusted Types policy (src/lib/trustedTypes.ts), which lets that
// one fixed string through and still refuses any other.
//
// jsdom has no Trusted Types, so this file enforces them as Chromium does
// (src/lib/testing/trustedTypes.ts). The data is real: the search page, repositories and plans
// the backend returned (fixtures/real/).
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, screen, within } from "@testing-library/react";
import type { DownloadGroup } from "../../lib/types";
import type { LocalRepo, LocalSearchHit, LocalSearchPage, SetupPlan } from "./types";
import { enforceTrustedTypes } from "../../lib/testing/trustedTypes";
import { SETUP } from "../../lib/words";
import searchJson from "./fixtures/real/search.json";
import bugRepoJson from "./fixtures/real/chatrlabs__Qwen-Image-2.1-Uncensored-Omni-Quantized/repo.json";
import bugPlanJson from "./fixtures/real/chatrlabs__Qwen-Image-2.1-Uncensored-Omni-Quantized/plan.json";
import editRepoJson from "./fixtures/real/ChrisColeTech__qwen-image-edit-uncensored-GGUF/repo.json";
import editPlanJson from "./fixtures/real/ChrisColeTech__qwen-image-edit-uncensored-GGUF/plan.json";
import controlRepoJson from "./fixtures/real/AtomicChat__Qwen-Image-2.1-Turbo-Uncensored-GGUF/repo.json";
import byHandRepoJson from "./fixtures/real/Lightricks__LTX-Video/repo.json";
import byHandCardJson from "./fixtures/real/Lightricks__LTX-Video/card.json";

const ipc = vi.hoisted(() => ({
  isTauri: () => true,
  listDownloads: vi.fn<() => Promise<DownloadGroup[]>>(),
  onDownloadProgress: vi.fn<(handler: (g: DownloadGroup) => void) => Promise<() => void>>(),
  hubTokenStatus: vi.fn<() => Promise<{ saved: boolean; account: string | null }>>(),
}));
vi.mock("../../lib/ipc", () => ipc);
vi.mock("./api");

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
const BUG_REPO = bugRepoJson as unknown as LocalRepo;
const BUG_PLAN = (bugPlanJson as unknown as { plan: SetupPlan }).plan;
const EDIT_REPO = editRepoJson as unknown as LocalRepo;
const EDIT_PLAN = (editPlanJson as unknown as { plan: SetupPlan }).plan;
const CONTROL_REPO = controlRepoJson as unknown as LocalRepo;
const BY_HAND_REPO = byHandRepoJson as unknown as LocalRepo;
const BY_HAND_CARD = byHandCardJson as unknown as LocalSearchHit;

let tt: ReturnType<typeof enforceTrustedTypes>;

beforeEach(() => {
  tt = enforceTrustedTypes();
  vi.clearAllMocks();
  useStudio.setState(INITIAL, true);
  useDownloads.setState({ groups: [] });
  ipc.listDownloads.mockResolvedValue([]);
  ipc.hubTokenStatus.mockResolvedValue({ saved: false, account: null });
  ipc.onDownloadProgress.mockResolvedValue(() => undefined);
});

afterEach(() => {
  cleanup();
  tt.stop();
});

/** Runs `steps` and returns what React reported as uncaught (the throw that unmounts the app). */
async function catching(steps: () => Promise<void>) {
  const uncaught: unknown[] = [];
  const onError = (e: ErrorEvent) => {
    uncaught.push(e.error ?? e.message);
    e.preventDefault();
  };
  window.addEventListener("error", onError);
  const quiet = vi.spyOn(console, "error").mockImplementation(() => {});
  try {
    await steps();
  } catch (e) {
    uncaught.push(e);
  } finally {
    window.removeEventListener("error", onError);
    quiet.mockRestore();
  }
  return uncaught;
}

/** Get models with `page` as the search's answer, searched as the owner did (the form sent),
 *  then `repo`'s card clicked. */
async function openFromSearch(repo: LocalRepo, plan: SetupPlan | null, page: LocalSearchPage = SEARCH) {
  vi.mocked(api.studioLocalRepo).mockResolvedValue(repo);
  if (plan) vi.mocked(api.studioSetupPlan).mockResolvedValue(plan);
  await showLocal(LOCAL, { popular: [] });
  vi.mocked(api.studioLocalSearch).mockResolvedValue(page);
  const box = screen.getByRole("searchbox", { name: "Search Hugging Face" });
  await act(async () => {
    fireEvent.change(box, { target: { value: "qwen image uncensored" } });
    fireEvent.submit(box.closest("form")!);
  });
  await flush();
  expect(api.studioLocalSearch).toHaveBeenCalled();
  const card = screen.getByRole("article", { name: repo.repo });
  await act(async () => {
    fireEvent.click(within(card).getByRole("button", { name: repo.repo }));
  });
  await flush();
  await flush();
}

it.each([
  ["chatrlabs/Qwen-Image-2.1-Uncensored-Omni-Quantized (2 model types)", BUG_REPO, BUG_PLAN],
  ["ChrisColeTech/qwen-image-edit-uncensored-GGUF (4 model types)", EDIT_REPO, EDIT_PLAN],
])("opening %s from a search keeps the app drawn under the app's Trusted Types rule", async (_, repo, plan) => {
  const uncaught = await catching(() => openFromSearch(repo, plan));

  // What blanked the owner's window: a string written to innerHTML, refused by Trusted Types.
  expect(tt.violations, "innerHTML set to a plain string (blocked by require-trusted-types-for)").toEqual([]);
  expect(uncaught.map(String)).toEqual([]);
  // The app is still drawn, with the repository's detail panel open and its Model type menu.
  expect(document.body.textContent?.length ?? 0).toBeGreaterThan(100);
  const dialog = screen.getByRole("dialog", { name: repo.repo });
  expect(within(dialog).getByRole("combobox")).toBeTruthy();
  // The policy that let Radix's style through still refuses any other HTML.
  const policy = tt.policies.get("default");
  expect(policy, "the app's default policy").toBeDefined();
  expect(() => policy!.createHTML!("<img src=x onerror=alert(1)>")).toThrow(TypeError);
});

it("Set it up yourself on Lightricks/LTX-Video (a second menu, drawn on a second click) keeps the app drawn", async () => {
  const uncaught = await catching(async () => {
    await openFromSearch(BY_HAND_REPO, null, { items: [BY_HAND_CARD] } as LocalSearchPage);
    const dialog = screen.getByRole("dialog", { name: BY_HAND_REPO.repo });
    const yourself = within(dialog).getAllByRole("button", { name: new RegExp(`^${SETUP.yourself}`) });
    await act(async () => {
      fireEvent.click(yourself[0]!);
    });
    await flush();
  });

  expect(tt.violations).toEqual([]);
  expect(uncaught.map(String)).toEqual([]);
  const dialog = screen.getByRole("dialog", { name: BY_HAND_REPO.repo });
  expect(within(dialog).getByRole("combobox")).toBeTruthy();
});

it("control: a repository with no file choice (no Model type menu) opens under the same rule", async () => {
  const uncaught = await catching(() => openFromSearch(CONTROL_REPO, null));

  expect(tt.violations).toEqual([]);
  expect(uncaught.map(String)).toEqual([]);
  expect(screen.getByRole("dialog", { name: CONTROL_REPO.repo })).toBeTruthy();
});
