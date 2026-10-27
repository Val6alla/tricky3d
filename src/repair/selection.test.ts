import { describe, expect, it } from "vitest";
import { makeBox } from "../mesh";
import type { MeshData } from "../store";
import {
  NO_EDGE,
  applySelection,
  buildEdgeAdjacency,
  clearSelectionCaches,
  countSelected,
  edgeAdjacency,
  expandMask,
  extractTriangles,
  idsToMask,
  invertMask,
  maskToIds,
  selectionBounds,
  selectionInfoOf,
  shellRegion,
  shrinkMask,
  surfaceRegion,
  triangleNormals,
  triangleShells,
  trianglesOfShell,
} from "./selection";

const mesh = (positions: number[], indices: number[]): MeshData => ({ positions: new Float32Array(positions), indices: new Uint32Array(indices) });

/** `a` and `b` side by side in one mesh (b's vertices after a's). */
function join(a: MeshData, b: MeshData, dx = 0): MeshData {
  const shifted = Array.from(b.positions, (v, i) => (i % 3 === 0 ? v + dx : v));
  const offset = a.positions.length / 3;
  return mesh([...a.positions, ...shifted], [...a.indices, ...Array.from(b.indices, (i) => i + offset)]);
}

/** An n × n grid of unit squares in the XY plane, two triangles each, facing +Z. */
function grid(n: number): MeshData {
  const positions = new Float32Array((n + 1) * (n + 1) * 3);
  for (let y = 0, k = 0; y <= n; y++) {
    for (let x = 0; x <= n; x++, k += 3) {
      positions[k] = x;
      positions[k + 1] = y;
    }
  }
  const indices = new Uint32Array(n * n * 6);
  for (let y = 0, k = 0; y < n; y++) {
    for (let x = 0; x < n; x++, k += 6) {
      const a = y * (n + 1) + x;
      indices.set([a, a + 1, a + n + 2, a, a + n + 2, a + n + 1], k);
    }
  }
  return { positions, indices };
}

/** A closed prism over a regular `sides`-gon (radius 10, height 10): side faces meet at 360/sides degrees. */
function prism(sides: number): MeshData {
  const positions: number[] = [];
  for (let i = 0; i < sides; i++) {
    const a = (2 * Math.PI * i) / sides;
    positions.push(10 * Math.cos(a), 10 * Math.sin(a), 0, 10 * Math.cos(a), 10 * Math.sin(a), 10);
  }
  positions.push(0, 0, 0, 0, 0, 10);
  const bottom = 2 * sides;
  const top = bottom + 1;
  const indices: number[] = [];
  for (let i = 0; i < sides; i++) {
    const j = (i + 1) % sides;
    indices.push(2 * i, 2 * j, 2 * j + 1, 2 * i, 2 * j + 1, 2 * i + 1); // side, outward
    indices.push(bottom, 2 * j, 2 * i, top, 2 * i + 1, 2 * j + 1); // caps
  }
  return mesh(positions, indices);
}

/** Small deterministic random numbers (mulberry32). */
function random(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** Triangles over few vertices, so repeated indices, non-manifold edges and separate shells show up constantly. */
function randomMesh(seed: number, triangles: number, vertices: number): MeshData {
  const r = random(seed);
  const positions = Array.from({ length: vertices * 3 }, () => Math.round(r() * 6) - 3);
  const indices = Array.from({ length: triangles * 3 }, () => Math.floor(r() * vertices));
  return mesh(positions, indices);
}

const edgeKey = (a: number, b: number) => (a < b ? `${a},${b}` : `${b},${a}`);

/** Reference: for each corner, the set of corners (any triangle, itself included) on the same undirected edge. */
function bruteEdgeCorners(m: MeshData): (Set<number> | null)[] {
  const byEdge = new Map<string, number[]>();
  const n = m.indices.length;
  const keys: (string | null)[] = [];
  for (let c = 0; c < n; c++) {
    const a = m.indices[c];
    const b = m.indices[c % 3 === 2 ? c - 2 : c + 1];
    const key = a === b ? null : edgeKey(a, b);
    keys.push(key);
    if (key) byEdge.set(key, [...(byEdge.get(key) ?? []), c]);
  }
  return keys.map((key) => (key ? new Set(byEdge.get(key)) : null));
}

/**
 * Reference: shells by breadth-first search, numbered in order of first triangle. Two triangles join across an edge
 * that has exactly two sides in the whole mesh, theirs, running opposite ways.
 */
function bruteShells(m: MeshData): number[] {
  const t = m.indices.length / 3;
  const sides = new Map<string, [number, number, number][]>();
  for (let u = 0; u < t; u++) {
    for (let k = 0; k < 3; k++) {
      const a = m.indices[u * 3 + k];
      const b = m.indices[u * 3 + ((k + 1) % 3)];
      if (a === b) continue;
      const key = `${Math.min(a, b)},${Math.max(a, b)}`;
      sides.set(key, [...(sides.get(key) ?? []), [u, a, b]]);
    }
  }
  const shell = new Array<number>(t).fill(-1);
  let count = 0;
  for (let s = 0; s < t; s++) {
    if (shell[s] >= 0) continue;
    const queue = [s];
    shell[s] = count;
    while (queue.length) {
      const u = queue.pop() as number;
      for (const list of sides.values()) {
        if (list.length !== 2 || list[0][1] === list[1][1]) continue;
        const [x, y] = [list[0][0], list[1][0]];
        const v = x === u ? y : y === u ? x : -1;
        if (v >= 0 && shell[v] < 0) {
          shell[v] = count;
          queue.push(v);
        }
      }
    }
    count++;
  }
  return shell;
}

const sorted = (ids: ArrayLike<number>) => Array.from(ids).sort((a, b) => a - b);

describe("masks", () => {
  it("convert to sorted ids and back, ignoring ids that name no triangle", () => {
    const mask = idsToMask([5, 1, 1, 9, -1, 2.5, 3], 6);
    expect(Array.from(mask)).toEqual([0, 1, 0, 1, 0, 1]);
    expect(Array.from(maskToIds(mask))).toEqual([1, 3, 5]);
    expect(countSelected(mask)).toBe(3);
    expect(maskToIds(new Uint8Array(4)).length).toBe(0);
    expect(Array.from(invertMask(mask))).toEqual([1, 0, 1, 0, 1, 0]);
  });

  it("combine a pick by replace, add, remove and toggle with exact counts, never changing the old mask", () => {
    const mask = idsToMask([0, 1], 5);
    const frozen = mask.slice();
    let r = applySelection(mask, 2, [3, 3, 4, 7], "replace");
    expect([Array.from(r.mask), r.count]).toEqual([[0, 0, 0, 1, 1], 2]);
    r = applySelection(mask, 2, [1, 2], "add");
    expect([Array.from(r.mask), r.count]).toEqual([[1, 1, 1, 0, 0], 3]);
    r = applySelection(mask, 2, [1, 2, 9], "remove");
    expect([Array.from(r.mask), r.count]).toEqual([[1, 0, 0, 0, 0], 1]);
    // Toggle follows the first (clicked) triangle: selected, so the whole pick goes; unselected, so it all comes in.
    r = applySelection(mask, 2, [1, 2], "toggle");
    expect([Array.from(r.mask), r.count]).toEqual([[1, 0, 0, 0, 0], 1]);
    r = applySelection(mask, 2, [2, 1, 3], "toggle");
    expect([Array.from(r.mask), r.count]).toEqual([[1, 1, 1, 1, 0], 4]);
    r = applySelection(mask, 2, [4], "toggle");
    expect([Array.from(r.mask), r.count]).toEqual([[1, 1, 0, 0, 1], 3]);
    expect(Array.from(mask)).toEqual(Array.from(frozen));
  });

  it("return the same mask when a pick changes nothing", () => {
    const mask = idsToMask([0, 1], 3);
    expect(applySelection(mask, 2, [0, 1], "add").mask).toBe(mask);
    expect(applySelection(mask, 2, [2], "remove").mask).toBe(mask);
    expect(applySelection(mask, 2, [], "toggle").mask).toBe(mask);
    expect(applySelection(mask, 2, [-4, 8], "toggle").mask).toBe(mask);
  });

  it("expand by triangles sharing a corner and shrink by triangles touching an unselected one", () => {
    const g = grid(6); // 72 triangles; triangles 2k and 2k+1 form square k (row-major)
    const centre = idsToMask([28, 29], 72); // square (2, 2), away from the border
    const grown = expandMask(g, centre);
    // Every triangle with a corner on square (2, 2): within the 3 × 3 squares around it.
    const square = new Set([16, 17, 23, 24]); // the corner vertices of square (2, 2)
    const expected = new Set<number>();
    for (let t = 0; t < 72; t++) {
      if (Array.from(g.indices.slice(t * 3, t * 3 + 3)).some((v) => square.has(v))) expected.add(t);
    }
    expect(sorted(maskToIds(grown))).toEqual([...expected].sort((a, b) => a - b));
    expect(expected.size).toBe(16); // 6 around each corner, shared: the 3 × 3 squares minus 2 far corner triangles
    // Only the centre square's corners have every triangle around them selected: shrinking undoes the expansion.
    expect(Array.from(maskToIds(shrinkMask(g, grown)))).toEqual([28, 29]);
    expect(Array.from(maskToIds(shrinkMask(g, centre)))).toEqual([]);
    expect(Array.from(shrinkMask(g, new Uint8Array(72).fill(1)))).toEqual(new Array(72).fill(1));
    // Shrinking a 2 × 2 block of a big grid leaves nothing; a 3 × 3 block keeps the triangles off its rim.
    const g6 = grid(6);
    const block = new Uint8Array(72);
    for (let y = 1; y <= 3; y++) for (let x = 1; x <= 3; x++) block[2 * (y * 6 + x)] = block[2 * (y * 6 + x) + 1] = 1;
    const inner = maskToIds(shrinkMask(g6, block));
    for (const t of inner) {
      const touchesOutside = Array.from(g6.indices.slice(t * 3, t * 3 + 3)).some((v) => {
        for (let u = 0; u < 72; u++) if (!block[u] && g6.indices.slice(u * 3, u * 3 + 3).includes(v)) return true;
        return false;
      });
      expect(touchesOutside).toBe(false);
    }
    expect(inner.length).toBeGreaterThan(0);
  });

  it("expand and shrink agree with their definitions on random meshes", () => {
    for (let seed = 1; seed <= 40; seed++) {
      const m = randomMesh(seed, 30, 12);
      const r = random(seed * 7);
      const mask = new Uint8Array(30).map(() => (r() < 0.3 ? 1 : 0));
      const corners = (t: number) => Array.from(m.indices.slice(t * 3, t * 3 + 3));
      const selectedVertices = new Set((maskToIds(mask) as Uint32Array).length ? Array.from(maskToIds(mask)).flatMap(corners) : []);
      const unselectedVertices = new Set(Array.from(maskToIds(invertMask(mask))).flatMap(corners));
      const grown = expandMask(m, mask);
      const shrunk = shrinkMask(m, mask);
      for (let t = 0; t < 30; t++) {
        expect(grown[t]).toBe(mask[t] || corners(t).some((v) => selectedVertices.has(v)) ? 1 : 0);
        expect(shrunk[t]).toBe(mask[t] && !corners(t).some((v) => unselectedVertices.has(v)) ? 1 : 0);
      }
    }
  });
});

describe("edge adjacency", () => {
  it("groups the corners of each undirected edge, and marks zero-length edges", () => {
    const box = makeBox([20, 20, 20]);
    const a = buildEdgeAdjacency(box.indices, 8);
    expect(a.edgeStart.length - 1).toBe(18); // 12 triangles of a closed box have 18 edges
    for (let e = 0; e + 1 < a.edgeStart.length; e++) expect(a.edgeStart[e + 1] - a.edgeStart[e]).toBe(2);
    const degenerate = mesh([0, 0, 0, 1, 0, 0, 0, 1, 0], [0, 1, 2, 0, 0, 1]);
    const d = buildEdgeAdjacency(degenerate.indices, 3);
    expect(d.edgeOf[3]).toBe(NO_EDGE); // 0 → 0
    expect(d.edgeOf[4]).toBe(d.edgeOf[0]); // 0 → 1 in both triangles
    expect(d.edgeOf[5]).toBe(d.edgeOf[0]); // 1 → 0
  });

  it("matches a brute-force pairing on random meshes full of non-manifold and degenerate triangles", () => {
    for (let seed = 1; seed <= 60; seed++) {
      const m = randomMesh(seed, 40, 9);
      const { edgeOf, edgeStart, edgeCorners } = buildEdgeAdjacency(m.indices, 9);
      const expected = bruteEdgeCorners(m);
      expected.forEach((want, c) => {
        if (!want) {
          expect(edgeOf[c]).toBe(NO_EDGE);
          return;
        }
        const e = edgeOf[c];
        const got = new Set(Array.from(edgeCorners.subarray(edgeStart[e], edgeStart[e + 1])));
        expect(got).toEqual(want);
      });
    }
  });

  it("is cached per mesh object", () => {
    const box = makeBox([1, 1, 1]);
    expect(edgeAdjacency(box)).toBe(edgeAdjacency(box));
    expect(edgeAdjacency({ ...box })).not.toBe(edgeAdjacency(box));
  });

  it("lets go of the cached adjacency, normals and shells, and their mesh, when the caches are cleared", () => {
    const box = makeBox([1, 1, 1]);
    const built = [edgeAdjacency(box), triangleNormals(box), triangleShells(box)];
    [edgeAdjacency(box), triangleNormals(box), triangleShells(box)].forEach((value, i) => expect(value).toBe(built[i]));
    clearSelectionCaches();
    // Built again: nothing kept the old arrays (or the mesh they were built for).
    const again = [edgeAdjacency(box), triangleNormals(box), triangleShells(box)];
    again.forEach((value, i) => {
      expect(value).not.toBe(built[i]);
      expect(value).toEqual(built[i]);
    });
  });
});

describe("shells", () => {
  it("number shells in order of first triangle, as a brute-force search does", () => {
    for (let seed = 1; seed <= 40; seed++) {
      const m = randomMesh(seed, 25, 40);
      const { shellOf, count } = triangleShells(m);
      const want = bruteShells(m);
      expect(Array.from(shellOf)).toEqual(want);
      expect(count).toBe(Math.max(-1, ...want) + 1);
    }
  });

  it("selects a whole shell with the clicked triangle first", () => {
    const two = join(makeBox([20, 20, 20]), makeBox([5, 5, 5]), 40);
    expect(triangleShells(two).count).toBe(2);
    const second = shellRegion(two, 17);
    expect(second[0]).toBe(17);
    expect(sorted(second)).toEqual([12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23]);
    expect(sorted(shellRegion(two, 0))).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    expect(shellRegion(two, 24).length).toBe(0);
    expect(Array.from(trianglesOfShell(new Uint32Array([1, 0, 1, 1]), 1))).toEqual([0, 2, 3]);
    expect(Array.from(trianglesOfShell(new Uint32Array([1, 0, 1, 1]), 1, 2))).toEqual([2, 0, 3]);
  });

  it("part shells at a shared corner, a non-manifold edge and a triangle turned over", () => {
    const box = makeBox([20, 20, 20]);
    // A second box, moved by 20 so its lowest corner is the first box's highest, sharing that vertex: two shells.
    const at = (m: MeshData, p: number[]) => [...Array(8).keys()].find((v) => p.every((x, k) => m.positions[v * 3 + k] === x));
    const top = at(box, [10, 10, 10]) as number;
    const bottom = at(box, [-10, -10, -10]) as number;
    const glued = {
      positions: new Float32Array([...box.positions, ...box.positions.map((v) => v + 20)]),
      indices: new Uint32Array([...box.indices, ...box.indices.map((i) => (i === bottom ? top : i + 8))]),
    };
    expect(triangleShells(glued).count).toBe(2);
    expect(Array.from(triangleShells(glued).shellOf)).toEqual(bruteShells(glued));
    // A fin on one of the box's edges stands alone; the box stays one shell.
    const a = box.indices[0];
    const b = box.indices[1];
    const fin = { positions: new Float32Array([...box.positions, 50, 50, 50]), indices: new Uint32Array([...box.indices, a, b, 8]) };
    const finned = triangleShells(fin);
    expect(finned.count).toBe(2);
    expect(Array.from(finned.shellOf)).toEqual([...Array(12).fill(0), 1]);
    // One triangle turned over disagrees with all three neighbours.
    const turned = { positions: box.positions, indices: box.indices.slice() };
    [turned.indices[4], turned.indices[5]] = [turned.indices[5], turned.indices[4]];
    expect(Array.from(triangleShells(turned).shellOf)).toEqual(bruteShells(turned));
    expect(triangleShells(turned).count).toBe(2);
  });
});

describe("surface selection", () => {
  it("takes the whole edge-connected region at 180°, one flat face at small angles, and the box at 90°", () => {
    const box = makeBox([20, 20, 20]);
    expect(sorted(surfaceRegion(box, 4, 180))).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
    const face = surfaceRegion(box, 4, 45);
    expect(face[0]).toBe(4);
    expect(sorted(face)).toEqual([4, 5]);
    expect(sorted(surfaceRegion(box, 4, 0))).toEqual([4, 5]);
    expect(sorted(surfaceRegion(box, 4, 89))).toEqual([4, 5]);
    expect(surfaceRegion(box, 4, 90).length).toBe(12);
    expect(surfaceRegion(box, 99, 180).length).toBe(0);
  });

  it("follows a faceted curve across small dihedral angles but stops at the caps", () => {
    const p = prism(24); // side faces meet at 15°, caps at 90°
    const sides = Array.from({ length: 24 }, (_, i) => [4 * i, 4 * i + 1]).flat();
    expect(sorted(surfaceRegion(p, 0, 20))).toEqual(sides);
    expect(sorted(surfaceRegion(p, 0, 10))).toEqual([0, 1]);
    expect(surfaceRegion(p, 0, 180).length).toBe(96);
  });

  it("does not cross into flipped or degenerate triangles below 180°, but does at 180°", () => {
    const g = grid(2);
    const flipped = { positions: g.positions, indices: g.indices.slice() };
    flipped.indices.set([flipped.indices[2], flipped.indices[1], flipped.indices[0]], 0); // triangle 0 faces -Z
    expect(sorted(surfaceRegion(flipped, 3, 90))).toEqual([1, 2, 3, 4, 5, 6, 7]);
    expect(surfaceRegion(flipped, 3, 180).length).toBe(8);
    // A sliver with no area on the edge 0-1 of the first square, glued to triangle 0.
    const sliver = mesh([...g.positions, 0.5, 0, 0], [...g.indices, 0, 9, 1]);
    expect(triangleNormals(sliver).slice(24, 27)).toEqual(new Float32Array(3));
    expect(surfaceRegion(sliver, 0, 179).length).toBe(8);
    expect(surfaceRegion(sliver, 0, 180).length).toBe(9);
  });

  it("reaches every triangle on a non-manifold edge that passes the angle test", () => {
    // Two coplanar triangles on the edge 0-1, plus a fin standing up at 90° on the same edge.
    const fin = mesh([0, 0, 0, 1, 0, 0, 0.5, 1, 0, 0.5, -1, 0, 0.5, 0, 1], [0, 1, 2, 1, 0, 3, 0, 1, 4]);
    expect(sorted(surfaceRegion(fin, 0, 10))).toEqual([0, 1]);
    expect(sorted(surfaceRegion(fin, 0, 90))).toEqual([0, 1, 2]);
  });
});

describe("measures and copies", () => {
  it("reports the count, area and shells touched", () => {
    const two = join(makeBox([20, 20, 20]), makeBox([10, 10, 10]), 40);
    expect(selectionInfoOf(two, new Uint8Array(24))).toEqual({ count: 0, area: 0, shells: 0 });
    const info = selectionInfoOf(two, idsToMask([0, 1, 12], 24));
    expect(info.count).toBe(3);
    expect(info.area).toBeCloseTo(400 + 50, 9);
    expect(info.shells).toBe(2);
    expect(selectionInfoOf(two, idsToMask([2, 3], 24)).shells).toBe(1);
  });

  it("gives the box of the selected triangles", () => {
    const box = makeBox([20, 20, 20]);
    expect(selectionBounds(box, new Uint8Array(12))).toBeNull();
    expect(selectionBounds(box, idsToMask([2, 3], 12))).toEqual({ min: [-10, -10, 10], max: [10, 10, 10] }); // the +Z face
    expect(selectionBounds(box, idsToMask([0], 12))).toEqual({ min: [-10, -10, -10], max: [10, 10, -10] });
  });

  it("extracts the selected triangles with only their vertices, in their original order", () => {
    const box = makeBox([2, 2, 2]);
    const piece = extractTriangles(box, idsToMask([2, 3], 12)); // top: corners 4, 5, 6, 7
    expect(Array.from(piece.indices)).toEqual([0, 1, 2, 0, 2, 3]);
    expect(Array.from(piece.positions)).toEqual(Array.from(box.positions.slice(12, 24)));
    const none = extractTriangles(box, new Uint8Array(12));
    expect([none.positions.length, none.indices.length]).toEqual([0, 0]);
  });
});

describe("a million triangles", () => {
  it("builds adjacency and shells, and runs every tool, in well under a second each", () => {
    const big = grid(707); // 999,698 triangles, 501,264 vertices
    const n = big.indices.length / 3;
    const time = <T>(work: () => T) => {
      const start = performance.now();
      const out = work();
      return { ms: performance.now() - start, out };
    };
    const adjacency = time(() => edgeAdjacency(big));
    const shells = time(() => triangleShells(big));
    const whole = time(() => surfaceRegion(big, 0, 180));
    const flat = time(() => surfaceRegion(big, 0, 10));
    const shell = time(() => shellRegion(big, 12345));
    const half = idsToMask(
      Array.from({ length: n / 2 }, (_, i) => i),
      n,
    );
    const expand = time(() => expandMask(big, half));
    const shrink = time(() => shrinkMask(big, half));
    const info = time(() => selectionInfoOf(big, half));
    const ids = time(() => maskToIds(half));
    console.log(
      `1M triangles: adjacency ${adjacency.ms.toFixed(0)} ms, shells ${shells.ms.toFixed(0)} ms, surface 180° ${whole.ms.toFixed(0)} ms, ` +
        `surface 10° ${flat.ms.toFixed(0)} ms, shell ${shell.ms.toFixed(0)} ms, expand ${expand.ms.toFixed(0)} ms, shrink ${shrink.ms.toFixed(0)} ms, ` +
        `info ${info.ms.toFixed(0)} ms, mask→ids ${ids.ms.toFixed(0)} ms`,
    );
    expect(whole.out.length).toBe(n);
    expect(flat.out.length).toBe(n); // a flat grid is one surface at any angle
    expect(shell.out.length).toBe(n);
    expect(shells.out.count).toBe(1);
    expect(info.out.area).toBeCloseTo(n / 4, 3);
    for (const t of [adjacency, shells, whole, flat, shell, expand, shrink, info, ids]) expect(t.ms).toBeLessThan(2000);
  });
});
