import { describe, expect, it } from "vitest";
import {
  cachedExactArea,
  computeAreasInBackground,
  infoRows,
  linearDet,
  measureExactArea,
  NOT_CLOSED,
  sameAreaWork,
  selectionText,
  similarityScale2,
  SYNC_AREA_TRIANGLES,
  transformedArea,
  worldVolume,
  type InfoRow,
} from "./info";
import type { MeshStats } from "./io/ipc";
import { IDENTITY, makeBox, translation } from "./mesh";
import type { MeshData, Part } from "./store";

// ---------- Column-major matrix helpers ----------

function mul(a: readonly number[], b: readonly number[]): number[] {
  const out = new Array<number>(16).fill(0);
  for (let c = 0; c < 4; c++)
    for (let r = 0; r < 4; r++) for (let k = 0; k < 4; k++) out[c * 4 + r] += a[k * 4 + r] * b[c * 4 + k];
  return out;
}
const compose = (...ms: number[][]) => ms.reduce((acc, m) => mul(acc, m), [...IDENTITY]);
const scale = (x: number, y: number, z: number) => [x, 0, 0, 0, 0, y, 0, 0, 0, 0, z, 0, 0, 0, 0, 1];
function rotX(t: number) {
  const c = Math.cos(t), s = Math.sin(t);
  return [1, 0, 0, 0, 0, c, s, 0, 0, -s, c, 0, 0, 0, 0, 1];
}
function rotZ(t: number) {
  const c = Math.cos(t), s = Math.sin(t);
  return [c, s, 0, 0, -s, c, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];
}
const shearXY = (k: number) => [1, 0, 0, 0, k, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

const ROTATION = compose(rotZ(0.7), rotX(-1.2));
const MATRICES: Record<string, number[]> = {
  identity: [...IDENTITY],
  rotation: compose(translation(5, -3, 8), ROTATION),
  "uniform scale": compose(ROTATION, scale(2.5, 2.5, 2.5)),
  "non-uniform scale": compose(translation(1, 2, 3), scale(2, 0.5, 3)),
  mirror: compose(ROTATION, scale(-1, 1, 1)),
  "mirrored non-uniform with shear": compose(ROTATION, shearXY(0.4), scale(1.5, -2, 0.7)),
  "flattened to a plane": scale(1, 2, 0),
};

// ---------- Brute-force references: transform every vertex, then measure ----------

function worldPoint(p: Float32Array, i: number, m: readonly number[]): [number, number, number] {
  const x = p[i * 3], y = p[i * 3 + 1], z = p[i * 3 + 2];
  return [
    m[0] * x + m[4] * y + m[8] * z + m[12],
    m[1] * x + m[5] * y + m[9] * z + m[13],
    m[2] * x + m[6] * y + m[10] * z + m[14],
  ];
}

function bruteForce(mesh: MeshData, m: readonly number[]): { area: number; volume: number } {
  let area = 0;
  let volume = 0;
  for (let t = 0; t < mesh.indices.length; t += 3) {
    const [a, b, c] = [0, 1, 2].map((k) => worldPoint(mesh.positions, mesh.indices[t + k], m));
    const u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    const v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    const n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    area += Math.hypot(n[0], n[1], n[2]) / 2;
    // Signed tetrahedron from the origin: a · (b × c) / 6.
    volume += (a[0] * (b[1] * c[2] - b[2] * c[1]) + a[1] * (b[2] * c[0] - b[0] * c[2]) + a[2] * (b[0] * c[1] - b[1] * c[0])) / 6;
  }
  return { area, volume };
}

/** A closed but irregular mesh: the box with every vertex moved a little, so no two faces match. */
function lumpyBox(): MeshData {
  const box = makeBox([20, 14, 9]);
  const p = Float32Array.from(box.positions, (v, i) => v + Math.sin(i * 1.7) * 1.3 + 4);
  return { positions: p, indices: box.indices };
}

/** Deterministic triangle soup with varied sizes and orientations. */
function soup(triangles: number): MeshData {
  let seed = 12345;
  const rnd = () => ((seed = (seed * 1103515245 + 12345) % 2147483648) / 2147483648) * 40 - 20;
  const positions = Float32Array.from({ length: triangles * 9 }, rnd);
  const indices = Uint32Array.from({ length: triangles * 3 }, (_, i) => i);
  return { positions, indices };
}

/** A flat grid of 2 × cells² triangles, cheap to build at any size. */
function grid(cells: number): MeshData {
  const side = cells + 1;
  const positions = new Float32Array(side * side * 3);
  for (let y = 0, k = 0; y < side; y++)
    for (let x = 0; x < side; x++) {
      positions[k++] = x * 0.5;
      positions[k++] = y * 0.5;
      positions[k++] = Math.sin(x * 0.3 + y * 0.2);
    }
  const indices = new Uint32Array(cells * cells * 6);
  for (let y = 0, t = 0; y < cells; y++)
    for (let x = 0; x < cells; x++) {
      const a = y * side + x;
      indices.set([a, a + 1, a + side + 1, a, a + side + 1, a + side], t);
      t += 6;
    }
  return { positions, indices };
}

function part(mesh: MeshData, matrix: readonly number[] = IDENTITY, id = "p"): Part {
  return { id, name: id, mesh, matrix: [...matrix], visible: true, transparent: false, locked: false, color: "#000" };
}

/** Local stats as Rust reports them, measured here by brute force. */
function localStats(mesh: MeshData, extra: Partial<MeshStats> = {}): MeshStats {
  const { area, volume } = bruteForce(mesh, IDENTITY);
  return {
    triangles: mesh.indices.length / 3,
    vertices: mesh.positions.length / 3,
    volume,
    area,
    shells: 1,
    openEdges: 0,
    nonManifoldEdges: 0,
    ...extra,
  };
}

const texts = (rows: InfoRow[]) => Object.fromEntries(rows.map((r) => [r.label, r.text]));
const close = (actual: number, expected: number, rel = 1e-9) =>
  expect(Math.abs(actual - expected)).toBeLessThanOrEqual(rel * Math.max(1, Math.abs(expected)));

// ---------- Tests ----------

describe("matrix classification", () => {
  it("computes the determinant of the linear part", () => {
    expect(linearDet(IDENTITY)).toBe(1);
    expect(linearDet(translation(4, 5, 6))).toBe(1);
    close(linearDet(scale(2, 3, 4)), 24);
    close(linearDet(MATRICES.rotation), 1);
    close(linearDet(MATRICES.mirror), -1);
    close(linearDet(MATRICES["mirrored non-uniform with shear"]), 1.5 * -2 * 0.7);
  });

  it("recognises rotation × uniform scale, mirrored or not, as a similarity", () => {
    expect(similarityScale2(IDENTITY)).toBe(1);
    close(similarityScale2(MATRICES.rotation)!, 1);
    close(similarityScale2(MATRICES["uniform scale"])!, 6.25);
    close(similarityScale2(MATRICES.mirror)!, 1);
    expect(similarityScale2(scale(0, 0, 0))).toBe(0);
    // Float noise from repeated gizmo steps stays a similarity.
    const noisy = [...MATRICES.rotation];
    noisy[0] += 1e-12;
    expect(similarityScale2(noisy)).not.toBeNull();
  });

  it("rejects non-uniform scale and shear", () => {
    expect(similarityScale2(MATRICES["non-uniform scale"])).toBeNull();
    expect(similarityScale2(shearXY(0.01))).toBeNull();
    expect(similarityScale2(scale(1, 1, 1.001))).toBeNull();
    expect(similarityScale2(MATRICES["flattened to a plane"])).toBeNull();
  });
});

describe("world area and volume", () => {
  const meshes = { "lumpy box": lumpyBox(), "triangle soup": soup(300) };

  for (const [meshName, mesh] of Object.entries(meshes)) {
    const local = localStats(mesh);
    for (const [name, m] of Object.entries(MATRICES)) {
      it(`exact area pass matches brute force: ${meshName}, ${name}`, () => {
        close(transformedArea(mesh, m), bruteForce(mesh, m).area);
      });

      it(`panel area matches brute force (similarity shortcut or exact pass): ${meshName}, ${name}`, () => {
        const text = texts(infoRows([part(mesh, m)], () => local).rows)["Area (cm²)"];
        expect(Math.abs(Number(text.replace(/,/g, "")) - bruteForce(mesh, m).area / 100)).toBeLessThanOrEqual(0.0005);
      });
    }
  }

  const box = lumpyBox();
  const boxLocal = bruteForce(box, IDENTITY).volume;
  for (const [name, m] of Object.entries(MATRICES)) {
    it(`volume is |det × local volume|: ${name}`, () => {
      close(worldVolume(boxLocal, m), Math.abs(bruteForce(box, m).volume));
    });
  }

  it("reports a mirrored part with the same, positive volume", () => {
    expect(bruteForce(box, MATRICES.mirror).volume).toBeLessThan(0);
    close(worldVolume(boxLocal, MATRICES.mirror), boxLocal);
  });

  it("reports an inside-out mesh's material volume as positive", () => {
    const flipped = Uint32Array.from(box.indices);
    for (let t = 0; t < flipped.length; t += 3) [flipped[t + 1], flipped[t + 2]] = [flipped[t + 2], flipped[t + 1]];
    const inverted = bruteForce({ positions: box.positions, indices: flipped }, IDENTITY).volume;
    expect(inverted).toBeLessThan(0);
    close(worldVolume(inverted, scale(2, 1, 1)), 2 * boxLocal);
  });

  it("sums slices to the whole", () => {
    const mesh = soup(100);
    const m = MATRICES["mirrored non-uniform with shear"];
    close(transformedArea(mesh, m, 0, 37) + transformedArea(mesh, m, 37, 100), transformedArea(mesh, m));
  });
});

describe("info rows", () => {
  const cube = makeBox([20, 20, 20]);
  const cubeStats: MeshStats = { triangles: 12, vertices: 8, volume: 8000, area: 2400, shells: 1, openEdges: 0, nonManifoldEdges: 0 };

  it("shows the known values of a 20 mm cube", () => {
    const { rows, pendingArea } = infoRows([part(cube, translation(100, 100, 10))], () => cubeStats);
    expect(texts(rows)).toEqual({
      "Length (mm)": "20.00",
      "Width (mm)": "20.00",
      "Height (mm)": "20.00",
      "Volume (cm³)": "8.000",
      "Area (cm²)": "24.000",
      Triangles: "12",
      Shells: "1",
      "Open edges": "0",
    });
    expect(rows.every((r) => r.status === "value" && !r.warning)).toBe(true);
    expect(pendingArea).toEqual([]);
  });

  it("follows the transform without new stats", () => {
    const m = compose(translation(3, 3, 3), rotZ(Math.PI / 4), scale(1, 2, 0.5));
    const t = texts(infoRows([part(cube, m)], () => cubeStats).rows);
    const diag = (20 + 40) / Math.SQRT2; // a 20 × 40 rectangle turned 45° about Z
    expect(t["Length (mm)"]).toBe(diag.toFixed(2));
    expect(t["Width (mm)"]).toBe(diag.toFixed(2));
    expect(t["Height (mm)"]).toBe("10.00");
    expect(t["Volume (cm³)"]).toBe("8.000");
    // 2 × (20·40 + 20·10 + 40·10) mm² = 2,800 mm².
    expect(t["Area (cm²)"]).toBe("28.000");
  });

  it("combines several parts: union box, summed stats, separated thousands", () => {
    const big: MeshStats = { ...cubeStats, triangles: 1_234_567, shells: 3, openEdges: 0 };
    const parts = [part(cube, translation(10, 10, 10), "a"), part(cube, compose(translation(50, 10, 10), scale(2, 2, 2)), "b")];
    const t = texts(infoRows(parts, (mesh) => (mesh === cube ? big : undefined)).rows);
    expect(t["Length (mm)"]).toBe("70.00"); // 0..20 and 30..70 along X
    expect(t["Height (mm)"]).toBe("40.00"); // 0..30 and -10..30 along Z
    expect(t["Volume (cm³)"]).toBe("72.000"); // 8 + 64
    expect(t["Area (cm²)"]).toBe("120.000"); // 24 + 96
    expect(t.Triangles).toBe("2,469,134");
    expect(t.Shells).toBe("6");
  });

  it("shows a warning instead of the volume when a mesh is open", () => {
    const parts = [part(cube, IDENTITY, "a"), part(makeBox([1, 1, 1]), IDENTITY, "b")];
    const rows = infoRows(parts, (mesh) => (mesh === cube ? cubeStats : { ...cubeStats, openEdges: 4 })).rows;
    const volume = rows.find((r) => r.label === "Volume (cm³)")!;
    expect(volume).toMatchObject({ text: "—", status: "none", warning: NOT_CLOSED });
    expect(texts(rows)["Open edges"]).toBe("4");
    expect(texts(rows)["Area (cm²)"]).toBe("48.000");
  });

  it("shows … while stats load and — when they are unavailable; dimensions never wait", () => {
    const loadingRows = infoRows([part(cube)], () => undefined).rows;
    expect(texts(loadingRows)["Length (mm)"]).toBe("20.00");
    expect(loadingRows.slice(3).map((r) => r.text)).toEqual(["…", "…", "…", "…", "…"]);
    expect(loadingRows.slice(3).every((r) => r.status === "loading")).toBe(true);

    const missing = infoRows([part(cube)], () => null).rows;
    expect(texts(missing)["Height (mm)"]).toBe("20.00");
    expect(missing.slice(3).map((r) => r.text)).toEqual(["—", "—", "—", "—", "—"]);

    // Unavailable wins over loading: the total can never be known.
    const other = makeBox([1, 1, 1]);
    const mixed = infoRows([part(cube, IDENTITY, "a"), part(other, IDENTITY, "b")], (m) => (m === cube ? undefined : null));
    expect(mixed.rows.slice(3).every((r) => r.status === "none")).toBe(true);
  });

  it("shows — everywhere when there is nothing to describe", () => {
    const rows = infoRows([], () => cubeStats).rows;
    expect(rows).toHaveLength(8);
    expect(rows.every((r) => r.text === "—" && r.status === "none")).toBe(true);
  });

  it("needs the exact area only for non-similarity matrices, and waits for it", () => {
    const box = makeBox([20, 20, 20]); // its own mesh, so nothing is cached yet
    const uniform = part(box, MATRICES["uniform scale"], "u");
    const stretched = part(box, MATRICES["non-uniform scale"], "s");
    // No render budget: whatever needs a pass is left pending, and only that.
    const result = infoRows([uniform, stretched], () => cubeStats, 0);
    expect(result.pendingArea).toEqual([stretched]);
    expect(cachedExactArea(stretched)).toBeUndefined();
    expect(texts(result.rows)["Area (cm²)"]).toBe("…");
    expect(texts(result.rows)["Volume (cm³)"]).toBe((8 * 15.625 + 8 * 3).toFixed(3));
  });

  it("measures at most the render budget per update, in total over all parts", () => {
    // Three 100-triangle meshes under a non-uniform scale and a budget of 250: two fit, one waits.
    const parts = [soup(100), soup(100), soup(100)].map((mesh, i) => part(mesh, MATRICES["non-uniform scale"], `p${i}`));
    const statsOf = (mesh: MeshData) => localStats(mesh);
    const first = infoRows(parts, statsOf, 250);
    expect(first.pendingArea).toEqual([parts[2]]);
    expect(texts(first.rows)["Area (cm²)"]).toBe("…");
    // The next update measures the rest; the first two are cached.
    const second = infoRows(parts, statsOf, 100);
    expect(second.pendingArea).toEqual([]);
    const expected = parts.reduce((sum, p) => sum + bruteForce(p.mesh, p.matrix).area, 0) / 100;
    expect(Math.abs(Number(texts(second.rows)["Area (cm²)"].replace(/,/g, "")) - expected)).toBeLessThanOrEqual(0.0005);
  });

  it("formats the selection count", () => {
    expect(selectionText(1, 1)).toBe("1 of 1 part selected");
    expect(selectionText(0, 3)).toBe("0 of 3 parts selected");
    expect(selectionText(2, 1500)).toBe("2 of 1,500 parts selected");
  });
});

describe("exact area cache", () => {
  /** The part moved by (dx, dy, dz): a new Part object with the same mesh and linear part. */
  const moved = (p: Part, dx: number, dy = 0, dz = 0) => part(p.mesh, compose(translation(dx, dy, dz), p.matrix), p.id);
  const run = (parts: Part[]) => new Promise<void>((resolve) => computeAreasInBackground(parts, resolve));

  it("caches per Part object", () => {
    const p = part(soup(50), MATRICES["non-uniform scale"]);
    expect(cachedExactArea(p)).toBeUndefined();
    const area = measureExactArea(p);
    close(area, bruteForce(p.mesh, p.matrix).area);
    expect(cachedExactArea(p)).toBe(area);
  });

  it("reuses the area after a move, and after undo to an earlier linear part", () => {
    const a = part(soup(80), MATRICES["mirrored non-uniform with shear"]);
    const area = measureExactArea(a);
    // A move only changes the translation: no budget needed, nothing pending.
    const result = infoRows([moved(a, 40, -7, 3)], (mesh) => localStats(mesh), 0);
    expect(result.pendingArea).toEqual([]);
    expect(texts(result.rows)["Area (cm²)"]).toBe((area / 100).toFixed(3));
    // Scale it, then go back to the first linear part somewhere else.
    measureExactArea(part(a.mesh, compose(a.matrix, scale(1, 2, 1))));
    expect(cachedExactArea(moved(a, -12))).toBe(area);
    // A different linear part is not a hit.
    expect(cachedExactArea(part(a.mesh, compose(a.matrix, scale(1, 1, 1.5))))).toBeUndefined();
  });

  it("tells when two pending lists need the same measurements", () => {
    const a = part(soup(10), MATRICES["non-uniform scale"], "a");
    const b = part(soup(10), MATRICES["non-uniform scale"], "b");
    expect(sameAreaWork([a, b], [moved(a, 5), moved(b, 0, 9)])).toBe(true);
    expect(sameAreaWork([a, b], [a, part(b.mesh, compose(b.matrix, scale(1.1, 1, 1)), "b")])).toBe(false);
    expect(sameAreaWork([a, b], [a, part(a.mesh, b.matrix, "b")])).toBe(false);
    expect(sameAreaWork([a, b], [a])).toBe(false);
    expect(sameAreaWork([], [])).toBe(true);
  });

  it("leaves big parts to the background run, which yields between slices and caches the result", async () => {
    const mesh = grid(500); // 500,000 triangles: more than one slice
    expect(mesh.indices.length / 3).toBeGreaterThan(SYNC_AREA_TRIANGLES);
    const p = part(mesh, MATRICES["mirrored non-uniform with shear"]);
    expect(infoRows([p], () => localStats(soup(1))).pendingArea).toEqual([p]);
    expect(cachedExactArea(p)).toBeUndefined();

    let done = 0;
    await new Promise<void>((resolve) =>
      computeAreasInBackground([p], () => {
        done += 1;
        resolve();
      }),
    );
    expect(done).toBe(1);
    close(cachedExactArea(p)!, transformedArea(mesh, p.matrix));
    expect(infoRows([p], () => localStats(soup(1))).pendingArea).toEqual([]);
    expect(infoRows([moved(p, 100)], () => localStats(soup(1))).pendingArea).toEqual([]);
  });

  it("measures many parts in shared slices and reports once", async () => {
    const parts = Array.from({ length: 6 }, (_, i) => part(grid(120), compose(translation(i * 80, 0, 0), scale(1 + i, 1, 2)), `p${i}`));
    let done = 0;
    await new Promise<void>((resolve) =>
      computeAreasInBackground(parts, () => {
        done += 1;
        resolve();
      }),
    );
    await new Promise((r) => setTimeout(r, 20));
    expect(done).toBe(1);
    for (const p of parts) close(cachedExactArea(p)!, transformedArea(p.mesh, p.matrix));
  });

  it("settles when one mesh is used under more linear parts than it remembers", async () => {
    // Twenty copies of one mesh, each stretched differently, all pending: a run must leave every
    // one of them cached, or the panel would start a new run after each one forever.
    const mesh = soup(40);
    const copies = Array.from({ length: 20 }, (_, i) => part(mesh, scale(1.1 + i / 10, 1, 1), `c${i}`));
    const statsOf = () => localStats(mesh);
    const pending = infoRows(copies, statsOf, 0).pendingArea;
    expect(pending).toHaveLength(20);
    await run(pending);
    expect(infoRows(copies, statsOf, 0).pendingArea).toEqual([]);
  });

  it("skips parts measured meanwhile, and a copy with the same linear part costs nothing", async () => {
    const a = part(soup(30), MATRICES["non-uniform scale"], "a");
    const copy = moved(a, 50);
    await run([a, copy]);
    expect(cachedExactArea(copy)).toBe(cachedExactArea(a));
  });

  it("stops when cancelled", async () => {
    const p = part(grid(400), MATRICES["non-uniform scale"]);
    let done = 0;
    const cancel = computeAreasInBackground([p], () => (done += 1));
    cancel();
    await new Promise((r) => setTimeout(r, 30));
    expect(done).toBe(0);
    expect(cachedExactArea(p)).toBeUndefined();
  });
});
