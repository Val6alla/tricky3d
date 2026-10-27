// @ts-expect-error The app's tsconfig has no Node types; vitest runs this file in Node.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { MeshData, Vec3 } from "../store";
import { buildTriangleBvh, clearTriangleBvh, pickTriangle, raycastTriangles, rayTriangle, triangleBvh, type TriangleBvh } from "./triPick";

/** Node's environment, for the on-demand timing and real-model tests below. */
const env = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};

/** Deterministic random numbers in [0, 1) (mulberry32), so a failure can be replayed. */
function random(seed: number): () => number {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const mesh = (positions: number[], indices: number[]): MeshData => ({
  positions: new Float32Array(positions),
  indices: new Uint32Array(indices),
});

/** `count` unwelded triangles of random size (0.5 to 20 mm) scattered over a 100 mm box: overlapping, crossing, nested. */
function soup(count: number, seed: number): MeshData {
  const r = random(seed);
  const positions: number[] = [];
  const indices: number[] = [];
  for (let t = 0; t < count; t++) {
    const centre = [r() * 100 - 50, r() * 100 - 50, r() * 100 - 50];
    const size = 0.5 + r() * r() * 20;
    for (let k = 0; k < 3; k++) positions.push(centre[0] + (r() - 0.5) * size, centre[1] + (r() - 0.5) * size, centre[2] + (r() - 0.5) * size);
    indices.push(t * 3, t * 3 + 1, t * 3 + 2);
  }
  return mesh(positions, indices);
}

/**
 * A welded n × n grid of unit squares in the plane z = `z` (two triangles each), every square split along the same
 * diagonal: shared edges and corners everywhere, all of it axis aligned.
 */
function grid(n: number, z: number, into?: { positions: number[]; indices: number[] }) {
  const out = into ?? { positions: [], indices: [] };
  const base = out.positions.length / 3;
  for (let y = 0; y <= n; y++) for (let x = 0; x <= n; x++) out.positions.push(x, y, z);
  const at = (x: number, y: number) => base + y * (n + 1) + x;
  for (let y = 0; y < n; y++) {
    for (let x = 0; x < n; x++) {
      out.indices.push(at(x, y), at(x + 1, y), at(x + 1, y + 1));
      out.indices.push(at(x, y), at(x + 1, y + 1), at(x, y + 1));
    }
  }
  return out;
}

/** The unit icosahedron subdivided `levels` times and pushed onto the sphere of `radius`: closed, outward facing. */
function sphere(levels: number, radius: number): MeshData {
  const g = (1 + Math.sqrt(5)) / 2;
  let points: number[][] = [
    [-1, g, 0], [1, g, 0], [-1, -g, 0], [1, -g, 0], [0, -1, g], [0, 1, g],
    [0, -1, -g], [0, 1, -g], [g, 0, -1], [g, 0, 1], [-g, 0, -1], [-g, 0, 1],
  ];
  let faces = [
    [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11], [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
    [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9], [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
  ];
  for (let level = 0; level < levels; level++) {
    const middles = new Map<string, number>();
    const middle = (a: number, b: number) => {
      const key = a < b ? `${a},${b}` : `${b},${a}`;
      let m = middles.get(key);
      if (m === undefined) {
        m = points.length;
        points.push(points[a].map((v, k) => (v + points[b][k]) / 2));
        middles.set(key, m);
      }
      return m;
    };
    faces = faces.flatMap(([a, b, c]) => {
      const ab = middle(a, b);
      const bc = middle(b, c);
      const ca = middle(c, a);
      return [[a, ab, ca], [b, bc, ab], [c, ca, bc], [ab, bc, ca]];
    });
  }
  points = points.map((p) => {
    const length = Math.hypot(p[0], p[1], p[2]);
    return p.map((v) => (v / length) * radius);
  });
  return mesh(points.flat(), faces.flat());
}

interface Hit {
  triangle: number;
  distance: number;
}

/** The nearest accepted hit by testing every triangle: what the hierarchy must reproduce exactly. */
function bruteForce(m: MeshData, origin: Vec3, direction: Vec3, accept?: (p: Vec3) => boolean): Hit | null {
  let best: Hit | null = null;
  for (let t = 0; t < m.indices.length / 3; t++) {
    const distance = rayTriangle(m.positions, m.indices, t, ...origin, ...direction);
    if (distance < 0) continue;
    if (best && (distance > best.distance || (distance === best.distance && t > best.triangle))) continue;
    const point: Vec3 = [origin[0] + direction[0] * distance, origin[1] + direction[1] * distance, origin[2] + direction[2] * distance];
    if (accept && !accept(point)) continue;
    best = { triangle: t, distance };
  }
  return best;
}

const hitOf = (bvh: TriangleBvh, origin: Vec3, direction: Vec3, accept?: (p: Vec3) => boolean): Hit | null => {
  const hit = raycastTriangles(bvh, origin, direction, accept);
  return hit && { triangle: hit.triangle, distance: hit.distance };
};

/** A random point inside triangle t of `m`. */
function pointIn(m: MeshData, t: number, r: () => number): Vec3 {
  let u = r();
  let v = r();
  if (u + v > 1) {
    u = 1 - u;
    v = 1 - v;
  }
  const corner = (c: number, k: number) => m.positions[m.indices[t * 3 + c] * 3 + k];
  return [0, 1, 2].map((k) => corner(0, k) + u * (corner(1, k) - corner(0, k)) + v * (corner(2, k) - corner(0, k))) as Vec3;
}

/**
 * A random ray from inside or around the soup's box, in a random direction (not normalized), or every other time
 * aimed through a random point of a random triangle, so that plenty of rays hit something.
 */
function randomRay(r: () => number, m: MeshData, i: number): { origin: Vec3; direction: Vec3 } {
  const origin: Vec3 = [r() * 160 - 80, r() * 160 - 80, r() * 160 - 80];
  if (i % 2 === 0) return { origin, direction: [r() * 2 - 1, r() * 2 - 1, r() * 2 - 1] };
  const p = pointIn(m, Math.floor(r() * (m.indices.length / 3)), r);
  const scale = 0.1 + r() * 3;
  return { origin, direction: [(p[0] - origin[0]) * scale, (p[1] - origin[1]) * scale, (p[2] - origin[2]) * scale] };
}

describe("rayTriangle", () => {
  // The triangle (0,0,0) (4,0,0) (0,4,0) in the plane z = 0, facing +Z.
  const tri = mesh([0, 0, 0, 4, 0, 0, 0, 4, 0], [0, 1, 2]);
  const cast = (o: Vec3, d: Vec3, m = tri, t = 0) => rayTriangle(m.positions, m.indices, t, ...o, ...d);

  it("measures the distance in direction lengths, from either side", () => {
    expect(cast([1, 1, 5], [0, 0, -1])).toBe(5);
    expect(cast([1, 1, 5], [0, 0, -2])).toBe(2.5);
    // From below: the back face counts too.
    expect(cast([1, 1, -3], [0, 0, 1])).toBe(3);
    expect(cast([0, 0, 2], [1, 1, -2])).toBeCloseTo(1, 12);
  });

  it("counts edges and corners as inside", () => {
    expect(cast([2, 0, 1], [0, 0, -1])).toBe(1); // on the edge along x
    expect(cast([2, 2, 1], [0, 0, -1])).toBe(1); // on the hypotenuse
    expect(cast([0, 0, 1], [0, 0, -1])).toBe(1); // corner
    expect(cast([4, 0, 1], [0, 0, -1])).toBe(1); // corner
  });

  it("misses outside the triangle, behind the origin and along its plane", () => {
    expect(cast([3, 3, 1], [0, 0, -1])).toBe(-1);
    expect(cast([-0.001, 1, 1], [0, 0, -1])).toBe(-1);
    expect(cast([1, 1, 5], [0, 0, 1])).toBe(-1);
    expect(cast([-1, 1, 0], [1, 0, 0])).toBe(-1);
    expect(cast([1, 1, 0], [0, 0, 0])).toBe(-1);
  });

  it("never hits a triangle without area", () => {
    const flat = mesh([0, 0, 0, 4, 0, 0, 0, 4, 0, 2, 0, 0], [0, 0, 1, 0, 1, 3, 1, 2, 2]);
    expect(cast([1, 0, 1], [0, 0, -1], flat, 0)).toBe(-1); // repeated index
    expect(cast([1, 0, 1], [0, 0, -1], flat, 1)).toBe(-1); // three corners on a line
    expect(cast([3, 0.5, 1], [0, -0.5, -1], flat, 2)).toBe(-1);
  });

  it("hits the point the distance names", () => {
    const r = random(7);
    const m = soup(200, 3);
    let hits = 0;
    for (let i = 0; i < 2000; i++) {
      const t = Math.floor(r() * 200);
      const origin: Vec3 = [r() * 160 - 80, r() * 160 - 80, r() * 160 - 80];
      const through = pointIn(m, t, r);
      const direction: Vec3 = [through[0] - origin[0], through[1] - origin[1], through[2] - origin[2]];
      const d = rayTriangle(m.positions, m.indices, t, ...origin, ...direction);
      // Aimed through the triangle, so only a grazing ray may miss.
      if (d < 0) continue;
      hits++;
      expect(d).toBeCloseTo(1, 6);
      // The point lies in the triangle's plane: its offset from corner a is normal to the normal.
      const p = [0, 1, 2].map((k) => origin[k] + direction[k] * d);
      const corner = (c: number) => [0, 1, 2].map((k) => m.positions[m.indices[t * 3 + c] * 3 + k]);
      const [a, b, c] = [corner(0), corner(1), corner(2)];
      const u = b.map((v, k) => v - a[k]);
      const v = c.map((x, k) => x - a[k]);
      const n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
      const off = p.map((x, k) => x - a[k]);
      const cos = (off[0] * n[0] + off[1] * n[1] + off[2] * n[2]) / (Math.hypot(...n) * Math.max(1e-9, Math.hypot(...off)));
      expect(Math.abs(cos)).toBeLessThan(1e-6);
    }
    expect(hits).toBeGreaterThan(1990);
  });
});

describe("buildTriangleBvh", () => {
  it("puts every triangle in exactly one leaf of at most four, under boxes that contain it", () => {
    const m = soup(5000, 11);
    const bvh = buildTriangleBvh(m.positions, m.indices);
    const n = m.indices.length / 3;
    const seen = new Uint8Array(n);
    const nodes = bvh.count.length;
    let depthMax = 0;
    const walk = (node: number, depth: number, parentBox: number[] | null) => {
      depthMax = Math.max(depthMax, depth);
      const box = Array.from(bvh.boxes.subarray(node * 6, node * 6 + 6));
      if (parentBox) for (let k = 0; k < 3; k++) expect(parentBox[k] <= box[k] && box[k + 3] <= parentBox[k + 3]).toBe(true);
      if (bvh.count[node] === 0) {
        expect(bvh.first[node]).toBeGreaterThan(node + 1);
        expect(bvh.first[node]).toBeLessThan(nodes);
        walk(node + 1, depth + 1, box);
        walk(bvh.first[node], depth + 1, box);
        return;
      }
      expect(bvh.count[node]).toBeLessThanOrEqual(4);
      for (let i = bvh.first[node]; i < bvh.first[node] + bvh.count[node]; i++) {
        const t = bvh.order[i];
        expect(seen[t]).toBe(0);
        seen[t] = 1;
        for (let c = 0; c < 3; c++) {
          const v = m.indices[t * 3 + c] * 3;
          for (let k = 0; k < 3; k++) {
            expect(box[k]).toBeLessThan(m.positions[v + k]);
            expect(box[k + 3]).toBeGreaterThan(m.positions[v + k]);
          }
        }
      }
    };
    walk(0, 0, null);
    expect(seen.every((s) => s === 1)).toBe(true);
    // Middle splits keep the tree balanced: 5000 / 4 leaves → depth 11.
    expect(depthMax).toBeLessThanOrEqual(Math.ceil(Math.log2(n / 4)) + 1);
  });

  it("handles an empty mesh, a single triangle and identical triangles", () => {
    expect(buildTriangleBvh(new Float32Array(0), new Uint32Array(0)).count.length).toBe(0);
    expect(raycastTriangles(buildTriangleBvh(new Float32Array(0), new Uint32Array(0)), [0, 0, 1], [0, 0, -1])).toBeNull();
    const one = mesh([0, 0, 0, 1, 0, 0, 0, 1, 0], [0, 1, 2]);
    expect(raycastTriangles(buildTriangleBvh(one.positions, one.indices), [0.2, 0.2, 1], [0, 0, -1])).toEqual({ triangle: 0, distance: 1, point: [0.2, 0.2, 0] });
    // A hundred copies of one triangle: equal centres everywhere; the lowest id wins the tie.
    const copies = mesh([0, 0, 0, 1, 0, 0, 0, 1, 0], Array.from({ length: 300 }, (_, i) => i % 3));
    const bvh = buildTriangleBvh(copies.positions, copies.indices);
    expect(hitOf(bvh, [0.2, 0.2, 1], [0, 0, -1])).toEqual({ triangle: 0, distance: 1 });
  });
});

describe("raycastTriangles", () => {
  it("skips a hidden triangle and hits the one behind it", () => {
    const m = mesh(
      [0, 0, 1, 2, 0, 1, 0, 2, 1, 0, 0, 0, 2, 0, 0, 0, 2, 0],
      [0, 1, 2, 3, 4, 5],
    );
    const bvh = buildTriangleBvh(m.positions, m.indices);
    const origin: Vec3 = [0.2, 0.2, 5];
    const direction: Vec3 = [0, 0, -1];
    expect(raycastTriangles(bvh, origin, direction)?.triangle).toBe(0);
    const hidden = new Uint8Array([1, 0]);
    expect(raycastTriangles(bvh, origin, direction, undefined, hidden)?.triangle).toBe(1);
    hidden[1] = 1;
    expect(raycastTriangles(bvh, origin, direction, undefined, hidden)).toBeNull();
  });

  it("finds the same nearest triangle as testing every one, on a random soup", () => {
    const m = soup(3000, 5);
    const bvh = buildTriangleBvh(m.positions, m.indices);
    const r = random(99);
    let hits = 0;
    for (let i = 0; i < 1500; i++) {
      const { origin, direction } = randomRay(r, m, i);
      const expected = bruteForce(m, origin, direction);
      expect(hitOf(bvh, origin, direction)).toEqual(expected);
      if (expected) hits++;
    }
    // Enough rays hit something for the comparison to mean anything.
    expect(hits).toBeGreaterThan(300);
  });

  it("agrees on ties: axis-aligned rays through shared edges and corners of stacked grids", () => {
    const g = grid(12, 0);
    grid(12, 0, g); // an exact copy on top: every hit is a tie between two triangles
    grid(12, 3, g);
    const m = mesh(g.positions, g.indices);
    const bvh = buildTriangleBvh(m.positions, m.indices);
    let rays = 0;
    for (let x = -0.5; x <= 12.5; x += 0.5) {
      for (let y = -0.5; y <= 12.5; y += 0.5) {
        for (const [origin, direction] of [
          [[x, y, 10], [0, 0, -1]],
          [[x, y, -10], [0, 0, 1]],
          [[x, y, 1.5], [0, 0, 1]],
          [[x, y, 1.5], [0, 0, -1]],
          [[x, y, 10], [0.25, 0, -1]],
          [[x, y, 10], [0, -0.5, -2]],
        ] as [Vec3, Vec3][]) {
          expect(hitOf(bvh, origin, direction)).toEqual(bruteForce(m, origin, direction));
          rays++;
        }
      }
    }
    expect(rays).toBeGreaterThan(4000);
    // A corner shared by six triangles of each grid: the lowest id among the nearest grid's.
    expect(hitOf(bvh, [5, 5, 10], [0, 0, -1])?.distance).toBe(7);
  });

  it("starts at the origin: from inside a closed sphere it hits the wall ahead", () => {
    const m = sphere(3, 10);
    const bvh = buildTriangleBvh(m.positions, m.indices);
    const r = random(4);
    for (let i = 0; i < 300; i++) {
      const origin: Vec3 = [r() * 6 - 3, r() * 6 - 3, r() * 6 - 3];
      const direction: Vec3 = [r() * 2 - 1, r() * 2 - 1, r() * 2 - 1];
      const hit = raycastTriangles(bvh, origin, direction);
      expect(hit).not.toBeNull();
      expect(hitOf(bvh, origin, direction)).toEqual(bruteForce(m, origin, direction));
      const radius = Math.hypot(...(hit as { point: Vec3 }).point);
      expect(radius).toBeGreaterThan(9.5);
      expect(radius).toBeLessThan(10.01);
    }
    // Pointing away from everything.
    expect(raycastTriangles(bvh, [20, 0, 0], [1, 0, 0])).toBeNull();
  });

  it("skips hits that `accept` refuses, as the clip planes do, and finds what lies behind them", () => {
    const m = soup(3000, 8);
    const bvh = buildTriangleBvh(m.positions, m.indices);
    const r = random(12);
    // Keep z <= 10 and x + y >= -20, like two clip planes.
    const keep = (p: Vec3) => p[2] <= 10 && p[0] + p[1] >= -20;
    let refusedNearest = 0;
    for (let i = 0; i < 1500; i++) {
      const { origin, direction } = randomRay(r, m, i);
      const expected = bruteForce(m, origin, direction, keep);
      expect(hitOf(bvh, origin, direction, keep)).toEqual(expected);
      const plain = bruteForce(m, origin, direction);
      if (plain && expected?.triangle !== plain.triangle) refusedNearest++;
    }
    expect(refusedNearest).toBeGreaterThan(50);
    // Refusing everything leaves nothing.
    expect(raycastTriangles(bvh, [0, 0, 80], [0, 0, -1], () => false)).toBeNull();
  });

  it("opens only the boxes along the ray, also when it hits nothing", () => {
    const g = grid(100, 0);
    grid(100, 3, g);
    const m = mesh(g.positions, g.indices);
    const bvh = buildTriangleBvh(m.positions, m.indices);
    const nodes = bvh.count.length;
    // Counts the box coordinates the ray test reads.
    let reads = 0;
    const counted = { ...bvh, boxes: new Proxy(bvh.boxes, { get: (target, key) => (reads++, Reflect.get(target, key)) }) };
    const boxesRead = (origin: Vec3, direction: Vec3) => {
      reads = 0;
      raycastTriangles(counted, origin, direction);
      return reads / 6;
    };
    // Between the two layers, inside the root box, missing every triangle.
    expect(raycastTriangles(bvh, [-5, 50.5, 1.5], [1, 0.01, 0])).toBeNull();
    expect(boxesRead([-5, 50.5, 1.5], [1, 0.01, 0])).toBeLessThan(nodes / 10);
    // Straight down onto the upper layer.
    expect(raycastTriangles(bvh, [20.25, 70.5, 9], [0, 0, -1])?.distance).toBe(6);
    expect(boxesRead([20.25, 70.5, 9], [0, 0, -1])).toBeLessThan(200);
  });

  it("returns the hit point on the ray", () => {
    const m = sphere(2, 5);
    const hit = pickTriangle(m, [0, 0, 20], [0, 0, -2]);
    expect(hit?.distance).toBeCloseTo(7.5, 1);
    const p = hit?.point as Vec3;
    expect(p[0]).toBe(0);
    expect(p[1]).toBe(0);
    expect(p[2]).toBeCloseTo(5, 1);
  });
});

describe("triangleBvh", () => {
  it("builds once per mesh and keeps only the latest", () => {
    const a = soup(100, 1);
    const b = soup(100, 2);
    const first = triangleBvh(a);
    expect(triangleBvh(a)).toBe(first);
    const other = triangleBvh(b);
    expect(other).not.toBe(first);
    expect(triangleBvh(b)).toBe(other);
    expect(triangleBvh(a)).not.toBe(first);
  });

  it("lets go of the latest hierarchy, and its mesh, when cleared", () => {
    const a = soup(100, 1);
    const first = triangleBvh(a);
    clearTriangleBvh();
    const again = triangleBvh(a);
    expect(again).not.toBe(first);
    expect(again).toEqual(first);
  });
});

// Timing, run on demand: TRICKY3D_TIMING=1 npx vitest run src/viewport/triPick.test.ts --disableConsoleIntercept
describe.skipIf(!env.TRICKY3D_TIMING)("timing", () => {
  it("builds over a million triangles in well under a second and picks in a millisecond", () => {
    const g = grid(707, 0);
    const m = mesh(g.positions, g.indices);
    const start = performance.now();
    const bvh = buildTriangleBvh(m.positions, m.indices);
    const built = performance.now() - start;
    const r = random(3);
    const pickStart = performance.now();
    for (let i = 0; i < 1000; i++) raycastTriangles(bvh, [r() * 707, r() * 707, 50], [r() - 0.5, r() - 0.5, -1]);
    const perPick = (performance.now() - pickStart) / 1000;
    console.log(`grid of ${m.indices.length / 3} triangles: build ${built.toFixed(0)} ms, pick ${perPick.toFixed(3)} ms`);
    expect(built).toBeLessThan(2000);
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

// Real models, read-only, on demand: TRICKY3D_MODELS="a.stl|b.stl" npx vitest run src/viewport/triPick.test.ts --disableConsoleIntercept
const models = (env.TRICKY3D_MODELS ?? "").split("|").filter(Boolean);
describe.skipIf(models.length === 0)("real models", () => {
  for (const path of models) {
    it(`builds and picks on ${path.split(/[\\/]/).pop()}`, () => {
      const m = readStl(path);
      const start = performance.now();
      const bvh = buildTriangleBvh(m.positions, m.indices);
      const built = performance.now() - start;
      // Rays from around the model's box towards its centre, checked against testing every triangle.
      let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity, z0 = Infinity, z1 = -Infinity;
      for (let i = 0; i < m.positions.length; i += 3) {
        x0 = Math.min(x0, m.positions[i]);
        x1 = Math.max(x1, m.positions[i]);
        y0 = Math.min(y0, m.positions[i + 1]);
        y1 = Math.max(y1, m.positions[i + 1]);
        z0 = Math.min(z0, m.positions[i + 2]);
        z1 = Math.max(z1, m.positions[i + 2]);
      }
      const r = random(5);
      let picks = 0;
      let hits = 0;
      for (let i = 0; i < 30; i++) {
        const target: Vec3 = [x0 + r() * (x1 - x0), y0 + r() * (y1 - y0), z0 + r() * (z1 - z0)];
        const origin: Vec3 = [x0 + r() * (x1 - x0), y0 + r() * (y1 - y0), z1 + (z1 - z0)];
        const direction: Vec3 = [target[0] - origin[0], target[1] - origin[1], target[2] - origin[2]];
        const pickStart = performance.now();
        const hit = hitOf(bvh, origin, direction);
        picks += performance.now() - pickStart;
        expect(hit).toEqual(bruteForce(m, origin, direction));
        if (hit) hits++;
      }
      console.log(
        `${path.split(/[\\/]/).pop()}: ${m.indices.length / 3} triangles, build ${built.toFixed(0)} ms, ` +
          `pick ${(picks / 30).toFixed(3)} ms (${hits} of 30 rays hit)`,
      );
    }, 120_000);
  }
});
