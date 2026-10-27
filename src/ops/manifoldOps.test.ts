// Runs the real manifold-3d wasm under node, the same module the worker loads in the app.
import Module, { type ManifoldToplevel } from "manifold-3d";
import { beforeAll, describe, expect, it } from "vitest";
import type { MeshData, Vec3 } from "../store";
import { booleanMeshes, planeSides, splitMesh, type BooleanOp } from "./manifoldOps";

let wasm: ManifoldToplevel;

beforeAll(async () => {
  wasm = await Module();
  wasm.setup();
});

/** Closed box with welded corners and outward triangles, like Rust mesh::make_box (12 triangles). */
function box(min: Vec3, max: Vec3): MeshData {
  const [x0, y0, z0] = min;
  const [x1, y1, z1] = max;
  // prettier-ignore
  const positions = new Float32Array([
    x0, y0, z0, x1, y0, z0, x1, y1, z0, x0, y1, z0,
    x0, y0, z1, x1, y0, z1, x1, y1, z1, x0, y1, z1,
  ]);
  // prettier-ignore
  const indices = new Uint32Array([
    0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4,
    2, 3, 7, 2, 7, 6, 1, 2, 6, 1, 6, 5, 3, 0, 4, 3, 4, 7,
  ]);
  return { positions, indices };
}

/** The cube without its two +Z triangles, like the open_box.stl fixture. */
function openBox(): MeshData {
  const cube = box([0, 0, 0], [20, 20, 20]);
  return { positions: cube.positions, indices: new Uint32Array([...cube.indices.subarray(0, 6), ...cube.indices.subarray(12)]) };
}

/** Icosahedron subdivided `levels` times, welded, radius `r` around the origin (5 levels: 20,480 triangles). */
function icosphere(levels: number, r: number): MeshData {
  const t = (1 + Math.sqrt(5)) / 2;
  const points: Vec3[] = [
    [-1, t, 0], [1, t, 0], [-1, -t, 0], [1, -t, 0],
    [0, -1, t], [0, 1, t], [0, -1, -t], [0, 1, -t],
    [t, 0, -1], [t, 0, 1], [-t, 0, -1], [-t, 0, 1],
  ];
  // prettier-ignore
  let faces = [
    0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8,
    3, 9, 4, 3, 4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
  ];
  for (let level = 0; level < levels; level++) {
    const middle = new Map<string, number>();
    const mid = (a: number, b: number) => {
      const key = a < b ? `${a},${b}` : `${b},${a}`;
      let i = middle.get(key);
      if (i === undefined) {
        i = points.push([(points[a][0] + points[b][0]) / 2, (points[a][1] + points[b][1]) / 2, (points[a][2] + points[b][2]) / 2]) - 1;
        middle.set(key, i);
      }
      return i;
    };
    const next: number[] = [];
    for (let f = 0; f < faces.length; f += 3) {
      const [a, b, c] = [faces[f], faces[f + 1], faces[f + 2]];
      const [ab, bc, ca] = [mid(a, b), mid(b, c), mid(c, a)];
      next.push(a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca);
    }
    faces = next;
  }
  const positions = new Float32Array(points.length * 3);
  points.forEach((p, i) => {
    const s = r / Math.hypot(...p);
    positions.set([p[0] * s, p[1] * s, p[2] * s], i * 3);
  });
  return { positions, indices: new Uint32Array(faces) };
}

/** Signed-tetrahedron volume in f64, as Rust mesh::stats computes it. */
function volume({ positions: p, indices }: MeshData): number {
  let v = 0;
  for (let t = 0; t < indices.length; t += 3) {
    const [a, b, c] = [indices[t] * 3, indices[t + 1] * 3, indices[t + 2] * 3];
    v +=
      p[a] * (p[b + 1] * p[c + 2] - p[b + 2] * p[c + 1]) -
      p[a + 1] * (p[b] * p[c + 2] - p[b + 2] * p[c]) +
      p[a + 2] * (p[b] * p[c + 1] - p[b + 1] * p[c]);
  }
  return v / 6;
}

/** Edge and shell counts over the mesh's own vertex indices, as Rust mesh::analyze counts them. */
function topology({ indices }: MeshData) {
  const uses = new Map<string, number>();
  for (let t = 0; t < indices.length; t += 3) {
    for (const [a, b] of [[indices[t], indices[t + 1]], [indices[t + 1], indices[t + 2]], [indices[t + 2], indices[t]]]) {
      const key = a < b ? `${a},${b}` : `${b},${a}`;
      uses.set(key, (uses.get(key) ?? 0) + 1);
    }
  }
  const parent = new Map<number, number>();
  const root = (v: number): number => {
    while (parent.get(v) !== v) v = parent.get(v)!;
    return v;
  };
  for (const v of indices) if (!parent.has(v)) parent.set(v, v);
  for (let t = 0; t < indices.length; t += 3) {
    for (const v of [indices[t + 1], indices[t + 2]]) parent.set(root(v), root(indices[t]));
  }
  const counts = [...uses.values()];
  return {
    triangles: indices.length / 3,
    openEdges: counts.filter((n) => n === 1).length,
    nonManifoldEdges: counts.filter((n) => n > 2).length,
    shells: [...parent.keys()].filter((v) => root(v) === v).length,
  };
}

const closed = (shells: number, triangles?: number) => ({ openEdges: 0, nonManifoldEdges: 0, shells, ...(triangles === undefined ? {} : { triangles }) });

function axisRange(mesh: MeshData, axis: number): [number, number] {
  let lo = Infinity;
  let hi = -Infinity;
  for (let i = axis; i < mesh.positions.length; i += 3) {
    lo = Math.min(lo, mesh.positions[i]);
    hi = Math.max(hi, mesh.positions[i]);
  }
  return [lo, hi];
}

/** True when the array is the whole of a buffer of its own, so it is not a view into wasm memory. */
const ownsBuffer = (a: Float32Array | Uint32Array) => a.byteOffset === 0 && a.byteLength === a.buffer.byteLength;

function ok<T extends { kind: string }>(outcome: T): Extract<T, { kind: "ok" }> {
  expect(outcome.kind).toBe("ok");
  return outcome as Extract<T, { kind: "ok" }>;
}

/** The mesh with every triangle's winding reversed: closed, but facing inward. */
function inward(mesh: MeshData): MeshData {
  const indices = mesh.indices.slice();
  for (let t = 0; t < indices.length; t += 3) [indices[t + 1], indices[t + 2]] = [indices[t + 2], indices[t + 1]];
  return { positions: mesh.positions, indices };
}

/** Several meshes as one, without welding (what Edit shells › Merge parts makes). */
function merged(...meshes: MeshData[]): MeshData {
  const positions = new Float32Array(meshes.reduce((n, m) => n + m.positions.length, 0));
  const indices = new Uint32Array(meshes.reduce((n, m) => n + m.indices.length, 0));
  let p = 0;
  let i = 0;
  for (const m of meshes) {
    positions.set(m.positions, p);
    indices.set(m.indices.map((v) => v + p / 3), i);
    p += m.positions.length;
    i += m.indices.length;
  }
  return { positions, indices };
}

/** Union of everything, as Boolean operations' Union runs it with every part in the green list. */
const unite = (meshes: MeshData[]) => booleanMeshes(wasm, "union", meshes, []);

/**
 * Closed n-sided prism around the Z axis line through (cx, cy), radius r to its corners, from z0 to z1, facing
 * outward: a cylinder as a CAD export tessellates it.
 */
function cylinder(cx: number, cy: number, r: number, z0: number, z1: number, n = 48): MeshData {
  const positions: number[] = [];
  for (const z of [z0, z1]) {
    for (let i = 0; i < n; i++) positions.push(cx + r * Math.cos((2 * Math.PI * i) / n), cy + r * Math.sin((2 * Math.PI * i) / n), z);
  }
  positions.push(cx, cy, z0, cx, cy, z1);
  const [bottom, top] = [2 * n, 2 * n + 1];
  const indices: number[] = [];
  for (let i = 0; i < n; i++) {
    const j = (i + 1) % n;
    indices.push(i, j, n + j, i, n + j, n + i); // side
    indices.push(bottom, j, i); // bottom faces -Z
    indices.push(top, n + i, n + j); // top faces +Z
  }
  return { positions: new Float32Array(positions), indices: new Uint32Array(indices) };
}

/** Area of the prism's n-gon: what `cylinder` loses against a true circle. */
const ngonArea = (r: number, n = 48) => (n / 2) * r * r * Math.sin((2 * Math.PI) / n);

/** Holes through a closed, connected surface: genus g from Euler's V - E + F = 2 - 2g. */
function genus({ indices }: MeshData): number {
  const edges = new Set<string>();
  for (let t = 0; t < indices.length; t += 3) {
    for (const [a, b] of [[indices[t], indices[t + 1]], [indices[t + 1], indices[t + 2]], [indices[t + 2], indices[t]]]) edges.add(a < b ? `${a},${b}` : `${b},${a}`);
  }
  const vertices = new Set(indices).size;
  return (2 - (vertices - edges.size + indices.length / 3)) / 2;
}

describe("booleanMeshes: union, difference and intersection", () => {
  const a = box([0, 0, 0], [20, 20, 20]);
  const b = box([10, 0, 0], [30, 20, 20]);
  const run = (op: BooleanOp, main: MeshData[], tool: MeshData[], noise = 0) => ok(booleanMeshes(wasm, op, main, tool, noise));

  it("gives 12, 4 and 4 cm³ for two 20 mm cubes overlapping by half", () => {
    const union = run("union", [a], [b]);
    const difference = run("difference", [a], [b]);
    const intersection = run("intersection", [a], [b]);
    expect(volume(union.mesh)).toBeCloseTo(12000, 6);
    expect(volume(difference.mesh)).toBeCloseTo(4000, 6);
    expect(volume(intersection.mesh)).toBeCloseTo(4000, 6);
    for (const r of [union, difference, intersection]) expect(topology(r.mesh)).toMatchObject(closed(1));
    // Green minus red keeps the part of A outside B; the intersection is the slab they share.
    expect(axisRange(difference.mesh, 0)).toEqual([0, 10]);
    expect(axisRange(intersection.mesh, 0)).toEqual([10, 20]);
    // Union puts both lists together: the same with B in the green list.
    expect(volume(run("union", [a, b], []).mesh)).toBeCloseTo(12000, 6);
  });

  it("subtracts a cylinder from a box as a real hole through it", () => {
    const plate = box([0, 0, 0], [40, 40, 20]);
    const pin = cylinder(20, 20, 5, -5, 25);
    const { mesh } = run("difference", [plate], [pin]);
    expect(volume(mesh)).toBeCloseTo(40 * 40 * 20 - ngonArea(5) * 20, 3);
    expect(topology(mesh)).toMatchObject(closed(1));
    expect(genus(mesh)).toBe(1);
    // Nothing is left inside the hole: no vertex closer to its axis than the prism's flat sides.
    const p = mesh.positions;
    let nearest = Infinity;
    for (let i = 0; i < p.length; i += 3) nearest = Math.min(nearest, Math.hypot(p[i] - 20, p[i + 1] - 20));
    expect(nearest).toBeGreaterThan(5 * Math.cos(Math.PI / 48) - 1e-4);
  });

  it("subtracts several red parts at once", () => {
    const plate = box([0, 0, 0], [60, 30, 10]);
    const pins = [cylinder(15, 15, 4, -1, 11), cylinder(45, 15, 4, -1, 11)];
    const { mesh } = run("difference", [plate], pins);
    expect(volume(mesh)).toBeCloseTo(60 * 30 * 10 - 2 * ngonArea(4) * 10, 3);
    expect(topology(mesh)).toMatchObject(closed(1));
    expect(genus(mesh)).toBe(2);
  });

  it("unites the green parts before subtracting, and intersects with all the red parts together", () => {
    // Two green cubes side by side (16 cm³), a red slab across both.
    const greens = [box([0, 0, 0], [20, 20, 20]), box([20, 0, 0], [40, 20, 20])];
    const reds = [box([10, 0, 15], [25, 20, 25]), box([25, 0, 15], [30, 20, 25])];
    expect(volume(run("difference", greens, reds).mesh)).toBeCloseTo(16000 - 20 * 20 * 5, 6);
    expect(volume(run("intersection", greens, reds).mesh)).toBeCloseTo(20 * 20 * 5, 6);
    expect(topology(run("difference", greens, reds).mesh)).toMatchObject(closed(1));
  });

  it("leaves an empty mesh when nothing is left", () => {
    expect(run("intersection", [a], [box([40, 0, 0], [60, 20, 20])]).mesh.indices.length).toBe(0);
    expect(run("difference", [a], [box([-1, -1, -1], [21, 21, 21])]).mesh.indices.length).toBe(0);
  });

  it("names the input that is not closed, counting the red list after the green one, and computes nothing", () => {
    expect(booleanMeshes(wasm, "difference", [a], [openBox()])).toEqual({ kind: "invalid", indices: [1] });
    expect(booleanMeshes(wasm, "intersection", [openBox(), a], [b])).toEqual({ kind: "invalid", indices: [0] });
  });

  it("drops result shells below the noise volume and says how many", () => {
    const speck = box([50, 0, 0], [50.1, 0.1, 0.1]); // 0.001 mm³
    const kept = run("union", [a, speck], [], 0);
    expect(topology(kept.mesh)).toMatchObject(closed(2));
    expect(kept.noise).toBe(0);
    const filtered = run("union", [a, speck], [], 0.1);
    expect(topology(filtered.mesh)).toEqual(closed(1, 12));
    expect(volume(filtered.mesh)).toBeCloseTo(8000, 6);
    expect(filtered.noise).toBe(1);
  });

  it("reports its steps in order, from 0 to the result", () => {
    const seen: [number, string][] = [];
    booleanMeshes(wasm, "difference", [a], [b], 0.1, (f, label) => seen.push([f, label]));
    expect(seen.map(([, label]) => label)).toEqual(["Reading part 1 of 2", "Reading part 2 of 2", "Fusing overlapping shells", "Subtracting", "Reading the result"]);
    const fractions = seen.map(([f]) => f);
    expect(fractions).toEqual([...fractions].sort((x, y) => x - y));
    expect(fractions[0]).toBe(0);
  });
});

describe("booleanMeshes union: inputs a plain batch union gets wrong", () => {
  const a = box([0, 0, 0], [20, 20, 20]);
  const b = box([10, 0, 0], [30, 20, 20]);

  it("turns a part that faces inward round, instead of uniting everything outside it", () => {
    const result = ok(unite([inward(a), b]));
    expect(volume(result.mesh)).toBeCloseTo(12000, 6);
    expect(topology(result.mesh)).toMatchObject(closed(1));
    expect(result.bodies).toEqual([1, 1]);
  });

  it("fuses the overlapping shells inside one part (a Merge parts result)", () => {
    const result = ok(unite([merged(a, b), box([25, 0, 0], [45, 20, 20])]));
    expect(volume(result.mesh)).toBeCloseTo(18000, 6);
    expect(topology(result.mesh)).toMatchObject(closed(1));
    expect(result.bodies).toEqual([1, 1]);
    // United alone it becomes one solid too.
    expect(volume(ok(unite([merged(a, b)])).mesh)).toBeCloseTo(12000, 6);
  });

  it("keeps a part's cavity, and lets another part fill it", () => {
    const hollow = merged(a, inward(box([5, 5, 5], [15, 15, 15])));
    const kept = ok(unite([hollow, box([40, 0, 0], [50, 10, 10])]));
    expect(volume(kept.mesh)).toBeCloseTo(8000 - 1000 + 1000, 6);
    expect(topology(kept.mesh)).toMatchObject(closed(3));
    expect(kept.bodies).toEqual([2, 1]);
    const filled = ok(unite([hollow, box([4, 4, 4], [16, 16, 16])]));
    expect(volume(filled.mesh)).toBeCloseTo(8000, 6);
    expect(topology(filled.mesh)).toMatchObject(closed(1));
  });

  it("turns round an inward shell that sits inside no other shell (a defect, not a cavity)", () => {
    const result = ok(unite([merged(a, inward(box([40, 0, 0], [50, 10, 10]))), b]));
    expect(volume(result.mesh)).toBeCloseTo(12000 + 1000, 6);
    expect(result.bodies).toEqual([2, 1]);
  });

  it("counts each part's own separate solids", () => {
    const result = ok(unite([merged(a, box([40, 0, 0], [60, 20, 20])), b]));
    expect(topology(result.mesh)).toMatchObject(closed(2));
    expect(result.bodies).toEqual([2, 1]);
  });
});

describe("booleanMeshes union", () => {
  it("fuses two overlapping 20 mm cubes into one closed 12 cm³ shell", () => {
    const { mesh } = ok(unite([box([0, 0, 0], [20, 20, 20]), box([10, 0, 0], [30, 20, 20])]));
    expect(volume(mesh)).toBeCloseTo(12000, 6);
    // The overlap's faces are cut in, not merged away: 28 triangles for what is geometrically one box.
    expect(topology(mesh)).toEqual(closed(1, 28));
    expect(ownsBuffer(mesh.positions) && ownsBuffer(mesh.indices)).toBe(true);
  });

  it("keeps disjoint cubes as separate closed shells, 16 cm³ in all", () => {
    const { mesh } = ok(unite([box([0, 0, 0], [20, 20, 20]), box([40, 0, 0], [60, 20, 20])]));
    expect(volume(mesh)).toBeCloseTo(16000, 6);
    expect(topology(mesh)).toEqual(closed(2, 24));
  });

  it("refuses an open box, naming every input that is not a closed solid", () => {
    const empty: MeshData = { positions: new Float32Array(0), indices: new Uint32Array(0) };
    expect(unite([box([0, 0, 0], [20, 20, 20]), openBox()])).toEqual({ kind: "invalid", indices: [1] });
    expect(unite([openBox(), box([0, 0, 0], [1, 1, 1]), empty])).toEqual({ kind: "invalid", indices: [0, 2] });
  });

  it("refuses broken input instead of crashing: an index past the vertices, a NaN vertex", () => {
    const outOfRange: MeshData = { positions: new Float32Array(9), indices: new Uint32Array([0, 1, 7]) };
    const cube = box([0, 0, 0], [20, 20, 20]);
    const nan: MeshData = { positions: cube.positions.slice(), indices: cube.indices };
    nan.positions[4] = NaN;
    expect(unite([outOfRange, nan])).toEqual({ kind: "invalid", indices: [0, 1] });
  });

  it("keeps a clean mesh's triangles: the constructor collapses nothing on the cube or the 20k icosphere", () => {
    const sphere = icosphere(5, 10);
    expect(sphere.indices.length / 3).toBe(20480);
    expect(topology(ok(unite([sphere])).mesh)).toEqual(closed(1, 20480));
    expect(topology(ok(unite([box([0, 0, 0], [20, 20, 20])])).mesh)).toEqual(closed(1, 12));
  });

  it("unites a cube with the icosphere it overlaps into one closed shell", () => {
    const sphere = icosphere(5, 10);
    const { mesh } = ok(unite([sphere, box([0, -5, -5], [20, 5, 5])]));
    const t = topology(mesh);
    expect(t).toMatchObject({ openEdges: 0, nonManifoldEdges: 0, shells: 1 });
    // The cube's part inside the sphere goes; the sphere loses the triangles under the cube's end.
    expect(volume(mesh)).toBeGreaterThan(volume(sphere));
    expect(volume(mesh)).toBeLessThan(volume(sphere) + 20 * 10 * 10);
  });

  it("only reads its inputs", () => {
    const inputs = [box([0, 0, 0], [20, 20, 20]), box([10, 0, 0], [30, 20, 20]), openBox()];
    const before = inputs.map((m) => ({ positions: m.positions.slice(), indices: m.indices.slice() }));
    unite(inputs.slice(0, 2));
    unite(inputs); // the open box goes through merge(), which changes that mesh's merge vectors
    inputs.forEach((m, i) => {
      expect(m.positions).toEqual(before[i].positions);
      expect(m.indices).toEqual(before[i].indices);
    });
  });
});

describe("splitMesh", () => {
  const zAt = (offset: number) => ({ normal: [0, 0, 1] as Vec3, offset });

  it("cuts the cube at z = 10 into two closed 4 cm³ halves, below then above", () => {
    const { below, above } = ok(splitMesh(wasm, box([0, 0, 0], [20, 20, 20]), zAt(10), 0));
    expect(below && above).toBeTruthy();
    expect(volume(below!)).toBeCloseTo(4000, 6);
    expect(volume(above!)).toBeCloseTo(4000, 6);
    expect(axisRange(below!, 2)).toEqual([0, 10]);
    expect(axisRange(above!, 2)).toEqual([10, 20]);
    expect(topology(below!)).toEqual(closed(1, 12));
    expect(topology(above!)).toEqual(closed(1, 12));
    for (const piece of [below!, above!]) expect(ownsBuffer(piece.positions) && ownsBuffer(piece.indices)).toBe(true);
  });

  it("cuts the 20k icosphere through its centre into two closed halves that add up within 0.1%", () => {
    const sphere = icosphere(5, 10);
    const { below, above } = ok(splitMesh(wasm, sphere, zAt(0), 0));
    const whole = volume(sphere);
    const sum = volume(below!) + volume(above!);
    expect(Math.abs(sum - whole) / whole).toBeLessThan(1e-3);
    expect(axisRange(below!, 2)[1]).toBe(0);
    expect(axisRange(above!, 2)[0]).toBe(0);
    for (const half of [below!, above!]) expect(topology(half)).toMatchObject(closed(1));
    // Report the counts so the PR can state them (the cut adds triangles along the outline and caps it).
    console.log(`icosphere ${sphere.indices.length / 3} triangles -> below ${below!.indices.length / 3}, above ${above!.indices.length / 3}; relative volume error ${(Math.abs(sum - whole) / whole).toExponential(2)}`);
  });

  it("cuts along a tilted plane with the side the normal points to as above", () => {
    const n = Math.SQRT1_2;
    const { below, above } = ok(splitMesh(wasm, box([0, 0, 0], [20, 20, 20]), { normal: [n, 0, n], offset: 20 * n }, 0));
    // The plane x + z = 20 halves the cube; above holds the corner (20, *, 20), below the origin.
    expect(volume(below!)).toBeCloseTo(4000, 4);
    expect(volume(above!)).toBeCloseTo(4000, 4);
    const sums = (m: MeshData) => Array.from({ length: m.positions.length / 3 }, (_, i) => m.positions[i * 3] + m.positions[i * 3 + 2]);
    expect(Math.min(...sums(above!))).toBeCloseTo(20, 4);
    expect(Math.max(...sums(above!))).toBe(40);
    expect(Math.min(...sums(below!))).toBe(0);
    expect(Math.max(...sums(below!))).toBeCloseTo(20, 4);
    for (const piece of [below!, above!]) expect(topology(piece)).toMatchObject(closed(1));
  });

  it("leaves a part the plane does not cross alone, also when the plane is on a face up to f32 rounding", () => {
    const cube = box([0, 0, 0], [20, 20, 20]);
    const untouched = { kind: "ok", below: null, above: null };
    for (const z of [50, -1, 0, 20]) expect(splitMesh(wasm, cube, zAt(z), 0)).toEqual(untouched);
    // A part moved by 0.1 mm: its box (f64) and its vertices (f32) disagree by about 1e-6 mm. manifold-3d alone
    // cut a zero-volume sliver there; the tolerance of the Rust clipper keeps the part whole.
    const moved = box([0, 0, 10.1], [20, 20, 30.1]);
    for (const d of [0, 1e-6, -1e-6, 1e-7]) expect(splitMesh(wasm, moved, zAt(30.1 + d), 0)).toEqual(untouched);
    expect(planeSides(moved, zAt(30.1))).toEqual({ below: true, above: false });
    // Just inside the tolerance still counts as on the face; well past it cuts.
    expect(ok(splitMesh(wasm, moved, zAt(30), 0)).above).not.toBeNull();
  });

  it("moves the above piece by gap along the normal and leaves below in place", () => {
    const cube = box([0, 0, 0], [20, 20, 20]);
    const plane = { normal: [0, 0.6, 0.8] as Vec3, offset: 10 };
    const still = ok(splitMesh(wasm, cube, plane, 0));
    const moved = ok(splitMesh(wasm, cube, plane, 5));
    expect(moved.below).toEqual(still.below);
    expect(moved.above!.indices).toEqual(still.above!.indices);
    const p = moved.above!.positions;
    const q = still.above!.positions;
    for (let i = 0; i < p.length; i += 3) {
      expect(p[i]).toBe(q[i]);
      // One rounding of the f64 sum, as in Rust ops::translate.
      expect(p[i + 1]).toBe(Math.fround(q[i + 1] + plane.normal[1] * 5));
      expect(p[i + 2]).toBe(Math.fround(q[i + 2] + plane.normal[2] * 5));
    }
  });

  it("reports open and empty meshes as invalid so the caller can use the Rust cutter", () => {
    expect(splitMesh(wasm, openBox(), zAt(10), 0)).toEqual({ kind: "invalid" });
    expect(splitMesh(wasm, { positions: new Float32Array(0), indices: new Uint32Array(0) }, zAt(10), 0)).toEqual({ kind: "invalid" });
  });

  it("only reads its input", () => {
    const sphere = icosphere(3, 10);
    const positions = sphere.positions.slice();
    const indices = sphere.indices.slice();
    splitMesh(wasm, sphere, zAt(1), 2);
    expect(sphere.positions).toEqual(positions);
    expect(sphere.indices).toEqual(indices);
  });
});
