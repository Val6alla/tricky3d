import { describe, expect, it } from "vitest";
import { decodeBundle, encodeBundle } from "./bundle";

describe("bundle", () => {
  it("round-trips header fields and mesh arrays at aligned offsets", () => {
    const a = { positions: new Float32Array([0, 1, 2, 3.5, -4, 5e-7, 6, 7, 8]), indices: new Uint32Array([0, 1, 2]) };
    // A view at an odd offset into a bigger buffer must be copied as its own bytes only.
    const big = new Float32Array([9, 9, 1, 2, 3]);
    const b = { positions: big.subarray(2), indices: new Uint32Array([0, 0, 0, 4294967295]) };
    const buffer = encodeBundle({ path: "C:/Ünïcode/part.stl", format: "obj" }, [
      { info: { name: "a", matrix: [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 10, 20, 30, 1] }, ...a },
      { info: { name: "b" }, ...b },
    ]);
    const { meta, meshes } = decodeBundle<{ path: string; format: string }, { name: string; matrix?: number[] }>(buffer);
    expect(meta).toEqual({ path: "C:/Ünïcode/part.stl", format: "obj" });
    expect(meshes.map((m) => m.info.name)).toEqual(["a", "b"]);
    expect(meshes[0].info.matrix?.[12]).toBe(10);
    expect(Array.from(meshes[0].positions)).toEqual(Array.from(a.positions));
    expect(Array.from(meshes[1].positions)).toEqual([1, 2, 3]);
    expect(Array.from(meshes[1].indices)).toEqual([0, 0, 0, 4294967295]);
    for (const m of meshes) {
      expect(m.positions.byteOffset % 8).toBe(0);
      expect(m.indices.byteOffset % 8).toBe(0);
      expect(m.edges).toBeNull();
    }
  });

  it("round-trips extra arrays of both types at aligned offsets, and leaves them out of the header when there are none", () => {
    const positions = new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]);
    const indices = new Uint32Array([0, 1, 2]);
    // An odd-length u32 view into a bigger buffer: only its own bytes are copied, and the blob after it stays aligned.
    const selection = new Uint32Array([7, 3, 5, 9]).subarray(1);
    const border = new Float32Array([0.5, -1.25, 3, 4, 5, 6]);
    const buffer = encodeBundle({ job: 3 }, [
      { info: { name: "a" }, positions, indices, extra: { selection, border, empty: new Uint32Array(0) } },
      { info: { name: "b" }, positions, indices },
    ]);
    const header = JSON.parse(new TextDecoder().decode(new Uint8Array(buffer, 4, new DataView(buffer).getUint32(0, true))));
    expect(header.meshes[0].extra.selection.slice(1)).toEqual([3, "u32"]);
    expect(header.meshes[0].extra.border.slice(1)).toEqual([6, "f32"]);
    expect(header.meshes[1].extra).toBeUndefined();

    const { meta, meshes } = decodeBundle<{ job: number }, { name: string }>(buffer);
    expect(meta).toEqual({ job: 3 });
    const extra = meshes[0].extra;
    expect(extra.selection).toBeInstanceOf(Uint32Array);
    expect(Array.from(extra.selection)).toEqual([3, 5, 9]);
    expect(extra.border).toBeInstanceOf(Float32Array);
    expect(Array.from(extra.border)).toEqual([0.5, -1.25, 3, 4, 5, 6]);
    expect(extra.empty.length).toBe(0);
    for (const blob of Object.values(extra)) expect(blob.byteOffset % 8).toBe(0);
    expect(meshes[0].info).toEqual({ name: "a" });
    expect(meshes[1].extra).toEqual({});
    expect(Array.from(meshes[1].indices)).toEqual([0, 1, 2]);
  });

  it("reads extra arrays laid out the way src-tauri/src/bundle.rs writes them", () => {
    // Header, padding to 8, positions (9 f32), padding, indices (3 u32), padding, extras: as bundle.rs encodes.
    const make = (base: number) =>
      JSON.stringify({
        changed: false,
        meshes: [{ positions: [base, 0], indices: [base, 0], extra: { selection: [base, 2, "u32"], inverted: [base + 8, 1, "u32"], nonManifold: [base + 16, 6, "f32"] } }],
      });
    let base = 0;
    while (Math.ceil((4 + new TextEncoder().encode(make(base)).length) / 8) * 8 !== base) {
      base = Math.ceil((4 + new TextEncoder().encode(make(base)).length) / 8) * 8;
    }
    const header = new TextEncoder().encode(make(base));
    const buffer = new ArrayBuffer(base + 40);
    new DataView(buffer).setUint32(0, header.length, true);
    new Uint8Array(buffer).set(header, 4);
    new Uint32Array(buffer, base, 2).set([4, 11]);
    new Uint32Array(buffer, base + 8, 1).set([2]);
    new Float32Array(buffer, base + 16, 6).set([1, 2, 3, 4, 5, 6]);
    const { meta, meshes } = decodeBundle<{ changed: boolean }, unknown>(buffer);
    expect(meta.changed).toBe(false);
    expect(meshes[0].positions.length).toBe(0);
    expect(Array.from(meshes[0].extra.selection)).toEqual([4, 11]);
    expect(Array.from(meshes[0].extra.inverted)).toEqual([2]);
    expect(Array.from(meshes[0].extra.nonManifold)).toEqual([1, 2, 3, 4, 5, 6]);
  });

  it("rejects extra arrays with an unknown type, a bad entry, or a range outside the buffer", () => {
    // A short header, then 16 bytes of data at 256: the buffer ends at 272 whatever the header says.
    const end = 272;
    const withExtra = (extra: unknown) => {
      const header = new TextEncoder().encode(JSON.stringify({ meshes: [{ positions: [256, 0], indices: [256, 0], extra }] }));
      const buffer = new ArrayBuffer(end);
      new DataView(buffer).setUint32(0, header.length, true);
      new Uint8Array(buffer).set(header, 4);
      return buffer;
    };
    expect(decodeBundle(withExtra({ a: [end - 8, 2, "u32"] })).meshes[0].extra.a.length).toBe(2);
    expect(() => decodeBundle(withExtra({ a: [end - 8, 2, "f64"] }))).toThrow(/unknown type/);
    expect(() => decodeBundle(withExtra({ a: [end - 8, 2] }))).toThrow();
    expect(() => decodeBundle(withExtra([1, 2, "u32"]))).toThrow();
    expect(() => decodeBundle(withExtra({ a: [end - 8, 3, "u32"] }))).toThrow(/out of range/);
    expect(() => decodeBundle(withExtra({ a: [-8, 1, "u32"] }))).toThrow(/out of range/);
    expect(() => decodeBundle(withExtra({ a: [end - 6, 1, "f32"] }))).toThrow(/out of range/);
  });

  it("handles an empty mesh list and rejects truncated data", () => {
    const { meta, meshes } = decodeBundle<{ x: number }, unknown>(encodeBundle({ x: 1 }, []));
    expect(meta).toEqual({ x: 1 });
    expect(meshes).toEqual([]);
    const buffer = encodeBundle({}, [{ info: {}, positions: new Float32Array(3), indices: new Uint32Array(3) }]);
    // The last 4 bytes are alignment padding; cutting 8 cuts into the index blob.
    expect(() => decodeBundle(buffer.slice(0, buffer.byteLength - 8))).toThrow();
  });
});
