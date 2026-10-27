import { describe, expect, it } from "vitest";
import { IDENTITY, makeBox, translation } from "../mesh";
import type { MeshData } from "../store";
import { bakeMesh, weldMesh } from "./bake";

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

describe("bakeMesh", () => {
  it("returns the mesh itself for the identity matrix", () => {
    const mesh = makeBox([20, 20, 20]);
    expect(bakeMesh(mesh, IDENTITY)).toBe(mesh);
    expect(bakeMesh(mesh, [...IDENTITY])).toBe(mesh);
  });

  it("moves vertices and shares the indices when the winding stays", () => {
    const mesh = makeBox([20, 20, 20]);
    const baked = bakeMesh(mesh, translation(10, 0, 10));
    expect(baked.indices).toBe(mesh.indices);
    expect(Array.from(baked.positions.subarray(0, 3))).toEqual([0, -10, 0]);
    expect(volume(baked)).toBeCloseTo(8000, 9);
  });

  it("reverses each triangle of a mirrored mesh so the volume stays positive", () => {
    const mesh = makeBox([20, 20, 20]);
    const mirror = [...IDENTITY];
    mirror[0] = -1;
    const baked = bakeMesh(mesh, mirror);
    expect(Array.from(baked.indices.subarray(0, 6))).toEqual([0, 1, 2, 0, 2, 3]);
    expect(Array.from(mesh.indices.subarray(0, 6))).toEqual([0, 2, 1, 0, 3, 2]);
    expect(volume(baked)).toBeCloseTo(8000, 9);
  });

  it("rounds each coordinate to f32 once, after the f64 sum, like Rust", () => {
    // 1 + 2^-24 + 2^-50 is just above half an f32 ulp past 1, so one rounding goes up to 1 + 2^-23.
    // Rounding the translation to f32 first would leave exactly half an ulp, which rounds to even: 1.
    const mesh: MeshData = { positions: new Float32Array([1, 0, 0]), indices: new Uint32Array(0) };
    const baked = bakeMesh(mesh, translation(2 ** -24 + 2 ** -50, 0, 0));
    expect(baked.positions[0]).toBe(1 + 2 ** -23);
  });

  it("matches the Rust bake bit for bit on a rotated, scaled, mirrored matrix", () => {
    // Expected bits from running the app's Rust mesh::bake on the same inputs.
    const m = [0.7071067811865476, 0.7071067811865475, 0, 0, -1.4142135623730951, 1.4142135623730954, 0, 0, 0, 0, -1, 0, 12.3, -4.56, 7.89, 1];
    const mesh: MeshData = { positions: new Float32Array([0.1, 0.2, 0.3, -17.25, 3.3333333, 1e-3]), indices: new Uint32Array([0, 1, 0]) };
    const baked = bakeMesh(mesh, m);
    expect(Array.from(new Uint32Array(baked.positions.buffer))).toEqual(RUST_BITS);
    expect(Array.from(baked.indices)).toEqual([0, 0, 1]);
  });

  it("never changes the input", () => {
    const mesh = makeBox([20, 10, 5]);
    const positions = mesh.positions.slice();
    const indices = mesh.indices.slice();
    const mirror = [...IDENTITY];
    mirror[10] = -2;
    bakeMesh(mesh, mirror);
    expect(mesh.positions).toEqual(positions);
    expect(mesh.indices).toEqual(indices);
  });
});

describe("weldMesh", () => {
  it("returns the mesh itself when every vertex has a position of its own", () => {
    const mesh = makeBox([20, 20, 20]);
    expect(weldMesh(mesh)).toBe(mesh);
  });

  it("joins vertices a bake moved onto one position, keeping every triangle in order", () => {
    // 0 and 2^-23 are apart; moved by 2^24 both land on 2^24 (the f32 step there is 2).
    const mesh: MeshData = {
      positions: new Float32Array([0, 0, 0, 2 ** -23, 0, 0, 0, 1, 0, 0, 0, 1]),
      indices: new Uint32Array([0, 2, 3, 1, 3, 2, 0, 1, 2]),
    };
    expect(weldMesh(mesh)).toBe(mesh);
    const baked = bakeMesh(mesh, translation(2 ** 24, 0, 0));
    const welded = weldMesh(baked);
    expect(Array.from(welded.positions)).toEqual([2 ** 24, 0, 0, 2 ** 24, 1, 0, 2 ** 24, 0, 1]);
    expect(Array.from(welded.indices)).toEqual([0, 1, 2, 0, 2, 1, 0, 0, 1]);
    expect(Array.from(baked.positions)).toHaveLength(12);
  });

  it("counts -0 as +0, as the import weld does", () => {
    const mesh: MeshData = { positions: new Float32Array([0, 0, 0, -0, 0, -0, 1, 0, 0]), indices: new Uint32Array([0, 1, 2]) };
    const welded = weldMesh(mesh);
    expect(welded.positions).toHaveLength(6);
    expect(Array.from(welded.indices)).toEqual([0, 0, 1]);
  });
});

/** f32 bit patterns src-tauri/src/mesh.rs `bake` produced for the matrix and vertices above. */
const RUST_BITS = [1094805480, 3230047030, 1089659208, 3230896776, 3242242654, 1090286256];
