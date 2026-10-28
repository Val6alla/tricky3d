// One icon per group of measurement tools, shared by the Measurement tab (src/features.ts) and the panel's Tools tab,
// whose group buttons always show them; and one per tool for the rows of the dropdowns: what it measures to (a point,
// an edge...), or what it gives (a radius, a position, an angle to a plane).
import type { LucideIcon } from "lucide-react";
import {
  Angle,
  BrickWall,
  CircleDashed,
  CircleDot,
  Crosshair,
  DraftingCompass,
  Globe,
  MapPin,
  PanelBottom,
  PanelLeft,
  PanelTop,
  Radius,
  Slash,
  Square,
  Target,
  TriangleRight,
} from "lucide-react";
import type { ChosenItem, MeasureTool, ToolGroup } from "./tools";
import type { Need } from "./types";

export const GROUP_ICONS: Record<ToolGroup, LucideIcon> = {
  point: Crosshair,
  edge: Slash,
  circle: CircleDot,
  circle3: CircleDashed,
  sphere4: Globe,
  surface: Square,
  wall: BrickWall,
  angle3: Angle,
};

const NEED_ICONS: Record<Need, LucideIcon> = {
  point: Crosshair,
  edge: Slash,
  circle: CircleDot,
  circle3: CircleDashed,
  sphere4: Globe,
  surface: Square,
  wall: BrickWall,
};

/** The planes as seen from the front: XY the floor, YZ a side wall, XZ the back wall. */
const PLANE_ICONS = { yz: PanelLeft, xz: PanelTop, xy: PanelBottom };

export const CHOSEN_ICONS: Record<ChosenItem["icon"], LucideIcon> = { ...NEED_ICONS, ...PLANE_ICONS, centre: Target };

export function toolIcon(t: MeasureTool): LucideIcon {
  const last = t.needs[t.needs.length - 1];
  switch (t.op) {
    case "distance":
      return NEED_ICONS[last];
    case "centre-distance":
      return Target;
    case "angle":
      return last === "edge" ? DraftingCompass : TriangleRight;
    case "angle-to-yz":
      return PLANE_ICONS.yz;
    case "angle-to-xz":
      return PLANE_ICONS.xz;
    case "angle-to-xy":
      return PLANE_ICONS.xy;
    case "radius":
      return Radius;
    case "position":
      return MapPin;
    case "thickness":
      return BrickWall;
    case "angle3":
      return Angle;
  }
}
