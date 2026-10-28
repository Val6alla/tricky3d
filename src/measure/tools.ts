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

/** What the Tools tab's Chosen tools box shows a tool working out. */
const OP_WORDS: Record<MeasureOp, string> = {
  distance: "Distance",
  "centre-distance": "Distance",
  angle: "Angle",
  "angle-to-yz": "Angle",
  "angle-to-xz": "Angle",
  "angle-to-xy": "Angle",
  radius: "Radius",
  position: "Position",
  thickness: "Wall thickness",
  angle3: "Angle",
};

/** One of the things a tool measures between, for the Chosen tools box. `icon` names the picture beside it. */
export interface ChosenItem {
  label: string;
  icon: Need | "centre" | "yz" | "xz" | "xy";
}

/** What each click (or set of clicks) picks, as the Chosen tools box names it. */
const ELEMENT_NAMES: Record<Need, string> = {
  point: "Point",
  edge: "Edge",
  circle: "Detected circle",
  circle3: "3-point circle",
  sphere4: "4-point sphere",
  surface: "Surface",
  wall: "Wall",
};

const PLANES = { "angle-to-yz": "yz", "angle-to-xz": "xz", "angle-to-xy": "xy" } as const;

/** What the tool measures between, in click order: two circle centres, an edge and the XY plane, a corner's three points. */
export function chosenItems(t: MeasureTool): ChosenItem[] {
  if (t.op === "centre-distance") return t.needs.map(() => ({ label: "Circle centre", icon: "centre" }));
  if (t.op === "angle3") return ["Point", "Corner", "Point"].map((label) => ({ label, icon: "point" }));
  const items: ChosenItem[] = t.needs.map((n) => ({ label: ELEMENT_NAMES[n], icon: n }));
  if (t.op === "angle-to-yz" || t.op === "angle-to-xz" || t.op === "angle-to-xy") {
    const plane = PLANES[t.op];
    items.push({ label: `${plane.toUpperCase()} plane`, icon: plane });
  }
  return items;
}

export const chosenOp = (t: MeasureTool) => OP_WORDS[t.op];

/** "Distance: Detected circle, 3-point circle": what the tool works out, and between what. */
export function chosenText(t: MeasureTool): string {
  return `${chosenOp(t)}: ${chosenItems(t)
    .map((i) => i.label)
    .join(", ")}`;
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

/** The Description's line for each thing a tool needs picked: the first of its kind, then a second one. */
const PICK_LINES: Record<Need, [string, string]> = {
  point: ["Pick a point.", "Pick a second point."],
  edge: ["Pick a straight edge.", "Pick a second straight edge."],
  circle: ["Pick a round edge, such as the rim of a hole, to find its circle.", "Pick a second round edge to find its circle."],
  circle3: ["Pick three points around a circle.", "Pick three points around a second circle."],
  sphere4: ["Pick four points on a sphere.", "Pick four points on a second sphere."],
  surface: ["Pick a flat surface.", "Pick a second flat surface."],
  wall: ["Pick a point on the outside of a wall.", "Pick a point on the outside of a second wall."],
};

/** A 3-point angle's points each play their own part. */
const ANGLE3_PICKS = ["Pick a point on one arm of the angle.", "Pick the corner.", "Pick a point on the other arm."];

/**
 * The Description's last line: exactly what each tool works out from its picks (src-tauri/src/measure.rs, evaluate).
 * Circles and spheres count from their centres, except two circles, which measure rim to rim; an edge counts as the
 * whole line it lies on, and a surface as the whole plane.
 */
const MEASURES: Record<string, string> = {
  "point-point": "Measures the straight-line distance between the two points.",
  "point-edge": "Measures the shortest distance from the point to the line the edge lies on, extended past its ends.",
  "point-circle": "Measures the distance from the point to the detected circle's centre.",
  "point-circle3": "Measures the distance from the point to the 3-point circle's centre.",
  "point-sphere": "Measures the distance from the point to the sphere's centre.",
  "point-surface": "Measures the shortest distance from the point to the plane the surface lies in.",
  "point-position": "Measures where the point is: its X, Y and Z coordinates.",

  "edge-point": "Measures the shortest distance from the line the edge lies on, extended past its ends, to the point.",
  "edge-edge": "Measures the shortest distance between the lines the two edges lie on, extended past their ends.",
  "edge-sphere": "Measures the shortest distance from the line the edge lies on, extended past its ends, to the sphere's centre.",
  "edge-edge-angle": "Measures the angle between the two edges, from 0° to 90°.",
  "edge-surface-angle": "Measures the angle between the edge and the plane the surface lies in, from 0° to 90°.",
  "edge-yz": "Measures the angle between the edge and the YZ plane, from 0° to 90°.",
  "edge-xz": "Measures the angle between the edge and the XZ plane, from 0° to 90°.",
  "edge-xy": "Measures the angle between the edge and the XY plane, from 0° to 90°.",

  "circle-point": "Measures the distance from the detected circle's centre to the point.",
  "circle-circle": "Measures the shortest distance between the two detected circles, from rim to rim.",
  "circle-circle3": "Measures the shortest distance between the detected circle and the 3-point circle, from rim to rim.",
  "circle-centres": "Measures the distance between the centres of the two detected circles.",
  "circle-sphere": "Measures the distance from the detected circle's centre to the sphere's centre.",
  "circle-radius": "Measures the detected circle's radius, or its diameter when Diameter is set in Options.",

  "circle3-point": "Measures the distance from the 3-point circle's centre to the point.",
  "circle3-circle3": "Measures the shortest distance between the two 3-point circles, from rim to rim.",
  "circle3-circle": "Measures the shortest distance between the 3-point circle and the detected circle, from rim to rim.",
  "circle3-sphere": "Measures the distance from the 3-point circle's centre to the sphere's centre.",
  "circle3-radius": "Measures the 3-point circle's radius, or its diameter when Diameter is set in Options.",

  "sphere-point": "Measures the distance from the sphere's centre to the point.",
  "sphere-edge": "Measures the shortest distance from the sphere's centre to the line the edge lies on, extended past its ends.",
  "sphere-circle": "Measures the distance from the sphere's centre to the detected circle's centre.",
  "sphere-circle3": "Measures the distance from the sphere's centre to the 3-point circle's centre.",
  "sphere-sphere": "Measures the distance between the centres of the two spheres.",
  "sphere-radius": "Measures the sphere's radius, or its diameter when Diameter is set in Options.",

  "surface-point": "Measures the shortest distance from the plane the surface lies in to the point.",
  "surface-edge-angle": "Measures the angle between the plane the surface lies in and the edge, from 0° to 90°.",
  "surface-surface-angle": "Measures the angle between the directions the two surfaces face, from 0° to 180°.",
  "surface-position": "Measures where the surface's centre is (X, Y and Z) and which way the surface faces.",
  "surface-yz": "Measures the angle between the surface and the YZ plane, from 0° to 90°.",
  "surface-xz": "Measures the angle between the surface and the XZ plane, from 0° to 90°.",
  "surface-xy": "Measures the angle between the surface and the XY plane, from 0° to 90°.",

  wall: "Measures the wall's thickness: from the point straight into the part, square to the surface, to where it comes out on the other side.",
  angle3: "Measures the angle at the corner between the lines to the other two points, from 0° to 180°.",
};

/** The Tools tab's Description, line by line: what to pick, in order, then what is measured. Joined, the tool's hint. */
export function describe(t: MeasureTool): string[] {
  const picks = t.op === "angle3" ? ANGLE3_PICKS : t.needs.map((n, i) => PICK_LINES[n][t.needs.indexOf(n) < i ? 1 : 0]);
  return [...picks, MEASURES[t.id]];
}

/** The tool of `group` its button picks: the one last used from that group, else the group's first. */
export function toolForGroup(group: ToolGroup, lastTools: Partial<Record<ToolGroup, string>>): MeasureTool {
  const last = toolById(lastTools[group] ?? "");
  return last?.group === group ? last : (TOOLS.find((t) => t.group === group) as MeasureTool);
}

/** What the next click picks, for the status bar: "Click an edge (2 of 2)". */
export function nextClickText(t: MeasureTool, done: number, points: number): string {
  const need = t.needs[done];
  if (!need) return "";
  const step = `${done + 1} of ${t.needs.length}`;
  if (need === "circle3" || need === "sphere4") return `Click point ${points + 1} of ${clicksFor(need)} on the ${NEED_LABELS[need].toLowerCase()} (${step}).`;
  return `Click ${CLICK_WORDS[need]} (${step}).`;
}
