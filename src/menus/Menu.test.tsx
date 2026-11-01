import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { featureById, type Feature } from "../features";
import { makeBox } from "../mesh";
import { useRepair } from "../repair/store";
import type { RepairSession } from "../repair/types";
import { useUi } from "../ui";
import { closeMenuOnRepairChange, isRepairMenu } from "./ContextMenu";
import { MenuItem } from "./Menu";

// Server rendering reads each store's initial state: not repairing, no triangle selected, no context menu open.

const feature = (id: string): Feature => {
  const f = featureById(id);
  if (!f) throw new Error(`no feature ${id}`);
  return f;
};

const item = (id: string) => renderToStaticMarkup(<MenuItem f={feature(id)} open={false} onHover={() => {}} onChoose={() => {}} />);

describe("MenuItem", () => {
  it("greys out an entry that needs selected triangles, with the reason in its tooltip", () => {
    const html = item("repair-ctx.expand");
    expect(html).toContain('class="menu-item is-disabled"');
    expect(html).toContain('aria-disabled="true"');
    expect(html).toContain('title="Expand selection (=) — Select triangles first."');
    expect(html).toContain('<span class="menu-shortcut">=</span>');
  });

  it("greys out a this-triangle entry when the menu did not open on a triangle", () => {
    const html = item("repair-ctx.add-shell");
    expect(html).toContain('class="menu-item is-disabled"');
    expect(html).toContain('title="Add this shell to selection — Right-click a triangle to use this."');
  });

  it("leaves an entry that can run as it is, and a Soon entry with its pill", () => {
    const html = item("repair-ctx.close-all");
    expect(html).toContain('class="menu-item"');
    expect(html).not.toContain("aria-disabled");
    expect(html).toContain('title="Close all holes — Fill every hole."');
    const soon = item("repair-ctx.bridge-gap");
    expect(soon).toContain('class="menu-item is-soon"');
    expect(soon).toContain('<span class="soon-pill">Soon</span>');
    expect(soon).not.toContain("aria-disabled");
  });
});

describe("isRepairMenu", () => {
  it("is the repair menu in repair mode when the menu opened on the working copy, and the part menu otherwise", () => {
    const at = { x: 10, y: 20, point: null };
    expect(isRepairMenu({ ...at, triangle: 5 }, true)).toBe(true);
    expect(isRepairMenu({ ...at, triangle: null }, true)).toBe(true);
    // Opened on a part in the tree: no triangle.
    expect(isRepairMenu(at, true)).toBe(false);
    expect(isRepairMenu({ ...at, triangle: 5 }, false)).toBe(false);
  });
});

describe("closeMenuOnRepairChange", () => {
  it("closes the open menu when repair mode starts or ends, and whenever a run or an undo replaces the working copy", () => {
    const mesh = makeBox([20, 20, 20]);
    const session: RepairSession = {
      partId: "part-1",
      partName: "Part",
      initialMesh: mesh,
      mesh,
      selection: new Uint8Array(12),
      selectedCount: 0,
      past: [],
      future: [],
      status: null,
      highlights: null,
      intersecting: null,
      lastRun: null,
      hiddenTriangles: null,
    };
    const menu = { x: 10, y: 20, point: null, triangle: 4 };
    const shown = () => useUi.getState().contextMenu;
    const stop = closeMenuOnRepairChange();
    try {
      useUi.getState().openContextMenu(menu);
      useRepair.setState({ session });
      expect(shown()).toBeNull();

      // The same working copy: a new selection or a run starting leaves the menu open.
      useUi.getState().openContextMenu(menu);
      const selection = new Uint8Array(12);
      selection[4] = 1;
      useRepair.setState({ session: { ...session, selection, selectedCount: 1 } });
      useRepair.setState({ busy: { job: 1, label: "Close all holes", progress: null, step: null, detail: null, local: null, cancelling: false } });
      expect(shown()).toEqual(menu);
      // The run's result: triangle 4 of the new mesh is another triangle, so the menu goes.
      useRepair.setState({ session: { ...session, mesh: makeBox([10, 10, 10]) }, busy: null });
      expect(shown()).toBeNull();

      useUi.getState().openContextMenu(menu);
      useRepair.setState({ session: null });
      expect(shown()).toBeNull();
    } finally {
      stop();
      useRepair.setState({ session: null, busy: null });
      useUi.setState({ contextMenu: null });
    }
  });
});
