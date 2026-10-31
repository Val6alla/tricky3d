// What the view draws for the analysis on show: the part painted by its results (grey with green and red for up- and
// downskins and support volume, by thickness for wall thickness), shapes Rust worked out in world space (the shadow
// on the platform, the support blocks), markers (the centre of gravity, the thinnest wall, measure points), and for
// mesh compare the refined copies coloured by distance. The parts layer leaves a painted part to this layer
// (PartsLook.hidden) and greys the part for the other kinds (PartsLook.colors).
import {
  BufferAttribute,
  BufferGeometry,
  Color,
  DoubleSide,
  Group,
  LineBasicMaterial,
  LineSegments,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  Plane,
  Raycaster,
  SphereGeometry,
  WireframeGeometry,
} from "three";
import { ANALYSIS_COLORS, band, compareColor, histogram, largestDistance, PART_CLASS, paintClass, wallColor, type Rgb } from "../analysis/colors";
import type { CompareView, PartAnalysis } from "../analysis/types";
import type { Part, Vec3 } from "../store";
import type { PartsLayer } from "./parts";

/** The analysis on show, its part, and whether a part has moved since it ran. */
export interface AnalysisView {
  part: Part;
  analysis: PartAnalysis;
  stale: boolean;
}

export interface AnalysisLayer {
  group: Group;
  /** Draws `view`, or nothing; `compare` says how a comparison is drawn. */
  sync(view: AnalysisView | null, compare: CompareView): void;
  /** The compared surface under the ray, with the distance there; null off it or for another kind. */
  probe(raycaster: Raycaster): { point: Vec3; value: number } | null;
  dispose(): void;
}

/** Render order of the markers, drawn over the part so they show through it. */
const MARKER_ORDER = 30;
const MARKER_COLOR = "#e0218a";
const EDGE_COLOR = "#f2c230";

/** The part's triangles one by one (no shared corners), each corner coloured with its triangle's colour. */
function paintedGeometry(part: Part, colorOf: (t: number) => Rgb): BufferGeometry {
  const { positions, indices } = part.mesh;
  const count = indices.length;
  const corners = new Float32Array(count * 3);
  const colors = new Float32Array(count * 3);
  for (let i = 0; i < count; i++) {
    const v = indices[i] * 3;
    corners[i * 3] = positions[v];
    corners[i * 3 + 1] = positions[v + 1];
    corners[i * 3 + 2] = positions[v + 2];
    const color = colorOf(Math.floor(i / 3));
    colors[i * 3] = color[0];
    colors[i * 3 + 1] = color[1];
    colors[i * 3 + 2] = color[2];
  }
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(corners, 3));
  geometry.setAttribute("color", new BufferAttribute(colors, 3));
  geometry.computeBoundingSphere();
  return geometry;
}

function soup(positions: Float32Array): BufferGeometry {
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(positions, 3));
  geometry.computeBoundingSphere();
  return geometry;
}

const surfaceMaterial = () =>
  new MeshStandardMaterial({ vertexColors: true, flatShading: true, roughness: 0.8, metalness: 0, side: DoubleSide, polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1 });

export function createAnalysisLayer(parts: PartsLayer): AnalysisLayer {
  const group = new Group();
  // Both sides, so a cut through the part shows the colours inside too.
  const painted = new Mesh(new BufferGeometry(), surfaceMaterial());
  painted.matrixAutoUpdate = false;
  // Shapes on the platform and blocks: see-through so the part and the platform show through.
  const shapes = new Mesh(
    new BufferGeometry(),
    new MeshBasicMaterial({ color: "#1f2933", transparent: true, opacity: 0.35, side: DoubleSide, depthWrite: false, polygonOffset: true, polygonOffsetFactor: -1, polygonOffsetUnits: -1 }),
  );
  const lines = new LineSegments(new BufferGeometry(), new LineBasicMaterial({ color: "#1f2933" }));
  const markerMaterial = new MeshBasicMaterial({ color: MARKER_COLOR, depthTest: false });
  const sphere = new SphereGeometry(1, 16, 12);
  const marker = new Mesh(sphere, markerMaterial);
  marker.renderOrder = MARKER_ORDER;
  const cross = new LineSegments(new BufferGeometry(), new LineBasicMaterial({ color: MARKER_COLOR, depthTest: false }));
  cross.renderOrder = MARKER_ORDER;
  // Mesh compare: a coloured copy per compared part, its triangle edges and the edges at the tolerance.
  const compared = new Group();
  const probes = new Group();
  group.add(painted, shapes, lines, marker, cross, compared, probes);
  group.visible = false;
  const surfaces: { mesh: Mesh<BufferGeometry, MeshStandardMaterial>; wire: LineSegments; edges: LineSegments; values: Float32Array; indices: Uint32Array }[] = [];

  let clipPlanes: Plane[] = [];
  const unwatch = parts.watchClipping((planes: Plane[]) => {
    clipPlanes = planes;
    for (const m of [painted.material, shapes.material, lines.material]) m.clippingPlanes = planes;
    for (const s of surfaces) for (const o of [s.mesh, s.wire, s.edges]) (o.material as MeshStandardMaterial).clippingPlanes = planes;
  });

  /** What is drawn now, so a sync with the same analysis, part and view does not rebuild anything. */
  let drawn: { analysis: PartAnalysis; mesh: Part["mesh"] } | null = null;
  let colouredWith: CompareView | null = null;

  function replace(target: Mesh | LineSegments, geometry: BufferGeometry) {
    target.geometry.dispose();
    target.geometry = geometry;
  }

  function clearSurfaces() {
    for (const s of surfaces) {
      s.mesh.geometry.dispose();
      s.mesh.material.dispose();
      for (const o of [s.wire, s.edges]) {
        o.geometry.dispose();
        (o.material as LineBasicMaterial).dispose();
      }
    }
    surfaces.length = 0;
    compared.clear();
  }

  function buildSurfaces(analysis: Extract<PartAnalysis, { kind: "compare" }>) {
    for (const s of analysis.drawing.surfaces ?? []) {
      const geometry = new BufferGeometry();
      geometry.setAttribute("position", new BufferAttribute(s.positions, 3));
      geometry.setAttribute("color", new BufferAttribute(new Float32Array(s.positions.length), 3));
      geometry.setIndex(new BufferAttribute(s.indices, 1));
      geometry.computeBoundingSphere();
      const mesh = new Mesh(geometry, surfaceMaterial());
      mesh.material.flatShading = false;
      geometry.computeVertexNormals();
      const wire = new LineSegments(new WireframeGeometry(geometry), new LineBasicMaterial({ color: "#000000", transparent: true, opacity: 0.45 }));
      const edges = new LineSegments(new BufferGeometry(), new LineBasicMaterial({ color: EDGE_COLOR }));
      for (const o of [mesh, wire, edges]) (o.material as MeshStandardMaterial).clippingPlanes = clipPlanes;
      compared.add(mesh, wire, edges);
      surfaces.push({ mesh, wire, edges, values: s.values, indices: s.indices });
    }
  }

  /** Colours the compared copies and finds the edges at the tolerance, for the panel's settings. */
  function colourSurfaces(analysis: Extract<PartAnalysis, { kind: "compare" }>, view: CompareView) {
    const largest = largestDistance(analysis);
    const h = view.bars ? histogram(analysis.drawing.surfaces ?? []) : undefined;
    for (const s of surfaces) {
      const colors = s.mesh.geometry.getAttribute("color") as BufferAttribute;
      const out = colors.array as Float32Array;
      s.values.forEach((d, i) => {
        const [r, g, b] = compareColor(d, view, largest, h);
        out[i * 3] = r;
        out[i * 3 + 1] = g;
        out[i * 3 + 2] = b;
      });
      colors.needsUpdate = true;
      const material = s.mesh.material;
      material.transparent = view.transparent;
      material.opacity = view.transparent ? 0.45 : 1;
      material.depthWrite = !view.transparent;
      material.needsUpdate = true;
      s.wire.visible = view.triangleEdges;
      s.edges.visible = view.toleranceEdges;
      if (view.toleranceEdges) {
        const positions = s.mesh.geometry.getAttribute("position").array as Float32Array;
        const segments: number[] = [];
        for (let t = 0; t < s.indices.length; t += 3) {
          for (const [a, b] of [
            [s.indices[t], s.indices[t + 1]],
            [s.indices[t + 1], s.indices[t + 2]],
            [s.indices[t + 2], s.indices[t]],
          ]) {
            // Each shared edge once: from its lower vertex.
            if (a < b && band(s.values[a], view.tolerance) !== band(s.values[b], view.tolerance)) {
              segments.push(positions[a * 3], positions[a * 3 + 1], positions[a * 3 + 2], positions[b * 3], positions[b * 3 + 1], positions[b * 3 + 2]);
            }
          }
        }
        replace(s.edges, soup(new Float32Array(segments)));
      }
    }
    colouredWith = view;
  }

  function placeMarkers(points: Vec3[], size: number) {
    probes.clear();
    for (const p of points) {
      const m = new Mesh(sphere, markerMaterial);
      m.position.set(p[0], p[1], p[2]);
      m.scale.setScalar(size);
      m.renderOrder = MARKER_ORDER;
      probes.add(m);
    }
  }

  function rebuild(view: AnalysisView) {
    const { analysis, part } = view;
    clearSurfaces();
    const classes = analysis.drawing.classes;
    const values = analysis.drawing.values;
    const paint =
      classes !== undefined
        ? (t: number) => new Color(ANALYSIS_COLORS[paintClass(analysis, classes[t] ?? PART_CLASS)]).toArray() as Rgb
        : analysis.kind === "wall" && values
          ? (t: number) => wallColor(values[t] ?? NaN, analysis.settings)
          : null;
    replace(painted, paint ? paintedGeometry(part, paint) : new BufferGeometry());
    replace(shapes, analysis.drawing.triangles ? soup(analysis.drawing.triangles) : new BufferGeometry());
    replace(lines, analysis.drawing.lines ? soup(analysis.drawing.lines) : new BufferGeometry());
    // Support blocks are red; the shadow is a dark shape on the platform.
    const red = analysis.kind === "support";
    shapes.material.color.set(red ? ANALYSIS_COLORS[2] : "#1f2933");
    shapes.material.opacity = red ? 0.3 : 0.35;
    let size = 1;
    if (analysis.kind === "gravity") {
      const r = analysis.result;
      size = Math.max(r.size[0], r.size[1], r.size[2], 1);
      const reach = size * 0.6;
      const [x, y, z] = r.centre;
      replace(cross, soup(new Float32Array([x - reach, y, z, x + reach, y, z, x, y - reach, z, x, y + reach, z, x, y, z - reach, x, y, z + reach])));
      marker.position.set(x, y, z);
    } else if (analysis.kind === "wall" && analysis.result.minFrom && analysis.result.minTo) {
      const [f, t] = [analysis.result.minFrom, analysis.result.minTo];
      size = Math.max(analysis.result.minThickness ?? 1, 1) * 4;
      replace(cross, soup(new Float32Array([...f, ...t])));
      marker.position.set(f[0], f[1], f[2]);
    } else {
      replace(cross, new BufferGeometry());
    }
    marker.scale.setScalar(size * 0.025);
    if (analysis.kind === "compare") {
      buildSurfaces(analysis);
      colouredWith = null;
    }
    drawn = { analysis, mesh: part.mesh };
  }

  return {
    group,

    sync(view, compare) {
      if (!view) {
        group.visible = false;
        return;
      }
      if (drawn?.analysis !== view.analysis || drawn.mesh !== view.part.mesh) rebuild(view);
      group.visible = true;
      const { analysis, part, stale } = view;
      painted.visible = painted.geometry.getAttribute("position") !== undefined && part.visible;
      painted.matrix.fromArray(part.matrix);
      painted.matrixWorldNeedsUpdate = true;
      // Shapes, markers and the compared copies sit where the parts were when the analysis ran.
      shapes.visible = lines.visible = !stale;
      marker.visible = cross.visible = (analysis.kind === "gravity" || analysis.kind === "wall") && !stale && cross.geometry.getAttribute("position") !== undefined;
      compared.visible = probes.visible = analysis.kind === "compare" && !stale;
      if (analysis.kind === "compare" && !stale) {
        if (colouredWith !== compare) colourSurfaces(analysis, compare);
        const sphereSize = Math.max(largestDistance(analysis) * 0.5, 0.25);
        placeMarkers((analysis.probes ?? []).map((p) => p.point), sphereSize);
      }
    },

    probe(raycaster) {
      if (!compared.visible) return null;
      for (const hit of raycaster.intersectObjects(
        surfaces.map((s) => s.mesh),
        false,
      )) {
        const s = surfaces.find((x) => x.mesh === hit.object);
        if (!s || !hit.face) continue;
        const pos = s.mesh.geometry.getAttribute("position");
        const { a, b, c } = hit.face;
        // Barycentric weights of the hit in its triangle, to blend the corners' distances.
        const pa = [pos.getX(a), pos.getY(a), pos.getZ(a)];
        const pb = [pos.getX(b), pos.getY(b), pos.getZ(b)];
        const pc = [pos.getX(c), pos.getY(c), pos.getZ(c)];
        const p = [hit.point.x, hit.point.y, hit.point.z];
        const area = (u: number[], v: number[], w: number[]) => {
          const e1 = [v[0] - u[0], v[1] - u[1], v[2] - u[2]];
          const e2 = [w[0] - u[0], w[1] - u[1], w[2] - u[2]];
          return Math.hypot(e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]);
        };
        const total = area(pa, pb, pc) || 1;
        const [wa, wb, wc] = [area(p, pb, pc) / total, area(pa, p, pc) / total, area(pa, pb, p) / total];
        const value = s.values[a] * wa + s.values[b] * wb + s.values[c] * wc;
        return { point: [hit.point.x, hit.point.y, hit.point.z], value };
      }
      return null;
    },

    dispose() {
      unwatch();
      clearSurfaces();
      for (const o of [painted, shapes, lines, cross]) o.geometry.dispose();
      sphere.dispose();
      for (const m of [painted.material, shapes.material, lines.material, markerMaterial, cross.material]) m.dispose();
    },
  };
}
