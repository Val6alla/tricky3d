// Runs the worker module itself under node with the real wasm: `self` is a stand-in that records what
// the worker posts. Only the wasm loading differs from the browser (node reads the file itself).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDENTITY, makeBox, translation } from "../mesh";
import type { MeshData } from "../store";
import type { WorkerPart, WorkerReply, WorkerRequest } from "./manifold.worker";

const load = vi.hoisted(() => ({ fail: null as Error | null }));

vi.mock("manifold-3d", async (importOriginal) => {
  const real = await importOriginal<typeof import("manifold-3d")>();
  // Node finds manifold.wasm next to manifold.js; the browser build's locateFile URL means nothing here.
  return { default: () => (load.fail ? Promise.reject(load.fail) : real.default()) };
});

interface Posted {
  message: WorkerReply;
  transfer: Transferable[];
}

let posted: Posted[];
let scope: { onmessage: ((event: { data: WorkerRequest }) => void) | null; postMessage(message: WorkerReply, transfer: Transferable[]): void };

beforeEach(async () => {
  load.fail = null;
  posted = [];
  scope = { onmessage: null, postMessage: (message, transfer) => posted.push({ message, transfer }) };
  vi.stubGlobal("self", scope);
  vi.resetModules(); // a fresh worker module: nothing loaded yet
  await import("./manifold.worker");
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

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

const send = (request: WorkerRequest) => scope.onmessage?.({ data: request });
/** The final replies, without the progress the worker posts on the way. */
const finals = () => posted.filter((p) => !("progress" in p.message));
const boolean = (id: number, kind: "union" | "difference", main: WorkerPart[], tool: WorkerPart[] = []): WorkerRequest => ({ id, op: "boolean", kind, main, tool, noiseVolume: 0 });
const cube = () => makeBox([20, 20, 20]); // centred on the origin, so the matrices place it

describe("manifold worker", () => {
  it("bakes each part's matrix, answers in request order and moves the result buffers", async () => {
    send(boolean(1, "union", [{ mesh: cube(), matrix: translation(10, 10, 10) }], [{ mesh: cube(), matrix: translation(20, 10, 10) }]));
    const mirror = [...IDENTITY];
    mirror[0] = -1; // a mirrored part must still come out facing outwards
    mirror[14] = 10;
    send({ id: 2, op: "split", part: { mesh: cube(), matrix: mirror }, plane: { normal: [0, 0, 1], offset: 5 }, gap: 0 });
    await vi.waitFor(() => expect(finals()).toHaveLength(2));
    expect(finals().map((p) => p.message.id)).toEqual([1, 2]);

    const [union, split] = finals().map((p) => p.message);
    if ("progress" in union || !union.ok || union.outcome.kind !== "ok" || !("mesh" in union.outcome)) throw new Error(`union: ${JSON.stringify(union)}`);
    expect(volume(union.outcome.mesh)).toBeCloseTo(12000, 6);
    expect(finals()[0].transfer).toEqual([union.outcome.mesh.positions.buffer, union.outcome.mesh.indices.buffer]);

    if ("progress" in split || !split.ok || split.outcome.kind !== "ok" || !("below" in split.outcome)) throw new Error(`split: ${JSON.stringify(split)}`);
    expect(volume(split.outcome.below!)).toBeCloseTo(2000, 6);
    expect(volume(split.outcome.above!)).toBeCloseTo(6000, 6);
    expect(finals()[1].transfer).toHaveLength(4);
  });

  it("posts the Boolean's progress under its id before the result, and subtracts the red parts", async () => {
    send(boolean(7, "difference", [{ mesh: cube(), matrix: translation(10, 10, 10) }], [{ mesh: cube(), matrix: translation(20, 10, 10) }]));
    await vi.waitFor(() => expect(finals()).toHaveLength(1));
    const progress = posted.slice(0, -1).map((p) => p.message);
    expect(progress.length).toBeGreaterThan(0);
    for (const m of progress) expect(m).toMatchObject({ id: 7, progress: { fraction: expect.any(Number), label: expect.any(String) } });
    const last = posted[posted.length - 1].message;
    if ("progress" in last || !last.ok || !("mesh" in last.outcome)) throw new Error(`difference: ${JSON.stringify(last)}`);
    expect(volume(last.outcome.mesh)).toBeCloseTo(4000, 6);
  });

  it("does not transfer the arrays it was sent", async () => {
    const mesh = cube();
    send({ id: 1, op: "split", part: { mesh, matrix: [...IDENTITY] }, plane: { normal: [1, 0, 0], offset: 50 }, gap: 0 });
    await vi.waitFor(() => expect(posted).toHaveLength(1));
    const reply = posted[0];
    if ("progress" in reply.message || !reply.message.ok || reply.message.outcome.kind !== "ok" || !("below" in reply.message.outcome)) throw new Error("expected a split");
    expect(reply.message.outcome.above).toBeNull();
    expect(reply.transfer).not.toContain(mesh.positions.buffer);
    expect(reply.transfer).not.toContain(mesh.indices.buffer);
  });

  it("answers every request with a load failure when the wasm does not load, without emscripten's wrapper", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    load.fail = new Error("Aborted(both async and sync fetching of the wasm failed). Build with -sASSERTIONS for more info.");
    send(boolean(1, "union", []));
    send(boolean(2, "union", []));
    await vi.waitFor(() => expect(posted).toHaveLength(2));
    for (const [i, p] of posted.entries()) {
      expect(p.message).toEqual({ id: i + 1, ok: false, stage: "load", detail: "both async and sync fetching of the wasm failed" });
    }
  });

  it("reports an operation that throws as a run failure, in words", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    // The plane is read inside the operation, after the input became a Manifold, so these throw mid-op.
    const throwing = (thrown: unknown) => ({
      get normal(): never {
        throw thrown;
      },
      offset: 0,
    });
    const split = (id: number, thrown: unknown) =>
      send({ id, op: "split", part: { mesh: cube(), matrix: [...IDENTITY] }, plane: throwing(thrown), gap: 0 } as unknown as WorkerRequest);
    split(1, new WebAssembly.RuntimeError("memory access out of bounds")); // how running out of the 4 GB heap shows
    split(2, 5243904); // a C++ exception: a pointer
    split(3, new TypeError("Cannot read properties of null"));
    await vi.waitFor(() => expect(posted).toHaveLength(3));
    expect(posted.map((p) => p.message)).toEqual([
      { id: 1, ok: false, stage: "run", detail: "it most likely ran out of memory (memory access out of bounds)" },
      { id: 2, ok: false, stage: "run", detail: "manifold-3d stopped with an internal error" },
      { id: 3, ok: false, stage: "run", detail: "Cannot read properties of null" },
    ]);
  });
});
