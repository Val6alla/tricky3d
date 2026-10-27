// World-space copies of part meshes for the manifold-3d worker, made exactly like the Rust jobs make
// theirs, so a part cut by manifold-3d and a part cut by the Rust clipper start from the same vertices.
import { IDENTITY } from "../mesh";
import type { MeshData } from "../store";

/**
 * `mesh` with the column-major matrix `m` baked into its vertices: the same maths as Rust mesh::bake.
 * Each coordinate is `m[0] * x + m[4] * y + m[8] * z + m[12]` in f64, summed left to right like the
 * Rust expression, then rounded to f32 once by the Float32Array store (round-half-even, as Rust's
 * `as f32`). Rounding per step instead would move some vertices by an f32 ulp and make the two cut
 * paths disagree. A mirroring matrix (negative determinant) swaps each triangle's 2nd and 3rd corners
 * so the surface keeps facing outwards.
 *
 * Returns `mesh` itself for the identity matrix, and shares its index array when the winding stays:
 * meshes are never mutated, so sharing is safe and saves a copy of millions of indices.
 */
export function bakeMesh(mesh: MeshData, m: readonly number[]): MeshData {
  if (IDENTITY.every((v, i) => m[i] === v)) return mesh;
  const p = mesh.positions;
  const positions = new Float32Array(p.length);
  for (let i = 0; i < p.length; i += 3) {
    const x = p[i];
    const y = p[i + 1];
    const z = p[i + 2];
    positions[i] = m[0] * x + m[4] * y + m[8] * z + m[12];
    positions[i + 1] = m[1] * x + m[5] * y + m[9] * z + m[13];
    positions[i + 2] = m[2] * x + m[6] * y + m[10] * z + m[14];
  }
  // `< 0` rather than `>= 0` below, so a NaN determinant keeps the winding, as in Rust.
  const mirrored = determinant(m) < 0;
  if (!mirrored) return { positions, indices: mesh.indices };
  const indices = mesh.indices.slice();
  for (let t = 0; t + 2 < indices.length; t += 3) {
    const second = indices[t + 1];
    indices[t + 1] = indices[t + 2];
    indices[t + 2] = second;
  }
  return { positions, indices };
}

/** Determinant of the 3×3 linear part, term for term as Rust mesh::linear_determinant. */
function determinant(m: readonly number[]): number {
  return m[0] * (m[5] * m[10] - m[9] * m[6]) - m[4] * (m[1] * m[10] - m[9] * m[2]) + m[8] * (m[1] * m[6] - m[5] * m[2]);
}

/**
 * `mesh` with the vertices that share a position joined, as import welds a file: equal f32 bit patterns are one
 * position (-0 counts as +0). Baking can move vertices that were apart onto one position, since f32 has fewer steps
 * far from the origin, and the repair actions would take them for a seam that is not joined. Each vertex becomes the
 * first vertex at its position, the others go, and the triangles keep their order (degenerate ones too). Returns
 * `mesh` itself when no two vertices share a position.
 */
export function weldMesh(mesh: MeshData): MeshData {
  const count = mesh.positions.length / 3;
  const bits = new Uint32Array(mesh.positions.buffer, mesh.positions.byteOffset, count * 3);
  const key = (i: number) => (bits[i] === 0x80000000 ? 0 : bits[i]);
  let size = 64;
  while (size < count * 2) size *= 2;
  // Open addressing with linear probing, at most half full: no allocation per vertex.
  const slots = new Int32Array(size).fill(-1);
  const first = new Uint32Array(count);
  let joined = 0;
  for (let v = 0; v < count; v++) {
    const x = key(v * 3);
    const y = key(v * 3 + 1);
    const z = key(v * 3 + 2);
    let h = Math.imul(x ^ Math.imul(y, 0x9e3779b1) ^ Math.imul(z, 0x85ebca77), 0xc2b2ae3d);
    h = Math.imul(h ^ (h >>> 15), 0x27d4eb2f);
    let slot = (h ^ (h >>> 13)) & (size - 1);
    for (;;) {
      const u = slots[slot];
      if (u < 0) {
        slots[slot] = v;
        first[v] = v;
        break;
      }
      if (key(u * 3) === x && key(u * 3 + 1) === y && key(u * 3 + 2) === z) {
        first[v] = u;
        joined++;
        break;
      }
      slot = (slot + 1) & (size - 1);
    }
  }
  if (joined === 0) return mesh;
  const id = new Uint32Array(count);
  const positions = new Float32Array((count - joined) * 3);
  let next = 0;
  for (let v = 0; v < count; v++) {
    if (first[v] !== v) continue;
    id[v] = next;
    positions.set(mesh.positions.subarray(v * 3, v * 3 + 3), next * 3);
    next++;
  }
  const indices = new Uint32Array(mesh.indices.length);
  for (let i = 0; i < indices.length; i++) indices[i] = id[first[mesh.indices[i]]];
  return { positions, indices };
}
