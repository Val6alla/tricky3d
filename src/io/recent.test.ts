import { describe, expect, it, vi } from "vitest";
import { activate, featureById, hasSubmenu } from "../features";
import { recentRows, useRecent } from "./recent";

describe("File › Recent files", () => {
  it("lists the files newest first by name, a shared name with its folder, the path as the hint", () => {
    const open = vi.fn();
    const rows = recentRows(
      [
        { path: String.raw`D:\Ships\Bismarck\deck.3mf`, kind: "project" },
        { path: "/home/me/parts/gun.stl", kind: "mesh" },
        { path: String.raw`D:\Ships\Hood\deck.3mf`, kind: "project" },
      ],
      open,
    );
    expect(rows.map((r) => r.label)).toEqual(["deck.3mf (Bismarck)", "gun.stl", "deck.3mf (Hood)"]);
    expect(rows.map((r) => r.hint)).toEqual([String.raw`D:\Ships\Bismarck\deck.3mf`, "/home/me/parts/gun.stl", String.raw`D:\Ships\Hood\deck.3mf`]);
    expect(rows.every((r) => r.status === "ready" && r.tab === "file")).toBe(true);
    rows[1].run?.();
    expect(open).toHaveBeenCalledWith("/home/me/parts/gun.stl");
  });

  it("says so when there are none", () => {
    const rows = recentRows([]);
    expect(rows).toHaveLength(1);
    expect(rows[0].label).toBe("No recent files");
    expect(rows[0].run).toBeUndefined();
  });

  it("is a submenu of the File menu built from the list as it is when it opens", () => {
    const recent = featureById("file.recent")!;
    expect(hasSubmenu(recent)).toBe(true);
    useRecent.setState({ files: [{ path: "/a/b.obj", kind: "mesh" }] });
    expect(recent.submenu?.().map((r) => (r === "-" ? r : r.label))).toEqual(["b.obj"]);
    useRecent.setState({ files: [] });
    expect(recent.submenu?.().map((r) => (r === "-" ? r : r.label))).toEqual(["No recent files"]);
  });
});

describe("File › Autosave", () => {
  it("has Off and every 2, 5 and 10 minutes, the one set ticked", async () => {
    const { useStore } = await import("../store");
    const ids = ["file.autosave-off", "file.autosave-2", "file.autosave-5", "file.autosave-10"];
    activate(featureById("file.autosave-2")!);
    expect(useStore.getState().autosaveMinutes).toBe(2);
    expect(ids.map((id) => featureById(id)!.isActive!(useStore.getState()))).toEqual([false, true, false, false]);
    activate(featureById("file.autosave-off")!);
    expect(useStore.getState().autosaveMinutes).toBe(0);
  });
});
