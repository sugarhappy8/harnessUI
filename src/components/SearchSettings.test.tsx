import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import type { SearchSettings as Settings, SearchSettingsInput, TestSearch } from "./sources/types";

let now: Settings;

const apiMock = vi.hoisted(() => ({
  getSearchSettings: vi.fn<() => Promise<Settings>>(),
  setSearchSettings: vi.fn<(s: SearchSettingsInput) => Promise<Settings>>(),
  setSearchKey: vi.fn<(provider: string, key: string) => Promise<Settings>>(),
  clearSearchKey: vi.fn<(provider: string) => Promise<Settings>>(),
  testSearch: vi.fn<() => Promise<TestSearch>>(),
}));
vi.mock("./sources/api", () => apiMock);

const { SearchSettings } = await import("./SearchSettings");

function settings(over: Partial<Settings> = {}): Settings {
  return {
    provider: "duckduckgo",
    searxngUrl: "",
    pages: 4,
    versionMatching: true,
    braveKeySet: false,
    tavilyKeySet: false,
    ...over,
  };
}

async function show() {
  await act(async () => {
    render(<SearchSettings />);
  });
}

const checked = (name: string) => screen.getByRole("radio", { name: new RegExp(name) }).getAttribute("aria-checked");

describe("SearchSettings", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    now = settings();
    apiMock.getSearchSettings.mockImplementation(async () => now);
    apiMock.setSearchSettings.mockImplementation(async (s) => {
      now = { ...now, ...s };
      return now;
    });
    apiMock.setSearchKey.mockImplementation(async (provider) => {
      now = { ...now, [provider === "brave" ? "braveKeySet" : "tavilyKeySet"]: true };
      return now;
    });
    apiMock.clearSearchKey.mockImplementation(async (provider) => {
      now = { ...now, [provider === "brave" ? "braveKeySet" : "tavilyKeySet"]: false };
      return now;
    });
  });
  afterEach(cleanup);

  it("says where searches and page requests go", async () => {
    await show();
    expect(
      screen.getByText(/the search is sent to the service chosen here, and the pages it finds are\s+then fetched from their own sites/),
    ).toBeTruthy();
    expect(checked("DuckDuckGo")).toBe("true");
    expect(screen.getByText(/never read from this computer or your local network/)).toBeTruthy();
  });

  it("saves the service that is chosen", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: /Brave Search/ }));
    });
    expect(apiMock.setSearchSettings).toHaveBeenCalledWith({
      provider: "brave",
      searxngUrl: "",
      pages: 4,
      versionMatching: true,
    });
    expect(checked("Brave Search")).toBe("true");
  });

  it("takes a key, says only whether one is set, and never shows it", async () => {
    now = settings({ provider: "brave" });
    await show();
    expect(screen.getByText("not set")).toBeTruthy();
    const field = screen.getByLabelText(/Brave Search API key/) as HTMLInputElement;
    expect(field.type).toBe("password");
    expect(screen.queryByRole("button", { name: "Remove key" })).toBeNull();
    expect((screen.getByRole("button", { name: "Set key" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(field, { target: { value: "BSA-SUPERSECRET" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Set key" }));
    });
    expect(apiMock.setSearchKey).toHaveBeenCalledWith("brave", "BSA-SUPERSECRET");
    expect(screen.getByText("set")).toBeTruthy();
    // The field is emptied and nothing on the page holds the key.
    expect((screen.getByLabelText(/Brave Search API key/) as HTMLInputElement).value).toBe("");
    expect(document.body.textContent).not.toContain("SUPERSECRET");
    for (const input of document.querySelectorAll("input")) expect(input.value).not.toContain("SUPERSECRET");
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Remove key" }));
    });
    expect(apiMock.clearSearchKey).toHaveBeenCalledWith("brave");
    expect(screen.getByText("not set")).toBeTruthy();
  });

  it("switches to SearXNG only together with its address", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("radio", { name: /SearXNG/ }));
    });
    expect(apiMock.setSearchSettings).not.toHaveBeenCalled();
    expect(screen.getByText("Enter the address to switch to SearXNG.")).toBeTruthy();
    // Nothing can be tested until the choice is saved.
    expect((screen.getByRole("button", { name: "Test search" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(screen.getByLabelText(/Address of your SearXNG instance/), {
      target: { value: "http://127.0.0.1:8888" },
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Set address" }));
    });
    expect(apiMock.setSearchSettings).toHaveBeenCalledWith({
      provider: "searxng",
      searxngUrl: "http://127.0.0.1:8888",
      pages: 4,
      versionMatching: true,
    });
    expect(checked("SearXNG")).toBe("true");
    expect(screen.queryByText("Enter the address to switch to SearXNG.")).toBeNull();
  });

  it("saves how many pages are read and whether versions are matched", async () => {
    await show();
    await act(async () => {
      fireEvent.change(screen.getByLabelText("Pages read per search"), { target: { value: "0" } });
    });
    expect(apiMock.setSearchSettings).toHaveBeenLastCalledWith(expect.objectContaining({ pages: 0 }));
    await act(async () => {
      fireEvent.click(screen.getByRole("checkbox", { name: /Match results to the library versions/ }));
    });
    expect(apiMock.setSearchSettings).toHaveBeenLastCalledWith(expect.objectContaining({ versionMatching: false, pages: 0 }));
  });

  it("runs a test search and shows what came back, as text", async () => {
    apiMock.testSearch.mockResolvedValue({
      provider: "DuckDuckGo",
      results: [
        { title: "<img src=x onerror=alert(1)>", site: "example.org" },
        { title: "Open source - Wikipedia", site: "en.wikipedia.org" },
      ],
    });
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Test search" }));
    });
    expect(screen.getByText("DuckDuckGo answered with 2 results.")).toBeTruthy();
    expect(screen.getByText("Open source - Wikipedia · en.wikipedia.org")).toBeTruthy();
    expect(document.querySelector("img")).toBeNull();
  });

  it("shows why a test search failed and what to do", async () => {
    apiMock.testSearch.mockRejectedValue({
      code: "search",
      message: "DuckDuckGo refused the search: it turns automated searches away for a while. Try again later, or choose another search service in Settings → Search.",
      nextStep: "Change the search service or its key above, then test again.",
      details: "the service refused the search",
    });
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Test search" }));
    });
    expect(screen.getByText(/DuckDuckGo refused the search/)).toBeTruthy();
    expect(screen.getByText(/Change the search service or its key above/)).toBeTruthy();
  });

  it("shows a refused setting as an error and keeps what was saved", async () => {
    now = settings({ provider: "searxng", searxngUrl: "http://127.0.0.1:8888" });
    apiMock.setSearchSettings.mockRejectedValue({
      code: "bad_request",
      message: "The SearXNG address must start with http:// or https://.",
      nextStep: "Check the value and try again.",
      details: "",
    });
    await show();
    fireEvent.change(screen.getByLabelText(/Address of your SearXNG instance/), { target: { value: "ftp://x" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Set address" }));
    });
    expect(screen.getByText("The SearXNG address must start with http:// or https://.")).toBeTruthy();
    expect(checked("SearXNG")).toBe("true");
  });
});
