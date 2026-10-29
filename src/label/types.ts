// What a placed label is, as label mode keeps it and as Rust receives it (src-tauri/src/label/mod.rs Spec).
import type { Picture } from "../library/ipc";
import type { Vec3 } from "../store";

/** Text, a picture, text from a list (one copy per entry), or text on a breakaway tag. */
export type LabelKind = "text" | "image" | "list" | "tag";

/** Raised or sunk, plain or mirrored (for moulds and stamps). */
export type BuildType = "add" | "add-mirrored" | "subtract" | "subtract-mirrored";

export const BUILD_TYPES: { value: BuildType; label: string }[] = [
  { value: "add", label: "Add to part" },
  { value: "add-mirrored", label: "Add to part (inverted)" },
  { value: "subtract", label: "Subtract from part" },
  { value: "subtract-mirrored", label: "Subtract from part (inverted)" },
];

export function isMirrored(build: BuildType): boolean {
  return build === "add-mirrored" || build === "subtract-mirrored";
}

/** Where a list label's entries come from: running numbers, or a column of a CSV file. */
export type ListSource =
  | { kind: "numbers"; start: number; step: number; count: number; prefix: string; suffix: string; digits: number }
  | { kind: "csv"; file: string; rows: string[][]; column: number; header: boolean };

export const DEFAULT_NUMBERS: ListSource = { kind: "numbers", start: 1, step: 1, count: 5, prefix: "", suffix: "", digits: 3 };

/** A font from label_fonts (src-tauri/src/label/font.rs FontInfo). */
export interface FontInfo {
  id: string;
  family: string;
  style: string;
  bundled: boolean;
}

export const BUNDLED_FONT: FontInfo = { id: "bundled", family: "Noto Sans (built in)", style: "Regular", bundled: true };

export interface PlacedLabel {
  id: string;
  kind: LabelKind;
  /** Text and Tag; it starts as the part's name. */
  caption: string;
  font: FontInfo;
  /** Points: how finely the letters' curves are cut, not how big they are. */
  fontSize: number;
  /** mm. */
  height: number;
  /** mm; follows the height while keepAspect is on. */
  width: number;
  keepAspect: boolean;
  /** mm, always positive: the build type says raised or sunk. */
  depth: number;
  build: BuildType;
  /** Tag: the plate's thickness, mm. */
  thickness: number;
  /** World point the label is centred on, and the surface normal there. */
  point: Vec3;
  normal: Vec3;
  /** Turn about the normal, degrees, anticlockwise seen from outside. */
  angle: number;
  /** Tag: the direction the plate stands out in; null is the normal. */
  tagDirection: Vec3 | null;
  /** Image: grey level below which the picture is part of the label, 0..1. */
  threshold: number;
  picture: Picture | null;
  list: ListSource;
  /** True from placing until Rust has sized it to where it went (session.ts fitLabel); not drawn till then. */
  fitting: boolean;
  /** True until the user sizes it: a picture picked then fits it again. */
  sizeAuto: boolean;
  /** True until the user sets Depth: until then the depth follows the starting rule (startDepth). */
  depthAuto: boolean;
  /** The part's thickness under the point placed at, mm; null when unknown. */
  partThickness: number | null;
}

export const KIND_LABELS: Record<LabelKind, string> = {
  text: "Text label",
  image: "Image label",
  list: "Text from list",
  tag: "Tag",
};

/** Tallest a label starts, mm (src-tauri/src/label/fit.rs MAX_HEIGHT). */
export const START_HEIGHT = 10;
/** Deepest a label starts, mm, and the share of the part's thickness a sunk one may take. */
export const START_DEPTH = 5;
export const SUNK_SHARE = 0.4;
/** A tag's caption starts this share of its plate's thickness deep, raised or sunk. */
export const TAG_DEPTH_SHARE = 0.3;

/**
 * A label as placed, before Rust has fitted it to where it went (session.ts fitLabel): every new label starts fresh,
 * nothing about its size comes from the labels before it. Keep aspect ratio is on.
 */
export function newLabel(id: string, kind: LabelKind, caption: string, point: Vec3, normal: Vec3, font: FontInfo = BUNDLED_FONT): PlacedLabel {
  const label: PlacedLabel = {
    id,
    kind,
    caption,
    font,
    fontSize: 48,
    height: START_HEIGHT,
    width: 0,
    keepAspect: true,
    depth: 0,
    build: "add",
    thickness: 2,
    point,
    normal,
    angle: 0,
    tagDirection: null,
    threshold: 0.5,
    picture: null,
    list: DEFAULT_NUMBERS,
    fitting: true,
    sizeAuto: true,
    depthAuto: true,
    partThickness: null,
  };
  return { ...label, depth: startDepth(label) };
}

export function isSunk(build: BuildType): boolean {
  return build === "subtract" || build === "subtract-mirrored";
}

/**
 * The depth a label starts at: the smaller of 5 mm and half its height, and sunk, at most 40 % of the part's
 * thickness under it. A tag's caption starts at 30 % of its plate's thickness, raised or sunk. Rounded down to
 * 0.01 mm, and never 0.
 */
export function startDepth(label: PlacedLabel): number {
  let depth: number;
  if (label.kind === "tag") {
    depth = TAG_DEPTH_SHARE * label.thickness;
  } else {
    depth = Math.min(START_DEPTH, label.height / 2);
    const under = label.partThickness;
    if (isSunk(label.build) && under !== null && under > 0) depth = Math.min(depth, SUNK_SHARE * under);
  }
  return Math.max(0.01, Math.floor(depth * 100 + 1e-9) / 100);
}

/** The running numbers a list makes: prefix, the number padded with zeros to `digits`, suffix. */
export function runningNumbers(s: Extract<ListSource, { kind: "numbers" }>): string[] {
  const count = Math.max(0, Math.min(500, Math.floor(s.count)));
  const out: string[] = [];
  for (let i = 0; i < count; i++) {
    const n = s.start + i * s.step;
    const sign = n < 0 ? "-" : "";
    out.push(`${s.prefix}${sign}${String(Math.abs(n)).padStart(Math.max(0, Math.floor(s.digits)), "0")}${s.suffix}`);
  }
  return out;
}

/** A list label's entries, empty cells left out. */
export function listEntries(source: ListSource): string[] {
  if (source.kind === "numbers") return runningNumbers(source);
  return source.rows
    .slice(source.header ? 1 : 0)
    .map((row) => (row[source.column] ?? "").trim())
    .filter((cell) => cell !== "");
}

/** The text a label's shape is drawn from: its caption, or a list's first entry. */
export function shapeText(label: PlacedLabel): string {
  return label.kind === "list" ? (listEntries(label.list)[0] ?? "") : label.caption;
}

/** The label as label_apply takes it. */
export function toSpec(label: PlacedLabel, width: number) {
  return {
    kind: label.kind,
    text: label.caption,
    entries: label.kind === "list" ? listEntries(label.list) : [],
    font: label.font.id,
    fontSize: label.fontSize,
    height: label.height,
    width,
    keepAspect: label.keepAspect,
    depth: label.depth,
    build: label.build,
    thickness: label.thickness,
    point: label.point,
    normal: label.normal,
    angle: label.angle,
    tagDirection: label.tagDirection,
    threshold: label.threshold,
  };
}
