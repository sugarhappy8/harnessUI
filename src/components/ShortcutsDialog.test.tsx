// The keyboard shortcut sheet, with Studio's viewer keys (0.1.0 Task B9).
import { afterEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, render, screen, within } from "@testing-library/react";

vi.mock("../lib/ipc", () => ({ isTauri: () => false }));

const { ShortcutsDialog } = await import("./ShortcutsDialog");

afterEach(cleanup);

describe("ShortcutsDialog", () => {
  it("lists the keys of Studio's viewer", async () => {
    await act(async () => {
      render(<ShortcutsDialog open onOpenChange={() => undefined} />);
    });
    const viewer = screen.getByRole("table", { name: "In Studio's viewer" });
    const rows = within(viewer)
      .getAllByRole("row")
      .map((row) => row.textContent);
    expect(rows).toEqual([
      "← →Previous or next result",
      "FStar or unstar",
      "RReuse all (Shift R: without the seed)",
      "AAnimate a picture",
      "SSave as…",
      "CCopy a picture",
      "DelDelete",
      "SpacePlay or pause a video",
      "EscClose the viewer (never stops a Studio job)",
    ]);
  });
});
