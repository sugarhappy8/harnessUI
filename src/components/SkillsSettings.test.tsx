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
});
