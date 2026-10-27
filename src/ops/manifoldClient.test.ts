import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { makeBox, translation } from "../mesh";
import type { MeshData } from "../store";
import type { WorkerReply, WorkerRequest } from "./manifold.worker";
import { CANCELLED } from "../repair/ipc";
import type { BooleanOutcome, SplitOutcome } from "./manifoldOps";

/**
 * Stands in for the browser Worker. postMessage runs a real structuredClone with the transfer list, so
 * transferred buffers are detached exactly as a real postMessage detaches them.
 */
class FakeWorker {
  static created: FakeWorker[] = [];
  static refuse: Error | null = null;
  received: WorkerRequest[] = [];
  terminated = false;
  onmessage: ((event: { data: WorkerReply }) => void) | null = null;
  onerror: ((event: { message?: string; preventDefault(): void }) => void) | null = null;
  onmessageerror: (() => void) | null = null;

  constructor(
    readonly url: URL,
    readonly options: { type?: string },
  ) {
    if (FakeWorker.refuse) throw FakeWorker.refuse;
    FakeWorker.created.push(this);
  }

  postMessage(message: WorkerRequest, transfer: Transferable[]) {
    this.received.push(structuredClone(message, { transfer }));
  }

  terminate() {
    this.terminated = true;
  }

  reply(data: WorkerReply) {
    this.onmessage?.({ data });
  }
}

type Client = typeof import("./manifoldClient");
let client: Client;

beforeEach(async () => {
  FakeWorker.created = [];
  FakeWorker.refuse = null;
  vi.stubGlobal("Worker", FakeWorker);
  vi.resetModules(); // a fresh client, with no worker yet
  client = await import("./manifoldClient");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const part = (mesh: MeshData = makeBox([20, 20, 20])) => ({ mesh, matrix: translation(1, 2, 3) });
const empty = (): MeshData => ({ positions: new Float32Array(0), indices: new Uint32Array(0) });
const okUnion = (id: number): WorkerReply => ({ id, ok: true, outcome: { kind: "ok", mesh: empty(), bodies: [1, 1], noise: 0 } });
const plane = { normal: [0, 0, 1] as [number, number, number], offset: 10 };

/** A union of two parts, as Boolean operations sends it. */
const unite = (onProgress?: (fraction: number, label: string) => void) =>
  client.manifoldBoolean({ kind: "union", main: [part()], tool: [part()], noiseVolume: 0 }, onProgress);

describe("manifoldClient", () => {
  it("stops the worker after it sat idle, and starts a fresh one for the next request", async () => {
    vi.useFakeTimers();
    try {
      const first = unite();
      const worker = FakeWorker.created[0];
      // Busy: the timer only starts once every request is answered.
      vi.advanceTimersByTime(client.IDLE_MS * 2);
      expect(worker.terminated).toBe(false);
      worker.reply(okUnion(1));
      await first;
      vi.advanceTimersByTime(client.IDLE_MS - 1);
      expect(worker.terminated).toBe(false);
      // A request in time keeps it.
      const second = client.manifoldSplit(part(), plane, 0);
      vi.advanceTimersByTime(client.IDLE_MS * 2);
      expect(worker.terminated).toBe(false);
      worker.reply({ id: 2, ok: true, outcome: { kind: "invalid" } });
      await second;
      vi.advanceTimersByTime(client.IDLE_MS);
      expect(worker.terminated).toBe(true);
      void unite();
      expect(FakeWorker.created).toHaveLength(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("starts one module worker on first use and keeps using it", async () => {
    expect(FakeWorker.created).toHaveLength(0);
    const first = unite();
    const second = client.manifoldSplit(part(), plane, 0);
    expect(FakeWorker.created).toHaveLength(1);
    const worker = FakeWorker.created[0];
    expect(worker.url.pathname.endsWith("/manifold.worker.ts")).toBe(true);
    expect(worker.options.type).toBe("module");
    expect(worker.received.map((r) => [r.id, r.op])).toEqual([
      [1, "boolean"],
      [2, "split"],
    ]);
    worker.reply(okUnion(1));
    worker.reply({ id: 2, ok: true, outcome: { kind: "invalid" } });
    await expect(first).resolves.toEqual({ kind: "ok", mesh: empty(), bodies: [1, 1], noise: 0 });
    await expect(second).resolves.toEqual({ kind: "invalid" });
    void unite();
    expect(FakeWorker.created).toHaveLength(1);
  });

  it("sends the worker copies of the store's arrays and never transfers the originals", () => {
    // Imported meshes are views into one bundle buffer; only the view's own bytes may be sent.
    const bundle = new ArrayBuffer(4096);
    const box = makeBox([20, 20, 20]);
    const mesh: MeshData = { positions: new Float32Array(bundle, 64, 24), indices: new Uint32Array(bundle, 512, 36) };
    mesh.positions.set(box.positions);
    mesh.indices.set(box.indices);
    void client.manifoldSplit({ mesh, matrix: translation(5, 0, 0) }, plane, 2);
    const sent = FakeWorker.created[0].received[0];
    if (sent.op !== "split") throw new Error("expected a split request");
    // The store's arrays are untouched: not detached, same contents.
    expect(bundle.byteLength).toBe(4096);
    expect(mesh.positions).toEqual(box.positions);
    expect(mesh.indices).toEqual(box.indices);
    // The worker got exactly the view's bytes, the matrix, plane and gap.
    expect(sent.part.mesh.positions).toEqual(box.positions);
    expect(sent.part.mesh.indices).toEqual(box.indices);
    expect(sent.part.mesh.positions.buffer.byteLength).toBe(24 * 4);
    expect(sent.part.matrix).toEqual(translation(5, 0, 0));
    expect([sent.plane, sent.gap]).toEqual([plane, 2]);
  });

  it("answers each request by its id", async () => {
    const meshA: MeshData = { positions: new Float32Array([1, 1, 1]), indices: new Uint32Array(0) };
    const meshB: MeshData = { positions: new Float32Array([2, 2, 2]), indices: new Uint32Array(0) };
    const a = unite();
    const b = unite();
    const worker = FakeWorker.created[0];
    worker.reply({ id: 2, ok: true, outcome: { kind: "ok", mesh: meshB, bodies: [1], noise: 0 } });
    worker.reply({ id: 1, ok: true, outcome: { kind: "ok", mesh: meshA, bodies: [1], noise: 0 } });
    expect(((await a) as Extract<BooleanOutcome, { kind: "ok" }>).mesh.positions[0]).toBe(1);
    expect(((await b) as Extract<BooleanOutcome, { kind: "ok" }>).mesh.positions[0]).toBe(2);
  });

  it("rejects every waiting request when the wasm does not load, then tries a new worker", async () => {
    const a = unite();
    const b = client.manifoldSplit(part(), plane, 0);
    const first = FakeWorker.created[0];
    first.reply({ id: 1, ok: false, stage: "load", detail: "both async and sync fetching of the wasm failed." });
    const message = "Could not load the geometry engine: both async and sync fetching of the wasm failed.";
    await expect(a).rejects.toThrow(message);
    await expect(b).rejects.toThrow(message);
    expect(first.terminated).toBe(true);
    // A late reply from the dropped worker changes nothing.
    first.reply({ id: 2, ok: true, outcome: { kind: "invalid" } });

    const c = client.manifoldSplit(part(), plane, 0);
    expect(FakeWorker.created).toHaveLength(2);
    const second = FakeWorker.created[1];
    expect(second.received[0].id).toBe(3);
    second.reply({ id: 3, ok: true, outcome: { kind: "ok", below: null, above: null } });
    await expect(c).resolves.toEqual({ kind: "ok", below: null, above: null } satisfies SplitOutcome);
  });

  it("drops the worker after an operation fails, so the next request gets a fresh wasm module", async () => {
    const a = unite();
    FakeWorker.created[0].reply({ id: 1, ok: false, stage: "run", detail: "Aborted(Cannot enlarge memory)" });
    await expect(a).rejects.toThrow("The geometry engine failed: Aborted(Cannot enlarge memory).");
    void unite();
    expect(FakeWorker.created).toHaveLength(2);
  });

  it("rejects with a sentence when the worker script does not load", async () => {
    const a = unite();
    const prevented = vi.fn();
    FakeWorker.created[0].onerror?.({ preventDefault: prevented });
    await expect(a).rejects.toThrow(/^Could not start the geometry engine\.$/);
    expect(prevented).toHaveBeenCalled();
    expect(FakeWorker.created[0].terminated).toBe(true);
  });

  it("rejects with a sentence when the worker cannot be created, and tries again next time", async () => {
    FakeWorker.refuse = new Error("Refused to create a worker because it violates the Content Security Policy.");
    await expect(unite()).rejects.toThrow(
      "Could not start the geometry engine: Refused to create a worker because it violates the Content Security Policy.",
    );
    FakeWorker.refuse = null;
    const b = unite();
    expect(FakeWorker.created).toHaveLength(1);
    FakeWorker.created[0].reply(okUnion(FakeWorker.created[0].received[0].id));
    await expect(b).resolves.toMatchObject({ kind: "ok" });
  });

  it("passes the worker's progress to the request it belongs to", async () => {
    const heardA: string[] = [];
    const heardB: string[] = [];
    const a = unite((f, label) => heardA.push(`${f} ${label}`));
    const b = unite((f, label) => heardB.push(`${f} ${label}`));
    const worker = FakeWorker.created[0];
    worker.reply({ id: 1, progress: { fraction: 0, label: "Reading part 1 of 2" } });
    worker.reply({ id: 2, progress: { fraction: 0.5, label: "Uniting" } });
    worker.reply({ id: 1, progress: { fraction: 0.9, label: "Reading the result" } });
    worker.reply(okUnion(1));
    // Progress after the answer, or for no request, is ignored.
    worker.reply({ id: 1, progress: { fraction: 1, label: "late" } });
    worker.reply({ id: 9, progress: { fraction: 1, label: "unknown" } });
    worker.reply(okUnion(2));
    await Promise.all([a, b]);
    expect(heardA).toEqual(["0 Reading part 1 of 2", "0.9 Reading the result"]);
    expect(heardB).toEqual(["0.5 Uniting"]);
  });

  it("cancels by stopping the worker: everything waiting rejects with CANCELLED, the next request starts afresh", async () => {
    const a = unite();
    const b = client.manifoldSplit(part(), plane, 0);
    const first = FakeWorker.created[0];
    client.cancelManifold();
    await expect(a).rejects.toBe(CANCELLED);
    await expect(b).rejects.toBe(CANCELLED);
    expect(first.terminated).toBe(true);
    // A reply the stopped worker had already sent changes nothing.
    first.reply(okUnion(1));
    const c = unite();
    expect(FakeWorker.created).toHaveLength(2);
    FakeWorker.created[1].reply(okUnion(FakeWorker.created[1].received[0].id));
    await expect(c).resolves.toMatchObject({ kind: "ok" });
    // Nothing to cancel is fine.
    client.cancelManifold();
    client.cancelManifold();
  });

  it("rejects when the reply cannot be read", async () => {
    const a = unite();
    FakeWorker.created[0].onmessageerror?.();
    await expect(a).rejects.toThrow("Could not read the geometry engine's reply.");
  });
});
