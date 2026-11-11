import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, within } from "@testing-library/react";
import type { ChatDetail, MessageDto } from "../../lib/types";
import type { Source } from "../sources/types";

const apiMock = vi.hoisted(() => ({
  listSources: vi.fn<(chatId: number) => Promise<Source[]>>(),
  onSourcesChanged: vi.fn(async () => () => undefined),
}));
vi.mock("../sources/api", () => apiMock);

const { SourcesTab } = await import("./SourcesTab");
const { resetSources, useSources } = await import("../sources/state");
const { useApp } = await import("../../store/app");

function source(over: Partial<Source> = {}): Source {
  return {
    n: 1,
    url: "https://docs.rs/tokio/1.38.0/tokio/task/fn.spawn_blocking.html",
    title: "spawn_blocking in tokio::task - Rust",
    site: "docs.rs",
    snippet: "",
    excerpt: "Runs the provided closure on a thread where blocking is acceptable.\n\n…\n\nClosures spawned using spawn_blocking cannot be cancelled abruptly.",
    contentSha256: "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
    official: true,
    versionNote: "matches tokio 1.38",
    queries: ["tokio spawn_blocking"],
    fetchedAt: Date.UTC(2026, 10, 11, 12, 30),
    createdAt: Date.UTC(2026, 10, 11, 12, 30),
    ...over,
  };
}

function answer(id: number, text: string): MessageDto {
  return {
    id,
    parentId: null,
    role: "assistant",
    text,
    reasoning: null,
    status: "complete",
    modelRef: null,
    promptTokens: null,
    completionTokens: null,
    tokensPerSecond: null,
    error: null,
    createdAt: 0,
    siblingIds: [id],
    imageIds: [],
    documents: [],
    toolCalls: [],
    toolResult: null,
  } as MessageDto;
}

function chat(messages: MessageDto[]): ChatDetail {
  return { chat: { id: 7 } as ChatDetail["chat"], messages };
}

async function show(sources: Source[], messages: MessageDto[] = []) {
  apiMock.listSources.mockResolvedValue(sources);
  useApp.setState({ detail: chat(messages), activeChatId: 7 });
  let result!: ReturnType<typeof render>;
  await act(async () => {
    result = render(<SourcesTab chatId={7} />);
  });
  return result;
}

describe("SourcesTab", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetSources();
  });
  afterEach(() => {
    cleanup();
    useApp.setState({ detail: null, activeChatId: null });
  });

  it("says what will appear when the chat has no sources", async () => {
    await show([]);
    expect(screen.getByText(/Pages a web search retrieved for this chat appear here/)).toBeTruthy();
    expect(apiMock.listSources).toHaveBeenCalledWith(7);
  });

  it("shows each source with what the model was given of it", async () => {
    await show([
      source(),
      source({
        n: 2,
        url: "http://intranet.example/wiki",
        title: "Team wiki",
        site: "intranet.example",
        snippet: "Internal notes about our Tokio setup.",
        excerpt: "",
        contentSha256: null,
        official: false,
        versionNote: null,
        queries: ["tokio spawn_blocking", "tokio wiki"],
        fetchedAt: null,
      }),
    ]);
    const first = within(screen.getByRole("article", { name: "Source 1" }));
    expect(first.getByText("spawn_blocking in tokio::task - Rust")).toBeTruthy();
    expect(first.getByText("docs.rs")).toBeTruthy();
    expect(first.getByText("official documentation")).toBeTruthy();
    expect(first.getByText("matches tokio 1.38")).toBeTruthy();
    expect(first.getByText("https://docs.rs/tokio/1.38.0/tokio/task/fn.spawn_blocking.html")).toBeTruthy();
    expect(first.getByText("tokio spawn_blocking")).toBeTruthy();
    expect(first.getByText("9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08")).toBeTruthy();
    expect(first.getByText(/Runs the provided closure on a thread where blocking is acceptable\./)).toBeTruthy();
    expect(first.getByText("Text of the page, as given to the model")).toBeTruthy();
    expect(first.getByRole("button", { name: "Open in browser" })).toBeTruthy();
    // A page that was not read: its summary, no hash, and that it was not read.
    const second = within(screen.getByRole("article", { name: "Source 2" }));
    expect(second.getByText("Internal notes about our Tokio setup.")).toBeTruthy();
    expect(second.getByText(/not read: the model was given only the search service's summary/)).toBeTruthy();
    expect(second.queryByText("official documentation")).toBeNull();
    expect(second.queryByText(/Text hash/)).toBeNull();
    expect(second.getByText("tokio spawn_blocking · tokio wiki")).toBeTruthy();
  });

  it("shows retrieved text as plain text, never as markup", async () => {
    const { container } = await show([
      source({
        title: "<img src=x onerror=alert(1)> title",
        snippet: "<script>window.__pwned = 1</script>",
        excerpt: "<b>bold</b> <a href='https://evil.example'>link</a> [click](https://evil.example)",
      }),
    ]);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("b")).toBeNull();
    expect(container.querySelector("a")).toBeNull();
    expect(screen.getByText("<img src=x onerror=alert(1)> title")).toBeTruthy();
    expect(screen.getByText(/<b>bold<\/b>/)).toBeTruthy();
  });

  it("checks the quotes of answers that cite a source against the retrieved text", async () => {
    await show(
      [source(), source({ n: 2, title: "A post", site: "blog.example.com", excerpt: "It worked well.", official: false, versionNote: null })],
      [
        answer(10, 'The docs say "runs the provided closure on a thread where blocking is acceptable" [1].'),
        answer(11, 'They also say "the closure always runs on the main thread" [1]. A post agrees [2].'),
      ],
    );
    const first = within(screen.getByRole("article", { name: "Source 1" }));
    const found = first.getByText("quote found");
    expect(found.closest("li")?.textContent).toContain("runs the provided closure on a thread where blocking is acceptable");
    const missing = first.getByText("quote not found in the retrieved text");
    expect(missing.closest("li")?.textContent).toContain("the closure always runs on the main thread");
    const second = within(screen.getByRole("article", { name: "Source 2" }));
    expect(second.getByText("Cited in one answer, without a quoted passage to check.")).toBeTruthy();
  });

  it("says when a source is not cited", async () => {
    await show([source()], [answer(10, "An answer without any citation.")]);
    expect(screen.getByText("Not cited in an answer yet.")).toBeTruthy();
  });

  it("marks the source a citation chip asked for", async () => {
    await show([source(), source({ n: 2, title: "Second" })]);
    await act(async () => {
      useSources.getState().requestFocus(7, 2);
    });
    expect(screen.getByRole("article", { name: "Source 2" }).className).toContain("border-accent");
    expect(screen.getByRole("article", { name: "Source 1" }).className).not.toContain("border-accent");
    // A request for another chat's source changes nothing here.
    await act(async () => {
      useSources.getState().requestFocus(8, 1);
    });
    expect(screen.getByRole("article", { name: "Source 1" }).className).not.toContain("border-accent");
  });
});
