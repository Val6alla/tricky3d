import { describe, expect, it } from "vitest";
import { navigationHint, selectHint, toggleKey } from "./platform";
import { selectMode } from "./store";

describe("selectMode", () => {
  const click = (keys: Partial<{ shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }>) => ({ shiftKey: false, ctrlKey: false, metaKey: false, ...keys });

  it("on macOS: Cmd toggles, Shift adds, and Control-click (the right click) never toggles", () => {
    expect(selectMode(click({}), true)).toBe("replace");
    expect(selectMode(click({ metaKey: true }), true)).toBe("toggle");
    expect(selectMode(click({ shiftKey: true }), true)).toBe("add");
    expect(selectMode(click({ ctrlKey: true }), true)).toBe("replace");
    expect(selectMode(click({ ctrlKey: true, shiftKey: true }), true)).toBe("add");
  });

  it("elsewhere: Ctrl toggles, Shift adds", () => {
    expect(selectMode(click({}), false)).toBe("replace");
    expect(selectMode(click({ ctrlKey: true }), false)).toBe("toggle");
    expect(selectMode(click({ shiftKey: true }), false)).toBe("add");
    expect(selectMode(click({ ctrlKey: true, shiftKey: true }), false)).toBe("toggle");
  });
});

describe("status-bar hints", () => {
  it("name Mac keys and trackpad gestures on macOS", () => {
    expect(toggleKey(true)).toBe("Cmd");
    expect(selectHint(true)).toBe(
      "Click a part or drag a box from empty space to select, Shift adds, Cmd toggles. Two-finger drag or right-drag orbits, Option+drag or Shift+two-finger drag pans, pinch or scroll zooms.",
    );
    expect(navigationHint(true)).not.toMatch(/Ctrl|middle/);
  });

  it("stay as they were elsewhere", () => {
    expect(toggleKey(false)).toBe("Ctrl");
    expect(selectHint(false)).toBe(
      "Click a part or drag a box from empty space to select, Shift adds, Ctrl toggles. Right-drag orbits, middle-drag pans, scroll zooms.",
    );
  });
});
