import { afterEach, describe, expect, it, vi } from "vitest";
import { makeBox } from "../mesh";
import { useStore } from "../store";
import { autosaveDue, autosaveNow, recoveryQuestion, resetAutosave } from "./autosave";

afterEach(() => {
  vi.unstubAllGlobals();
  resetAutosave();
  useStore.getState().newProject();
});

describe("autosave", () => {
  it("is due only for a project with parts and unsaved changes that changed since the last one", () => {
    const s = useStore.getState();
    expect(autosaveDue(s, null)).toBe(false); // empty
    s.addParts([{ name: "a", mesh: makeBox([10, 10, 10]) }]);
    const changed = useStore.getState();
    expect(autosaveDue(changed, null)).toBe(true);
    expect(autosaveDue(changed, { parts: changed.parts, platform: changed.platform })).toBe(false);
    // Saved: the user's file holds it all.
    changed.markSaved("/p/a.3mf", "a");
    expect(autosaveDue(useStore.getState(), null)).toBe(false);
  });

  it("writes once per change, and again after the next change", async () => {
    const write = vi.fn(async () => {});
    useStore.getState().addParts([{ name: "a", mesh: makeBox([10, 10, 10]) }]);
    expect(await autosaveNow(write)).toBe(true);
    expect(await autosaveNow(write)).toBe(false);
    expect(write).toHaveBeenCalledTimes(1);
    const [platform, meshes] = write.mock.calls[0] as unknown as [number[], { name: string }[]];
    expect(platform).toEqual(useStore.getState().platform);
    expect(meshes.map((m) => m.name)).toEqual(["a"]);
    useStore.getState().addParts([{ name: "b", mesh: makeBox([5, 5, 5]) }]);
    expect(await autosaveNow(write)).toBe(true);
    expect(write).toHaveBeenCalledTimes(2);
  });

  it("tries again at the next interval after a failed write", async () => {
    // The failure is said in a toast, which times itself out.
    vi.stubGlobal("window", { setTimeout: () => 0, clearTimeout: () => {} });
    const write = vi.fn().mockRejectedValueOnce("Could not save: the disk is full.").mockResolvedValue(undefined);
    useStore.getState().addParts([{ name: "a", mesh: makeBox([10, 10, 10]) }]);
    expect(await autosaveNow(write)).toBe(false);
    expect(await autosaveNow(write)).toBe(true);
  });

  it("offers a crashed run's autosave with its time, and its date when it is from another day", () => {
    const now = new Date(2026, 10, 2, 9, 0);
    const today = new Date(2026, 10, 2, 8, 41).getTime();
    expect(recoveryQuestion({ id: "x", savedAt: today }, now)).toMatch(/did not close properly last time\. Restore the project it saved automatically at .*41/);
    const earlier = new Date(2026, 9, 30, 23, 5).getTime();
    expect(recoveryQuestion({ id: "x", savedAt: earlier }, now)).toMatch(/ on .* at /);
  });
});
