import {
  Box3,
  BufferAttribute,
  BufferGeometry,
  DoubleSide,
  Group,
  LineBasicMaterial,
  LineSegments,
  Matrix3,
  Matrix4,
  Mesh as ThreeMesh,
  MeshBasicMaterial,
  Object3D,
  OrthographicCamera,
  Raycaster,
  Scene,
  Sphere,
  Vector2,
  Vector3,
} from "three";
import { useBoolean, type BooleanState } from "../boolean/store";
import { useCut } from "../repair/cut/store";
import { boundsOf, centreOf, partBounds, unionBounds, type Bounds } from "../mesh";
import { IS_MAC } from "../platform";
import { selectNone, selectTriangles } from "../repair/session";
import { useRepair, type RepairState } from "../repair/store";
import { PART_COLORS, useStore, type AppState, type Part, type Projection, type SelectMode } from "../store";
import { useUi } from "../ui";
import type { ViewportApi, ZoomTarget } from "./api";
import { buildAxes } from "./axes";
import { anyTriangleInRect, projectVertices, trianglesInRect, type ScreenRect } from "./boxSelect";
import { createBrackets } from "./brackets";
import { createCameraMotion } from "./cameraMotion";
import { applyView, createCameras, fitView, MIN_HALF_HEIGHT, PRESETS, rayFromCamera, readView, setAspect, zoomLimits, type ViewCamera, type ViewState } from "./cameras";
import { createClipLayer } from "./clipping";
import { attachOptionDragPan, attachWheelNavigation, createControls } from "./controls";
import { attachEditTools } from "./editTools";
import { createGizmo } from "./gizmo";
import { createPartsLayer, PLAIN_LOOK, type PartsLook } from "./parts";
import { attachPicking, type PickHit, type TrianglePick } from "./picking";
import { createPlanePreview } from "./planePreview";
import { buildPlatform } from "./platform";
import { createRepairLayer } from "./repairLayer";
import { createRulers, RULER_THICKNESS } from "./rulers";
import { addLights, createRenderer, fitClipPlanes, watchPixelRatio } from "./scene";
import { readTheme, watchColorScheme } from "./theme";
import { createToolLayer } from "./toolLayer";
import { pickTriangle } from "./triPick";
import { createViewCube } from "./viewCube";
import { createMeasureLayer, type MeasureItem } from "./measureLayer";
import { worldPerPixel } from "./gizmoMath";
import { valueText } from "../measure/format";
import type { MeasureHit } from "../measure/session";
import { useMeasure } from "../measure/store";
import { labelWidth, shapeOf } from "../label/session";
import { useLabel } from "../label/store";
import { createLabelLayer } from "./labelLayer";
import { createAnalysisLayer, type AnalysisView } from "./analysisLayer";
import { isOutOfDate, shownAnalysis, useAnalysis } from "../analysis/session";
import { ANALYSIS_COLORS, PART_CLASS } from "../analysis/colors";
import { useLibrary } from "../library/store";
import { useCollisions } from "../analysis/collisions";

export interface ViewportEngine {
  api: ViewportApi;
  dispose(): void;
}

/**
 * The three.js viewport inside `host`. Renders on demand: controls, store changes, theme changes
 * and camera animations request a frame, and a resize draws at once; nothing runs while idle.
 * In repair mode (a session in the repair store) the repaired part is hidden and the repair layer
 * draws its working copy instead, the other parts are dimmed, the gizmo and the selection brackets
 * stay hidden, and clicks select triangles; every repair store change that alters the drawing
 * requests a frame. In Boolean operations the parts in the green and red lists are drawn in those colours (the one
 * selected in the lists lighter) and the others dimmed, a computed result is drawn in place of its inputs, and the
 * gizmo stays hidden. A left drag from empty space selects with a box (src/viewport/picking.ts): visible, unlocked
 * parts at least partly inside it, or in repair mode the working copy's triangles facing the camera.
 * dispose() releases everything it created, so React StrictMode can mount it twice.
 */
export function createViewportEngine(host: HTMLElement, cubeOverlay: HTMLElement): ViewportEngine {
  // Z is up; set before any camera, light or control copies the default.
  Object3D.DEFAULT_UP.set(0, 0, 1);

  const initial = useStore.getState();
  let theme = readTheme();
  const renderer = createRenderer(host);
  const canvas = renderer.domElement;
  const scene = new Scene();
  const lights = addLights(scene);
  const cameras = createCameras();
  let camera: ViewCamera = cameraFor(initial.view.projection);
  const controls = createControls(camera, canvas);

  // Declared before the layers below: they may call requestRender while they are being created.
  const sceneBox = new Box3();
  const partsBox = new Box3();
  const sceneSphere = new Sphere();
  /** sceneSphere widened to the gizmo and the cut plane preview, for the clip planes. */
  const clipSphere = new Sphere();
  const raycaster = new Raycaster();
  const pointer = new Vector2();
  /** Scratch for the clip test of a working-copy hit, and for the working copy's box in the scene bounds. */
  const hitPoint = new Vector3();
  /** The box selection's rectangle, over the canvas. */
  const boxOutline = document.createElement("div");
  boxOutline.className = "viewport-box";
  boxOutline.hidden = true;
  host.appendChild(boxOutline);
  const workingBox = new Box3();
  /** Exact world box of the visible selected parts (in repair mode, of the working copy), for the brackets and rulers. */
  let selectionBox: Bounds | null = null;
  /** World point whose depth sets the ruler scale: the selection's centre, else the scene's. */
  const rulerReference = new Vector3();
  let width = 0;
  let height = 0;
  let framed = false;
  let frame = 0;

  const parts = createPartsLayer(theme);
  const repair = createRepairLayer(theme, parts);
  const tools = createToolLayer(theme, parts);
  let platform = buildPlatform(initial.platform, theme);
  let axes = buildAxes(initial.platform, theme);
  const brackets = createBrackets(theme);
  const planePreview = createPlanePreview(theme, requestRender);
  const measure = createMeasureLayer(theme, host);
  const labels = createLabelLayer(theme, host);
  const analysisLayer = createAnalysisLayer(parts);
  // Collision detection: where parts cross (red, drawn over them) and the closest points of parts too close.
  const collisionGroup = new Group();
  const crossing = new ThreeMesh(new BufferGeometry(), new MeshBasicMaterial({ color: "#d64541", side: DoubleSide, transparent: true, opacity: 0.85, depthTest: false }));
  crossing.renderOrder = 25;
  const closest = new LineSegments(new BufferGeometry(), new LineBasicMaterial({ color: "#e08a1e", depthTest: false }));
  closest.renderOrder = 25;
  collisionGroup.add(crossing, closest);
  collisionGroup.visible = false;
  function syncCollisions() {
    const c = useCollisions.getState();
    const drawing = c.result?.drawing;
    collisionGroup.visible = c.open && c.settings.display && !!drawing && !c.outdated;
    if (!collisionGroup.visible || !drawing) return;
    for (const [target, data] of [
      [crossing, drawing.triangles],
      [closest, drawing.lines],
    ] as const) {
      target.geometry.dispose();
      const geometry = new BufferGeometry();
      if (data) geometry.setAttribute("position", new BufferAttribute(data, 3));
      target.geometry = geometry;
    }
  }
  scene.add(parts.group, repair.group, tools.group, platform.group, axes.group, brackets.group, planePreview.group, measure.group, labels.group, analysisLayer.group, collisionGroup);
  renderer.localClippingEnabled = true;
  const clipLayer = createClipLayer(parts, requestRender);
  const gizmo = createGizmo({ scene, camera, canvas, host, orbit: controls, requestRender });
  const rulers = createRulers(host, theme);

  const motion = createCameraMotion(applyNow);
  const cube = createViewCube(cubeOverlay, theme, () => camera, (preset) => api.setViewPreset(preset), requestRender);

  function cameraFor(projection: Projection): ViewCamera {
    return projection === "orthographic" ? cameras.orthographic : cameras.perspective;
  }

  function requestRender() {
    if (!frame) frame = requestAnimationFrame(renderFrame);
  }

  function renderFrame(now: number) {
    frame = 0;
    if (motion.step(now)) requestRender();
    if (!width || !height) return;
    lights.follow(camera);
    const orbitDistance = camera.position.distanceTo(controls.target);
    fitClipPlanes(camera, sceneSphere, orbitDistance);
    // After the clip planes, since the perspective near plane bounds the gizmo's scale. The gizmo keeps its
    // size on screen, so zoomed far out it reaches well past the scene; the clip planes then widen to take it in.
    // The cut plane preview is a little larger than the box it covers, so its corners can reach past the scene too.
    const gizmoReach = gizmo.beforeRender(width, height);
    repair.beforeRender(camera, height * renderer.getPixelRatio());
    tools.beforeRender(camera, height);
    measure.beforeRender(camera, width, height);
    labels.beforeRender(camera, width, height);
    const previewReach = planePreview.reach();
    if (gizmoReach || previewReach) {
      clipSphere.copy(sceneSphere);
      if (gizmoReach) clipSphere.union(gizmoReach);
      if (previewReach) clipSphere.union(previewReach);
      fitClipPlanes(camera, clipSphere, orbitDistance);
    }
    renderer.setViewport(0, 0, width, height);
    renderer.clear();
    renderer.render(scene, camera);
    cube.render(renderer, camera, width, height);
    // The orbit target stands in when the camera has moved past the reference point.
    if (useStore.getState().view.showRulers) rulers.draw(camera, width, height, rulerReference, selectionBox, controls.target);
  }

  function applyNow(view: ViewState) {
    applyView(camera, view, controls.target);
    controls.update();
  }

  function animateTo(view: ViewState) {
    motion.start(readView(camera, controls.target), view);
    requestRender();
  }

  /** Where a new camera command starts from: the end of a running move, so quick commands chain. */
  function baseView(): ViewState {
    return motion.destination() ?? readView(camera, controls.target);
  }

  const aspect = () => (width && height ? width / height : 1);

  /** Share of the view a zoom-to-fit may use: the rulers cover a strip on the left and bottom. */
  function fitFill(): [number, number] {
    if (!useStore.getState().view.showRulers || !width || !height) return [1, 1];
    const clear = 2 * RULER_THICKNESS;
    return [Math.max(0.5, 1 - clear / width), Math.max(0.5, 1 - clear / height)];
  }
  const isOrthographic = () => camera instanceof OrthographicCamera;

  function resize() {
    width = host.clientWidth;
    height = host.clientHeight;
    if (!width || !height) return;
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.setSize(width, height, false);
    setAspect(cameras, aspect());
    if (!framed) {
      framed = true;
      applyNow(fitView(platformBounds(useStore.getState()), PRESETS.iso, aspect(), isOrthographic(), fitFill()));
    }
    // Resizing clears the canvas; drawing now rather than next frame keeps it from flashing blank.
    cancelAnimationFrame(frame);
    renderFrame(performance.now());
  }

  /** Keeps target, direction and apparent size: ortho half-height = distance · tan(fov / 2). */
  function switchProjection(projection: Projection) {
    const next = cameraFor(projection);
    if (next === camera) return;
    const view = readView(camera, controls.target);
    camera = next;
    controls.object = next;
    gizmo.setCamera(next);
    applyNow(view);
  }

  function rebuildPlatform(s: AppState) {
    scene.remove(platform.group, axes.group);
    platform.dispose();
    axes.dispose();
    platform = buildPlatform(s.platform, theme);
    axes = buildAxes(s.platform, theme);
    scene.add(platform.group, axes.group);
  }

  function applyToggles(s: AppState) {
    platform.group.visible = s.view.showPlatform;
    axes.group.visible = s.view.showAxes;
    rulers.setVisible(s.view.showRulers);
  }

  /** The part in repair mode, or null. */
  const repairingId = () => useRepair.getState().session?.partId ?? null;
  const booleanOpen = () => useBoolean.getState().session !== null;
  const measureOpen = () => useMeasure.getState().session !== null;

  /** The analysis on show, while no other mode is open (src/analysis/session.ts). */
  function analysisView(): AnalysisView | null {
    if (repairingId() || booleanOpen() || measureOpen() || useLabel.getState().session !== null || useLibrary.getState().session !== null) return null;
    const found = shownAnalysis();
    return found ? { ...found, stale: isOutOfDate(found.part, found.analysis) } : null;
  }

  /** How the mode draws the parts: see PartsLook. */
  function look(): PartsLook {
    const id = repairingId();
    if (id) return { hidden: id, colors: new Map(), dimOthers: true, pickable: new Set() };
    const session = useBoolean.getState().session;
    if (!session) {
      // An analysis that paints the part draws it itself; the others show it grey, except the standard analysis.
      const view = analysisView();
      if (!view || view.analysis.kind === "standard") return PLAIN_LOOK;
      if (view.analysis.kind === "compare") {
        // The compared copies stand in for their parts while the parts are where they were compared.
        if (view.stale) return PLAIN_LOOK;
        const roles = new Set((view.analysis.drawing.surfaces ?? []).map((s) => s.role));
        const hide = new Set<string>();
        if (roles.has("reference")) hide.add(view.part.id);
        if (roles.has("comparison")) hide.add(view.analysis.settings.comparisonId);
        return { hidden: null, hiddenMore: hide, colors: new Map(), dimOthers: false, pickable: null };
      }
      if (view.analysis.drawing.classes || view.analysis.drawing.values) return { hidden: view.part.id, colors: new Map(), dimOthers: false, pickable: null };
      return { hidden: null, colors: new Map([[view.part.id, ANALYSIS_COLORS[PART_CLASS]]]), dimOthers: false, pickable: null };
    }
    const pickable = new Set([...session.main, ...session.tool]);
    if (session.result) return { hidden: null, colors: new Map([[session.result.part.id, session.result.part.color]]), dimOthers: true, pickable };
    const colors = new Map<string, string>();
    for (const id of session.main) colors.set(id, id === session.selected ? theme.booleanMainSelected : theme.booleanMain);
    for (const id of session.tool) colors.set(id, id === session.selected ? theme.booleanToolSelected : theme.booleanTool);
    return { hidden: null, colors, dimOthers: true, pickable };
  }

  function syncParts(s: AppState) {
    const result = useBoolean.getState().session?.result;
    // In Boolean operations the lists, not the selection, decide the colours; in the analysis view the analysis does.
    const plain = booleanOpen() || analysisView() !== null;
    parts.sync(result ? [...s.parts, result.part] : s.parts, plain ? [] : s.selection, s.view.shading, look());
    analysisLayer.sync(analysisView(), useAnalysis.getState().compare);
  }

  /**
   * In repair mode the gizmo stays hidden: the store's selection is the repaired part, which only the repair changes.
   * In Boolean operations too: the parts stay where they are while the mode works on them. While an analysis is on
   * show as well: its results are for the part where it stood.
   */
  function syncGizmo(s: AppState) {
    const hidden = repairingId() || booleanOpen() || measureOpen() || useLabel.getState().session !== null || analysisView() !== null;
    gizmo.sync(hidden ? { ...s, selection: [] } : s);
  }

  /** Measure mode: the measurements of the shown parts with their results and labels, and the clicks in progress. */
  /** Label generation: the placed labels with their shapes and widths, and which is selected. */
  function syncLabels() {
    const l = useLabel.getState();
    if (!l.session) {
      labels.sync(null);
      return;
    }
    const selected = l.session.selected;
    labels.sync(
      l.session.labels.map((label) => {
        const shape = shapeOf(label);
        return { label, shape: shape?.status === "ready" ? shape.reply : null, width: labelWidth(label) ?? label.height, selected: label.id === selected };
      }),
    );
  }

  function syncMeasure() {
    const m = useMeasure.getState();
    if (!m.session) {
      measure.sync(null);
      return;
    }
    const items: MeasureItem[] = [];
    for (const part of useStore.getState().parts) {
      if (!part.visible) continue;
      for (const measurement of part.measurements ?? []) {
        const entry = m.results[measurement.id];
        const result = entry?.result ?? null;
        items.push({ id: measurement.id, result, text: result ? valueText(measurement, result) : "", error: entry?.error ?? null, selected: m.session.selected === measurement.id });
      }
    }
    measure.sync({ items, pending: [...m.session.picks, ...m.session.points], options: m.options });
  }

  /** The working copy in the repaired part's colour and the view's shading. True when anything drawn changed. */
  function syncRepair(): boolean {
    const r = useRepair.getState();
    const s = useStore.getState();
    const part = r.session ? s.parts.find((p) => p.id === r.session?.partId) : undefined;
    return repair.sync(r, part?.color ?? PART_COLORS[0], s.view.shading);
  }

  /**
   * Recomputes the selection box (exact, cached per part) and the ruler reference point. In repair mode the
   * rulers measure the working copy, and the brackets, which mark selected parts, are hidden.
   */
  function updateSelection(s: AppState) {
    const session = useBoolean.getState().session;
    if (repairingId()) {
      selectionBox = repair.bounds();
      brackets.update(null);
    } else if (session) {
      // The brackets mark the part selected in the lists, or the result shown.
      const marked = session.result ? [session.result.part] : s.parts.filter((p) => p.id === session.selected && p.visible);
      selectionBox = unionBounds(marked.map(partBounds));
      brackets.update(selectionBox);
    } else {
      const selected = new Set(s.selection);
      selectionBox = unionBounds(s.parts.filter((p) => p.visible && selected.has(p.id)).map(partBounds));
      brackets.update(selectionBox);
    }
    if (selectionBox) {
      rulerReference.fromArray(centreOf(selectionBox));
    } else if (!parts.visibleBox(partsBox).isEmpty()) {
      partsBox.getCenter(rulerReference);
    } else {
      rulerReference.set(s.platform[0] / 2, s.platform[1] / 2, s.platform[2] / 2);
    }
  }

  /** Bounds used for clip planes and zoom limits; rulers and labels sit just outside the platform. */
  function updateSceneBounds(s: AppState) {
    const [x, y, z] = s.platform;
    sceneBox.min.set(0, 0, 0);
    sceneBox.max.set(x, y, z);
    sceneBox.expandByScalar(Math.max(x, y, z) * 0.12);
    sceneBox.union(parts.visibleBox(partsBox));
    // A Boolean result is drawn as a part, so visibleBox already takes it in.
    const working = repair.bounds();
    if (working) {
      workingBox.min.fromArray(working.min);
      workingBox.max.fromArray(working.max);
      sceneBox.union(workingBox);
    }
    sceneBox.getBoundingSphere(sceneSphere);
    Object.assign(controls, zoomLimits(sceneSphere.radius));
  }

  function onStoreChange(s: AppState, prev: AppState) {
    let dirty = false;
    if (s.platform !== prev.platform) {
      rebuildPlatform(s);
      applyToggles(s);
      dirty = true;
    }
    if (s.parts !== prev.parts || s.selection !== prev.selection || s.view.shading !== prev.view.shading) {
      syncParts(s);
      dirty = true;
    }
    // The working copy takes the repaired part's colour and the shading.
    if ((s.parts !== prev.parts || s.view.shading !== prev.view.shading) && syncRepair()) dirty = true;
    if (s.parts !== prev.parts || s.platform !== prev.platform) updateSceneBounds(s);
    if (s.parts !== prev.parts || s.selection !== prev.selection || s.platform !== prev.platform) updateSelection(s);
    const gizmoChanged = s.gizmoMode !== prev.gizmoMode || s.view.alwaysShowGizmo !== prev.view.alwaysShowGizmo;
    if (s.parts !== prev.parts || s.selection !== prev.selection || gizmoChanged) {
      syncGizmo(s);
      dirty = true;
    }
    if (s.view !== prev.view) {
      applyToggles(s);
      switchProjection(s.view.projection);
      dirty = true;
    }
    if (s.parts !== prev.parts && measureOpen()) {
      syncMeasure();
      dirty = true;
    }
    if (dirty) requestRender();
  }

  /** Cut surfaces' points and line, while the cutting mode is on. */
  function syncTools() {
    const r = useRepair.getState();
    tools.syncCut(r.session && r.editTool === "cut" ? useCut.getState() : null);
  }

  function onRepairChange(r: RepairState, prev: RepairState) {
    const s = useStore.getState();
    let dirty = false;
    if (r.editTool !== prev.editTool || (r.session === null) !== (prev.session === null)) {
      syncTools();
      dirty = true;
    }
    if (r.session?.partId !== prev.session?.partId) {
      // Entering or leaving repair mode: the part hides or shows, the others dim or come back, and so does the gizmo.
      syncParts(s);
      syncGizmo(s);
      dirty = true;
    }
    if (syncRepair()) dirty = true;
    if (!dirty) return;
    // The working copy's box joins the scene bounds and the rulers.
    updateSceneBounds(s);
    updateSelection(s);
    requestRender();
  }

  /** Lists, selection or result changed: colours, the drawn result, the brackets, and the gizmo on entering or leaving. */
  function onBooleanChange(b: BooleanState, prev: BooleanState) {
    if (b.session === prev.session) return;
    const s = useStore.getState();
    syncParts(s);
    if ((b.session === null) !== (prev.session === null)) syncGizmo(s);
    updateSceneBounds(s);
    updateSelection(s);
    requestRender();
  }

  /**
   * Zoom to area: the rectangle (canvas pixels) fills the view, keeping the view direction. Its centre goes to the
   * nearest part under it, or to the orbit target's depth when it covers no part.
   */
  function zoomToRect(rect: ScreenRect) {
    const r = canvas.getBoundingClientRect();
    if (!r.width || !r.height || !aim(r.left + (rect.left + rect.right) / 2, r.top + (rect.top + rect.bottom) / 2)) return;
    const view = baseView();
    const forward = camera.getWorldDirection(new Vector3());
    const targetDepth = hitPoint.subVectors(controls.target, camera.position).dot(forward);
    const hit = raycaster.intersectObjects(parts.pickTargets(), false).find((h) => !clipLayer.isClipped(h.point));
    const ray = raycaster.ray;
    const point = hit ? hit.point : ray.at((targetDepth - hitPoint.subVectors(ray.origin, camera.position).dot(forward)) / ray.direction.dot(forward), new Vector3());
    // What the view shows at the point's depth, as a share of what it shows at the target's (perspective only).
    const depthScale = isOrthographic() ? 1 : hitPoint.subVectors(point, camera.position).dot(forward) / targetDepth;
    const share = Math.max((rect.right - rect.left) / r.width, (rect.bottom - rect.top) / r.height);
    animateTo({ ...view, target: point.clone(), halfHeight: Math.max(MIN_HALF_HEIGHT, view.halfHeight * depthScale * share) });
  }

  /** Points the raycaster through a client position; false when the canvas has no size. */
  function aim(clientX: number, clientY: number): boolean {
    const rect = canvas.getBoundingClientRect();
    if (!rect.width || !rect.height) return false;
    pointer.set(((clientX - rect.left) / rect.width) * 2 - 1, -((clientY - rect.top) / rect.height) * 2 + 1);
    scene.updateMatrixWorld();
    camera.updateMatrixWorld();
    rayFromCamera(raycaster, pointer, camera);
    return true;
  }

  /** Measure mode: the part under the pointer, the triangle and world point hit, and about 8 px there in mm. */
  function pickForMeasure(clientX: number, clientY: number): MeasureHit | null {
    if (!aim(clientX, clientY)) return null;
    const hit = raycaster.intersectObjects(parts.pickTargets(), false).find((h) => !clipLayer.isClipped(h.point));
    const partId: unknown = hit?.object.userData.partId;
    if (!hit || typeof partId !== "string" || hit.faceIndex === undefined || hit.faceIndex === null) return null;
    const { x, y, z } = hit.point;
    return { partId, triangle: hit.faceIndex, point: [x, y, z], tolerance: 8 * worldPerPixel(camera, hit.point, height) };
  }

  function pickPart(clientX: number, clientY: number): PickHit | null {
    if (!aim(clientX, clientY)) return null;
    // Geometry hidden by a clip plane cannot be clicked; the first hit that is drawn counts.
    const hit = raycaster.intersectObjects(parts.pickTargets(), false).find((h) => !clipLayer.isClipped(h.point));
    if (!hit) return null;
    const partId: unknown = hit.object.userData.partId;
    return typeof partId === "string" ? { partId, point: hit.point.clone() } : null;
  }

  /**
   * Repair mode: the working copy's nearest triangle under the pointer that the clip planes leave in place. With
   * Transparent cuts the removed region is a ghost, which shows no selection, so it is skipped too (isCut).
   */
  function pickWorkingCopy(clientX: number, clientY: number): TrianglePick | null {
    const session = useRepair.getState().session;
    if (!session || !aim(clientX, clientY)) return null;
    const { origin: o, direction: d } = raycaster.ray;
    // The working copy is in world space, so the world ray needs no transform.
    const hit = pickTriangle(session.mesh, [o.x, o.y, o.z], [d.x, d.y, d.z], (p) => !clipLayer.isCut(hitPoint.fromArray(p)), session.hiddenTriangles);
    return hit && { triangle: hit.triangle, point: hit.point };
  }

  /** Shows the box selection's rectangle (canvas pixels; the canvas fills the host), or hides it. */
  function drawBox(rect: ScreenRect | null) {
    boxOutline.hidden = !rect;
    if (!rect) return;
    boxOutline.style.left = `${rect.left}px`;
    boxOutline.style.top = `${rect.top}px`;
    boxOutline.style.width = `${rect.right - rect.left}px`;
    boxOutline.style.height = `${rect.bottom - rect.top}px`;
  }

  /**
   * Selects what the box covers. Parts: visible and unlocked ones with a triangle at least partly inside it that the
   * clip planes do not hide. Repair mode: the working copy's triangles facing the camera at least partly inside it,
   * except where the clip planes cut them away. Nothing covered clears the selection, as a click on empty space does,
   * unless a modifier is held.
   */
  function selectBox(rect: ScreenRect, mode: SelectMode) {
    // Boolean operations only draws a rectangle for Zoom to area (src/viewport/picking.ts); Zoom to › Area too.
    if (useUi.getState().viewTool === "zoom-area") {
      useUi.getState().setViewTool(null);
      zoomToRect(rect);
      return;
    }
    if (booleanOpen()) {
      zoomToRect(rect);
      return;
    }
    const { width: w, height: h } = canvas.getBoundingClientRect();
    if (!w || !h) return;
    camera.updateMatrixWorld();
    const view = new Matrix4().multiplyMatrices(camera.projectionMatrix, camera.matrixWorldInverse);
    const session = useRepair.getState().session;
    if (session) {
      const { positions, indices } = session.mesh;
      const screen = projectVertices(positions, view.elements, w, h);
      const hidden = session.hiddenTriangles;
      // The working copy is in world space.
      const shown = (t: number) =>
        !(hidden && hidden[t]) && [0, 1, 2].some((k) => !clipLayer.isCut(hitPoint.fromArray(positions, 3 * indices[3 * t + k])));
      const ids = trianglesInRect(indices, screen, rect, true, shown);
      if (ids.length) selectTriangles(ids, mode);
      else if (mode === "replace") selectNone();
      return;
    }
    const store = useStore.getState();
    const model = new Matrix4();
    const ids = store.parts
      .filter((part) => part.visible && !part.locked)
      .filter((part) => {
        model.fromArray(part.matrix);
        const { positions, indices } = part.mesh;
        const screen = projectVertices(positions, view.clone().multiply(model).elements, w, h);
        const corner = (t: number, k: number) => hitPoint.fromArray(positions, 3 * indices[3 * t + k]).applyMatrix4(model);
        return anyTriangleInRect(indices, screen, rect, (t) => [0, 1, 2].some((k) => !clipLayer.isClipped(corner(t, k))));
      })
      .map((part) => part.id);
    if (ids.length) store.select(ids, mode);
    else if (mode === "replace") store.clearSelection();
  }

  function onThemeChange() {
    theme = readTheme();
    platform.applyTheme(theme);
    axes.applyTheme(theme);
    parts.applyTheme(theme);
    // The list colours come from the theme too.
    syncParts(useStore.getState());
    repair.applyTheme(theme);
    tools.applyTheme(theme);
    syncTools();
    brackets.applyTheme(theme);
    planePreview.applyTheme(theme);
    measure.applyTheme(theme);
    syncMeasure();
    labels.applyTheme(theme);
    syncLabels();
    rulers.applyTheme(theme);
    cube.applyTheme(theme);
    requestRender();
  }

  const api: ViewportApi = {
    zoomTo(target: ZoomTarget) {
      animateTo(fitView(zoomBounds(useStore.getState(), target), baseView(), aspect(), isOrthographic(), fitFill()));
    },
    zoomToBounds(bounds) {
      animateTo(fitView(bounds, baseView(), aspect(), isOrthographic(), fitFill()));
    },
    setViewPreset(preset) {
      animateTo({ ...baseView(), ...PRESETS[preset] });
    },
    centreOn(point) {
      animateTo({ ...baseView(), target: new Vector3(point[0], point[1], point[2]) });
    },
    viewDirection() {
      const d = camera.getWorldDirection(new Vector3());
      return [d.x, d.y, d.z];
    },
    probeAt(clientX, clientY) {
      if (!aim(clientX, clientY)) return null;
      return analysisLayer.probe(raycaster);
    },
    surfaceAt(clientX, clientY) {
      if (!aim(clientX, clientY)) return null;
      const hit = raycaster.intersectObjects(parts.pickTargets(), false).find((h) => !clipLayer.isClipped(h.point));
      const partId: unknown = hit?.object.userData.partId;
      if (!hit || typeof partId !== "string" || !hit.face) return null;
      // The face normal in world space: through the normal matrix, so a scaled part's normals stay square to it.
      const n = hit.face.normal.clone().applyMatrix3(new Matrix3().getNormalMatrix(hit.object.matrixWorld)).normalize();
      return { partId, point: [hit.point.x, hit.point.y, hit.point.z], normal: [n.x, n.y, n.z] };
    },
    rayAt(clientX, clientY) {
      if (!aim(clientX, clientY)) return null;
      const { origin: o, direction: d } = raycaster.ray;
      return { origin: [o.x, o.y, o.z], direction: [d.x, d.y, d.z] };
    },
  };

  // Initial state (a remount can find repair mode open), then listeners.
  syncParts(initial);
  syncRepair();
  syncTools();
  applyToggles(initial);
  updateSceneBounds(initial);
  updateSelection(initial);
  syncGizmo(initial);
  syncMeasure();
  syncLabels();
  controls.target.set(initial.platform[0] / 2, initial.platform[1] / 2, initial.platform[2] / 2);

  controls.addEventListener("change", requestRender);
  controls.addEventListener("start", motion.cancel);
  const unsubscribeStore = useStore.subscribe(onStoreChange);
  const unsubscribeRepair = useRepair.subscribe(onRepairChange);
  const unsubscribeBoolean = useBoolean.subscribe(onBooleanChange);
  const unsubscribeMeasure = useMeasure.subscribe((m, prev) => {
    if ((m.session === null) !== (prev.session === null)) {
      syncGizmo(useStore.getState());
      syncParts(useStore.getState());
    }
    syncMeasure();
    requestRender();
  });
  const unsubscribeLabel = useLabel.subscribe((l, prev) => {
    if ((l.session === null) !== (prev.session === null)) {
      syncGizmo(useStore.getState());
      syncParts(useStore.getState());
    }
    syncLabels();
    requestRender();
  });
  const unsubscribeCollisions = useCollisions.subscribe((c, prev) => {
    if (c.result === prev.result && c.open === prev.open && c.settings === prev.settings && c.outdated === prev.outdated) return;
    syncCollisions();
    requestRender();
  });
  const unsubscribeAnalysis = useAnalysis.subscribe((a, prev) => {
    if (a.shown === prev.shown && a.compare === prev.compare) return;
    if (a.shown !== prev.shown) syncGizmo(useStore.getState());
    syncParts(useStore.getState());
    requestRender();
  });
  const unwatchTheme = watchColorScheme(onThemeChange);
  const unsubscribeCut = useCut.subscribe(() => {
    syncTools();
    requestRender();
  });
  // Before picking: a press Move nodes takes is claimed before picking sees it.
  const editTools = attachEditTools({
    canvas,
    host,
    pick: pickWorkingCopy,
    ray: (x, y) => (aim(x, y) ? raycaster.ray : null),
    viewDirection: () => camera.getWorldDirection(new Vector3()),
    preview: (vertex, at) => {
      repair.previewVertex(vertex, at);
      requestRender();
    },
    marker: (at) => tools.setMarker(at),
    requestRender,
  });
  const detachPicking = attachPicking(
    canvas,
    pickPart,
    pickWorkingCopy,
    () => gizmo.isActive() || editTools.isActive(),
    {
      draw: drawBox,
      select: selectBox,
    },
    pickForMeasure,
  );
  const detachWheel = attachWheelNavigation(host, controls, motion.cancel, IS_MAC);
  const detachOptionPan = IS_MAC ? attachOptionDragPan(host, controls, () => gizmo.isActive()) : () => {};
  const unwatchPixelRatio = watchPixelRatio(resize);
  const resizeObserver = new ResizeObserver(resize);
  resizeObserver.observe(host);
  resize();

  return {
    api,
    dispose() {
      cancelAnimationFrame(frame);
      frame = 0;
      motion.cancel();
      resizeObserver.disconnect();
      unwatchPixelRatio();
      detachWheel();
      detachOptionPan();
      detachPicking();
      editTools.dispose();
      unsubscribeCut();
      boxOutline.remove();
      unwatchTheme();
      unsubscribeStore();
      unsubscribeRepair();
      unsubscribeBoolean();
      unsubscribeMeasure();
      measure.dispose();
      unsubscribeLabel();
      labels.dispose();
      unsubscribeAnalysis();
      analysisLayer.dispose();
      unsubscribeCollisions();
      crossing.geometry.dispose();
      crossing.material.dispose();
      closest.geometry.dispose();
      closest.material.dispose();
      controls.removeEventListener("change", requestRender);
      controls.removeEventListener("start", motion.cancel);
      gizmo.dispose();
      controls.dispose();
      cube.dispose();
      rulers.dispose();
      clipLayer.dispose();
      repair.dispose();
      tools.dispose();
      planePreview.dispose();
      brackets.dispose();
      parts.dispose();
      platform.dispose();
      axes.dispose();
      lights.dispose();
      // Frees the GL context now; otherwise StrictMode and HMR remounts pile up contexts until GC.
      renderer.forceContextLoss();
      renderer.dispose();
      canvas.remove();
    },
  };
}

function platformBounds(s: AppState): Bounds {
  const [x, y, z] = s.platform;
  return { min: [0, 0, 0], max: [x, y, z] };
}

function partsBounds(list: Part[]): Bounds | null {
  return unionBounds(list.map((p) => boundsOf(p.mesh, p.matrix)));
}

/** Box for a zoom target. Hidden parts never count; empty choices fall back to all parts, then the platform. */
function zoomBounds(s: AppState, target: ZoomTarget): Bounds {
  const platform = platformBounds(s);
  const visible = s.parts.filter((p) => p.visible);
  switch (target) {
    case "platform":
      return platform;
    case "everything":
      return unionBounds([platform, partsBounds(visible)]) ?? platform;
    case "parts":
      return partsBounds(visible) ?? platform;
    case "selected": {
      const selected = new Set(s.selection);
      return partsBounds(visible.filter((p) => selected.has(p.id))) ?? partsBounds(visible) ?? platform;
    }
  }
}
