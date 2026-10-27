// The manifold-3d worker: Booleans and plane split off the UI thread. manifoldClient.ts is its only
// user. The wasm file is bundled with the app and fetched from the app's own origin, never a CDN.
import Module, { type ManifoldToplevel } from "manifold-3d";
import wasmUrl from "manifold-3d/manifold.wasm?url";
import type { MeshData } from "../store";
import { bakeMesh } from "./bake";
import { booleanMeshes, splitMesh, type BooleanOp, type BooleanOutcome, type SplitOutcome } from "./manifoldOps";
import type { CutPlane } from "./types";

/** A part as the main thread sends it: its own copy of the mesh (part-local) and the part's matrix. */
export interface WorkerPart {
  mesh: MeshData;
  matrix: readonly number[];
}

export type WorkerJob =
  | { op: "boolean"; kind: BooleanOp; main: WorkerPart[]; tool: WorkerPart[]; noiseVolume: number }
  | { op: "split"; part: WorkerPart; plane: CutPlane; gap: number };

export type WorkerRequest = WorkerJob & { id: number };

/**
 * One final reply per request, in request order, after any number of progress replies. `ok: false` means this worker
 * is no longer usable: the wasm did not load ("load"), or an operation threw ("run") and may have left the module
 * half-way through, so the client drops the worker and starts a fresh one for the next request.
 */
export type WorkerReply =
  | { id: number; progress: { fraction: number; label: string } }
  | { id: number; ok: true; outcome: BooleanOutcome | SplitOutcome }
  | { id: number; ok: false; stage: "load" | "run"; detail: string };

/** The parts of a dedicated worker's global scope used here; the project's TS lib is DOM, not WebWorker. */
interface WorkerScope {
  onmessage: ((event: MessageEvent<WorkerRequest>) => void) | null;
  postMessage(message: WorkerReply, transfer: Transferable[]): void;
}

const scope = self as unknown as WorkerScope;

let loading: Promise<ManifoldToplevel> | null = null;

/** Loads the wasm on the first request. A failure is final for this worker; the client starts another. */
function load(): Promise<ManifoldToplevel> {
  loading ??= Module({ locateFile: () => wasmUrl }).then((wasm) => {
    wasm.setup();
    return wasm;
  });
  return loading;
}

function run(wasm: ManifoldToplevel, request: WorkerRequest): BooleanOutcome | SplitOutcome {
  // World space first, with the same rounding as the Rust jobs (see bake.ts).
  const world = (p: WorkerPart) => bakeMesh(p.mesh, p.matrix);
  if (request.op === "split") return splitMesh(wasm, world(request.part), request.plane, request.gap);
  const progress = (fraction: number, label: string) => scope.postMessage({ id: request.id, progress: { fraction, label } }, []);
  return booleanMeshes(wasm, request.kind, request.main.map(world), request.tool.map(world), request.noiseVolume, progress);
}

/** The result arrays, which are this worker's own copies, are moved to the main thread rather than copied. */
function buffersOf(outcome: BooleanOutcome | SplitOutcome): ArrayBuffer[] {
  if (outcome.kind !== "ok") return [];
  const meshes = "mesh" in outcome ? [outcome.mesh] : [outcome.below, outcome.above];
  const buffers = new Set<ArrayBuffer>();
  for (const mesh of meshes) {
    for (const array of mesh ? [mesh.positions, mesh.indices] : []) {
      if (array.buffer instanceof ArrayBuffer) buffers.add(array.buffer);
    }
  }
  return [...buffers];
}

/**
 * The reason, for the user's error message, from what manifold-3d's emscripten build throws:
 * - A wasm abort (the wasm failed to fetch or compile) reads "Aborted(<reason>). Build with
 *   -sASSERTIONS for more info."; only <reason> is kept.
 * - Past the 4 GB wasm heap, a failed allocation surfaces as the trap "memory access out of bounds"
 *   (seen when refining a cube to 190M triangles); no C++ bad_alloc reaches JS.
 * - A C++ exception arrives as a bare number (a pointer into wasm memory), which says nothing.
 */
function detailOf(err: unknown): string {
  if (typeof err === "number") return "manifold-3d stopped with an internal error";
  const message = err instanceof Error ? err.message : String(err);
  if (err instanceof WebAssembly.RuntimeError && message === "memory access out of bounds") {
    return "it most likely ran out of memory (memory access out of bounds)";
  }
  return message.replace(/\.?\s*Build with -sASSERTIONS for more info\.?$/, "").replace(/^Aborted\(([\s\S]*)\)$/, "$1");
}

// Every request waits on the same load promise and the operations are synchronous, so replies leave in
// the order the requests arrived.
scope.onmessage = (event) => {
  const request = event.data;
  load().then(
    (wasm) => {
      let outcome: BooleanOutcome | SplitOutcome;
      try {
        outcome = run(wasm, request);
      } catch (err) {
        console.error("manifold-3d operation failed", err);
        scope.postMessage({ id: request.id, ok: false, stage: "run", detail: detailOf(err) }, []);
        return;
      }
      scope.postMessage({ id: request.id, ok: true, outcome }, buffersOf(outcome));
    },
    (err: unknown) => {
      console.error("manifold-3d did not load", err);
      scope.postMessage({ id: request.id, ok: false, stage: "load", detail: detailOf(err) }, []);
    },
  );
};
