import type { Vector3 } from "three";
import { activePlanes, anyEnabled, isPointClipped, sceneBounds, useClip, type PlaneSpec } from "../clip";
import { useStore } from "../store";
import type { PartsLayer } from "./parts";

export interface ClipLayer {
  /** True when the clip planes hide this world point (not drawn at all); picking skips such hits. Same planes as the drawing. */
  isClipped(point: Vector3): boolean;
  /**
   * True when the clip planes remove this world point, drawn as a ghost or not: repair picking skips such hits. A
   * part's ghost shows the part selected, so it may be clicked; the working copy's ghost shows no selection, and a
   * triangle picked there would be one the user cannot see.
   */
  isCut(point: Vector3): boolean;
  dispose(): void;
}

/**
 * Applies the clip planes from the clip store (src/clip.ts) to the parts layer and re-renders when
 * they change. The renderer has localClippingEnabled set. The planes depend on the scene bounds
 * (unset offsets sit mid-range, set ones are clamped to it), so part and platform changes count too.
 * With "Transparent cuts" the clipped-away region is drawn as a ghost, so nothing counts as clipped and a
 * click on a ghost selects its part, the same as a click on a see-through part. Repair picking asks isCut,
 * which ignores the ghost.
 */
export function createClipLayer(parts: PartsLayer, requestRender: () => void): ClipLayer {
  let planes: PlaneSpec[] = [];
  let ghost = false;

  /** `render` is false for the first call: the engine is still being built and draws once it is ready. */
  function update(render = true) {
    const clip = useClip.getState();
    const s = useStore.getState();
    // Scene bounds cost a pass over every visible part's cached box; skip them while nothing clips.
    const specs = anyEnabled(clip) ? activePlanes(clip, sceneBounds(s.platform, s.parts)) : [];
    const nextGhost = clip.transparentCuts;
    if (nextGhost === ghost && samePlanes(planes, specs)) return;
    planes = specs;
    ghost = nextGhost;
    parts.setClipping(planes, ghost);
    if (render) requestRender();
  }

  const unsubscribeClip = useClip.subscribe(() => update());
  const unsubscribeStore = useStore.subscribe((s, prev) => {
    if (planes.length && (s.parts !== prev.parts || s.platform !== prev.platform)) update();
  });
  update(false);

  const isCut = (point: Vector3) => isPointClipped(planes, [point.x, point.y, point.z]);
  return {
    isClipped: (point) => !ghost && isCut(point),
    isCut,
    dispose() {
      unsubscribeClip();
      unsubscribeStore();
      parts.setClipping([], false);
    },
  };
}

function samePlanes(current: readonly PlaneSpec[], next: readonly PlaneSpec[]): boolean {
  return (
    current.length === next.length &&
    current.every((p, i) => p.normal.every((v, a) => v === next[i].normal[a]) && p.constant === next[i].constant)
  );
}
