import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Tauri itself is a stand-in: invoke answers with bundles laid out like src-tauri/src/bundle.rs, and listen keeps the
// handler so the tests can send progress events.
const tauri = vi.hoisted(() => ({
  invoke: vi.fn(),
  listen: vi.fn(),
  unlisten: vi.fn(),
  handler: null as ((event: { payload: unknown }) => void) | null,
}));
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({ ...(await importOriginal<typeof import("@tauri-apps/api/core")>()), invoke: tauri.invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: tauri.listen }));

import { decodeBundle } from "../io/bundle";
import type { MeshStats } from "../io/ipc";
import { makeBox } from "../mesh";
import { cachedProblemEdges, cachedStats } from "../meshCache";
import {
  CANCELLED,
  cancelRepair,
  checkMeshes,
  exportScriptFile,
  fetchCatalog,
  fetchCutPath,
  fetchShells,
  forgetCutMesh,
  importScriptFile,
  loadStoredScripts,
  runRepair,
  saveStoredScripts,
} from "./ipc";
import type { RepairProgress, RepairStatus, ScriptStep } from "./types";

type Blob = Float32Array | Uint32Array;

/** A response bundle as bundle.rs writes it: header, then positions, indices, edges and extras, each 8-aligned. */
function rustBundle(meta: Record<string, unknown>, mesh: { info?: Record<string, unknown>; positions?: Float32Array; indices?: Uint32Array; edges?: Float32Array; extra?: Record<string, Blob> }): ArrayBuffer {
  const align = (n: number) => Math.ceil(n / 8) * 8;
  const blobs: [string, Blob][] = [
    ["positions", mesh.positions ?? new Float32Array(0)],
    ["indices", mesh.indices ?? new Uint32Array(0)],
    ...(mesh.edges ? [["edges", mesh.edges] as [string, Blob]] : []),
    ...Object.entries(mesh.extra ?? {}).map(([k, v]) => [`extra.${k}`, v] as [string, Blob]),
  ];
  const rel: number[] = [];
  let data = 0;
  for (const [, b] of blobs) {
    rel.push(data);
    data = align(data + b.byteLength);
  }
  const header = (base: number) => {
    const entry: Record<string, unknown> = { ...mesh.info };
    const extra: Record<string, unknown> = {};
    blobs.forEach(([name, b], i) => {
      if (name.startsWith("extra.")) extra[name.slice(6)] = [base + rel[i], b.length, b instanceof Float32Array ? "f32" : "u32"];
      else entry[name] = [base + rel[i], b.length];
    });
    if (Object.keys(extra).length) entry.extra = extra;
    return new TextEncoder().encode(JSON.stringify({ ...meta, meshes: [entry] }));
  };
  let base = align(4 + header(0).length);
  while (align(4 + header(base).length) > base) base = align(4 + header(base).length);
  const bytes = header(base);
  const buffer = new ArrayBuffer(base + data);
  new DataView(buffer).setUint32(0, bytes.length, true);
  new Uint8Array(buffer).set(bytes, 4);
  blobs.forEach(([, b], i) => new Uint8Array(buffer).set(new Uint8Array(b.buffer, b.byteOffset, b.byteLength), base + rel[i]));
  return buffer;
}

const status: RepairStatus = {
  edges: 18,
  triangles: 12,
  vertices: 8,
  shells: 1,
  borderEdges: 0,
  nonManifoldEdges: 0,
  invertedTriangles: 0,
  holes: 0,
  degenerateTriangles: 0,
  closed: true,
  oriented: true,
  volume: 8000,
  area: 2400,
};

const stats: MeshStats = { triangles: 12, vertices: 8, volume: 8000, area: 2400, shells: 1, openEdges: 0, nonManifoldEdges: 0 };
const steps: ScriptStep[] = [{ type: "close_all_holes", enabled: true, params: {} }];
const report = { action: "close_all_holes", label: "Close all holes", message: "Closed 1 hole, added 2 triangles.", warnings: [], changed: true, millis: 3 };

beforeEach(() => {
  tauri.invoke.mockReset();
  tauri.unlisten.mockReset();
  tauri.handler = null;
  tauri.listen.mockReset().mockImplementation(async (_name: string, handler: (event: { payload: unknown }) => void) => {
    tauri.handler = handler;
    return tauri.unlisten;
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("runRepair", () => {
  it("sends the working copy with its selection and the steps, and reads a changed result with everything primed", async () => {
    const input = makeBox([10, 10, 10]);
    const fixed = makeBox([20, 20, 20]);
    const edges = new Float32Array([1, 2, 3, 4, 5, 6]);
    tauri.invoke.mockResolvedValue(
      rustBundle(
        { changed: true, reports: [report], status, warnings: ["One hole was left open."] },
        {
          info: { stats },
          positions: fixed.positions,
          indices: fixed.indices,
          edges,
          extra: {
            selection: new Uint32Array([0, 5]),
            border: new Float32Array(0),
            nonManifold: new Float32Array(6),
            inverted: new Uint32Array([3]),
            degenerate: new Uint32Array(0),
          },
        },
      ),
    );
    const result = await runRepair(input, steps, { job: 7, label: "Close all holes", selection: new Uint32Array([1, 4]), withStatus: true });

    const [command, request] = tauri.invoke.mock.calls[0];
    expect(command).toBe("repair_run");
    const sent = decodeBundle<Record<string, unknown>, Record<string, unknown>>(request);
    expect(sent.meta).toEqual({ job: 7, label: "Close all holes", actions: steps, withStatus: true });
    expect(Array.from(sent.meshes[0].positions)).toEqual(Array.from(input.positions));
    expect(Array.from(sent.meshes[0].indices)).toEqual(Array.from(input.indices));
    expect(Array.from(sent.meshes[0].extra.selection)).toEqual([1, 4]);
    expect(sent.meshes[0].extra.selection).toBeInstanceOf(Uint32Array);

    expect(result.changed).toBe(true);
    expect(Array.from(result.mesh.positions)).toEqual(Array.from(fixed.positions));
    expect(Array.from(result.mesh.indices)).toEqual(Array.from(fixed.indices));
    expect(result.stats).toEqual(stats);
    expect(Array.from(result.edges ?? [])).toEqual([1, 2, 3, 4, 5, 6]);
    expect(Array.from(result.selection)).toEqual([0, 5]);
    expect(result.reports).toEqual([report]);
    expect(result.status).toEqual(status);
    expect(Array.from(result.highlights?.inverted ?? [])).toEqual([3]);
    expect(result.highlights?.nonManifold.length).toBe(6);
    expect(result.intersecting).toBeNull();
    expect(result.warnings).toEqual(["One hole was left open."]);
    expect(cachedStats(result.mesh)).toEqual(stats);
    expect(cachedProblemEdges(result.mesh)).toBe(result.edges);
    // No progress handler asked for: nothing listened.
    expect(tauri.listen).not.toHaveBeenCalled();
  });

  it("gives the input mesh itself back when nothing changed, with the intersecting triangles of Detect", async () => {
    const input = makeBox([10, 10, 10]);
    tauri.invoke.mockResolvedValue(
      rustBundle(
        { changed: false, reports: [{ ...report, changed: false, message: "Found 2 self-intersecting triangles." }], status: null, warnings: [] },
        { extra: { selection: new Uint32Array(0), intersecting: new Uint32Array([2, 9]) } },
      ),
    );
    const result = await runRepair(input, steps, { job: 1, label: "Detect", selection: new Uint32Array(0), withStatus: false });
    expect(result.mesh).toBe(input);
    expect([result.changed, result.stats, result.edges, result.status, result.highlights]).toEqual([false, null, null, null, null]);
    expect(Array.from(result.intersecting ?? [])).toEqual([2, 9]);
    expect(cachedStats(input)).toBeUndefined();
  });

  it("forwards only its own job's progress, and stops listening when the run ends, whether it worked or not", async () => {
    const input = makeBox([1, 1, 1]);
    let finish!: (b: ArrayBuffer) => void;
    tauri.invoke.mockReturnValue(new Promise((resolve) => (finish = resolve)));
    const seen: RepairProgress[] = [];
    const run = runRepair(input, steps, { job: 3, label: "Script", selection: new Uint32Array(0), withStatus: true, onProgress: (p) => seen.push(p) });
    await vi.waitFor(() => expect(tauri.invoke).toHaveBeenCalled());
    expect(tauri.listen.mock.calls[0][0]).toBe("repair-progress");
    const mine = { job: 3, step: 1, steps: 4, label: "Close all holes", fraction: 0.3 };
    tauri.handler?.({ payload: { ...mine, job: 2 } });
    tauri.handler?.({ payload: mine });
    finish(rustBundle({ changed: false, reports: [], status: null, warnings: [] }, { extra: { selection: new Uint32Array(0) } }));
    await run;
    expect(seen).toEqual([mine]);
    expect(tauri.unlisten).toHaveBeenCalledTimes(1);

    tauri.invoke.mockRejectedValue(CANCELLED);
    await expect(runRepair(input, steps, { job: 4, label: "Script", selection: new Uint32Array(0), withStatus: false, onProgress: () => {} })).rejects.toBe("cancelled");
    expect(tauri.unlisten).toHaveBeenCalledTimes(2);
  });

  it("rejects a result whose arrays have the wrong type or that has no mesh", async () => {
    const input = makeBox([1, 1, 1]);
    tauri.invoke.mockResolvedValue(rustBundle({ changed: false }, { extra: { selection: new Float32Array(2) } }));
    await expect(runRepair(input, [], { job: 1, label: "x", selection: new Uint32Array(0), withStatus: false })).rejects.toThrow(/wrong type/);
    const empty = new ArrayBuffer(16);
    const header = new TextEncoder().encode('{"changed":true}');
    new DataView(empty).setUint32(0, header.length, true);
    tauri.invoke.mockResolvedValue(empty);
    await expect(runRepair(input, [], { job: 1, label: "x", selection: new Uint32Array(0), withStatus: false })).rejects.toThrow();
  });
});

describe("other commands", () => {
  it("call their commands with the documented arguments", async () => {
    tauri.invoke.mockResolvedValue(undefined);
    await cancelRepair(5);
    expect(tauri.invoke).toHaveBeenLastCalledWith("repair_cancel", { job: 5 });
    await fetchCatalog();
    expect(tauri.invoke).toHaveBeenLastCalledWith("repair_catalog");
    await loadStoredScripts();
    expect(tauri.invoke).toHaveBeenLastCalledWith("repair_scripts_load");
    const data = { scripts: [], order: ["builtin.simple"] };
    await saveStoredScripts(data);
    expect(tauri.invoke).toHaveBeenLastCalledWith("repair_scripts_save", { data });
    await importScriptFile();
    expect(tauri.invoke).toHaveBeenLastCalledWith("repair_script_import");
    const script = { id: "own.1", name: "Mine", builtIn: false, actions: steps };
    await exportScriptFile(script);
    expect(tauri.invoke).toHaveBeenLastCalledWith("repair_script_export", { script });
  });

  it("check meshes as one bundle, in order", async () => {
    const a = makeBox([1, 1, 1]);
    const b = makeBox([2, 2, 2]);
    tauri.invoke.mockResolvedValue([{ closed: true }, { closed: false }]);
    expect(await checkMeshes([a, b])).toEqual([{ closed: true }, { closed: false }]);
    const [command, request] = tauri.invoke.mock.calls[0];
    expect(command).toBe("repair_check");
    const sent = decodeBundle<unknown, unknown>(request).meshes;
    expect(sent.map((m) => m.positions[0])).toEqual([a.positions[0], b.positions[0]]);
  });

  it("send the working copy for the cut line once, then only its key, and again when Rust no longer has it", async () => {
    const mesh = makeBox([20, 20, 20]);
    const options = { smooth: false, step: 2, snap: 0.5, closed: true };
    const points = [
      { at: [0, 0, 20] as [number, number, number], triangle: 2 },
      { at: [10, 0, 20] as [number, number, number], triangle: 3 },
    ];
    const answer = () => rustBundle({ pieces: [2, null], spans: [[0, 1], [1, 0]], anchors: [[0, 0, 20], [10, 0, 20]] }, { extra: { path: new Float32Array([0, 0, 20, 10, 0, 20]) } });
    tauri.invoke.mockImplementation(async () => answer());
    const sent = (call: number) => decodeBundle<Record<string, unknown>, unknown>(tauri.invoke.mock.calls[call][1]);
    const path = await fetchCutPath(mesh, points, options);
    expect(tauri.invoke.mock.calls[0][0]).toBe("repair_cut_path");
    const first = sent(0);
    expect(first.meta).toMatchObject({ points: [[0, 0, 20, 2], [10, 0, 20, 3]], smooth: false, step: 2, snap: 0.5, closed: true });
    expect(first.meshes[0].indices.length).toBe(36);
    expect(path.pieces.map((p) => (p ? Array.from(p) : null))).toEqual([[0, 0, 20, 10, 0, 20], null]);
    expect(path.spans).toEqual([[0, 1], [1, 0]]);
    // The same working copy again: only its key goes.
    await fetchCutPath(mesh, points, options);
    expect(sent(1).meshes[0].indices.length).toBe(0);
    expect(sent(1).meta.meshKey).toBe(first.meta.meshKey);
    // Rust lost it (it was reset): asked again with the mesh.
    tauri.invoke.mockReset().mockRejectedValueOnce("mesh needed").mockImplementation(async () => answer());
    await fetchCutPath(mesh, points, options);
    expect(tauri.invoke.mock.calls.map((c) => decodeBundle<unknown, unknown>(c[1]).meshes[0].indices.length)).toEqual([0, 36]);
    // Another working copy has a key of its own and goes whole.
    const other = makeBox([30, 30, 30]);
    await fetchCutPath(other, points, options);
    expect(sent(2).meshes[0].indices.length).toBe(36);
    expect(sent(2).meta.meshKey).not.toBe(first.meta.meshKey);
    // The mode ends: Rust lets go of it, and the next line sends the mesh again.
    tauri.invoke.mockReset().mockResolvedValue(undefined);
    await forgetCutMesh();
    expect(tauri.invoke).toHaveBeenCalledWith("repair_cut_reset");
    tauri.invoke.mockReset().mockImplementation(async () => answer());
    await fetchCutPath(other, points, options);
    expect(sent(0).meshes[0].indices.length).toBe(36);
    // A line that came back damaged is an error.
    tauri.invoke.mockReset().mockResolvedValue(rustBundle({ pieces: [5] }, { extra: { path: new Float32Array(3) } }));
    await expect(fetchCutPath(other, points, options)).rejects.toThrow(/damaged/);
  });

  it("read the shell table and each triangle's shell", async () => {
    const rows = [{ triangles: 12, area: 2400, bboxVolume: 8000, volume: 8000, watertight: true }];
    tauri.invoke.mockResolvedValue(rustBundle({ rows }, { extra: { shellOf: new Uint32Array(12) } }));
    const shells = await fetchShells(makeBox([20, 20, 20]));
    expect(tauri.invoke.mock.calls[0][0]).toBe("repair_shells");
    expect(shells.rows).toEqual(rows);
    expect(shells.shellOf).toBeInstanceOf(Uint32Array);
    expect(shells.shellOf.length).toBe(12);
    tauri.invoke.mockResolvedValue(rustBundle({ rows }, {}));
    await expect(fetchShells(makeBox([1, 1, 1]))).rejects.toThrow(/shell of each triangle/);
  });
});
