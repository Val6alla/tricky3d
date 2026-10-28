// Measure mode's tools, as the Measurement tab's dropdowns list them: what each one needs clicked, in order, and what
// Rust works out from it. The ribbon (src/features.ts), the panel's Tools tab and the session all read this table.
import type { MeasureOp, Need } from "./types";

/** The Measurement tab's buttons: six dropdowns, then Wall thickness and 3-point angle. */
export type ToolGroup = "point" | "edge" | "circle" | "circle3" | "sphere4" | "surface" | "wall" | "angle3";

export interface MeasureTool {
  id: string;
  group: ToolGroup;
  label: string;
  needs: Need[];
  op: MeasureOp;
}

export const GROUP_LABELS: Record<ToolGroup, string> = {
  point: "Point",
  edge: "Edge",
  circle: "Circle",
  circle3: "3-point circle",
  sphere4: "4-point sphere",
  surface: "Surface",
  wall: "Wall thickness",
  angle3: "3-point angle",
};

const tool = (id: string, group: ToolGroup, label: string, op: MeasureOp, ...needs: Need[]): MeasureTool => ({ id, group, label, op, needs });

export const TOOLS: MeasureTool[] = [
  tool("point-point", "point", "Distance point to point", "distance", "point", "point"),
  tool("point-edge", "point", "Distance point to edge", "distance", "point", "edge"),
  tool("point-circle", "point", "Distance point to detected circle", "distance", "point", "circle"),
  tool("point-circle3", "point", "Distance point to 3-point circle", "distance", "point", "circle3"),
  tool("point-sphere", "point", "Distance point to sphere", "distance", "point", "sphere4"),
  tool("point-surface", "point", "Distance point to surface", "distance", "point", "surface"),
  tool("point-position", "point", "Position point", "position", "point"),

  tool("edge-point", "edge", "Distance edge to point", "distance", "edge", "point"),
  tool("edge-edge", "edge", "Distance edge to edge", "distance", "edge", "edge"),
  tool("edge-sphere", "edge", "Distance edge to sphere", "distance", "edge", "sphere4"),
  tool("edge-edge-angle", "edge", "Angle edge to edge", "angle", "edge", "edge"),
  tool("edge-surface-angle", "edge", "Angle edge to surface", "angle", "edge", "surface"),
  tool("edge-yz", "edge", "Angle edge to YZ plane", "angle-to-yz", "edge"),
  tool("edge-xz", "edge", "Angle edge to XZ plane", "angle-to-xz", "edge"),
  tool("edge-xy", "edge", "Angle edge to XY plane", "angle-to-xy", "edge"),

  tool("circle-point", "circle", "Distance circle to point", "distance", "circle", "point"),
  tool("circle-circle", "circle", "Distance circle to circle", "distance", "circle", "circle"),
  tool("circle-circle3", "circle", "Distance detected circle to 3-point circle", "distance", "circle", "circle3"),
  tool("circle-centres", "circle", "Distance circle centre to circle centre", "centre-distance", "circle", "circle"),
  tool("circle-sphere", "circle", "Distance circle to sphere", "distance", "circle", "sphere4"),
  tool("circle-radius", "circle", "Radius circle", "radius", "circle"),

  tool("circle3-point", "circle3", "Distance circle to point", "distance", "circle3", "point"),
  tool("circle3-circle3", "circle3", "Distance 3-point circle to 3-point circle", "distance", "circle3", "circle3"),
  tool("circle3-circle", "circle3", "Distance 3-point circle to detected circle", "distance", "circle3", "circle"),
  tool("circle3-sphere", "circle3", "Distance circle to sphere", "distance", "circle3", "sphere4"),
  tool("circle3-radius", "circle3", "Radius circle", "radius", "circle3"),

  tool("sphere-point", "sphere4", "Distance sphere to point", "distance", "sphere4", "point"),
  tool("sphere-edge", "sphere4", "Distance sphere to edge", "distance", "sphere4", "edge"),
  tool("sphere-circle", "sphere4", "Distance sphere to detected circle", "distance", "sphere4", "circle"),
  tool("sphere-circle3", "sphere4", "Distance sphere to 3-point circle", "distance", "sphere4", "circle3"),
  tool("sphere-sphere", "sphere4", "Distance sphere to sphere", "distance", "sphere4", "sphere4"),
  tool("sphere-radius", "sphere4", "Radius sphere", "radius", "sphere4"),

  tool("surface-point", "surface", "Distance surface to point", "distance", "surface", "point"),
  tool("surface-edge-angle", "surface", "Angle surface to edge", "angle", "surface", "edge"),
  tool("surface-surface-angle", "surface", "Angle surface to surface", "angle", "surface", "surface"),
  tool("surface-position", "surface", "Position surface", "position", "surface"),
  tool("surface-yz", "surface", "Angle surface to YZ plane", "angle-to-yz", "surface"),
  tool("surface-xz", "surface", "Angle surface to XZ plane", "angle-to-xz", "surface"),
  tool("surface-xy", "surface", "Angle surface to XY plane", "angle-to-xy", "surface"),

  tool("wall", "wall", "Wall thickness", "thickness", "wall"),
  tool("angle3", "angle3", "3-point angle", "angle3", "point", "point", "point"),
];

export const DEFAULT_TOOL = "point-point";

const byId = new Map(TOOLS.map((t) => [t.id, t]));

export function toolById(id: string): MeasureTool | undefined {
  return byId.get(id);
}

const NEED_LABELS: Record<Need, string> = {
  point: "Point",
  edge: "Edge",
  circle: "Circle",
  circle3: "3-point circle",
  sphere4: "4-point sphere",
  surface: "Surface",
  wall: "Wall",
};

const OP_WORDS: Record<MeasureOp, string> = {
  distance: "Distance",
  "centre-distance": "Distance between centres",
  angle: "Angle",
  "angle-to-yz": "Angle to the YZ plane",
  "angle-to-xz": "Angle to the XZ plane",
  "angle-to-xy": "Angle to the XY plane",
  radius: "Radius",
  position: "Position",
  thickness: "Wall thickness",
  angle3: "Angle",
};

/** "Distance: Point, Edge": what the tool works out, and what it needs clicked. */
export function chosenText(t: MeasureTool): string {
  return `${OP_WORDS[t.op]}: ${t.needs.map((n) => NEED_LABELS[n]).join(", ")}`;
}

/** How many clicks a need takes. */
export const clicksFor = (n: Need) => (n === "circle3" ? 3 : n === "sphere4" ? 4 : 1);

const CLICK_WORDS: Record<Need, string> = {
  point: "a point",
  edge: "an edge",
  circle: "the rim of a round hole or edge",
  circle3: "three points on a circle",
  sphere4: "four points on a sphere",
  surface: "a flat surface",
  wall: "a point on a wall",
};

const RESULT_WORDS: Record<MeasureOp, string> = {
  distance: "The distance between them is shown, and along X, Y and Z if Options asks for it. Circles and spheres count from their centres; two circles from rim to rim.",
  "centre-distance": "The distance between the two centres is shown.",
  angle: "The angle between them is shown.",
  "angle-to-yz": "Its angle to the YZ plane is shown.",
  "angle-to-xz": "Its angle to the XZ plane is shown.",
  "angle-to-xy": "Its angle to the XY plane is shown.",
  radius: "Its radius is shown, or its diameter if Options asks for it.",
  position: "Its position on the platform is shown.",
  thickness: "The distance straight through the material to the opposite wall is shown.",
  angle3: "The angle at the middle point is shown.",
};

/** The Tools tab's Description: what to click, in order, and what comes out. */
export function describe(t: MeasureTool): string {
  const clicks = t.needs.map((n) => CLICK_WORDS[n]);
  const order = clicks.length === 1 ? `Click ${clicks[0]}.` : `Click ${clicks.slice(0, -1).join(", then ")}, then ${clicks[clicks.length - 1]}.`;
  return `${order} ${RESULT_WORDS[t.op]}`;
}

/** What the next click picks, for the status bar: "Click an edge (2 of 2)". */
export function nextClickText(t: MeasureTool, done: number, points: number): string {
  const need = t.needs[done];
  if (!need) return "";
  const step = `${done + 1} of ${t.needs.length}`;
  if (need === "circle3" || need === "sphere4") return `Click point ${points + 1} of ${clicksFor(need)} on the ${NEED_LABELS[need].toLowerCase()} (${step}).`;
  return `Click ${CLICK_WORDS[need]} (${step}).`;
}
