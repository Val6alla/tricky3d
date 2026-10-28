import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Rust is not needed: no click is made, and an open mode with no measurements asks it nothing.
const ipc = vi.hoisted(() => ({ measurePick: vi.fn(), measureFit: vi.fn(), measureEvaluate: vi.fn(), forgetMeasureMeshes: vi.fn() }));
vi.mock("./ipc", () => ipc);

import { activate, featureById, type Feature } from "../features";
import { submenuEntries } from "../menus/entries";
import { MenuItem } from "../menus/Menu";
import { useStore } from "../store";
import { useUi } from "../ui";
import { toolIcon } from "./icons";
import { GROUP_ORDER, ToolsView } from "./MeasurePanel";
import { closeMeasure, openMeasure, pickMeasureGroup, pickMeasureTool } from "./session";
import { DEFAULT_OPTIONS, LAST_TOOL_KEY, useMeasure } from "./store";
import { GROUP_LABELS, TOOLS, chosenText, describe as describeTool, toolById, type MeasureTool } from "./tools";

const m = () => useMeasure.getState();
const tool = (id: string) => toolById(id) as MeasureTool;
const feature = (id: string) => featureById(id) as Feature;

/** The Tools tab as it shows the measure store's state now. */
const toolsTab = () => renderToStaticMarkup(<ToolsView tool={toolById(m().session?.tool ?? "")} lastTools={m().lastTools} />);

/** The group buttons in order: the main button's attributes and icon, and whether an arrow follows it. */
function groupButtons(html: string) {
  return [...html.matchAll(/<div class="ms-group( is-active)?"[^>]*><button type="button" class="ms-group-main" aria-label="([^"]*)" aria-pressed="(true|false)"[^>]*><svg[^>]*class="lucide lucide-([a-z0-9-]+)[^"]*"[^>]*>.*?<\/button>(<button[^>]*class="ms-group-arrow"[^>]*>)?/g)].map((b) => ({
    active: b[1] !== undefined,
    label: b[2],
    pressed: b[3] === "true",
    icon: b[4],
    arrow: b[5] ?? null,
  }));
}

/** The lucide class name of a tool's icon, as it renders: "circle-dot". */
function iconClass(t: MeasureTool) {
  const Icon = toolIcon(t);
  return /class="lucide lucide-([a-z0-9-]+)/.exec(renderToStaticMarkup(<Icon />))?.[1];
}

const textOf = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();

/** The Chosen tools box as shown: its accessible name, its text, and the icon of each thing measured between. */
function chosenBox(html: string) {
  const box = html.slice(html.indexOf("Chosen tools"), html.indexOf("Description"));
  return {
    name: /aria-label="([^"]*)"/.exec(box)?.[1],
    text: textOf(box.replace("Chosen tools", "")),
    icons: [...box.matchAll(/lucide lucide-([a-z0-9-]+)/g)].map((i) => i[1]),
  };
}

/** The Description box's lines, as shown. */
const descriptionLines = (html: string) => [...html.slice(html.indexOf("Description")).matchAll(/<p>([^<]*)<\/p>/g)].map((l) => l[1].replace(/&#x27;/g, "'"));

/** Each group's own icon, which its button always shows. */
const GROUP_ICON_NAMES = ["crosshair", "slash", "circle-dot", "circle-dashed", "globe", "square", "brick-wall", "angle"];

/** The window's storage, kept in a map. */
function fakeStorage(entries: Record<string, string> = {}) {
  const data = new Map(Object.entries(entries));
  return { data, getItem: (key: string) => data.get(key) ?? null, setItem: (key: string, value: string) => void data.set(key, value) };
}

beforeEach(() => {
  vi.stubGlobal("window", { setTimeout: () => 0, confirm: () => true });
  vi.stubGlobal("isTauri", true);
  ipc.forgetMeasureMeshes.mockResolvedValue(undefined);
  useMeasure.setState({ session: null, results: {}, busy: false, options: DEFAULT_OPTIONS, lastTool: "point-point", lastTools: {} });
  useStore.getState().newProject();
  useUi.setState({ toasts: [], dialog: null, ribbonTab: "home" });
});

afterEach(() => {
  closeMeasure();
  vi.unstubAllGlobals();
  vi.clearAllMocks();
});

describe("the Tools tab", () => {
  it("shows Tool selection, Chosen tools and Description boxes, with one row of eight group buttons and no letter codes", () => {
    openMeasure();
    const html = toolsTab();
    expect([...html.matchAll(/<legend[^>]*>([^<]*)<\/legend>/g)].map((l) => l[1])).toEqual(["Tool selection", "Chosen tools", "Description"]);
    // One toolbar holds every button: nothing else sits in the row.
    expect(html.match(/role="toolbar"/g)).toHaveLength(1);
    const buttons = groupButtons(html);
    expect(buttons.map((b) => b.label)).toEqual([
      "Point: Distance point to point",
      "Edge: Distance edge to point",
      "Circle: Distance circle to point",
      "3-point circle: Distance circle to point",
      "4-point sphere: Distance sphere to point",
      "Surface: Distance surface to point",
      "Wall thickness",
      "3-point angle",
    ]);
    expect(GROUP_ORDER.map((g) => GROUP_LABELS[g])).toEqual(["Point", "Edge", "Circle", "3-point circle", "4-point sphere", "Surface", "Wall thickness", "3-point angle"]);
    // The first six have an arrow that opens their list; Wall thickness and 3-point angle are single tools.
    expect(buttons.map((b) => b.arrow !== null)).toEqual([true, true, true, true, true, true, false, false]);
    for (const b of buttons.slice(0, 6)) expect(b.arrow).toMatch(/aria-haspopup="menu" aria-expanded="false"/);
    // Distance point to point is picked when the mode first opens: its group is the one highlighted.
    expect(buttons.map((b) => b.pressed)).toEqual([true, false, false, false, false, false, false, false]);
    expect(buttons.map((b) => b.active)).toEqual([true, false, false, false, false, false, false, false]);
    expect(buttons.map((b) => b.icon)).toEqual(GROUP_ICON_NAMES);
    // The old panel's grid of 41 buttons, each tagged P, E, C, 3C..., is gone.
    expect(html).not.toContain("ms-tool-tag");
    expect(html.match(/<button/g)).toHaveLength(14);
  });

  it("lists each group's tools in its dropdown as rows of an icon and the tool's full name, each icon its own", () => {
    for (const group of GROUP_ORDER.slice(0, 6)) {
      const rows = submenuEntries(`measure.${group}`);
      const tools = TOOLS.filter((t) => t.group === group);
      expect(rows.map((r) => (r === "-" ? r : r.label))).toEqual(tools.map((t) => t.label));
      const icons = tools.map(iconClass);
      expect(new Set(icons).size).toBe(tools.length);
      rows.forEach((row, i) => {
        const html = renderToStaticMarkup(<MenuItem f={row as Feature} open={false} onHover={() => {}} onChoose={() => {}} />);
        expect(html).toMatch(new RegExp(`^<button[^>]*role="menuitemcheckbox"[^>]*><svg[^>]*class="lucide lucide-${icons[i]}( [a-z-]+)* menu-icon"[^>]*>.*</svg><span class="menu-label">${tools[i].label}</span></button>$`));
      });
    }
    expect(submenuEntries("measure.circle").map((r) => (r as Feature).label)).toEqual([
      "Distance circle to point",
      "Distance circle to circle",
      "Distance detected circle to 3-point circle",
      "Distance circle centre to circle centre",
      "Distance circle to sphere",
      "Radius circle",
    ]);
    expect(TOOLS.filter((t) => t.group === "circle").map(iconClass)).toEqual(["crosshair", "circle-dot", "circle-dashed", "target", "globe", "radius"]);
  });

  it("picks a tool from a dropdown row: it becomes the tool and its group is highlighted, the button keeping its group's icon", () => {
    openMeasure();
    // A row of the Circle dropdown, chosen as the menu does.
    activate(feature("measure.circle-radius"));
    expect(m().session?.tool).toBe("circle-radius");
    expect(feature("measure.circle-radius").measureActive?.(m())).toBe(true);
    expect(feature("measure.circle-point").measureActive?.(m())).toBe(false);
    const buttons = groupButtons(toolsTab());
    expect(buttons.map((b) => b.pressed)).toEqual([false, false, true, false, false, false, false, false]);
    expect(buttons.map((b) => b.active)).toEqual([false, false, true, false, false, false, false, false]);
    expect(buttons[2]).toMatchObject({ label: "Circle: Radius circle", icon: "circle-dot" });
    expect(buttons[0]).toMatchObject({ label: "Point: Distance point to point", icon: "crosshair" });
  });

  it("shows in Chosen tools what the tool works out and each thing it measures between, side by side with its icon", () => {
    // The reference's own example.
    openMeasure("circle-circle3");
    let html = toolsTab();
    expect(chosenBox(html)).toEqual({ name: "Distance: Detected circle, 3-point circle", text: "Distance: Detected circle 3-point circle", icons: ["circle-dot", "circle-dashed"] });
    expect(html).toMatch(/<p class="ms-chosen"[^>]*><span class="ms-chosen-op">Distance:<\/span><span class="ms-chosen-items"><span class="ms-chosen-item"><svg[^>]*>.*?<\/svg>Detected circle<\/span><span class="ms-chosen-item"><svg[^>]*>.*?<\/svg>3-point circle<\/span><\/span><\/p>/);
    // Nothing else: no tool name, no arrows.
    expect(html).not.toContain("ms-chosen-name");
    expect(html).not.toContain("→");

    pickMeasureTool("circle-point");
    expect(chosenBox(toolsTab())).toEqual({ name: "Distance: Detected circle, Point", text: "Distance: Detected circle Point", icons: ["circle-dot", "crosshair"] });
    // One-element tools show one element.
    pickMeasureTool("circle-radius");
    expect(chosenBox(toolsTab())).toEqual({ name: "Radius: Detected circle", text: "Radius: Detected circle", icons: ["circle-dot"] });
    pickMeasureTool("circle3-radius");
    expect(chosenBox(toolsTab())).toEqual({ name: "Radius: 3-point circle", text: "Radius: 3-point circle", icons: ["circle-dashed"] });
    pickMeasureTool("point-position");
    expect(chosenBox(toolsTab())).toEqual({ name: "Position: Point", text: "Position: Point", icons: ["crosshair"] });
    // An angle to a plane: the plane is the second element.
    pickMeasureTool("edge-xy");
    expect(chosenBox(toolsTab())).toEqual({ name: "Angle: Edge, XY plane", text: "Angle: Edge XY plane", icons: ["slash", "panel-bottom"] });
    pickMeasureTool("circle-centres");
    expect(chosenBox(toolsTab())).toEqual({ name: "Distance: Circle centre, Circle centre", text: "Distance: Circle centre Circle centre", icons: ["target", "target"] });
    // Wall thickness and the 3-point angle, with their elements.
    pickMeasureTool("wall");
    expect(chosenBox(toolsTab())).toEqual({ name: "Wall thickness: Wall", text: "Wall thickness: Wall", icons: ["brick-wall"] });
    pickMeasureTool("angle3");
    html = toolsTab();
    expect(chosenBox(html)).toEqual({ name: "Angle: Point, Corner, Point", text: "Angle: Point Corner Point", icons: ["crosshair", "crosshair", "crosshair"] });
  });

  it("shows in Description a line per thing to pick, in order, then what is measured", () => {
    openMeasure("circle-circle3");
    expect(descriptionLines(toolsTab())).toEqual([
      "Pick a round edge, such as the rim of a hole, to find its circle.",
      "Pick three points around a circle.",
      "Measures the shortest distance between the detected circle and the 3-point circle, from rim to rim.",
    ]);
    pickMeasureTool("point-point");
    expect(descriptionLines(toolsTab())).toEqual(["Pick a point.", "Pick a second point.", "Measures the straight-line distance between the two points."]);
    pickMeasureTool("sphere-radius");
    expect(descriptionLines(toolsTab())).toEqual(["Pick four points on a sphere.", "Measures the sphere's radius, or its diameter when Diameter is set in Options."]);
    pickMeasureTool("wall");
    expect(descriptionLines(toolsTab())).toEqual([
      "Pick a point on the outside of a wall.",
      "Measures the wall's thickness: from the point straight into the part, square to the surface, to where it comes out on the other side.",
    ]);
    pickMeasureTool("angle3");
    expect(descriptionLines(toolsTab())).toEqual([
      "Pick a point on one arm of the angle.",
      "Pick the corner.",
      "Pick a point on the other arm.",
      "Measures the angle at the corner between the lines to the other two points, from 0° to 180°.",
    ]);
  });
});

describe("the tools' Chosen tools and Description text", () => {
  it("names what each tool measures between, telling a 3-point circle from a detected one", () => {
    expect(chosenText(tool("point-point"))).toBe("Distance: Point, Point");
    expect(chosenText(tool("circle-point"))).toBe("Distance: Detected circle, Point");
    expect(chosenText(tool("circle3-point"))).toBe("Distance: 3-point circle, Point");
    expect(chosenText(tool("circle-circle3"))).toBe("Distance: Detected circle, 3-point circle");
    expect(chosenText(tool("circle-centres"))).toBe("Distance: Circle centre, Circle centre");
    expect(chosenText(tool("circle-radius"))).toBe("Radius: Detected circle");
    expect(chosenText(tool("sphere-edge"))).toBe("Distance: 4-point sphere, Edge");
    expect(chosenText(tool("surface-surface-angle"))).toBe("Angle: Surface, Surface");
    expect(chosenText(tool("surface-yz"))).toBe("Angle: Surface, YZ plane");
    expect(chosenText(tool("point-position"))).toBe("Position: Point");
    expect(chosenText(tool("wall"))).toBe("Wall thickness: Wall");
    expect(chosenText(tool("angle3"))).toBe("Angle: Point, Corner, Point");
  });

  it("describes all 41 tools: a Pick line per thing to pick, then one line of what is measured, each tool's own", () => {
    expect(TOOLS).toHaveLength(41);
    const measured = new Set<string>();
    for (const t of TOOLS) {
      const lines = describeTool(t);
      const picks = t.op === "angle3" ? 3 : t.needs.length;
      expect([t.id, lines.length]).toEqual([t.id, picks + 1]);
      for (const line of lines.slice(0, -1)) expect([t.id, line]).toEqual([t.id, expect.stringMatching(/^Pick [^.]+\.$/)]);
      expect([t.id, lines[picks]]).toEqual([t.id, expect.stringMatching(/^Measures [^]+\.$/)]);
      measured.add(lines[picks]);
      // The ribbon's dropdown row says the same, as its tooltip and status-bar hint.
      expect(feature(`measure.${t.id}`).hint).toBe(lines.join(" "));
    }
    expect(measured.size).toBe(41);
    // A second thing of the same kind is named as such.
    expect(describeTool(tool("edge-edge")).slice(0, 2)).toEqual(["Pick a straight edge.", "Pick a second straight edge."]);
    expect(describeTool(tool("circle3-circle3")).slice(0, 2)).toEqual(["Pick three points around a circle.", "Pick three points around a second circle."]);
    expect(describeTool(tool("surface-point"))).toEqual(["Pick a flat surface.", "Pick a point.", "Measures the shortest distance from the plane the surface lies in to the point."]);
  });
});

describe("each group's button", () => {
  it("picks the tool last used from its group, or the group's first before any", () => {
    openMeasure();
    pickMeasureGroup("sphere4");
    expect(m().session?.tool).toBe("sphere-point");
    activate(feature("measure.circle-centres"));
    activate(feature("measure.point-edge"));
    expect(m().session?.tool).toBe("point-edge");
    pickMeasureGroup("circle");
    expect(m().session?.tool).toBe("circle-centres");
    pickMeasureGroup("point");
    expect(m().session?.tool).toBe("point-edge");
    // A tool picked on the Measurement tab's ribbon counts too.
    activate(feature("measure.surface-xz"));
    pickMeasureGroup("edge");
    pickMeasureGroup("surface");
    expect(m().session?.tool).toBe("surface-xz");
    // Wall thickness and 3-point angle are one tool each.
    pickMeasureGroup("wall");
    expect(m().session?.tool).toBe("wall");
    pickMeasureGroup("angle3");
    expect(m().session?.tool).toBe("angle3");
    expect(m().lastTools).toEqual({ point: "point-edge", edge: "edge-point", circle: "circle-centres", sphere4: "sphere-point", surface: "surface-xz", wall: "wall", angle3: "angle3" });
  });

  it("always shows its group's own icon, whichever tool of the group it picks", () => {
    // Each of these tools has an icon other than its group's, which a button following its tool would show.
    const picked = ["edge-yz", "circle-radius", "surface-position", "point-position", "circle3-sphere", "sphere-radius"];
    expect(picked.map((id) => iconClass(tool(id)))).toEqual(["panel-left", "radius", "map-pin", "map-pin", "globe", "radius"]);
    openMeasure();
    for (const id of picked) activate(feature(`measure.${id}`));
    const buttons = groupButtons(toolsTab());
    expect(buttons.map((b) => b.icon)).toEqual(GROUP_ICON_NAMES);
    expect(buttons.slice(0, 6).map((b) => b.label)).toEqual([
      "Point: Position point",
      "Edge: Angle edge to YZ plane",
      "Circle: Radius circle",
      "3-point circle: Distance circle to sphere",
      "4-point sphere: Radius sphere",
      "Surface: Position surface",
    ]);
    expect(buttons.map((b) => b.active)).toEqual([false, false, false, false, true, false, false, false]);
  });

  it("starts a new measurement: the clicks of the one in progress are dropped", () => {
    openMeasure();
    useMeasure.setState((s) => ({ session: s.session && { ...s.session, owner: "part-1", picks: [{ feature: { kind: "point", p: [0, 0, 0] }, world: { kind: "point", p: [0, 0, 0] }, outline: [] }] } }));
    pickMeasureGroup("circle");
    expect(m().session).toMatchObject({ tool: "circle-point", picks: [], points: [], owner: null });
  });
});

describe("Measure reopening", () => {
  it("opens on the tool used last, and every button keeps its tool", () => {
    activate(feature("home.measure"));
    activate(feature("measure.edge-yz"));
    activate(feature("measure.circle3-sphere"));
    // The case from brief 10b: Position point picked, then Cancel.
    activate(feature("measure.point-position"));
    closeMeasure();
    activate(feature("home.measure"));
    expect(m().session?.tool).toBe("point-position");
    let buttons = groupButtons(toolsTab());
    expect(buttons.map((b) => b.pressed)).toEqual([true, false, false, false, false, false, false, false]);
    expect(buttons.map((b) => b.icon)).toEqual(GROUP_ICON_NAMES);
    expect(buttons.slice(0, 4).map((b) => b.label)).toEqual(["Point: Position point", "Edge: Angle edge to YZ plane", "Circle: Distance circle to point", "3-point circle: Distance circle to sphere"]);
    expect(chosenBox(toolsTab()).name).toBe("Position: Point");
    pickMeasureGroup("edge");
    expect(m().session?.tool).toBe("edge-yz");
    pickMeasureGroup("point");
    expect(m().session?.tool).toBe("point-position");

    // Analyze › Measure opens on it too; Analyze › Wall thickness opens on its own tool, which is then the last one.
    pickMeasureTool("sphere-sphere");
    closeMeasure();
    activate(feature("analyze.measure"));
    expect(m().session?.tool).toBe("sphere-sphere");
    closeMeasure();
    activate(feature("analyze.wall-thickness"));
    expect(m().session?.tool).toBe("wall");
    closeMeasure();
    activate(feature("home.measure"));
    expect(m().session?.tool).toBe("wall");
    buttons = groupButtons(toolsTab());
    expect(buttons.map((b) => b.active)).toEqual([false, false, false, false, false, false, true, false]);
  });

  it("opens on the tool used last after the app restarts, from the remembered setting", async () => {
    const storage = fakeStorage();
    vi.stubGlobal("localStorage", storage);
    activate(feature("home.measure"));
    activate(feature("measure.circle-centres"));
    expect(storage.data.get(LAST_TOOL_KEY)).toBe("circle-centres");
    closeMeasure();

    // A restart: the modules load again, and the measure store reads the setting.
    vi.resetModules();
    const fresh = { features: await import("../features"), store: await import("./store"), session: await import("./session") };
    fresh.features.activate(fresh.features.featureById("home.measure") as Feature);
    expect(fresh.store.useMeasure.getState().session?.tool).toBe("circle-centres");
    fresh.session.closeMeasure();

    // A setting that names no tool (an older or newer app's, or junk) gives Distance point to point.
    for (const junk of ["no-such-tool", ""]) {
      storage.data.set(LAST_TOOL_KEY, junk);
      vi.resetModules();
      expect((await import("./store")).useMeasure.getState().lastTool).toBe("point-point");
    }
    // No storage at all: the same.
    vi.stubGlobal("localStorage", undefined);
    vi.resetModules();
    expect((await import("./store")).useMeasure.getState().lastTool).toBe("point-point");
  });
});
