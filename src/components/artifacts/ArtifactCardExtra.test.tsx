import { afterEach, describe, expect, it } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useApp } from "../../store/app";
import type { ToolCallView } from "../ToolCallCard";
import { artifactCardExtra } from "./ArtifactCardExtra";
import { ProblemList } from "./ProblemList";
import { useArtifacts } from "./state";

function call(text: string): ToolCallView {
  return { callId: "c1", name: "create_artifact", arguments: {}, recovered: false, status: "ok", text };
}

describe("the card of a call that saved an artifact", () => {
  afterEach(() => {
    cleanup();
    useApp.setState({ activeChatId: null, workbench: { open: false, tab: "task" } });
    useArtifacts.setState({ focus: null });
  });

  it("shows nothing for a call that saved nothing", () => {
    expect(artifactCardExtra(call(""))).toBeNull();
    expect(artifactCardExtra(call("No artifact #3 in this chat."))).toBeNull();
    expect(artifactCardExtra(call("The title is empty. Give the artifact a short title."))).toBeNull();
  });

  it("opens the panel on the artifact and the version the call saved", () => {
    useApp.setState({ activeChatId: 5, workbench: { open: false, tab: "task" } });
    render(<>{artifactCardExtra(call('Artifact #3 "Tip <b>calculator</b>" (html), version 2 saved.\nRendered without errors.'))}</>);
    // The title is the model's: it is text.
    expect(screen.getByText(/Tip <b>calculator<\/b>/)).toBeTruthy();
    expect(document.querySelector("b")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open artifact" }));
    expect(useApp.getState().workbench).toEqual({ open: true, tab: "artifacts" });
    expect(useArtifacts.getState().focus).toMatchObject({ chatId: 5, id: 3, version: 2 });
  });
});

describe("the problems above a preview", () => {
  afterEach(cleanup);

  it("shows them as text and writes them into the message box when asked", () => {
    const composed: string[] = [];
    const onCompose = (e: Event) => composed.push(String((e as CustomEvent<unknown>).detail));
    window.addEventListener("app:compose", onCompose);
    render(
      <ProblemList
        title="Tips"
        id={3}
        version={2}
        problems={[
          { type: "error", message: "<img src=x onerror=alert(1)>", line: 14, col: 9 },
          { type: "console", message: "failed", line: null, col: null },
        ]}
      />,
    );
    expect(screen.getByText("The preview reported 2 problems")).toBeTruthy();
    expect(screen.getByText("14:9 <img src=x onerror=alert(1)>")).toBeTruthy();
    expect(document.querySelector("img")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Ask the model to fix these" }));
    window.removeEventListener("app:compose", onCompose);
    expect(composed).toEqual([
      'The preview of "Tips" (artifact #3, version 2) reported these problems:\n' +
        "- 14:9 <img src=x onerror=alert(1)>\n" +
        "- console.error: failed\n" +
        "Please fix it and save a corrected version of the artifact.",
    ]);
  });

  it("is not there when there is nothing to show", () => {
    const { container } = render(<ProblemList title="T" id={1} version={1} problems={[]} />);
    expect(container.firstChild).toBeNull();
  });
});
