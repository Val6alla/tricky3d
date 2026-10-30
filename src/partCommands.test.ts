import { Box3 } from "three";
import { beforeEach, describe, expect, it } from "vitest";
import { runFeature } from "./features";
import { rotationAbout, multiply } from "./matrix";
import { boundsOf, makeBox, translation, type Bounds } from "./mesh";
import { useStore, type Part } from "./store";
import { createPartsLayer, PLAIN_LOOK } from "./viewport/parts";
import type { VpTheme } from "./viewport/theme";

// The part right-click menu's Duplicate and its show, hide and transparency entries, run as the menu runs them, and
// what the view then draws.

const s = () => useStore.getState();
const part = (id: string) => s().parts.find((p) => p.id === id) as Part;
const box = (id: string) => boundsOf(part(id).mesh, part(id).matrix) as Bounds;
const labels = () => s().history.past.map((e) => e.label);
const lastLabel = () => labels()[labels().length - 1];
const overlap = (a: Bounds, b: Bounds) => [0, 1, 2].every((k) => a.min[k] < b.max[k] && b.min[k] < a.max[k]);

/** Three boxes: 10 mm wide at x 0…10, 50 mm wide at x 12…62, and a 4 mm one at x 0…4 further along Y. */
function threeParts() {
  return s().addParts([
    { name: "A", mesh: makeBox([10, 10, 10]), matrix: translation(5, 0, 5) },
    { name: "B", mesh: makeBox([50, 20, 4]), matrix: translation(37, 0, 2) },
    { name: "C", mesh: makeBox([4, 4, 4]), matrix: translation(2, 100, 2) },
  ]);
}

beforeEach(() => {
  s().newProject();
});

describe("Duplicate", () => {
  it("adds one copy of each selected part after the parts, named as a copy, and selects the copies", () => {
    const [a, b, c] = threeParts();
    s().select([b, a]);
    runFeature("context.duplicate");
    expect(s().parts.map((p) => p.name)).toEqual(["A", "B", "C", "A (copy)", "B (copy)"]);
    const copies = s().parts.slice(3).map((p) => p.id);
    expect(s().selection).toEqual(copies);
    expect(new Set([a, b, c, ...copies]).size).toBe(5);
    // The copies look like their originals.
    expect(s().parts.slice(3).map((p) => [p.mesh, p.color])).toEqual([
      [part(a).mesh, part(a).color],
      [part(b).mesh, part(b).color],
    ]);
  });

  it("does nothing with nothing selected", () => {
    threeParts();
    s().clearSelection();
    const steps = labels();
    runFeature("context.duplicate");
    expect(s().parts).toHaveLength(3);
    expect(labels()).toEqual(steps);
  });

  it("puts a copy beside its original: its width plus 5 mm along +X, turned and lifted the same", () => {
    // A 20 × 10 × 4 box turned 90° about Z: 10 mm wide along X.
    const matrix = multiply(translation(50, 60, 2), rotationAbout(2, Math.PI / 2));
    const [a] = s().addParts([{ name: "A", mesh: makeBox([20, 10, 4]), matrix }]);
    expect(box(a).max[0] - box(a).min[0]).toBeCloseTo(10);
    const [copy] = s().duplicateParts([a]);
    expect(part(copy).matrix).toEqual(matrix.map((v, i) => (i === 12 ? v + 15 : v)));
    expect(box(copy).min[0]).toBeCloseTo(box(a).max[0] + 5);
    expect(box(copy).min[1]).toBeCloseTo(box(a).min[1]);
    expect(box(copy).min[2]).toBeCloseTo(box(a).min[2]);
  });

  it("keeps several parts' layout and puts the copies beside the whole group, overlapping no original and no other copy", () => {
    const [a, b, c] = threeParts();
    s().select([a, b, c]);
    runFeature("context.duplicate");
    const copies = s().selection;
    const originals = [a, b, c];
    // The group spans x 0…62: every copy moves 62 + 5 mm along +X, so they keep their places relative to each other.
    copies.forEach((id, i) => expect(part(id).matrix).toEqual(part(originals[i]).matrix.map((v, k) => (k === 12 ? v + 67 : v))));
    for (const copy of copies) for (const other of [...originals, ...copies]) if (other !== copy) expect([copy, other, overlap(box(copy), box(other))]).toEqual([copy, other, false]);
  });

  it("makes copies that change on their own, without their original", () => {
    const [a] = threeParts();
    const measured = [{ id: "m1", tool: "point-point", features: [], axes: [false, false, false] as [boolean, boolean, boolean], diameter: false }];
    s().setMeasurements(new Map([[a, measured]]), "Measure");
    s().setPartLocked(a, true);
    s().patchParts([a], { packing: { priority: 8, rotation: "none" } }, "Packing settings");
    const [copy] = s().duplicateParts([a]);
    // The original's measurements stay with it; the copy packs as the original does, its lock included.
    expect(part(copy).measurements).toBeUndefined();
    expect(part(copy).locked).toBe(true);
    expect(part(copy).packing).toEqual({ priority: 8, rotation: "none" });
    const original = part(a);

    s().setPartLocked(copy, false);
    s().patchParts([copy], { packing: { priority: 1, rotation: "free" } }, "Packing settings");
    s().setPartMatrices(new Map([[copy, translation(150, 150, 5)]]), "Move");
    s().renamePart(copy, "Spare");
    s().setPartsVisible([copy], false);
    s().setPartsTransparent([copy], true);
    s().patchParts([copy], { color: "#123456" }, "Colour");
    expect(part(a)).toBe(original);

    // A new mesh for the original (a repair, a cut) leaves the copy's.
    const copyMesh = part(copy).mesh;
    const [repaired] = s().replaceParts([{ remove: [a], add: [{ name: "A", mesh: makeBox([10, 10, 11]), matrix: original.matrix }] }], "Repair");
    expect(part(repaired).mesh).not.toBe(copyMesh);
    expect(part(copy).mesh).toBe(copyMesh);
    // Deleting the copy leaves the original's replacement, in the original's place.
    s().deleteParts([copy]);
    expect(s().parts.map((p) => p.id)).toEqual([repaired, ...s().parts.slice(1).map((p) => p.id)]);
    expect(s().parts.map((p) => p.name)).toEqual(["A", "B", "C"]);
  });

  it("is one undo step, and redo brings the same copies back", () => {
    const [a, b] = threeParts();
    s().select([a, b]);
    runFeature("context.duplicate");
    const copies = s().selection;
    const after = s().parts;
    expect(lastLabel()).toBe("Duplicate");
    s().undo();
    expect(s().parts.map((p) => p.name)).toEqual(["A", "B", "C"]);
    expect(s().selection).toEqual([a, b]);
    s().redo();
    expect(s().parts).toBe(after);
    expect(s().selection).toEqual(copies);
  });
});

/** Every token reads as one grey: the colours are not under test. */
const theme = new Proxy({}, { get: () => "#808080" }) as VpTheme;

/** What the view draws for the parts now: each part's root, its surface material, and what a click can hit. */
function drawn() {
  const layer = createPartsLayer(theme);
  layer.sync(s().parts, s().selection, "shaded", PLAIN_LOOK);
  const roots = new Map(layer.group.children.map((root) => [root.children.find((c) => c.userData.partId)?.userData.partId as string, root]));
  const picks = new Set(layer.pickTargets().map((o) => o.userData.partId as string));
  const look = (id: string) => {
    const root = roots.get(id);
    const front = root?.children[0] as unknown as { material: { transparent: boolean; opacity: number; depthWrite: boolean } };
    return { shown: root?.visible, clickable: picks.has(id), seeThrough: front.material.transparent, opacity: front.material.opacity };
  };
  const extent = layer.visibleBox(new Box3());
  layer.dispose();
  return { look, extent };
}

describe("Show and hide", () => {
  it("hides the selected parts, then shows them again, one undo step each; hidden parts are not drawn or clickable", () => {
    const [a, b, c] = threeParts();
    s().select([a, b]);
    runFeature("context.toggle-visible");
    expect(s().parts.map((p) => p.visible)).toEqual([false, false, true]);
    expect(lastLabel()).toBe("Hide");
    let view = drawn();
    expect([view.look(a), view.look(b), view.look(c)].map((l) => [l.shown, l.clickable])).toEqual([
      [false, false],
      [false, false],
      [true, true],
    ]);
    // Zoom to all and the rulers' extent only count what is drawn.
    expect(view.extent.min.y).toBeCloseTo(98);

    runFeature("context.toggle-visible");
    expect(s().parts.map((p) => p.visible)).toEqual([true, true, true]);
    expect(labels().slice(-2)).toEqual(["Hide", "Show"]);
    view = drawn();
    expect(view.look(a)).toMatchObject({ shown: true, clickable: true });
    s().undo();
    expect(s().parts.map((p) => p.visible)).toEqual([false, false, true]);
  });

  it("shows a selection that is partly hidden, rather than hiding the rest of it", () => {
    const [a, b] = threeParts();
    s().setPartsVisible([a], false);
    s().select([a, b]);
    runFeature("context.toggle-visible");
    expect(s().parts.map((p) => p.visible)).toEqual([true, true, true]);
  });

  it("makes the selected parts see-through and solid again, and a see-through part can still be clicked", () => {
    const [a, b, c] = threeParts();
    s().select([a]);
    runFeature("context.toggle-transparent");
    expect(s().parts.map((p) => p.transparent)).toEqual([true, false, false]);
    expect(lastLabel()).toBe("Make transparent");
    const view = drawn();
    expect(view.look(a)).toEqual({ shown: true, clickable: true, seeThrough: true, opacity: 0.35 });
    expect(view.look(b)).toMatchObject({ seeThrough: false, opacity: 1 });

    // A mixed selection turns see-through as a whole first.
    s().select([a, b, c]);
    runFeature("context.toggle-transparent");
    expect(s().parts.map((p) => p.transparent)).toEqual([true, true, true]);
    runFeature("context.toggle-transparent");
    expect(s().parts.map((p) => p.transparent)).toEqual([false, false, false]);
    expect(labels().slice(-3)).toEqual(["Make transparent", "Make transparent", "Make solid"]);
    expect(drawn().look(a)).toMatchObject({ seeThrough: false, opacity: 1 });
  });

  it("hides every part that is not selected, in one undo step, and records nothing when they already are", () => {
    const [a, b, c] = threeParts();
    s().setPartsTransparent([c], true);
    s().select([b]);
    runFeature("context.hide-unselected");
    expect(s().parts.map((p) => p.visible)).toEqual([false, true, false]);
    // Hiding leaves the see-through setting for when the part shows again.
    expect(part(c).transparent).toBe(true);
    expect(lastLabel()).toBe("Hide");
    const view = drawn();
    expect([a, b, c].map((id) => view.look(id).shown)).toEqual([false, true, false]);
    const steps = labels().length;
    runFeature("context.hide-unselected");
    expect(labels()).toHaveLength(steps);
    s().undo();
    expect(s().parts.map((p) => p.visible)).toEqual([true, true, true]);
  });

  it("shows every part solid again in one undo step, which undo takes back exactly", () => {
    const [a, b, c] = threeParts();
    s().setPartsVisible([a], false);
    s().setPartsTransparent([b], true);
    s().setPartsVisible([c], false);
    s().setPartsTransparent([c], true);
    const before = s().parts;
    s().clearSelection();
    runFeature("context.show-all");
    expect(s().parts.map((p) => [p.visible, p.transparent])).toEqual([
      [true, false],
      [true, false],
      [true, false],
    ]);
    expect(lastLabel()).toBe("Show all");
    const view = drawn();
    expect([a, b, c].map((id) => [view.look(id).shown, view.look(id).seeThrough])).toEqual([
      [true, false],
      [true, false],
      [true, false],
    ]);
    const steps = labels().length;
    runFeature("context.show-all");
    expect(labels()).toHaveLength(steps);
    s().undo();
    expect(s().parts).toBe(before);
  });
});
