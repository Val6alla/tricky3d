import { Vector3 } from "three";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useClip } from "../clip";
import { useStore } from "../store";
import { createClipLayer } from "./clipping";
import type { PartsLayer } from "./parts";

afterEach(() => {
  useClip.getState().reset();
});

describe("createClipLayer", () => {
  it("tells picking which points the planes remove: drawn parts see through a ghost, the working copy never does", () => {
    const parts = { setClipping: vi.fn() } as unknown as PartsLayer;
    const layer = createClipLayer(parts, () => {});
    const width = useStore.getState().platform[0];
    const clip = useClip.getState();
    // X plane in the middle of the platform: everything beyond it is cut away.
    clip.setEnabled("x", true, [0, width]);
    clip.setOffset("x", width / 2);
    const removed = new Vector3(width * 0.75, 1, 1);
    const kept = new Vector3(width * 0.25, 1, 1);
    expect([layer.isClipped(removed), layer.isCut(removed)]).toEqual([true, true]);
    expect([layer.isClipped(kept), layer.isCut(kept)]).toEqual([false, false]);

    // Transparent cuts: a part's ghost is drawn in its selection colour, so a click on it may pick the part. The
    // working copy's ghost shows no selection, so a triangle there must not be picked over what is drawn behind it.
    clip.setTransparentCuts(true);
    expect([layer.isClipped(removed), layer.isCut(removed)]).toEqual([false, true]);
    expect([layer.isClipped(kept), layer.isCut(kept)]).toEqual([false, false]);
    layer.dispose();
  });
});
