import { describe, expect, it, beforeEach } from "vitest";
import { applyTheme, loadTheme } from "./theme";

describe("theme", () => {
  beforeEach(() => {
    window.localStorage.clear();
    document.documentElement.removeAttribute("data-theme");
  });

  it("defaults to system", () => {
    expect(loadTheme()).toBe("system");
  });

  it("applies and persists an explicit choice", () => {
    applyTheme("dark");
    expect(document.documentElement.getAttribute("data-theme")).toBe("dark");
    expect(loadTheme()).toBe("dark");
  });

  it("system removes the attribute so the OS setting applies", () => {
    applyTheme("light");
    applyTheme("system");
    expect(document.documentElement.hasAttribute("data-theme")).toBe(false);
  });
});
