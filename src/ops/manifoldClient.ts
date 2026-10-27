// Main-thread side of the manifold-3d worker. One worker for the app, started on first use, so the
// wasm (about 0.5 MB) is only fetched and compiled when someone runs a Boolean or a Cut.
import { CANCELLED } from "../repair/ipc";
import type { MeshData } from "../store";
import type { WorkerJob, WorkerPart, WorkerReply } from "./manifold.worker";
import type { BooleanOp, BooleanOutcome, BooleanProgress, SplitOutcome } from "./manifoldOps";
import type { CutPlane } from "./types";

type Outcome = BooleanOutcome | SplitOutcome;

interface Waiting {
  resolve(outcome: Outcome): void;
  /** An Error, or CANCELLED after cancelManifold(). */
  reject(error: Error | typeof CANCELLED): void;
  onProgress?: BooleanProgress;
}

let worker: Worker | null = null;
let nextId = 1;
/** Requests sent to the current worker and not answered yet, by id. */
const waiting = new Map<number, Waiting>();

/**
 * How long the worker may sit idle before it is stopped. Its wasm heap never shrinks: after uniting two
 * million-triangle parts it holds well over a gigabyte for as long as it lives. A new worker loads the
 * wasm again in a fraction of a second, so the next Boolean or Cut barely notices.
 */
export const IDLE_MS = 20_000;
let idleTimer: ReturnType<typeof setTimeout> | null = null;

function stopIdleTimer() {
  if (idleTimer !== null) clearTimeout(idleTimer);
  idleTimer = null;
}

/** Stops the worker once nothing has been asked of it for IDLE_MS. */
function startIdleTimer() {
  stopIdleTimer();
  idleTimer = setTimeout(() => {
    idleTimer = null;
    if (waiting.size > 0) return;
    worker?.terminate();
    worker = null;
  }, IDLE_MS);
}

type PartIn = { mesh: MeshData; matrix: readonly number[] };

/** What Boolean operations asks for: see booleanMeshes. */
export interface BooleanJob {
  kind: BooleanOp;
  main: readonly PartIn[];
  tool: readonly PartIn[];
  /** mm³; result shells smaller than this are dropped. */
  noiseVolume: number;
}

/**
 * The world-space result of a Boolean, or which inputs (by index into main, then tool) are not closed solids.
 * `onProgress` hears the worker's steps. Rejects with CANCELLED when cancelManifold() stopped it.
 */
export function manifoldBoolean(job: BooleanJob, onProgress?: BooleanProgress): Promise<BooleanOutcome> {
  const main = job.main.map(copyPart);
  const tool = job.tool.map(copyPart);
  const request: WorkerJob = { op: "boolean", kind: job.kind, main: main.map((c) => c.part), tool: tool.map((c) => c.part), noiseVolume: job.noiseVolume };
  return send<BooleanOutcome>(request, [...main, ...tool].flatMap((c) => c.buffers), onProgress);
}

/** The part cut by `plane` into two closed world-space solids, or "invalid" when it is not a closed solid. */
export function manifoldSplit(part: PartIn, plane: CutPlane, gap: number): Promise<SplitOutcome> {
  const copy = copyPart(part);
  return send<SplitOutcome>({ op: "split", part: copy.part, plane, gap }, copy.buffers);
}

/**
 * The worker's own copy of a part's arrays, moved to it rather than cloned. The store's arrays are
 * shared with the store and the undo history, so they are never transferred; and they can be views
 * into one big import buffer, which a structured clone would copy whole.
 */
function copyPart({ mesh, matrix }: PartIn): { part: WorkerPart; buffers: ArrayBuffer[] } {
  const positions = mesh.positions.slice();
  const indices = mesh.indices.slice();
  return { part: { mesh: { positions, indices }, matrix }, buffers: [positions.buffer, indices.buffer] };
}

/**
 * Sends a job and resolves with its outcome. The worker answers in order and by id; the outcome's
 * type is the one for `job.op`, which is what the callers ask for.
 */
function send<T extends Outcome>(job: WorkerJob, transfer: ArrayBuffer[], onProgress?: BooleanProgress): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    stopIdleTimer();
    let current: Worker;
    try {
      current = worker ?? start();
    } catch (err) {
      reject(new Error(sentence("Could not start the geometry engine", err)));
      return;
    }
    const id = nextId++;
    waiting.set(id, { resolve: (outcome) => resolve(outcome as T), reject, onProgress });
    try {
      current.postMessage({ id, ...job }, transfer);
    } catch (err) {
      waiting.delete(id);
      reject(new Error(sentence("Could not send the parts to the geometry engine", err)));
    }
  });
}

function start(): Worker {
  const started = new Worker(new URL("./manifold.worker.ts", import.meta.url), { type: "module" });
  started.onmessage = (event: MessageEvent<WorkerReply>) => {
    if (started !== worker) return;
    const reply = event.data;
    if ("progress" in reply) {
      waiting.get(reply.id)?.onProgress?.(reply.progress.fraction, reply.progress.label);
      return;
    }
    if (!reply.ok) {
      const lead = reply.stage === "load" ? "Could not load the geometry engine" : "The geometry engine failed";
      fail(sentence(lead, reply.detail));
      return;
    }
    const request = waiting.get(reply.id);
    waiting.delete(reply.id);
    request?.resolve(reply.outcome);
    if (waiting.size === 0) startIdleTimer();
  };
  // Fires when the worker script cannot be fetched or parsed (an ErrorEvent with no message in most
  // engines), or for an error the worker did not catch.
  started.onerror = (event: ErrorEvent) => {
    if (started !== worker) return;
    event.preventDefault();
    fail(sentence("Could not start the geometry engine", event.message ?? ""));
  };
  started.onmessageerror = () => {
    if (started === worker) fail("Could not read the geometry engine's reply.");
  };
  worker = started;
  return started;
}

/**
 * Stops whatever the worker is doing: it is dropped (a synchronous manifold-3d call cannot be interrupted any other
 * way) and everything sent to it rejects with CANCELLED. The next request starts a new worker.
 */
export function cancelManifold(): void {
  stopIdleTimer();
  fail(CANCELLED);
}

/**
 * Drops the worker and rejects everything sent to it with `reason` (a message becomes an Error). The next request
 * starts a new worker, so a failed load or a crash is retried rather than remembered.
 */
function fail(reason: string) {
  worker?.terminate();
  worker = null;
  const rejected = [...waiting.values()];
  waiting.clear();
  for (const request of rejected) request.reject(reason === CANCELLED ? CANCELLED : new Error(reason));
}

/** "Lead: detail." with the detail's own trailing full stop dropped, or "Lead." without one. */
function sentence(lead: string, detail: unknown): string {
  const text = (detail instanceof Error ? detail.message : String(detail)).trim().replace(/[.\s]+$/, "");
  return text ? `${lead}: ${text}.` : `${lead}.`;
}
