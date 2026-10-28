// Repair commands that used to sit on the Home tab and now live in the Repair tab and the repair right-click menu
// (Home › Prepare keeps Run repair script, and the part menu Repair many parts at once). Each one uses the open repair
// session, or opens one on the selected part first, then runs a repair action in Rust through runAction: off the
// window's thread, with progress and Cancel, and one inner undo step when the mesh changed. The local edits and the
// selections that need the whole mesh (Select by criteria, Find overlapping triangles) are actions of their own in
// src-tauri/src/repair; this file only picks which one to run.
import { CLOSE_BOOLEAN_FIRST, isBooleanOpen } from "../boolean/session";
import { browserOnly } from "../io/files";
import { meshOperationRunning, runMeshOperation, WAIT_MESSAGE } from "../ops/meshOps";
import { useStore, type Replacement } from "../store";
import { toast, useUi } from "../ui";
import { checkFromStatus, primeCheck } from "./badges";
import { toggleCutMode } from "./cut/session";
import { runRepair } from "./ipc";
import { toggleMoveNodes } from "./nodes";
import { loadScripts, scriptById } from "./scripts";
import {
  CLOSE_REPAIR_FIRST,
  SELECT_PART_FIRST,
  SELECT_TRIANGLES_FIRST,
  extractShells,
  hideSelectedTriangles,
  isRepairing,
  refreshStatus,
  runAction,
  withRepairSession as openRepairSession,
} from "./session";
import { useRepair } from "./store";
import type { ActionId, RepairStatus } from "./types";

const DEFAULT_SCRIPT = "builtin.default";

function withRepairSession(fn: () => void | Promise<void>): Promise<void> {
  if (!isRepairing() && isBooleanOpen()) {
    toast(CLOSE_BOOLEAN_FIRST);
    return Promise.resolve();
  }
  if (!isRepairing() && meshOperationRunning()) {
    toast(WAIT_MESSAGE);
    return Promise.resolve();
  }
  return openRepairSession(fn);
}

function damageText(status: RepairStatus): string {
  const rows: [number, string][] = [
    [status.borderEdges, "border edges"],
    [status.holes, "holes"],
    [status.invertedTriangles, "inverted triangles"],
    [status.degenerateTriangles, "degenerate faces"],
    [status.nonManifoldEdges, "non-manifold edges"],
  ];
  const problems = rows.filter(([n]) => n > 0).map(([n, label]) => `${n} ${label}`);
  if (problems.length === 0 && status.closed && status.oriented) return "No damage warnings: mesh is closed and oriented.";
  return problems.length > 0 ? `Damage warnings: ${problems.join(", ")}.` : "Damage warnings: mesh is open or not oriented.";
}

function countText(status: RepairStatus): string {
  return [
    `Border edges ${status.borderEdges}`,
    `Holes ${status.holes}`,
    `Inv. orientation ${status.invertedTriangles}`,
    `Degenerate faces ${status.degenerateTriangles}`,
    `Non-manifold edges ${status.nonManifoldEdges}`,
  ].join(", ");
}

async function withStatus(): Promise<RepairStatus | null> {
  if (!useRepair.getState().session?.status) await refreshStatus();
  return useRepair.getState().session?.status ?? null;
}

export function damageWarnings(): Promise<void> {
  return withRepairSession(async () => {
    useRepair.getState().setPanelTab("status");
    useRepair.getState().setHighlight({ holes: true, triangles: true, degenerate: true, errors: true });
    const status = await withStatus();
    toast(status ? damageText(status) : "Damage warnings are not available yet.");
  });
}

export function problemCounts(): Promise<void> {
  return withRepairSession(async () => {
    useRepair.getState().setPanelTab("status");
    await refreshStatus();
    const status = useRepair.getState().session?.status;
    toast(status ? countText(status) : "Problem counts are not available yet.");
  });
}

export function shellList(): Promise<void> {
  return withRepairSession(() => {
    useRepair.getState().setPanelTab("shells");
  });
}

/** Selects every damaged triangle; the action's message says how many. */
export function selectByCriteria(): Promise<void> {
  return withRepairSession(() => runAction("select_problem_triangles", {}, new Uint32Array(0)));
}

/** Selects the triangles that cross or repeat others over the whole part, and highlights the crossing ones. */
export function findOverlaps(): Promise<void> {
  return withRepairSession(() => runAction("find_overlaps", {}, new Uint32Array(0)));
}

export function hideTriangles(): Promise<void> {
  return withRepairSession(() => {
    const result = hideSelectedTriangles();
    if (result.kind === "hidden") toast(`Hidden ${result.count} triangles. Click Hide selected triangles again to show all.`);
    else if (result.kind === "shown") toast("All triangles shown.");
    else toast(SELECT_TRIANGLES_FIRST);
  });
}

/** Every hole, or those that border the selection, filled to carry on the curve of the surface round it. */
export function closeHolesCurved(): Promise<void> {
  return withRepairSession(() => runAction("close_holes_curved"));
}

/**
 * Joins two opposing open edges; with none to join, stitches open edges and closes the holes beside the selection.
 * Parked for now: features.ts lists Bridge a gap as Soon, so nothing calls this until it is ready again.
 */
export function bridgeGap(): Promise<void> {
  return withRepairSession(() => runAction("bridge_gap"));
}

export function offsetHoleEdge(): Promise<void> {
  return withRepairSession(() => runAction("grow_hole_band"));
}

const cleanup = (id: Parameters<typeof runAction>[0]) => () => withRepairSession(() => runAction(id));

export const removeDuplicateFaces = cleanup("remove_double_triangles");
export const removeDegenerate = cleanup("remove_degenerate_faces");
export const mergeDuplicatePoints = cleanup("sanitize_duplicate_nodes");
export const removeTinyShells = cleanup("remove_tiny_shells");
export const fixSelfIntersections = cleanup("remove_self_intersections");
export const removeProblemAreas = cleanup("remove_problem_areas");
export const invertShells = cleanup("invert_negative_shells");
export const wrapSurface = cleanup("wrap_part_surface");

export function refineTriangles(): Promise<void> {
  return withRepairSession(() => runAction("subdivide_triangles"));
}

/** Fills three-edge holes with a triangle each; with none, closes trivial holes. */
export function addTriangle(): Promise<void> {
  return withRepairSession(() => runAction("add_triangle"));
}

/** An action that works on the selected triangles only. */
function editSelected(id: ActionId): Promise<void> {
  return withRepairSession(async () => {
    if ((useRepair.getState().session?.selectedCount ?? 0) === 0) {
      toast(SELECT_TRIANGLES_FIRST);
      return;
    }
    await runAction(id);
  });
}

export function addPoint(): Promise<void> {
  return editSelected("add_point");
}

/** Move nodes: drag a vertex under the pointer; its triangles follow. A second click turns it off. */
export function moveNodes(): Promise<void> {
  return withRepairSession(toggleMoveNodes);
}

/** Cut surfaces: the cutting mode, with its panel on the right. A second click leaves it. */
export function cutSurfaces(): Promise<void> {
  return withRepairSession(toggleCutMode);
}

export function extractSelectedShells(): Promise<void> {
  return withRepairSession(() => extractShells());
}

export function runRepairScript(): Promise<void> {
  return withRepairSession(() => {
    useUi.getState().openDialog({ kind: "repair-script-picker" });
  });
}

export function editRepairScripts(): Promise<void> {
  return withRepairSession(() => {
    useRepair.getState().setPanelTab("scripts");
  });
}

let batchJob = 1_000_000;

export async function batchRepair(): Promise<void> {
  if (isRepairing()) {
    toast(CLOSE_REPAIR_FIRST);
    return;
  }
  if (isBooleanOpen()) {
    toast(CLOSE_BOOLEAN_FIRST);
    return;
  }
  if (browserOnly("Repair")) return;
  await runMeshOperation("Repair", "Could not repair the parts.", async () => {
    if (!(await loadScripts())) return;
    const script = scriptById(DEFAULT_SCRIPT);
    if (!script) {
      toast("Default repair could not be loaded.");
      return;
    }
    const store = useStore.getState();
    const chosen = new Set(store.selection);
    const targets = store.parts.filter((p) => !p.locked && (chosen.size === 0 || chosen.has(p.id)) && p.mesh.indices.length >= 3);
    if (targets.length === 0) {
      const lockedLeft = store.parts.some((p) => p.locked && (chosen.size === 0 || chosen.has(p.id)));
      toast(lockedLeft ? "Locked parts were left as they are." : SELECT_PART_FIRST);
      return;
    }
    const replacements: Replacement[] = [];
    for (const part of targets) {
      const run = await runRepair(part.mesh, script.actions, {
        job: batchJob++,
        label: script.name,
        selection: new Uint32Array(0),
        withStatus: true,
      });
      if (!run.changed) {
        if (run.status) primeCheck(part.mesh, checkFromStatus(run.status));
        continue;
      }
      if (run.status) primeCheck(run.mesh, checkFromStatus(run.status));
      replacements.push({
        remove: [part.id],
        add: [{ name: part.name, mesh: run.mesh, matrix: [...part.matrix], color: part.color, visible: part.visible, transparent: part.transparent, locked: part.locked }],
      });
    }
    const n = replacements.length;
    if (n > 0) {
      useStore.getState().replaceParts(replacements, n === 1 ? `Repair ${replacements[0].add[0].name}` : `Repair ${n} parts`);
    }
    toast(n === 0 ? "No parts were repaired." : n === 1 ? "Repaired 1 part." : `Repaired ${n} parts.`);
  });
}
