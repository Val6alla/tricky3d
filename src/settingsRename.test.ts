import { describe, expect, it } from "vitest";
import { renameOldSettings } from "./settingsRename";

function storage(entries: Record<string, string>) {
  const data = new Map(Object.entries(entries));
  return {
    data,
    get length() {
      return data.size;
    },
    key: (i: number) => [...data.keys()][i] ?? null,
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => void data.set(key, value),
    removeItem: (key: string) => void data.delete(key),
  };
}

describe("settings saved before the rename", () => {
  it("move to their new keys", () => {
    const s = storage({ "meshbench.sidebarWidth": "333", "meshbench.showPlatform": "1", other: "x" });
    renameOldSettings(s);
    expect(Object.fromEntries(s.data)).toEqual({ "tricky3d.sidebarWidth": "333", "tricky3d.showPlatform": "1", other: "x" });
  });

  it("never replace a setting saved under the new name", () => {
    const s = storage({ "meshbench.hideSoon": "1", "tricky3d.hideSoon": "0" });
    renameOldSettings(s);
    expect(Object.fromEntries(s.data)).toEqual({ "tricky3d.hideSoon": "0" });
  });

  it("change nothing the second time", () => {
    const s = storage({ "meshbench.importPlacement": "always" });
    renameOldSettings(s);
    renameOldSettings(s);
    expect(Object.fromEntries(s.data)).toEqual({ "tricky3d.importPlacement": "always" });
  });
});
