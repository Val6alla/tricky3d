/**
 * Line-segment positions (x, y, z, x, y, z per edge) for every edge that is not shared by exactly
 * two triangles: open edges (one triangle) and non-manifold edges (three or more).
 */
export function problemEdgePositions(positions: Float32Array, indices: Uint32Array): Float32Array {
  return edgeSegments(positions, indices, null, 0);
}

/**
 * Line-segment positions for "shaded + edges": creases where two triangles' normals differ by at
 * least `angleDeg`, plus every edge not shared by exactly two triangles. On a welded mesh this
 * matches three's EdgesGeometry, which is far too slow for big meshes (a string key per edge).
 */
export function featureEdgePositions(positions: Float32Array, indices: Uint32Array, angleDeg: number): Float32Array {
  return edgeSegments(positions, indices, triangleNormals(positions, indices), Math.cos((angleDeg * Math.PI) / 180));
}

/**
 * Keeps the edges used by other than two triangles and, when `normals` is given, the edges whose
 * two triangles' normals have a dot product <= minDot.
 */
function edgeSegments(positions: Float32Array, indices: Uint32Array, normals: Float32Array | null, minDot: number): Float32Array {
  const vertexCount = positions.length / 3;
  const { start, keys, scale } = edgeBuckets(indices, vertexCount);
  const kept: number[] = []; // lower, higher vertex per kept edge
  for (let lo = 0; lo < vertexCount; lo++) {
    const end = start[lo + 1];
    for (let i = start[lo]; i < end; ) {
      const hi = Math.floor(keys[i] / scale);
      let j = i + 1;
      while (j < end && Math.floor(keys[j] / scale) === hi) j++;
      if (j - i !== 2 || (normals && dot(normals, keys[i] - hi * scale, keys[i + 1] - hi * scale) <= minDot)) kept.push(lo, hi);
      i = j;
    }
  }

  const out = new Float32Array(kept.length * 3);
  for (let k = 0; k < kept.length; k++) {
    const v = kept[k] * 3;
    out[k * 3] = positions[v];
    out[k * 3 + 1] = positions[v + 1];
    out[k * 3 + 2] = positions[v + 2];
  }
  return out;
}

/**
 * Every triangle edge as the key higher vertex * scale + triangle, bucketed by its lower vertex
 * (counting sort) and sorted within the bucket, so the copies of one edge sit next to each other.
 * Linear time with no hash map, which matters for meshes with millions of triangles. `scale` is a
 * power of two, so keys split back exactly while vertexCount * scale < 2^53.
 * Degenerate triangles repeat an index; their zero-length edges are left out.
 */
function edgeBuckets(indices: Uint32Array, vertexCount: number) {
  const end = indices.length - (indices.length % 3);
  const scale = 2 ** Math.ceil(Math.log2(end / 3 + 1));
  const start = new Uint32Array(vertexCount + 1);
  for (let c = 0; c < end; c++) {
    const a = indices[c];
    const b = indices[c % 3 === 2 ? c - 2 : c + 1];
    if (a !== b) start[(a < b ? a : b) + 1]++;
  }
  for (let v = 0; v < vertexCount; v++) start[v + 1] += start[v];

  const fill = start.slice(0, vertexCount);
  const keys = new Float64Array(start[vertexCount]);
  for (let c = 0; c < end; c++) {
    const a = indices[c];
    const b = indices[c % 3 === 2 ? c - 2 : c + 1];
    const t = (c - (c % 3)) / 3;
    if (a < b) keys[fill[a]++] = b * scale + t;
    else if (b < a) keys[fill[b]++] = a * scale + t;
  }
  for (let v = 0; v < vertexCount; v++) sortRange(keys, start[v], start[v + 1]);
  return { start, keys, scale };
}

/** A bucket holds one vertex's few edges, so insertion sort; the built-in sort guards big fans. */
function sortRange(keys: Float64Array, from: number, to: number) {
  if (to - from > 16) {
    keys.subarray(from, to).sort();
    return;
  }
  for (let i = from + 1; i < to; i++) {
    const key = keys[i];
    let j = i - 1;
    while (j >= from && keys[j] > key) {
      keys[j + 1] = keys[j];
      j--;
    }
    keys[j + 1] = key;
  }
}

function dot(normals: Float32Array, s: number, t: number): number {
  return normals[s * 3] * normals[t * 3] + normals[s * 3 + 1] * normals[t * 3 + 1] + normals[s * 3 + 2] * normals[t * 3 + 2];
}

/** Unit normal per triangle (x, y, z at t * 3); zero for a triangle with no area. */
function triangleNormals(positions: Float32Array, indices: Uint32Array): Float32Array {
  const end = indices.length - (indices.length % 3);
  const normals = new Float32Array(end);
  for (let t = 0; t < end; t += 3) {
    const a = indices[t] * 3;
    const b = indices[t + 1] * 3;
    const c = indices[t + 2] * 3;
    const ux = positions[b] - positions[a];
    const uy = positions[b + 1] - positions[a + 1];
    const uz = positions[b + 2] - positions[a + 2];
    const vx = positions[c] - positions[a];
    const vy = positions[c + 1] - positions[a + 1];
    const vz = positions[c + 2] - positions[a + 2];
    const nx = uy * vz - uz * vy;
    const ny = uz * vx - ux * vz;
    const nz = ux * vy - uy * vx;
    const length = Math.sqrt(nx * nx + ny * ny + nz * nz);
    if (length === 0) continue;
    normals[t] = nx / length;
    normals[t + 1] = ny / length;
    normals[t + 2] = nz / length;
  }
  return normals;
}
