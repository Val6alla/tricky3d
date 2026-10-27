// Damage badges in the parts tree: whether each part mesh is closed and oriented, checked in Rust in the background
// and cached per mesh object. Meshes are immutable, so a result stays valid as long as the mesh lives, and an undo
// that brings a mesh back finds its result again. The tree re-renders on useBadges((s) => s.version).
import { isTauri } from "@tauri-apps/api/core";
import { create } from "zustand";
import { formatCount } from "../info";
import { useStore, type MeshData } from "../store";
import { checkMeshes } from "./ipc";
import type { RepairCheck, RepairStatus } from "./types";

const checks = new WeakMap<MeshData, RepairCheck>();
/** Meshes whose check failed: not tried again, so a mesh Rust rejects cannot keep the loop busy. */
const failed = new WeakSet<MeshData>();

/** Bumped whenever a check result arrives, so components showing badges re-render. */
export const useBadges = create<{ version: number }>()(() => ({ version: 0 }));

/** The mesh's check, or undefined while it has not been checked. */
export function cachedCheck(mesh: MeshData): RepairCheck | undefined {
  return checks.get(mesh);
}

let bumpPending = false;

/**
 * Re-renders the badges, at most once per frame: after Split shells into thousands of parts the checks arrive one
 * after another, and the tree only needs to keep up with the screen.
 */
function bumpVersion() {
  if (typeof requestAnimationFrame === "undefined") {
    useBadges.setState((s) => ({ version: s.version + 1 }));
    return;
  }
  if (bumpPending) return;
  bumpPending = true;
  requestAnimationFrame(() => {
    bumpPending = false;
    useBadges.setState((s) => ({ version: s.version + 1 }));
  });
}

export async function ensureMeshChecks(ids: readonly string[]): Promise<void> {
  if (!isTauri() || ids.length === 0) return;
  const meshes: MeshData[] = [];
  const parts = useStore.getState().parts;
  for (const id of ids) {
    const mesh = parts.find((p) => p.id === id)?.mesh;
    if (mesh && !cachedCheck(mesh) && !failed.has(mesh)) meshes.push(mesh);
  }
  if (meshes.length === 0) return;
  try {
    const checks = await checkMeshes(meshes);
    checks.forEach((check, i) => {
      if (check) primeCheck(meshes[i], check);
    });
  } catch (err) {
    for (const mesh of meshes) failed.add(mesh);
    console.error("repair_check failed", err);
  }
}

/** Stores a check made elsewhere (Apply repair knows its result's status), so it is not checked again. */
export function primeCheck(mesh: MeshData, check: RepairCheck): void {
  checks.set(mesh, check);
  bumpVersion();
}

/** The badge check that a repair status implies. */
export function checkFromStatus(status: RepairStatus): RepairCheck {
  const { closed, oriented, holes, borderEdges, nonManifoldEdges, invertedTriangles } = status;
  return { closed, oriented, holes, borderEdges, nonManifoldEdges, invertedTriangles };
}

const plural = (n: number, one: string, many: string) => `${formatCount(n)} ${n === 1 ? one : many}`;

/**
 * The badge tooltip, "Needs repair: 4 holes, 80,437 inverted triangles", or null when the mesh is closed and
 * oriented (no badge). Open edges are named only when there are border edges but no hole loop was counted.
 */
export function badgeText(check: RepairCheck): string | null {
  if (check.closed && check.oriented) return null;
  const parts: string[] = [];
  if (check.holes > 0) parts.push(plural(check.holes, "hole", "holes"));
  else if (check.borderEdges > 0) parts.push(plural(check.borderEdges, "open edge", "open edges"));
  if (check.nonManifoldEdges > 0) parts.push(plural(check.nonManifoldEdges, "non-manifold edge", "non-manifold edges"));
  if (check.invertedTriangles > 0) parts.push(plural(check.invertedTriangles, "inverted triangle", "inverted triangles"));
  if (parts.length === 0) {
    const what = [!check.closed && "not closed", !check.oriented && "not oriented"].filter(Boolean).join(" and ");
    return `Needs repair: the mesh is ${what}`;
  }
  return `Needs repair: ${parts.join(", ")}`;
}

/** The first part mesh, in tree order, that has neither a check nor a failed one. */
function nextUnchecked(): MeshData | null {
  for (const part of useStore.getState().parts) {
    if (!checks.has(part.mesh) && !failed.has(part.mesh)) return part.mesh;
  }
  return null;
}

/**
 * Checks every part mesh without a cached result, one at a time in Rust, now and whenever the parts change, without
 * blocking anything: a million-triangle part gets its badge whenever its check finishes. Returns the function that
 * stops it (a check already sent still lands in the cache). Does nothing outside the desktop app.
 */
export function startBadgeChecks(): () => void {
  if (!isTauri()) return () => {};
  let stopped = false;
  let running = false;
  const pump = async () => {
    if (running) return;
    running = true;
    try {
      for (let mesh = nextUnchecked(); mesh && !stopped; mesh = nextUnchecked()) {
        try {
          const [check] = await checkMeshes([mesh]);
          if (check) primeCheck(mesh, check);
          else failed.add(mesh);
        } catch (err) {
          failed.add(mesh);
          console.error("repair_check failed", err);
        }
      }
    } finally {
      running = false;
    }
  };
  const unsubscribe = useStore.subscribe((s, prev) => {
    if (s.parts !== prev.parts) void pump();
  });
  void pump();
  return () => {
    stopped = true;
    unsubscribe();
  };
}
