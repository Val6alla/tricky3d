// Move nodes (Home › Repair › Move nodes, and the Repair tab's): drag a vertex of the working copy and the triangles
// round it follow. The viewport finds the vertex under the pointer and shows the drag live (src/viewport/editTools.ts);
// letting go moves the vertex in Rust (the move_node action, src-tauri/src/repair/nodes.rs), which checks the move,
// as one inner undo step.
import type { MeshData, Vec3 } from "../store";
import { runTool } from "./session";
import { useRepair } from "./store";

/** True while Move nodes is on. */
export function isMovingNodes(): boolean {
  const r = useRepair.getState();
  return r.session !== null && r.editTool === "move-nodes";
}

/** Turns Move nodes on for the open repair session; a second call turns it off again. */
export function toggleMoveNodes(): void {
  const r = useRepair.getState();
  if (!r.session) return;
  r.setEditTool(r.editTool === "move-nodes" ? null : "move-nodes");
}

/** The corner of triangle `t` nearest `point`: the vertex a hover there picks. */
export function nearestCorner(mesh: MeshData, t: number, point: Vec3): number {
  const { positions, indices } = mesh;
  let best = indices[t * 3];
  let bestDistance = Infinity;
  for (let k = 0; k < 3; k++) {
    const v = indices[t * 3 + k];
    const d = Math.hypot(positions[v * 3] - point[0], positions[v * 3 + 1] - point[1], positions[v * 3 + 2] - point[2]);
    if (d < bestDistance) {
      bestDistance = d;
      best = v;
    }
  }
  return best;
}

/** A position as the tooltip shows it: mm with four decimals, the precision the repair tolerances use. */
export function formatPosition(p: Vec3): string {
  return `X ${p[0].toFixed(4)}  Y ${p[1].toFixed(4)}  Z ${p[2].toFixed(4)}`;
}

/** Moves `vertex` of the working copy to `to` (world space, mm). True when the working copy changed. */
export async function moveNode(vertex: number, to: Vec3): Promise<boolean> {
  const result = await runTool("move_node", { vertex, x: to[0], y: to[1], z: to[2] }, { withStatus: false, announce: false });
  return result?.changed === true;
}
