import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { Provider, ProviderKind } from "../../lib/types";

// [0.0.7 P5] Settings → Endpoints: the ready-made choices for the cloud AI companies, the
// "Cloud" mark, Edit and the context size of an endpoint.

const KEY_OVER_HTTP =
  "An API key is sent over plain http:// only to this computer or to a local network address such as 192.168.1.20, so that no one on the way can read it. Change the address to start with https://, or use the server's local IP address.";

let providers: Provider[] = [];

const ipcMock = vi.hoisted(() => ({
  listProviders: vi.fn<() => Promise<Provider[]>>(),
  saveProvider: vi.fn<(p: Record<string, unknown>) => Promise<Provider>>(),
  deleteProvider: vi.fn<(id: number) => Promise<void>>(async () => undefined),
  testProvider: vi.fn<(id: number) => Promise<unknown[]>>(),
  getSetting: vi.fn<(key: string) => Promise<unknown>>(async () => null),
  setSetting: vi.fn<(key: string, value: unknown) => Promise<void>>(async () => undefined),
  isTauri: () => false,
}));
vi.mock("../../lib/ipc", () => ipcMock);
const refreshModelOptions = vi.hoisted(() => vi.fn(async () => undefined));
const openChat = vi.hoisted(() => vi.fn(async (id: number | null) => void id));
vi.mock("../../store/app", () => ({
  toErrorInfo: (e: unknown) => ({ code: "bad_request", message: String(e), nextStep: "", details: "" }),
  useApp: (select: (s: Record<string, unknown>) => unknown) =>
    select({ refreshModelOptions, activeChatId: 5, openChat }),
}));
const openUrl = vi.hoisted(() => vi.fn(async (url: string) => void url));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));

const { Endpoints } = await import("./Endpoints");
const { switchChoice, hostOf, choiceFor, abilitiesOf } = await import("./endpointPresets");

function provider(over: Partial<Provider>): Provider {
  return {
    id: 1,
    name: "Anthropic",
    kind: "anthropic",
    baseUrl: "https://api.anthropic.com",
    hasApiKey: true,
    enabled: true,
    cloud: true,
    contextLength: null,
    ...over,
  };
}

async function show() {
  await act(async () => {
    render(<Endpoints />);
  });
}

function field(name: string): HTMLInputElement {
  return screen.getByLabelText(name) as HTMLInputElement;
}

async function choose(label: string, value: string) {
  await act(async () => {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  });
}

async function type(label: string, value: string) {
  await act(async () => {
    fireEvent.change(field(label), { target: { value } });
  });
}

async function submit() {
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
  });
}

describe("Settings → Endpoints", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    providers = [];
    ipcMock.listProviders.mockImplementation(async () => providers);
    ipcMock.saveProvider.mockImplementation(async (p) => {
      const saved = provider({ ...(p as Partial<Provider>), id: (p.id as number | null) ?? 7, hasApiKey: !!p.apiKey });
      providers = [...providers.filter((x) => x.id !== saved.id), saved];
      return saved;
    });
    ipcMock.testProvider.mockImplementation(async () => [{}, {}]);
  });
  afterEach(cleanup);

  it("marks only the endpoints on the internet as Cloud, saying where their chats go", async () => {
    providers = [
      provider({}),
      provider({ id: 2, name: "Ollama", kind: "ollama", baseUrl: "http://192.168.1.5:11434", hasApiKey: false, cloud: false }),
    ];
    await show();
    const rows = screen.getAllByRole("listitem");
    expect(rows).toHaveLength(2);
    const mark = rows[0]!.querySelector("[data-cloud-mark]") as HTMLElement;
    expect(mark.title).toBe("Chats with this endpoint's models go to api.anthropic.com.");
    expect(mark.textContent).toContain("Cloud");
    expect(rows[1]!.querySelector("[data-cloud-mark]")).toBeNull();
    expect(rows[0]!.textContent).toContain("API key saved");
  });

  it("cards say what each company can do", async () => {
    // 0.1.0 Studio: OpenRouter makes pictures and video, OpenAI pictures only (its video API
    // shut down in September 2026); the others chat only.
    expect(abilitiesOf("openrouter")).toBe("Chat · Pictures · Video");
    expect(abilitiesOf("openai")).toBe("Chat · Pictures");
    expect(abilitiesOf("anthropic")).toBe("Chat");
    expect(abilitiesOf("abliteration")).toBe("Chat");
    expect(abilitiesOf("ollama")).toBe("Chat");
    expect(abilitiesOf("generic")).toBe("Chat");
    // A kind from a newer app counts as "Other server".
    expect(abilitiesOf("someday" as ProviderKind)).toBe("Chat");
    providers = [
      provider({ id: 1, name: "OpenRouter", kind: "openrouter", baseUrl: "https://openrouter.ai/api/v1" }),
      provider({ id: 2, name: "OpenAI", kind: "openai", baseUrl: "https://api.openai.com/v1" }),
      provider({}),
    ];
    await show();
    const rows = screen.getAllByRole("listitem");
    expect(rows.map((r) => r.querySelector("[data-abilities]")?.textContent)).toEqual([
      "Chat · Pictures · Video",
      "Chat · Pictures",
      "Chat",
    ]);
  });

  it("fills in a company's name and address, asks for its key and says where chats go", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add an endpoint/ }));
    });
    // The companies come first, Anthropic's own Messages API at the top.
    const kinds = [...(screen.getByLabelText("Company or server") as HTMLSelectElement).options].map((o) => o.value);
    expect(kinds).toEqual(["anthropic", "openai", "openrouter", "abliteration", "ollama", "lmstudio", "vllm", "generic"]);
    expect((screen.getByLabelText("Company or server") as HTMLSelectElement).value).toBe("anthropic");
    expect(field("Name").value).toBe("Anthropic");
    expect(field("Address").value).toBe("https://api.anthropic.com");
    expect(field("API key").required).toBe(true);
    expect(field("API key").type).toBe("password");
    const note = () => document.querySelector("[data-company-note]");
    expect(note()!.textContent).toContain("Chats that use its models go to Anthropic");

    const expected: [string, string, string, string][] = [
      ["openai", "OpenAI", "https://api.openai.com/v1", "OpenAI"],
      ["openrouter", "OpenRouter", "https://openrouter.ai/api/v1", "OpenRouter"],
      ["abliteration", "abliteration.ai", "https://api.abliteration.ai/v1", "abliteration.ai"],
    ];
    for (const [kind, name, url, company] of expected) {
      await choose("Company or server", kind);
      expect(field("Name").value).toBe(name);
      expect(field("Address").value).toBe(url);
      expect(field("API key").required).toBe(true);
      expect(note()!.textContent).toContain(`Chats that use its models go to ${company}`);
    }
    // "Other server (OpenAI-compatible)" stays, with an optional key and no company.
    await choose("Company or server", "generic");
    expect(field("Address").value).toBe("https://example.com/v1");
    expect(field("API key").required).toBe(false);
    expect(note()).toBeNull();
  });

  it("keeps a name and address the user typed when the choice changes", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add an endpoint/ }));
    });
    await type("Name", "Work");
    await choose("Company or server", "openai");
    expect(field("Name").value).toBe("Work");
    expect(field("Address").value).toBe("https://api.openai.com/v1");
    await type("Address", "https://gateway.example.com/v1");
    await choose("Company or server", "openrouter");
    expect(field("Address").value).toBe("https://gateway.example.com/v1");
    expect(switchChoice({ name: "", baseUrl: "" }, "ollama", "anthropic")).toEqual({
      name: "Anthropic",
      baseUrl: "https://api.anthropic.com",
    });
    expect(hostOf("https://openrouter.ai/api/v1")).toBe("openrouter.ai");
    expect(hostOf("not an address")).toBe("not an address");
    // A kind from a newer app counts as "Other server".
    expect(choiceFor("someday" as never).kind).toBe("generic");
  });

  it("opens the company's key page in the browser", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add an endpoint/ }));
    });
    const pages: [string, string, string][] = [
      ["anthropic", "platform.claude.com", "https://platform.claude.com/settings/keys"],
      ["openai", "platform.openai.com", "https://platform.openai.com/api-keys"],
      ["openrouter", "openrouter.ai/keys", "https://openrouter.ai/keys"],
      ["abliteration", "abliteration.ai", "https://abliteration.ai"],
    ];
    for (const [kind, page, url] of pages) {
      await choose("Company or server", kind);
      await act(async () => {
        fireEvent.click(within(document.querySelector("[data-company-note]") as HTMLElement).getByRole("button", { name: page }));
      });
      expect(openUrl).toHaveBeenLastCalledWith(url);
    }
  });

  it("saves the key and the context size, then lists the endpoint's models for the picker", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add an endpoint/ }));
    });
    await type("API key", "  sk-ant-test  ");
    await choose("Context size", "200000");
    await submit();
    expect(ipcMock.saveProvider).toHaveBeenCalledWith({
      id: null,
      name: "Anthropic",
      kind: "anthropic",
      baseUrl: "https://api.anthropic.com",
      apiKey: "sk-ant-test",
      clearApiKey: false,
      contextLength: 200000,
    });
    expect(ipcMock.testProvider).toHaveBeenCalledWith(7);
    expect(refreshModelOptions).toHaveBeenCalled();
    // The open chat is read again, so its "Cloud" mark follows the endpoint.
    expect(openChat).toHaveBeenCalledWith(5);
    const row = screen.getByRole("listitem");
    expect(row.textContent).toContain("Connected · 2 model(s)");
    expect(row.textContent).toContain(`at most ${(200000).toLocaleString()} tokens`);
    // The key never shows anywhere on the page.
    expect(document.body.textContent).not.toContain("sk-ant-test");
    expect(screen.queryByRole("form")).toBeNull();
  });

  it("shows the backend's refusal of a key over plain http:// and keeps the form", async () => {
    ipcMock.saveProvider.mockImplementation(async () => {
      throw KEY_OVER_HTTP;
    });
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add an endpoint/ }));
    });
    await choose("Company or server", "generic");
    await type("Address", "http://203.0.113.9/v1");
    await type("API key", "sk-test");
    await submit();
    expect(screen.getByRole("alert").textContent).toContain(KEY_OVER_HTTP);
    expect(field("Address").value).toBe("http://203.0.113.9/v1");
    expect(ipcMock.testProvider).not.toHaveBeenCalled();
  });

  it("edits an endpoint, keeping its saved key unless a new one is typed", async () => {
    providers = [provider({ contextLength: 65536 })];
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Edit Anthropic" }));
    });
    expect(field("API key").required).toBe(false);
    expect(field("API key").placeholder).toBe("saved (leave empty to keep it)");
    expect((screen.getByLabelText("Context size") as HTMLSelectElement).value).toBe("65536");
    await choose("Context size", "");
    await submit();
    expect(ipcMock.saveProvider).toHaveBeenCalledWith({
      id: 1,
      name: "Anthropic",
      kind: "anthropic",
      baseUrl: "https://api.anthropic.com",
      apiKey: null,
      clearApiKey: false,
      contextLength: null,
    });
    expect(ipcMock.testProvider).toHaveBeenCalledWith(1);
  });

  it("never carries a saved key to a new address", async () => {
    providers = [
      provider({ id: 3, name: "OpenAI", kind: "openai", baseUrl: "https://api.openai.com/v1" }),
      provider({ id: 4, name: "Box", kind: "generic", baseUrl: "http://192.168.1.5:8000/v1", cloud: false }),
    ];
    await show();
    // Another company: the key must be pasted again.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Edit OpenAI" }));
    });
    expect(field("API key").required).toBe(false);
    expect(document.getElementById("p-key-moved")).toBeNull();
    await choose("Company or server", "openrouter");
    expect(field("Address").value).toBe("https://openrouter.ai/api/v1");
    expect(field("API key").required).toBe(true);
    expect(field("API key").placeholder).toBe("paste your key");
    expect(document.getElementById("p-key-moved")!.textContent).toContain(
      "The saved key is for api.openai.com and is not sent to a new address.",
    );
    await type("API key", "sk-or-new");
    await submit();
    expect(ipcMock.saveProvider).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: 3, kind: "openrouter", apiKey: "sk-or-new", clearApiKey: false }),
    );
    // The same host with another path keeps it.
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Edit Box" }));
    });
    await type("Address", "http://192.168.1.5:8000/v2");
    expect(field("API key").placeholder).toBe("saved (leave empty to keep it)");
    expect(document.getElementById("p-key-moved")).toBeNull();
    // A server moved to another address without a key: the saved key is dropped.
    await type("Address", "http://192.168.1.6:8000/v1");
    expect(field("API key").required).toBe(false);
    expect(document.getElementById("p-key-moved")!.textContent).toContain("Paste a key if this server needs one.");
    await submit();
    expect(ipcMock.saveProvider).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: 4, baseUrl: "http://192.168.1.6:8000/v1", apiKey: null, clearApiKey: true }),
    );
  });

  it("removes a saved key on request", async () => {
    providers = [provider({})];
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Edit Anthropic" }));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("checkbox", { name: "Remove the saved key" }));
    });
    // A company's endpoint then needs a new key.
    expect(field("API key").required).toBe(true);
    await type("API key", "sk-ant-new");
    await submit();
    expect(ipcMock.saveProvider).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: 1, apiKey: "sk-ant-new", clearApiKey: false }),
    );
    // A server's endpoint can go without one.
    providers = [provider({ id: 2, name: "Box", kind: "generic", baseUrl: "http://10.0.0.2:8000/v1", cloud: false })];
    cleanup();
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Edit Box" }));
    });
    await act(async () => {
      fireEvent.click(screen.getByRole("checkbox", { name: "Remove the saved key" }));
    });
    await submit();
    expect(ipcMock.saveProvider).toHaveBeenLastCalledWith(
      expect.objectContaining({ id: 2, apiKey: null, clearApiKey: true }),
    );
  });

  it("says how much a company's Automatic sends, and that more costs more", async () => {
    await show();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Add an endpoint/ }));
    });
    const help = () => document.getElementById("p-ctx-help")!.textContent!;
    expect(help()).toContain(`at most ${(200_000).toLocaleString()} tokens`);
    expect(help()).toContain("100,000 for Claude Haiku 5.5");
    expect(help()).not.toContain("much more of your credit");
    await choose("Context size", "1000000");
    expect(help()).toContain("uses much more of your credit");
    await choose("Context size", "65536");
    expect(help()).not.toContain("much more of your credit");
    await choose("Company or server", "openrouter");
    await choose("Context size", "");
    expect(help()).toContain(`at most ${(128_000).toLocaleString()} tokens`);
    await choose("Company or server", "ollama");
    expect(help()).toContain("the size chosen below for servers that report none");
  });
});
