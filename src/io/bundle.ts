/**
 * Binary container for meshes crossing the Rust↔JS boundary, in both directions. The Rust side
 * (src-tauri/src/bundle.rs) reads and writes the same layout:
 *
 *   u32 little-endian   H, the header length in bytes
 *   H bytes             UTF-8 JSON header
 *   zero bytes          padding up to a multiple of 8
 *   blobs               little-endian Float32 / Uint32 arrays, each starting at a multiple of 8
 *
 * The header's `meshes` array names each blob as [byteOffset, elementCount], with byteOffset
 * counted from the start of the buffer. A mesh may also carry named extra arrays (repair mode's
 * selection, highlights and shell ids) in an `extra` object of [byteOffset, elementCount, "f32" |
 * "u32"] entries. Mesh arrays never go through JSON. JS typed arrays use the platform byte order,
 * which is little-endian on every platform tricky3D targets.
 */

/** [byteOffset from the start of the buffer, element count]. */
export type BlobRef = [number, number];

/** An extra array's element type: little-endian f32 or u32. */
export type ExtraKind = "f32" | "u32";

/** [byteOffset from the start of the buffer, element count, element type]. */
export type ExtraRef = [number, number, ExtraKind];

/** An extra array's values. */
export type ExtraBlob = Float32Array | Uint32Array;

export interface MeshBlobs {
  positions: BlobRef;
  indices: BlobRef;
  /** Problem-edge segments (see src/viewport/edges.ts); only Rust → JS. */
  edges?: BlobRef;
  /** Named extra arrays, either direction; absent when the mesh has none. */
  extra?: Record<string, ExtraRef>;
}

/** One mesh going out: its arrays plus any JSON fields to send with it (name, matrix...). */
export interface OutMesh {
  info: Record<string, unknown>;
  positions: Float32Array;
  indices: Uint32Array;
  /** Named extra arrays to send with the mesh, such as repair mode's selection. */
  extra?: Record<string, ExtraBlob>;
}

/** One mesh read back: the JSON fields with the blobs replaced by views into the buffer. */
export interface InMesh<Info> {
  info: Info;
  positions: Float32Array;
  indices: Uint32Array;
  edges: Float32Array | null;
  /** Named extra arrays; an empty object when the mesh has none. */
  extra: Record<string, ExtraBlob>;
}

const ALIGN = 8;
const align = (n: number) => Math.ceil(n / ALIGN) * ALIGN;

const kindOf = (blob: ExtraBlob): ExtraKind => (blob instanceof Float32Array ? "f32" : "u32");

/** Packs `meta` and `meshes` into one buffer; `meta` must not have a `meshes` key. */
export function encodeBundle(meta: Record<string, unknown>, meshes: OutMesh[]): ArrayBuffer {
  // Blob offsets depend on the header length and the header holds the offsets, so the offsets are
  // first laid out relative to the data section, then shifted once the header length is known.
  const relative: { positions: number; indices: number; extra: { name: string; at: number; blob: ExtraBlob }[] }[] = [];
  let dataBytes = 0;
  for (const m of meshes) {
    const positions = dataBytes;
    dataBytes = align(dataBytes + m.positions.byteLength);
    const indices = dataBytes;
    dataBytes = align(dataBytes + m.indices.byteLength);
    const extra = Object.entries(m.extra ?? {}).map(([name, blob]) => {
      const at = dataBytes;
      dataBytes = align(dataBytes + blob.byteLength);
      return { name, at, blob };
    });
    relative.push({ positions, indices, extra });
  }

  const encoder = new TextEncoder();
  const headerFor = (base: number) =>
    encoder.encode(
      JSON.stringify({
        ...meta,
        meshes: meshes.map((m, i) => {
          const r = relative[i];
          const entry: Record<string, unknown> = {
            ...m.info,
            positions: [base + r.positions, m.positions.length],
            indices: [base + r.indices, m.indices.length],
          };
          if (r.extra.length) {
            entry.extra = Object.fromEntries(r.extra.map((e): [string, ExtraRef] => [e.name, [base + e.at, e.blob.length, kindOf(e.blob)]]));
          }
          return entry;
        }),
      }),
    );
  // The header grows with the base offset's digit count; settle on a base the header fits under.
  let base = align(4 + headerFor(0).byteLength);
  let header = headerFor(base);
  while (align(4 + header.byteLength) > base) {
    base = align(4 + header.byteLength);
    header = headerFor(base);
  }

  const buffer = new ArrayBuffer(base + dataBytes);
  const bytes = new Uint8Array(buffer);
  new DataView(buffer).setUint32(0, header.byteLength, true);
  bytes.set(header, 4);
  const copy = (blob: Float32Array | Uint32Array, at: number) =>
    bytes.set(new Uint8Array(blob.buffer, blob.byteOffset, blob.byteLength), base + at);
  meshes.forEach((m, i) => {
    copy(m.positions, relative[i].positions);
    copy(m.indices, relative[i].indices);
    for (const e of relative[i].extra) copy(e.blob, e.at);
  });
  return buffer;
}

interface RawMesh extends MeshBlobs {
  [key: string]: unknown;
}

/** Splits a bundle into its header fields and typed-array views (no copies) of each mesh's blobs. */
export function decodeBundle<Meta, Info>(buffer: ArrayBuffer): { meta: Meta; meshes: InMesh<Info>[] } {
  if (buffer.byteLength < 4) throw new Error("Mesh data is truncated.");
  const headerLength = new DataView(buffer).getUint32(0, true);
  if (4 + headerLength > buffer.byteLength) throw new Error("Mesh data header is truncated.");
  const header = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, 4, headerLength))) as { meshes?: RawMesh[] };
  const { meshes: raw = [], ...meta } = header;

  const view = <T>(ref: BlobRef, Kind: { new (b: ArrayBuffer, o: number, n: number): T; BYTES_PER_ELEMENT: number }): T => {
    const [offset, count] = ref;
    const whole = Number.isInteger(offset) && Number.isInteger(count) && offset >= 0 && count >= 0;
    if (!whole || offset % 4 !== 0 || offset + count * Kind.BYTES_PER_ELEMENT > buffer.byteLength) {
      throw new Error("Mesh data is out of range.");
    }
    return new Kind(buffer, offset, count);
  };

  /** The named extra arrays of one mesh entry; an entry without `extra` has none. */
  const extraViews = (refs: unknown): Record<string, ExtraBlob> => {
    const out: Record<string, ExtraBlob> = {};
    if (refs === undefined || refs === null) return out;
    if (typeof refs !== "object" || Array.isArray(refs)) throw new Error("Mesh data is damaged.");
    for (const [name, ref] of Object.entries(refs as Record<string, unknown>)) {
      if (!Array.isArray(ref) || ref.length !== 3) throw new Error("Mesh data is damaged.");
      const [offset, count, kind] = ref as [number, number, unknown];
      if (kind === "f32") out[name] = view([offset, count], Float32Array);
      else if (kind === "u32") out[name] = view([offset, count], Uint32Array);
      else throw new Error(`Mesh data has an extra array of an unknown type (${String(kind)}).`);
    }
    return out;
  };

  const meshes = raw.map((m): InMesh<Info> => {
    const { positions, indices, edges, extra, ...info } = m;
    return {
      info: info as Info,
      positions: view(positions, Float32Array),
      indices: view(indices, Uint32Array),
      edges: edges ? view(edges, Float32Array) : null,
      extra: extraViews(extra),
    };
  });
  return { meta: meta as Meta, meshes };
}
