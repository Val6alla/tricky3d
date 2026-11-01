// Analyses as a project file holds them: JSON in each part's metadata (src-tauri/src/project.rs), the drawing's
// arrays as base64. Reading checks every field, since the file may come from anywhere; whatever does not fit is left
// out.
import type { Vec3 } from "../store";
import type { AnalysisDrawing, CompareSurface, PartAnalysis, Probe, Spread } from "./types";

const isNum = (v: unknown): v is number => typeof v === "number" && Number.isFinite(v);
const isBool = (v: unknown): v is boolean => typeof v === "boolean";
const isVec3 = (v: unknown): v is Vec3 => Array.isArray(v) && v.length === 3 && v.every(isNum);
const isObj = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v);
const isSpread = (v: unknown): v is Spread => isObj(v) && isNum(v.min) && isNum(v.max) && isNum(v.average) && isNum(v.deviation);
const hasNums = (o: Record<string, unknown>, keys: string[]) => keys.every((k) => isNum(o[k]));

/** Bytes as base64, in slices so big arrays do not overflow the argument list. */
export function toBase64(bytes: Uint8Array): string {
  let text = "";
  for (let i = 0; i < bytes.length; i += 0x8000) text += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(text);
}

export function fromBase64(text: string): Uint8Array | null {
  try {
    const raw = atob(text);
    const out = new Uint8Array(raw.length);
    for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
    return out;
  } catch {
    return null;
  }
}

/** Floats from base64; with `gaps`, NaN may stand for "no value" (never an infinity). */
function floats(text: unknown, gaps = false): Float32Array | undefined {
  if (typeof text !== "string") return undefined;
  const bytes = fromBase64(text);
  if (!bytes || bytes.length % 4 !== 0) return undefined;
  const out = new Float32Array(bytes.buffer, 0, bytes.length / 4);
  return out.every((v) => Number.isFinite(v) || (gaps && Number.isNaN(v))) ? out : undefined;
}

function uints(text: unknown): Uint32Array | undefined {
  if (typeof text !== "string") return undefined;
  const bytes = fromBase64(text);
  if (!bytes || bytes.length % 4 !== 0) return undefined;
  return new Uint32Array(bytes.buffer, 0, bytes.length / 4);
}

const uintText = (a: Uint32Array) => toBase64(new Uint8Array(a.buffer, a.byteOffset, a.byteLength));

function surfaceOf(raw: unknown): CompareSurface | null {
  if (!isObj(raw) || (raw.role !== "reference" && raw.role !== "comparison")) return null;
  const positions = floats(raw.positions);
  const indices = uints(raw.indices);
  const values = floats(raw.values);
  if (!positions || !indices || !values || positions.length % 3 !== 0 || indices.length % 3 !== 0) return null;
  const count = positions.length / 3;
  if (values.length !== count || indices.some((i) => i >= count)) return null;
  return { role: raw.role, positions, indices, values };
}

const floatText = (a: Float32Array) => toBase64(new Uint8Array(a.buffer, a.byteOffset, a.byteLength));

function drawingOf(raw: unknown, triangles: number): AnalysisDrawing {
  const d: AnalysisDrawing = {};
  if (!isObj(raw)) return d;
  if (typeof raw.classes === "string") {
    const classes = fromBase64(raw.classes);
    // Colours for another mesh than the part's own are no use.
    if (classes && classes.length === triangles) d.classes = classes;
  }
  const t = floats(raw.triangles);
  if (t && t.length % 9 === 0) d.triangles = t;
  const l = floats(raw.lines);
  if (l && l.length % 6 === 0) d.lines = l;
  const v = floats(raw.values, true);
  if (v && v.length === triangles) d.values = v;
  if (Array.isArray(raw.surfaces)) {
    const surfaces = raw.surfaces.map(surfaceOf).filter((x): x is CompareSurface => x !== null);
    if (surfaces.length > 0) d.surfaces = surfaces;
  }
  return d;
}

function validResult(kind: unknown, r: unknown, settings: unknown): boolean {
  if (!isObj(r)) return false;
  switch (kind) {
    case "standard":
      return (
        isVec3(r.min) &&
        isVec3(r.max) &&
        isVec3(r.size) &&
        hasNums(r, ["volume", "area", "points", "triangles", "edges", "shells", "holes", "boundaryEdges", "boundaryLength", "flippedTriangles", "badEdges"]) &&
        isBool(r.volumeValid) &&
        isBool(r.closed) &&
        isBool(r.orientable) &&
        ["edgesPerPoint", "trianglesPerEdge", "triangleQuality", "edgeLength"].every((k) => isSpread(r[k]))
      );
    case "gravity":
      return isVec3(r.centre) && isVec3(r.min) && isVec3(r.max) && isVec3(r.size) && isNum(r.volume);
    case "shadow":
      return isNum(r.area);
    case "support":
      return hasNums(r, ["angle", "volume", "area", "triangles"]) && isObj(settings) && isNum(settings.angle);
    case "skin": {
      const totals = (t: unknown) => isObj(t) && isBool(t.analyzed) && hasNums(t, ["angle", "area", "components"]);
      const s = settings;
      return (
        totals(r.up) &&
        totals(r.down) &&
        isObj(s) &&
        isBool(s.upskins) &&
        isBool(s.downskins) &&
        isBool(s.filterSmall) &&
        hasNums(s, ["upAngle", "downAngle", "minComponent"])
      );
    }
    case "removability":
      return (
        isBool(r.removable) &&
        hasNums(r, ["bodies", "trappedBodies", "trappedArea"]) &&
        (r.minDistance === undefined || isNum(r.minDistance)) &&
        (settings === null || settings === undefined || (isObj(settings) && isNum(settings.minDistance)))
      );
    case "wall": {
      const s = settings;
      const point = (v: unknown) => v === null || isVec3(v);
      return (
        hasNums(r, ["threshold", "acceptance", "thinPercent", "thinArea", "totalArea", "unmeasured"]) &&
        isBool(r.passed) &&
        isBool(r.complete) &&
        (r.minThickness === null || isNum(r.minThickness)) &&
        point(r.minFrom) &&
        point(r.minTo) &&
        isObj(s) &&
        hasNums(s, ["threshold", "acceptance"]) &&
        isBool(s.stopEarly) &&
        (s.simple === undefined || isBool(s.simple)) &&
        (r.decided === undefined || isBool(r.decided)) &&
        (r.stoppedAtThin === undefined || isBool(r.stoppedAtThin)) &&
        isBool(s.colouredPart) &&
        (s.colors === "red-green" || s.colors === "red-green-gradient" || s.colors === "blue-yellow-gradient")
      );
    }
    case "compare": {
      const s = settings;
      const side = (v: unknown) =>
        v === null || (isObj(v) && hasNums(v, ["maxPositive", "maxNegative", "averageSigned", "maxUnsigned", "averageUnsigned", "points"]));
      const fit = (v: unknown) =>
        v === null ||
        (isObj(v) && Array.isArray(v.rotation) && v.rotation.length === 3 && v.rotation.every(isVec3) && isVec3(v.translation) && hasNums(v, ["angle", "rms"]));
      return (
        side(r.forward) &&
        side(r.backward) &&
        fit(r.fit) &&
        isObj(s) &&
        typeof s.comparisonId === "string" &&
        typeof s.comparisonName === "string" &&
        Array.isArray(s.comparisonMatrix) &&
        s.comparisonMatrix.length === 16 &&
        s.comparisonMatrix.every(isNum) &&
        (s.direction === "both" || s.direction === "reference-to-comparison" || s.direction === "comparison-to-reference") &&
        isBool(s.align)
      );
    }
    default:
      return false;
  }
}

function probesOf(raw: unknown): Probe[] {
  if (!Array.isArray(raw)) return [];
  return raw.filter((p): p is Probe => isObj(p) && typeof p.id === "string" && isVec3(p.point) && isNum(p.value));
}

/** The analysis a saved entry describes for a part of `triangles` triangles, or null when it is not one. */
export function analysisOf(raw: unknown, triangles: number): PartAnalysis | null {
  if (!isObj(raw)) return null;
  const matrix = raw.matrix;
  if (typeof raw.id !== "string" || !Array.isArray(matrix) || matrix.length !== 16 || !matrix.every(isNum)) return null;
  if (!validResult(raw.kind, raw.result, raw.settings)) return null;
  const probes = raw.kind === "compare" ? probesOf(raw.probes) : [];
  return {
    id: raw.id,
    kind: raw.kind,
    settings: raw.settings ?? null,
    result: raw.result,
    matrix: [...matrix],
    drawing: drawingOf(raw.drawing, triangles),
    ...(probes.length > 0 ? { probes } : {}),
  } as PartAnalysis;
}

/** The analyses in a part's saved JSON; undefined when there are none (or none that fit). */
export function readAnalyses(json: string | undefined, triangles: number): PartAnalysis[] | undefined {
  if (!json) return undefined;
  let raw: unknown;
  try {
    raw = JSON.parse(json);
  } catch {
    return undefined;
  }
  if (!Array.isArray(raw)) return undefined;
  const list = raw.map((a) => analysisOf(a, triangles)).filter((a): a is PartAnalysis => a !== null);
  return list.length > 0 ? list : undefined;
}

/** The JSON a project keeps for a part's analyses. */
export function writeAnalyses(list: readonly PartAnalysis[]): string {
  return JSON.stringify(
    list.map((a) => ({
      id: a.id,
      kind: a.kind,
      settings: a.settings,
      result: a.result,
      matrix: a.matrix,
      ...(a.kind === "compare" && a.probes && a.probes.length > 0 ? { probes: a.probes } : {}),
      drawing: {
        ...(a.drawing.classes ? { classes: toBase64(a.drawing.classes) } : {}),
        ...(a.drawing.values ? { values: floatText(a.drawing.values) } : {}),
        ...(a.drawing.triangles ? { triangles: floatText(a.drawing.triangles) } : {}),
        ...(a.drawing.lines ? { lines: floatText(a.drawing.lines) } : {}),
        ...(a.drawing.surfaces
          ? {
              surfaces: a.drawing.surfaces.map((x) => ({
                role: x.role,
                positions: floatText(x.positions),
                indices: uintText(x.indices),
                values: floatText(x.values),
              })),
            }
          : {}),
      },
    })),
  );
}
