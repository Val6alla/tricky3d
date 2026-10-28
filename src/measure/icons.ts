// One icon per kind of measurement tool, shared by the Measurement tab (src/features.ts) and the panel's Tools tab.
import type { LucideIcon } from "lucide-react";
import { Angle, BrickWall, CircleDashed, CircleDot, Crosshair, Globe, Slash, Square } from "lucide-react";
import type { ToolGroup } from "./tools";

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
