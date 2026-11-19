// The chip that says where a chat or a Studio run goes: the chat header's Cloud mark, Studio's
// mark for a model on this computer and its chip for a cloud model are one component.
import { afterEach, describe, expect, it } from "vitest";
import { cleanup, render } from "@testing-library/react";
import type { ReactElement } from "react";
import { Cloud } from "lucide-react";
import { CloudMark } from "./CloudMark";
import { PlaceMark } from "./PlaceMark";
import { LocalMark } from "./studio/LocalMark";

const CHIP =
  "flex shrink-0 items-center gap-1 rounded-full border border-border px-1.5 text-xs whitespace-nowrap text-fg-muted";

function mark(ui: ReactElement): HTMLElement {
  return render(ui).container.firstElementChild as HTMLElement;
}

/** The chip's parts in order: the icon's class, its words, the words only a screen reader hears. */
function parts(el: HTMLElement): [string | null, string, string | null] {
  const [icon, words, hidden] = Array.from(el.childNodes);
  return [
    (icon as Element).getAttribute("class"),
    words?.textContent ?? "",
    hidden ? `${(hidden as Element).getAttribute("class")}|${hidden.textContent}` : null,
  ];
}

describe("the place mark", () => {
  afterEach(cleanup);

  it("the chat's Cloud mark is as it was", () => {
    const el = mark(<CloudMark host="api.anthropic.com" />);
    expect(el.tagName).toBe("SPAN");
    expect(el.getAttribute("data-cloud-mark")).toBe("true");
    expect(el.title).toBe("Messages and files in this chat go to api.anthropic.com.");
    expect(el.getAttribute("class")).toBe(CHIP);
    expect(el.childNodes).toHaveLength(3);
    expect(parts(el)).toEqual([
      "lucide lucide-cloud",
      "Cloud",
      "sr-only|: Messages and files in this chat go to api.anthropic.com.",
    ]);
    cleanup();
    expect(mark(<CloudMark host="" />)).toBeNull();
  });

  it("Studio's local mark is the same chip", () => {
    const el = mark(<LocalMark />);
    expect(el.getAttribute("data-local-mark")).toBe("true");
    expect(el.getAttribute("data-cloud-mark")).toBeNull();
    expect(el.title).toBe("Your prompt and pictures stay on this computer.");
    expect(el.getAttribute("class")).toBe(CHIP);
    expect(parts(el)).toEqual([
      "lucide lucide-microchip",
      "On this computer",
      "sr-only|: Your prompt and pictures stay on this computer.",
    ]);
  });

  it("names its own place, and without a tooltip says nothing more", () => {
    const el = mark(<PlaceMark mark="cloud" icon={Cloud} label="Cloud" className="ml-auto" />);
    expect(el.getAttribute("data-cloud-mark")).toBe("true");
    expect(el.hasAttribute("title")).toBe(false);
    expect(el.getAttribute("class")).toBe(`${CHIP} ml-auto`);
    expect(el.childNodes).toHaveLength(2);
    expect(parts(el)).toEqual(["lucide lucide-cloud", "Cloud", null]);
  });
});
