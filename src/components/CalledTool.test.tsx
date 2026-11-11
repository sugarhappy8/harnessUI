import { describe, expect, it } from "vitest";
import type { ReactNode } from "react";
import type { ToolCallView } from "./ToolCallCard";
import { calledTool, calledToolExtra } from "./CalledTool";

function call(over: Partial<ToolCallView> = {}): ToolCallView {
  return {
    callId: "c1",
    name: "call_tool",
    arguments: { name: "create_artifact", arguments: { title: "Plan", kind: "markdown", content: "# Plan" } },
    recovered: false,
    status: "ok",
    text: 'Artifact #3 "Plan" (markdown), version 1 saved.',
    ...over,
  };
}

describe("a call made through call_tool", () => {
  it("stands for the tool it names, with that tool's arguments and the same result", () => {
    const target = calledTool(call());
    expect(target).toEqual({
      ...call(),
      name: "create_artifact",
      arguments: { title: "Plan", kind: "markdown", content: "# Plan" },
    });
    // Arguments left out: none.
    expect(calledTool(call({ arguments: { name: " git_status " } }))).toMatchObject({
      name: "git_status",
      arguments: {},
    });
    expect(calledTool(call({ arguments: { name: "git_status", arguments: [1, 2] } }))?.arguments).toEqual({});
  });

  it("is nothing else for any other call, or without a usable name", () => {
    expect(calledTool(call({ name: "read_file", arguments: { name: "x" } }))).toBeNull();
    expect(calledTool(call({ arguments: {} }))).toBeNull();
    expect(calledTool(call({ arguments: { name: 7 } }))).toBeNull();
    expect(calledTool(call({ arguments: { name: "  " } }))).toBeNull();
    // It does not stand for itself.
    expect(calledTool(call({ arguments: { name: "call_tool", arguments: { name: "read_file" } } }))).toBeNull();
  });

  it("shows on its card what the tool it ran shows on its own", () => {
    const seen: ToolCallView[] = [];
    const extras: Record<string, (c: ToolCallView) => ReactNode> = {
      create_artifact: (c) => {
        seen.push(c);
        return `open ${String(c.arguments.title)}`;
      },
    };
    expect(calledToolExtra(call(), extras)).toBe("open Plan");
    expect(seen).toHaveLength(1);
    expect(seen[0]?.name).toBe("create_artifact");
    expect(seen[0]?.text).toContain("Artifact #3");
    // A tool that shows nothing of its own.
    expect(calledToolExtra(call({ arguments: { name: "git_status" } }), extras)).toBeNull();
    expect(calledToolExtra(call({ name: "read_file" }), extras)).toBeNull();
  });

  it("never takes a name the model wrote for something every object has", () => {
    const extras: Record<string, (c: ToolCallView) => ReactNode> = { create_artifact: () => "open" };
    for (const name of ["constructor", "toString", "__proto__", "hasOwnProperty", "valueOf"]) {
      expect(calledToolExtra(call({ arguments: { name } }), extras)).toBeNull();
    }
  });
});
