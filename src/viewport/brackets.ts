import { Group, Vector2, type InterleavedBufferAttribute } from "three";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import { LineSegments2 } from "three/examples/jsm/lines/LineSegments2.js";
import { LineSegmentsGeometry } from "three/examples/jsm/lines/LineSegmentsGeometry.js";
import type { Bounds } from "../mesh";
import { BRACKET_FLOATS, bracketSegments } from "./bracketsShape";
import type { VpTheme } from "./theme";
import type { SceneLayer } from "./util";

export interface Brackets extends SceneLayer {
  /** Shows brackets at the 8 corners of `box` (world mm), or hides them for null. */
  update(box: Bounds | null): void;
}

/** Line width in CSS pixels, the same on every display density. */
const LINE_WIDTH = 2;
/** Width of the outline drawn under each line: 1 CSS px on each side. */
const HALO_WIDTH = LINE_WIDTH + 2;
/** After parts, transparent parts and the platform; the gizmo (drawn last) still sits on top. */
const RENDER_ORDER = 10;

/**
 * Orange corner brackets around the selection's combined box. Drawn without depth test so parts
 * never hide them, and never hit by picking. A thin outline in the surface colour runs under each
 * line, so the brackets stay readable where they cross the selected parts, which are orange too.
 * update() runs on every gizmo drag step, so it only rewrites one 144-float buffer in place.
 */
export function createBrackets(theme: VpTheme): Brackets {
  const geometry = new LineSegmentsGeometry().setPositions(new Float32Array(BRACKET_FLOATS));
  const buffer = (geometry.getAttribute("instanceStart") as InterleavedBufferAttribute).data;
  const drawingBuffer = new Vector2();

  /** One pass over the shared segments: `width` CSS px, drawn after lower `order`s. */
  function pass(width: number, order: number) {
    const material = new LineMaterial({ linewidth: width, depthTest: false, depthWrite: false, transparent: true });
    const lines = new LineSegments2(geometry, material);
    lines.renderOrder = order;
    // The buffer changes in place without new bounds, so culling could hide the brackets.
    lines.frustumCulled = false;
    lines.raycast = () => {};
    // LineMaterial widths are in units of `resolution`; drawing-buffer pixels times the ratio keeps CSS px.
    lines.onBeforeRender = (renderer) => {
      material.resolution.copy(renderer.getDrawingBufferSize(drawingBuffer));
      material.linewidth = width * renderer.getPixelRatio();
    };
    return { lines, material };
  }

  const halo = pass(HALO_WIDTH, RENDER_ORDER);
  const line = pass(LINE_WIDTH, RENDER_ORDER + 1);

  const group = new Group();
  group.add(halo.lines, line.lines);
  group.visible = false;

  function applyTheme(t: VpTheme) {
    halo.material.color.set(t.surface);
    line.material.color.set(t.selected);
  }
  applyTheme(theme);

  return {
    group,
    applyTheme,
    update(box) {
      group.visible = box !== null;
      if (!box) return;
      bracketSegments(box, buffer.array as Float32Array);
      buffer.needsUpdate = true;
    },
    dispose() {
      geometry.dispose();
      halo.material.dispose();
      line.material.dispose();
    },
  };
}
