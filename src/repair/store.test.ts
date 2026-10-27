import { describe, expect, it } from "vitest";
import { DEFAULT_HIGHLIGHT, DEFAULT_VIEW, useRepair } from "./store";

describe("repair store", () => {
  it("starts with the designed defaults", () => {
    const s = useRepair.getState();
    expect(s.session).toBeNull();
    expect(s.busy).toBeNull();
    expect(s.catalog).toBeNull();
    expect(s.selectTool).toBe("triangle");
    expect(s.panelTab).toBe("status");
    expect(s.autoUpdate).toBe(false);
    expect(s.shellsAutoSelect).toBe(true);
    expect(s.highlight).toEqual({ holes: true, triangles: true, edgesFrom: null, degenerate: true, errors: true });
    expect(s.view).toEqual({ slimColored: false, slimEdges: false, slimAngle: 10, surfaceTolerance: 180 });
  });

  it("changes settings with plain setters, keeping the fields a patch leaves out", () => {
    const s = () => useRepair.getState();
    s().setSelectTool("surface");
    s().setPanelTab("shells");
    s().setAutoUpdate(true);
    s().setShellsAutoSelect(false);
    s().setEditingScriptId("builtin.extended");
    s().setHighlight({ edgesFrom: 30, holes: false });
    s().setView({ slimAngle: 15 });
    expect(s()).toMatchObject({ selectTool: "surface", panelTab: "shells", autoUpdate: true, shellsAutoSelect: false, editingScriptId: "builtin.extended" });
    expect(s().highlight).toEqual({ ...DEFAULT_HIGHLIGHT, edgesFrom: 30, holes: false });
    expect(s().view).toEqual({ ...DEFAULT_VIEW, slimAngle: 15 });
    expect(DEFAULT_VIEW.slimAngle).toBe(10);
  });
});
