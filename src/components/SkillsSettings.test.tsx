import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen } from "@testing-library/react";
import type { SkillList } from "../lib/types";

const ipcMock = vi.hoisted(() => ({
  listSkills: vi.fn<() => Promise<SkillList>>(),
}));
vi.mock("../lib/ipc", () => ipcMock);

const { SkillsSettings } = await import("./SkillsSettings");

describe("Settings → Skills", () => {
  afterEach(cleanup);

  // A skill folder from somewhere else can be named with a right-to-left override; its path
  // must not read reversed.
  it("shows folder paths with hidden and right-to-left characters as codes", async () => {
    ipcMock.listSkills.mockResolvedValue({
      skills: [],
      problems: [{ path: "/home/u/.agents/skills/X-\u202Elmth.gnp", error: "name must be 1–64 characters" }],
      sharedEnabled: true,
      sharedPath: "/home/u/.agents/\u202Eslliks",
      folder: "/home/u/skills",
    });
    await act(async () => {
      render(<SkillsSettings />);
    });
    const problem = screen.getByText("/home/u/.agents/skills/X-\\u{202E}lmth.gnp");
    expect(problem.tagName).toBe("BDI");
    expect(problem.getAttribute("dir")).toBe("ltr");
    const shared = screen.getByText("/home/u/.agents/\\u{202E}slliks");
    expect(shared.tagName).toBe("BDI");
    expect(new XMLSerializer().serializeToString(document.body)).not.toContain("\u202E");
  });

  // A broken SKILL.md gives a YAML error of several lines with a snippet of the file
  // (tools::skills::parse, SkillError::Yaml). Its lines stay lines, not \u{000A} codes, while
  // a right-to-left override copied from the file into the snippet still shows as a code.
  it("keeps the lines of a YAML error and shows hidden characters in it as codes", async () => {
    const error =
      "the frontmatter is not valid YAML: error: line 3 column 4: illegal placement of ':' indicator\n" +
      " --> <input>:3:4\r\n" +
      "  |\n" +
      "1 | name: a\u202Etxt\n" +
      "2 | description: [unclosed\n" +
      "3 | foo:\t: :\n" +
      "  |    ^ illegal placement of ':' indicator";
    ipcMock.listSkills.mockResolvedValue({
      skills: [],
      problems: [{ path: "/home/u/skills/a", error }],
      sharedEnabled: false,
      sharedPath: "/home/u/.agents/skills",
      folder: "/home/u/skills",
    });
    await act(async () => {
      render(<SkillsSettings />);
    });
    const item = screen.getByText("/home/u/skills/a").closest("li") as HTMLElement;
    const text = item.textContent ?? "";
    expect(text).not.toContain("\\u{000A}");
    expect(text).not.toContain("\\u{000D}");
    expect(text).toContain("1 | name: a\\u{202E}txt\n2 | description: [unclosed");
    expect(text).toContain("illegal placement of ':' indicator\n --> <input>:3:4\n  |\n");
    expect(new XMLSerializer().serializeToString(item)).not.toContain("\u202E");
    // The browser keeps the line breaks rather than folding them into spaces.
    const message = screen.getByText(/the frontmatter is not valid YAML/);
    expect(message.className).toContain("whitespace-pre-line");
  });
});
