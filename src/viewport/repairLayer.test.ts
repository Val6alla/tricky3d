// @ts-expect-error The app's tsconfig has no Node types; vitest runs this file in Node.
import { readFileSync } from "node:fs";
import { BufferAttribute, OrthographicCamera, PerspectiveCamera, ShaderLib, Vector3, type BufferGeometry, type Material, type Mesh, type Plane } from "three";
import type { LineSegments2 } from "three/examples/jsm/lines/LineSegments2.js";
import { describe, expect, it } from "vitest";
import { makeBox } from "../mesh";
import { buildEdgeAdjacency, triangleNormals } from "../repair/selection";
import { DEFAULT_HIGHLIGHT, DEFAULT_VIEW, useRepair, type RepairState } from "../repair/store";
import type { Highlights, RepairSession } from "../repair/types";
import type { MeshData } from "../store";
import { createRepairLayer, edgesOfMask, sharpEdges, slimMask, trianglesOf, trianglesOfMask, vertexSpacing, wireIndex } from "./repairLayer";
import type { VpTheme } from "./theme";

/** Every token the viewport reads. */
const theme: VpTheme = {
  platform: "#000",
  gridMinor: "#000",
  gridMajor: "#000",
  ruler: "#000",
  selected: "#000",
  backface: "#d93a3a",
  openEdge: "#000",
  edge: "#3a4250",
  axisX: "#000",
  axisY: "#000",
  axisZ: "#000",
  gizmoCentre: "#000",
  repairHole: "#ffcf26",
  repairError: "#ff8a1c",
  repairDegenerate: "#e04ce0",
  repairSelected: "#33c25a",
  repairSlim: "#3d8bf2",
  cutPoint: "#ff5cb1",
  cutCurrent: "#d4006f",
  cutLine: "#e8202a",
  node: "#18b4f0",
  measure: "#1d6ee6",
  measureSelected: "#8b3dff",
  measurePending: "#e0368c",
  booleanMain: "#3aa655",
  booleanMainSelected: "#86e09a",
  booleanTool: "#d64545",
  booleanToolSelected: "#ff9a9a",
  surface: "#000",
  surface2: "#000",
  border: "#000",
  borderStrong: "#000",
  text: "#000",
  textMuted: "#000",
  accent: "#0b8a8f",
  font: "sans-serif",
};

const mesh = (positions: number[], indices: number[]): MeshData => ({
  positions: new Float32Array(positions),
  indices: new Uint32Array(indices),
});

/** The 20 mm box: 12 right-angled isosceles triangles (45°, 45°, 90°), 12 edges at 90° and 6 flat diagonals. */
const BOX = makeBox([20, 20, 20]);

/** Vertex pairs as sorted "a-b" strings, to compare edge sets whatever the order and direction. */
const pairs = (index: ArrayLike<number>) => {
  const out: string[] = [];
  for (let i = 0; i < index.length; i += 2) out.push(`${Math.min(index[i], index[i + 1])}-${Math.max(index[i], index[i + 1])}`);
  return out.sort();
};

describe("index subsets", () => {
  it("wireIndex lists each triangle's three edges", () => {
    const wire = wireIndex(new Uint32Array([0, 1, 2, 2, 1, 3, 9]));
    expect(Array.from(wire)).toEqual([0, 1, 1, 2, 2, 0, 2, 1, 1, 3, 3, 2]);
    expect(wireIndex(BOX.indices).length).toBe(12 * 6);
  });

  it("trianglesOf copies the named triangles' corners in order and skips ids past the mesh", () => {
    const indices = new Uint32Array([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    expect(Array.from(trianglesOf(indices, [2, 0]))).toEqual([6, 7, 8, 0, 1, 2]);
    expect(Array.from(trianglesOf(indices, new Uint32Array([1, 3, 99])))).toEqual([3, 4, 5]);
    expect(trianglesOf(indices, []).length).toBe(0);
  });

  it("trianglesOfMask and edgesOfMask follow the mask in triangle order", () => {
    const indices = new Uint32Array([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    const mask = new Uint8Array([1, 0, 1]);
    expect(Array.from(trianglesOfMask(indices, mask))).toEqual([0, 1, 2, 6, 7, 8]);
    expect(Array.from(edgesOfMask(indices, mask))).toEqual([0, 1, 1, 2, 2, 0, 6, 7, 7, 8, 8, 6]);
    expect(trianglesOfMask(indices, new Uint8Array(3)).length).toBe(0);
    // A mask longer than the mesh is cut to it.
    expect(Array.from(trianglesOfMask(indices, new Uint8Array([0, 0, 0, 1])))).toEqual([]);
  });
});

describe("vertexSpacing", () => {
  it("gives each vertex its shortest edge", () => {
    // Every box corner has 20 mm edges and longer face diagonals.
    const box = vertexSpacing(BOX.positions, BOX.indices);
    expect(Array.from(box.spacing)).toEqual(new Array(8).fill(20));
    expect(box.widest).toBe(20);
    // A long thin triangle: its two near corners are 0.5 apart, the far one is 10 away from both.
    const thin = mesh([0, 0, 0, 0.5, 0, 0, 0.25, 10, 0], [0, 1, 2]);
    const s = vertexSpacing(thin.positions, thin.indices);
    expect(s.spacing[0]).toBe(0.5);
    expect(s.spacing[1]).toBe(0.5);
    expect(s.spacing[2]).toBeCloseTo(Math.hypot(0.25, 10), 5);
    expect(s.widest).toBeCloseTo(Math.hypot(0.25, 10), 5);
  });

  it("ignores edges without length and leaves unused vertices at 0", () => {
    // Vertex 3 sits on vertex 0; vertex 4 is used by no triangle.
    const m = mesh([0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0, 0, 9, 9, 9], [0, 1, 2, 0, 3, 1, 1, 1, 2]);
    const s = vertexSpacing(m.positions, m.indices);
    expect(Array.from(s.spacing)).toEqual([2, 2, 3, 2, 0]);
    expect(vertexSpacing(new Float32Array(0), new Uint32Array(0)).widest).toBe(0);
  });
});

describe("slimMask", () => {
  const equilateral = [0, 0, 0, 2, 0, 0, 1, Math.sqrt(3), 0];
  // Smallest angle atan(0.02 / 1) ≈ 1.15°.
  const needle = [0, 0, 0, 1, 0, 0, 1, 0.02, 0];

  it("marks triangles whose smallest angle is below the setting", () => {
    const m = mesh([...equilateral, ...needle], [0, 1, 2, 3, 4, 5]);
    expect(Array.from(slimMask(m.positions, m.indices, 10))).toEqual([0, 1]);
    expect(Array.from(slimMask(m.positions, m.indices, 1))).toEqual([0, 0]);
    expect(Array.from(slimMask(m.positions, m.indices, 59.9))).toEqual([0, 1]);
    expect(Array.from(slimMask(m.positions, m.indices, 60.1))).toEqual([1, 1]);
    expect(Array.from(slimMask(m.positions, m.indices, 0))).toEqual([0, 0]);
  });

  it("finds the smallest angle at any corner", () => {
    // The box's triangles have 45° at two corners: slim from just above 45°.
    expect(slimMask(BOX.positions, BOX.indices, 44.9).every((s) => s === 0)).toBe(true);
    expect(slimMask(BOX.positions, BOX.indices, 45.1).every((s) => s === 1)).toBe(true);
    // The needle turned so that each corner in turn has the small angle.
    const m = mesh(needle, [0, 1, 2, 1, 2, 0, 2, 0, 1]);
    expect(Array.from(slimMask(m.positions, m.indices, 2))).toEqual([1, 1, 1]);
  });

  it("counts triangles without area as slim", () => {
    const m = mesh([0, 0, 0, 1, 0, 0, 2, 0, 0, 0, 1, 0], [0, 0, 1, 0, 1, 2, 0, 1, 3]);
    expect(Array.from(slimMask(m.positions, m.indices, 1))).toEqual([1, 1, 0]);
  });
});

describe("sharpEdges", () => {
  const sharp = (m: MeshData, degrees: number) => sharpEdges(m.indices, buildEdgeAdjacency(m.indices, m.positions.length / 3), triangleNormals(m), degrees);

  it("keeps the edges whose triangles meet at more than the angle", () => {
    // The box's 12 edges meet at 90°; its face diagonals are flat.
    expect(sharp(BOX, 0).length).toBe(24);
    expect(sharp(BOX, 45).length).toBe(24);
    expect(sharp(BOX, 89.9).length).toBe(24);
    expect(sharp(BOX, 90.1).length).toBe(0);
    const diagonals = new Set(["0-2", "4-6", "0-5", "3-6", "0-7", "1-6"]);
    expect(pairs(sharp(BOX, 10)).some((p) => diagonals.has(p))).toBe(false);
  });

  it("measures the surface's angle whichever way a triangle faces", () => {
    // The top face's second triangle flipped: its diagonal stays flat, the box edges stay at 90°.
    const indices = BOX.indices.slice();
    [indices[10], indices[11]] = [indices[11], indices[10]];
    const flipped = { positions: BOX.positions, indices };
    expect(pairs(sharp(flipped, 10))).toEqual(pairs(sharp(BOX, 10)));
  });

  it("leaves out border edges, non-manifold edges and triangles without area", () => {
    // Two triangles folded at 90° along 0-1, a third one on 0-1 (non-manifold), and a sliver on 1-2.
    const m = mesh([0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, -1, 0, 2, 0, 0], [0, 1, 2, 1, 0, 3, 0, 1, 4, 1, 5, 2]);
    expect(sharp(m, 10).length).toBe(0);
    // Without the third triangle on 0-1 the fold counts.
    const folded = mesh([0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1], [0, 1, 2, 1, 0, 3]);
    expect(pairs(sharp(folded, 10))).toEqual(["0-1"]);
    // A sliver with a repeated index has no normal: its edges never count.
    const sliver = mesh([0, 0, 0, 1, 0, 0, 0, 1, 0], [0, 1, 2, 1, 0, 0]);
    expect(sharp(sliver, 0).length).toBe(0);
  });
});

/** A repair session over `m`, as session.ts makes one. */
function sessionOf(m: MeshData, patch: Partial<RepairSession> = {}): RepairSession {
  return {
    partId: "part-1",
    partName: "Part",
    initialMesh: m,
    mesh: m,
    selection: new Uint8Array(m.indices.length / 3),
    selectedCount: 0,
    past: [],
    future: [],
    status: null,
    highlights: null,
    intersecting: null,
    lastRun: null,
    hiddenTriangles: null,
    ...patch,
  };
}

const stateOf = (session: RepairSession | null, patch: Partial<RepairState> = {}): RepairState => ({
  ...useRepair.getState(),
  session,
  highlight: DEFAULT_HIGHLIGHT,
  view: DEFAULT_VIEW,
  ...patch,
});

const highlightsOf = (patch: Partial<Highlights>): Highlights => ({
  border: new Float32Array(0),
  nonManifold: new Float32Array(0),
  inverted: new Uint32Array(0),
  degenerate: new Uint32Array(0),
  ...patch,
});

/** A repair layer with a stand-in for the parts layer's clip planes; `clip(ghost)` replays a clip change. */
function layer() {
  const planes: Plane[] = [];
  const ghostPlanes: Plane[] = [];
  let listener: ((planes: Plane[], ghostPlanes: Plane[], ghost: boolean) => void) | null = null;
  const repair = createRepairLayer(theme, {
    watchClipping(l) {
      listener = l;
      l(planes, ghostPlanes, false);
      return () => (listener = null);
    },
  });
  const object = (name: string) => {
    const found = repair.group.getObjectByName(name);
    if (!found) throw new Error(`no ${name}`);
    return found as Mesh<BufferGeometry, Material>;
  };
  /** What an overlay draws: its index up to the draw range, or null when hidden. */
  const drawn = (name: string) => {
    const o = object(name);
    if (!o.visible) return null;
    const index = o.geometry.index;
    return index ? Array.from((index.array as Uint32Array).subarray(0, o.geometry.drawRange.count)) : null;
  };
  const clip = (ghost: boolean, count = 1) => {
    planes.length = count;
    listener?.(planes, ghostPlanes, ghost);
  };
  return { repair, object, drawn, planes, ghostPlanes, clip, isWatching: () => listener !== null };
}

describe("createRepairLayer", () => {
  it("draws nothing outside repair mode", () => {
    const { repair } = layer();
    expect(repair.sync(stateOf(null), "#a9b1bc", "shaded")).toBe(false);
    expect(repair.group.visible).toBe(false);
    expect(repair.bounds()).toBeNull();
  });

  it("draws the working copy with its wireframe, every overlay over the same position buffer", () => {
    const { repair, object } = layer();
    const session = sessionOf(BOX);
    expect(repair.sync(stateOf(session), "#a9b1bc", "shaded")).toBe(true);
    expect(repair.group.visible).toBe(true);
    const surface = object("surface").geometry;
    expect(surface.index?.array).toBe(BOX.indices);
    expect(surface.getAttribute("position").array).toBe(BOX.positions);
    expect(object("inside").geometry).toBe(surface);
    expect(object("wireframe").geometry.index?.array.length).toBe(12 * 6);
    const shared = surface.getAttribute("position");
    for (const name of ["wireframe", "selected triangles", "inverted triangles", "degenerate triangles", "intersecting triangles", "slim triangles", "sharp edges", "slim edges"]) {
      expect(object(name).geometry.getAttribute("position")).toBe(shared);
    }
    expect(repair.bounds()).toEqual({ min: [-10, -10, -10], max: [10, 10, 10] });
    // Nothing to highlight yet.
    for (const name of ["selected triangles", "inverted triangles", "hole edges", "error edges", "sharp edges", "slim triangles"]) {
      expect(object(name).visible).toBe(false);
    }
    // The same session again changes nothing, nor does a run's progress (another field of the store).
    expect(repair.sync(stateOf(session), "#a9b1bc", "shaded")).toBe(false);
    const busy = { job: 1, label: "Extended repair", progress: 0.5, step: null, detail: null, local: null, cancelling: false };
    expect(repair.sync(stateOf(session, { busy }), "#a9b1bc", "shaded")).toBe(false);
    // A new selection mask, even an empty one, is a change.
    expect(repair.sync(stateOf({ ...session, selection: new Uint8Array(12) }), "#a9b1bc", "shaded")).toBe(true);
  });

  it("leaves hidden triangles out of the surface and the overlays, then draws the original index again", () => {
    const { repair, object, drawn } = layer();
    const hidden = new Uint8Array(12);
    hidden[0] = 1;
    const session = sessionOf(BOX, { hiddenTriangles: hidden, highlights: highlightsOf({ inverted: new Uint32Array([0, 1]) }) });
    repair.sync(stateOf(session), "#a9b1bc", "shaded");
    expect(object("surface").geometry.index?.array).not.toBe(BOX.indices);
    expect(object("surface").geometry.index?.array.length).toBe(11 * 3);
    expect(drawn("inverted triangles")).toEqual(Array.from(BOX.indices.subarray(3, 6)));
    repair.sync(stateOf(sessionOf(BOX, { highlights: session.highlights })), "#a9b1bc", "shaded");
    expect(object("surface").geometry.index?.array).toBe(BOX.indices);
  });

  it("colours the surface and shades its inside darker; wireframe shading leaves the surface out", () => {
    const { repair, object } = layer();
    const session = sessionOf(BOX);
    repair.sync(stateOf(session), "#808080", "shaded");
    const front = object("surface").material as unknown as { color: { getHexString(): string } };
    const back = object("inside").material as unknown as { color: { r: number } };
    expect(front.color.getHexString()).toBe("808080");
    expect(back.color.r).toBeLessThan(0.5 * 0.5);
    expect(repair.sync(stateOf(session), "#808080", "wireframe")).toBe(true);
    expect(object("surface").visible).toBe(false);
    expect(object("inside").visible).toBe(false);
    expect(object("wireframe").visible).toBe(true);
  });

  it("draws the wireframe whole when it is all there is: part colour, full opacity, no fade, never left out", () => {
    const { repair, object } = layer();
    const session = sessionOf(BOX);
    repair.sync(stateOf(session), "#808080", "shaded");
    const wire = object("wireframe") as unknown as Mesh<BufferGeometry, Material> & { material: { color: { getHexString(): string } } };
    const shader = { uniforms: {} as Record<string, { value: number }>, vertexShader: ShaderLib.basic.vertexShader, fragmentShader: ShaderLib.basic.fragmentShader };
    (wire.material.onBeforeCompile as unknown as (s: typeof shader) => void)(shader);
    // Seen from 11 m the box's lines would sit under 3 device pixels apart.
    const far = new PerspectiveCamera(35, 1, 0.1, 100000);
    far.position.set(0, 0, 11000);
    far.lookAt(new Vector3(0, 0, 0));
    const shown = () => {
      repair.beforeRender(far, 1000);
      return wire.visible;
    };

    // Over the shaded surface: a faint dark overlay that fades out where the lines crowd, and goes when all would.
    expect([shader.uniforms.fade.value, wire.material.opacity, wire.material.color.getHexString(), shown()]).toEqual([1, 0.3, "3a4250", false]);

    // View › Wireframe: the wire is the working copy, drawn like a wireframe part.
    expect(repair.sync(stateOf(session), "#808080", "wireframe")).toBe(true);
    expect([shader.uniforms.fade.value, wire.material.opacity, wire.material.color.getHexString(), shown()]).toEqual([0, 1, "808080", true]);
    repair.sync(stateOf(session), "#ff0000", "wireframe");
    expect(wire.material.color.getHexString()).toBe("ff0000");
    // A theme change keeps the part colour while the wire is all there is.
    repair.applyTheme({ ...theme, edge: "#000000" });
    expect(wire.material.color.getHexString()).toBe("ff0000");

    // Shaded again: the overlay in the theme's edge colour.
    repair.sync(stateOf(session), "#ff0000", "shaded-edges");
    expect([shader.uniforms.fade.value, wire.material.opacity, wire.material.color.getHexString(), shown()]).toEqual([1, 0.3, "000000", false]);
  });

  it("draws the selection, and a selection change rebuilds nothing else", () => {
    const { repair, object, drawn } = layer();
    const inverted = new Uint32Array([4]);
    const base = sessionOf(BOX, { highlights: highlightsOf({ inverted }) });
    repair.sync(stateOf(base), "#a9b1bc", "shaded");
    expect(drawn("inverted triangles")).toEqual(Array.from(BOX.indices.subarray(12, 15)));
    const invertedIndex = object("inverted triangles").geometry.index;

    const selection = new Uint8Array(12);
    selection[1] = selection[3] = 1;
    expect(repair.sync(stateOf({ ...base, selection, selectedCount: 2 }), "#a9b1bc", "shaded")).toBe(true);
    expect(drawn("selected triangles")).toEqual([...BOX.indices.subarray(3, 6), ...BOX.indices.subarray(9, 12)]);
    expect(object("inverted triangles").geometry.index).toBe(invertedIndex);

    // A smaller selection reuses the index buffer and draws less of it.
    const buffer = object("selected triangles").geometry.index;
    const one = new Uint8Array(12);
    one[7] = 1;
    repair.sync(stateOf({ ...base, selection: one, selectedCount: 1 }), "#a9b1bc", "shaded");
    expect(object("selected triangles").geometry.index).toBe(buffer);
    expect(drawn("selected triangles")).toEqual(Array.from(BOX.indices.subarray(21, 24)));
    // A bigger one replaces it.
    const all = new Uint8Array(12).fill(1);
    repair.sync(stateOf({ ...base, selection: all, selectedCount: 12 }), "#a9b1bc", "shaded");
    expect(drawn("selected triangles")).toEqual(Array.from(BOX.indices));
    // None selected: hidden.
    repair.sync(stateOf({ ...base, selection: new Uint8Array(12), selectedCount: 0 }), "#a9b1bc", "shaded");
    expect(object("selected triangles").visible).toBe(false);
  });

  it("follows the Status tab switches", () => {
    const { repair, object, drawn } = layer();
    const border = new Float32Array([0, 0, 0, 1, 0, 0, 1, 0, 0, 1, 1, 0]);
    const nonManifold = new Float32Array([5, 5, 5, 6, 6, 6]);
    const session = sessionOf(BOX, {
      highlights: highlightsOf({ border, nonManifold, inverted: new Uint32Array([0]), degenerate: new Uint32Array([2, 50]) }),
      intersecting: new Uint32Array([5, 6]),
    });
    repair.sync(stateOf(session), "#a9b1bc", "shaded");
    const holes = object("hole edges") as unknown as LineSegments2;
    expect(holes.visible).toBe(true);
    expect(object("hole edges outline").visible).toBe(true);
    expect(holes.geometry.getAttribute("instanceStart").count).toBe(2);
    expect((object("error edges") as unknown as LineSegments2).geometry.getAttribute("instanceStart").count).toBe(1);
    expect(drawn("inverted triangles")).toEqual(Array.from(BOX.indices.subarray(0, 3)));
    // Id 50 names no triangle of the box and is left out.
    expect(drawn("degenerate triangles")).toEqual(Array.from(BOX.indices.subarray(6, 9)));
    expect(drawn("intersecting triangles")).toEqual(Array.from(BOX.indices.subarray(15, 21)));

    const off = { holes: false, triangles: false, degenerate: false, errors: false, edgesFrom: null };
    expect(repair.sync(stateOf(session, { highlight: off }), "#a9b1bc", "shaded")).toBe(true);
    for (const name of ["hole edges", "hole edges outline", "error edges", "error edges outline", "inverted triangles", "degenerate triangles", "intersecting triangles"]) {
      expect(object(name).visible).toBe(false);
    }

    // Edges from 45°: the box's 12 edges, not its diagonals.
    repair.sync(stateOf(session, { highlight: { ...off, edgesFrom: 45 } }), "#a9b1bc", "shaded");
    expect(drawn("sharp edges")?.length).toBe(24);
    repair.sync(stateOf(session, { highlight: { ...off, edgesFrom: 120 } }), "#a9b1bc", "shaded");
    expect(object("sharp edges").visible).toBe(false);
  });

  it("follows the View tab's slim faces", () => {
    const { repair, object, drawn } = layer();
    const session = sessionOf(BOX);
    const view = (patch: Partial<typeof DEFAULT_VIEW>) => ({ ...DEFAULT_VIEW, ...patch });
    // The box's smallest angles are 45°.
    repair.sync(stateOf(session, { view: view({ slimColored: true, slimAngle: 50 }) }), "#a9b1bc", "shaded");
    expect(drawn("slim triangles")).toEqual(Array.from(BOX.indices));
    expect(object("slim edges").visible).toBe(false);
    repair.sync(stateOf(session, { view: view({ slimColored: true, slimEdges: true, slimAngle: 40 }) }), "#a9b1bc", "shaded");
    expect(object("slim triangles").visible).toBe(false);
    expect(object("slim edges").visible).toBe(false);
    repair.sync(stateOf(session, { view: view({ slimEdges: true, slimAngle: 46 }) }), "#a9b1bc", "shaded");
    expect(drawn("slim edges")?.length).toBe(12 * 6);
  });

  it("moves to a new working copy and frees the old one's buffers together", () => {
    const { repair, object } = layer();
    repair.sync(stateOf(sessionOf(BOX)), "#a9b1bc", "shaded");
    const old = object("surface").geometry;
    let disposed = 0;
    old.addEventListener("dispose", () => disposed++);
    const next = makeBox([10, 10, 10]);
    expect(repair.sync(stateOf(sessionOf(next)), "#a9b1bc", "shaded")).toBe(true);
    expect(disposed).toBe(1);
    expect(object("surface").geometry.getAttribute("position").array).toBe(next.positions);
    expect(repair.bounds()).toEqual({ min: [-5, -5, -5], max: [5, 5, 5] });
    // Leaving repair mode frees the rest.
    const last = object("surface").geometry;
    last.addEventListener("dispose", () => disposed++);
    expect(repair.sync(stateOf(null), "#a9b1bc", "shaded")).toBe(true);
    expect(disposed).toBe(2);
    expect(repair.group.visible).toBe(false);
    expect(repair.bounds()).toBeNull();
  });

  it("clips every drawing with the parts' planes and shows the ghost only with Transparent cuts", () => {
    const { repair, object, planes, ghostPlanes, clip, isWatching } = layer();
    repair.sync(stateOf(sessionOf(BOX)), "#a9b1bc", "shaded");
    for (const name of ["surface", "inside", "wireframe", "selected triangles", "sharp edges", "hole edges", "error edges outline"]) {
      expect(object(name).material.clippingPlanes).toBe(planes);
    }
    expect(object("ghost").material.clippingPlanes).toBe(ghostPlanes);
    expect(object("ghost").visible).toBe(false);
    clip(true);
    expect(object("ghost").visible).toBe(true);
    clip(true, 0);
    expect(object("ghost").visible).toBe(false);
    clip(false);
    expect(object("ghost").visible).toBe(false);
    repair.dispose();
    expect(isWatching()).toBe(false);
  });

  it("fades the wireframe per vertex in its shader, and leaves it out when every line would be faded", () => {
    const { repair, object } = layer();
    repair.sync(stateOf(sessionOf(BOX)), "#a9b1bc", "shaded");
    const wire = object("wireframe");
    expect(wire.geometry.getAttribute("spacing").array).toEqual(new Float32Array(8).fill(20));
    // The shader the material asks for: the basic line shader with the fade patched in.
    const shader = { uniforms: {} as Record<string, { value: number }>, vertexShader: ShaderLib.basic.vertexShader, fragmentShader: ShaderLib.basic.fragmentShader };
    (wire.material.onBeforeCompile as unknown as (s: typeof shader) => void)(shader);
    expect(shader.vertexShader).toContain("attribute float spacing;");
    expect(shader.vertexShader).toContain("vFade = mix(1.0, smoothstep(3.0, 12.0, spacing * perUnit), fade);");
    expect(shader.fragmentShader).toContain("diffuseColor.a *= vFade;\n#include <opaque_fragment>");

    const centre = new Vector3(0, 0, 0);
    const from = (distance: number) => {
      const camera = new PerspectiveCamera(35, 1, 0.1, 100000);
      camera.position.set(0, 0, distance);
      camera.lookAt(centre);
      repair.beforeRender(camera, 1000);
      return wire.visible;
    };
    // The widest spacing (20 mm) at the box's nearest sphere point spans 20 · 1000 / (2 (d − 17.3) tan 17.5°) device pixels.
    expect(from(100)).toBe(true);
    expect(shader.uniforms.bufferHeight.value).toBe(1000);
    expect(from(10000)).toBe(true);
    expect(from(11000)).toBe(false);
    // From inside the box every line is near.
    expect(from(1)).toBe(true);
    // Orthographic: the scale comes from the zoom (200 mm tall at zoom 1).
    const ortho = new OrthographicCamera(-100, 100, 100, -100, -1000, 1000);
    ortho.position.set(0, 0, 500);
    ortho.zoom = 0.01;
    ortho.updateProjectionMatrix();
    repair.beforeRender(ortho, 1000);
    expect(wire.visible).toBe(false);
    ortho.zoom = 1;
    ortho.updateProjectionMatrix();
    repair.beforeRender(ortho, 1000);
    expect(wire.visible).toBe(true);
  });

  it("keeps an index buffer it can reuse when the geometry grows it", () => {
    // Guards the reason setIndex copies instead of swapping: the shared position attribute stays the same object.
    const { repair, object } = layer();
    repair.sync(stateOf(sessionOf(BOX)), "#a9b1bc", "shaded");
    const position = object("selected triangles").geometry.getAttribute("position");
    const all = new Uint8Array(12).fill(1);
    repair.sync(stateOf(sessionOf(BOX, { selection: all, selectedCount: 12 })), "#a9b1bc", "shaded");
    expect(object("selected triangles").geometry.getAttribute("position")).toBe(position);
    expect(object("selected triangles").geometry.index).toBeInstanceOf(BufferAttribute);
  });
});

/** A binary STL with its corners welded by exact coordinates, as the Rust importer does. */
function readStl(path: string): MeshData {
  const buffer = readFileSync(path);
  const view = new DataView(buffer.buffer, buffer.byteOffset, buffer.byteLength);
  const count = view.getUint32(80, true);
  const ids = new Map<string, number>();
  const positions: number[] = [];
  const indices = new Uint32Array(count * 3);
  for (let t = 0; t < count; t++) {
    for (let k = 0; k < 3; k++) {
      const at = 84 + t * 50 + 12 + k * 12;
      const x = view.getFloat32(at, true);
      const y = view.getFloat32(at + 4, true);
      const z = view.getFloat32(at + 8, true);
      const key = `${x},${y},${z}`;
      let id = ids.get(key);
      if (id === undefined) {
        id = positions.length / 3;
        ids.set(key, id);
        positions.push(x, y, z);
      }
      indices[t * 3 + k] = id;
    }
  }
  return { positions: new Float32Array(positions), indices };
}

const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};

// Real models, read-only, on demand: TRICKY3D_MODELS="a.stl|b.stl" npx vitest run src/viewport/repairLayer.test.ts --disableConsoleIntercept
const models = (env.TRICKY3D_MODELS ?? "").split("|").filter(Boolean);
describe.skipIf(models.length === 0)("real models", () => {
  for (const path of models) {
    it(`times each drawing change on ${path.split(/[\/]/).pop()}`, () => {
      const m = readStl(path);
      const n = m.indices.length / 3;
      const { repair } = layer();
      const times: string[] = [];
      const time = (label: string, work: () => unknown) => {
        const start = performance.now();
        work();
        times.push(`${label} ${(performance.now() - start).toFixed(1)} ms`);
      };
      const base = sessionOf(m);
      time("new working copy", () => repair.sync(stateOf(base), "#a9b1bc", "shaded"));
      const all = new Uint8Array(n).fill(1);
      time("select all", () => repair.sync(stateOf({ ...base, selection: all, selectedCount: n }), "#a9b1bc", "shaded"));
      const one = new Uint8Array(n);
      one[n >> 1] = 1;
      time("select one", () => repair.sync(stateOf({ ...base, selection: one, selectedCount: 1 }), "#a9b1bc", "shaded"));
      // A status with a tenth of the triangles inverted and 2,000 border edges.
      const inverted = new Uint32Array(Math.floor(n / 10)).map((_, i) => i * 10);
      const border = m.positions.slice(0, 2000 * 6);
      const status = { ...base, highlights: highlightsOf({ inverted, border }) };
      time("status highlights", () => repair.sync(stateOf(status), "#a9b1bc", "shaded"));
      const slim = (slimAngle: number) => stateOf(status, { view: { ...DEFAULT_VIEW, slimColored: true, slimEdges: true, slimAngle } });
      time("slim faces 10°", () => repair.sync(slim(10), "#a9b1bc", "shaded"));
      time("slim faces 12°", () => repair.sync(slim(12), "#a9b1bc", "shaded"));
      const sharp = (edgesFrom: number) => stateOf(status, { highlight: { ...DEFAULT_HIGHLIGHT, edgesFrom } });
      time("edges from 30° (first)", () => repair.sync(sharp(30), "#a9b1bc", "shaded"));
      time("edges from 40°", () => repair.sync(sharp(40), "#a9b1bc", "shaded"));
      // The next working copy (as after an action or an undo), with the switches above still on.
      const next = sessionOf({ positions: m.positions, indices: m.indices });
      time("next working copy, edges and slim faces on", () => repair.sync(stateOf(next, { view: slim(12).view, highlight: sharp(40).highlight }), "#a9b1bc", "shaded"));
      time("next working copy, default switches", () => repair.sync(stateOf(sessionOf({ positions: m.positions, indices: m.indices })), "#a9b1bc", "shaded"));
      time("leave", () => repair.sync(stateOf(null), "#a9b1bc", "shaded"));
      console.log(`${path.split(/[\/]/).pop()} (${n} triangles): ${times.join(", ")}`);
    }, 120_000);
  }
});
