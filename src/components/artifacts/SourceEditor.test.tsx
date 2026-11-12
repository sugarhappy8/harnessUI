import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { SourceEditor } from "./SourceEditor";

function Editor(props: { initial: string }) {
  const [text, setText] = useState(props.initial);
  return <SourceEditor label="Source" value={text} onChange={setText} />;
}

describe("SourceEditor", () => {
  afterEach(cleanup);

  it("Tab puts two spaces where the caret is, and the caret after them", () => {
    render(<Editor initial={"ab\ncd"} />);
    const box = screen.getByLabelText("Source") as HTMLTextAreaElement;
    box.setSelectionRange(1, 1);
    const typed = fireEvent.keyDown(box, { key: "Tab" });
    expect(typed).toBe(false); // the key did not move the focus on
    expect(box.value).toBe("a  b\ncd");
    expect(box.selectionStart).toBe(3);
    expect(box.selectionEnd).toBe(3);
  });

  it("Tab replaces what is selected", () => {
    render(<Editor initial={"abcdef"} />);
    const box = screen.getByLabelText("Source") as HTMLTextAreaElement;
    box.setSelectionRange(2, 4);
    fireEvent.keyDown(box, { key: "Tab" });
    expect(box.value).toBe("ab  ef");
  });

  it("leaves Shift+Tab alone, and Tab after Escape, so the keyboard can leave the box", () => {
    render(<Editor initial={"ab"} />);
    const box = screen.getByLabelText("Source") as HTMLTextAreaElement;
    expect(fireEvent.keyDown(box, { key: "Tab", shiftKey: true })).toBe(true);
    fireEvent.keyDown(box, { key: "Escape" });
    expect(fireEvent.keyDown(box, { key: "Tab" })).toBe(true);
    expect(box.value).toBe("ab");
    // Back in the box, Tab indents again.
    fireEvent.blur(box);
    expect(fireEvent.keyDown(box, { key: "Tab" })).toBe(false);
  });

  it("passes on what is typed", () => {
    render(<Editor initial={"a"} />);
    const box = screen.getByLabelText("Source") as HTMLTextAreaElement;
    fireEvent.change(box, { target: { value: "a<b>" } });
    expect(box.value).toBe("a<b>");
  });
});
