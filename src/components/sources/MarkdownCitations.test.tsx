import { describe, expect, it, vi } from "vitest";
import { fireEvent, render } from "@testing-library/react";
import { Markdown, type Citations } from "../Markdown";

function citations(onOpen = vi.fn()): Citations {
  return {
    sources: new Map([
      [1, { title: "spawn_blocking in tokio::task - Rust", site: "docs.rs" }],
      [2, { title: "", site: "blog.example.com" }],
    ]),
    onOpen,
  };
}

describe("citations in an answer", () => {
  it("shows [n] of a stored source as a chip with its title and site, and opens it on click", () => {
    const onOpen = vi.fn();
    const { container } = render(
      <Markdown text={"It runs on another thread [1]. A post agrees [2]."} citations={citations(onOpen)} />,
    );
    const chips = container.querySelectorAll("button[data-citation]");
    expect(chips).toHaveLength(2);
    expect(chips[0]!.textContent).toBe("1");
    expect(chips[0]!.getAttribute("title")).toBe("spawn_blocking in tokio::task - Rust — docs.rs");
    expect(chips[0]!.getAttribute("aria-label")).toBe("Source 1: spawn_blocking in tokio::task - Rust — docs.rs");
    expect(chips[1]!.getAttribute("title")).toBe("blog.example.com");
    fireEvent.click(chips[0]!);
    expect(onOpen).toHaveBeenCalledWith(1);
    expect(container.textContent).toBe("It runs on another thread 1. A post agrees 2.");
  });

  it("marks [n] that has no source, and keeps it as the text it is", () => {
    const { container } = render(<Markdown text={"Made up [9], real [1]."} citations={citations()} />);
    const missing = container.querySelector("[data-citation-missing]");
    expect(missing?.textContent).toBe("[9]");
    expect(missing?.getAttribute("title")).toBe("No such source");
    expect(missing?.tagName).toBe("SPAN");
    expect(missing?.className).toContain("decoration-dotted");
    expect(container.querySelectorAll("button[data-citation]")).toHaveLength(1);
    expect(container.textContent).toBe("Made up [9], real 1.");
  });

  it("leaves code and links alone", () => {
    const text = "Inline `a[1]` and a [link [1]](https://example.com).\n\n```js\nconst x = a[1];\n```\n\nCited [1].";
    const { container } = render(<Markdown text={text} citations={citations()} />);
    const chips = container.querySelectorAll("button[data-citation]");
    expect(chips).toHaveLength(1);
    expect(container.querySelector("pre")?.textContent).toContain("a[1]");
    expect(container.querySelector("code")?.textContent).toBe("a[1]");
    expect(container.querySelector("[data-citation-missing]")).toBeNull();
  });

  it("does not let the model point a citation somewhere else", () => {
    // `[1]: url` makes `[1]` a link in Markdown. In an answer it stays the citation of
    // source 1, and a number without a source stays marked.
    const text = "Claim [1]. Another [7].\n\n[1]: https://evil.example/landing\n[7]: https://evil.example/other";
    const { container } = render(<Markdown text={text} citations={citations()} />);
    expect(container.querySelectorAll("button[data-citation='1']")).toHaveLength(1);
    expect(container.querySelector("[data-citation-missing]")?.textContent).toBe("[7]");
    expect(container.textContent).not.toContain("evil.example");
  });

  it("is plain text when the chat's sources are not known", () => {
    const { container } = render(<Markdown text={"Claim [1] and [9]."} />);
    expect(container.querySelector("[data-citation]")).toBeNull();
    expect(container.querySelector("[data-citation-missing]")).toBeNull();
    expect(container.textContent).toBe("Claim [1] and [9].");
  });

  it("never turns answer text into markup", () => {
    const text = 'A <cite data-cite="1">forged</cite> chip and <button data-citation="1">x</button> [1].';
    const { container } = render(<Markdown text={text} citations={citations()} />);
    // The tags are shown as the text they are; only the real `[1]` is a chip.
    expect(container.querySelectorAll("button[data-citation]")).toHaveLength(1);
    expect(container.querySelector("cite")).toBeNull();
    expect(container.textContent).toContain('<cite data-cite="1">forged</cite>');
  });

  it("takes a list in one bracket when every number is a source", () => {
    const { container } = render(<Markdown text={"Both [1, 2]. An array [1, 5]."} citations={citations()} />);
    expect(container.querySelectorAll("button[data-citation]")).toHaveLength(2);
    expect(container.textContent).toBe("Both 12. An array [1, 5].");
  });
});
