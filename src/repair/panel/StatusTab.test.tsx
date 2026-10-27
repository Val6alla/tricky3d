import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import type { RepairStatus } from "../types";
import { EDGES_OFF, StatusTab, edgesFromSlider, edgesFromText, edgesSliderValue, isProblemRow, refreshOnAutoUpdate } from "./StatusTab";
import { ViewTab } from "./ViewTab";

describe("Edges from slider", () => {
  it("is off at the far end", () => {
    expect(edgesSliderValue(null)).toBe(EDGES_OFF);
    expect(edgesFromSlider(EDGES_OFF)).toBeNull();
    expect(edgesFromText(null)).toBe("off");
  });

  it("stands for whole degrees below the far end", () => {
    expect(edgesSliderValue(30)).toBe(30);
    expect(edgesFromSlider(30)).toBe(30);
    expect(edgesFromSlider(0)).toBe(0);
    expect(edgesFromSlider(EDGES_OFF - 1)).toBe(EDGES_OFF - 1);
    expect(edgesFromSlider(29.6)).toBe(30);
    expect(edgesFromText(30)).toBe("30°");
  });

  it("keeps an angle off the far end, so a setting never reads as off by accident", () => {
    expect(edgesSliderValue(180)).toBe(EDGES_OFF - 1);
    expect(edgesSliderValue(500)).toBe(EDGES_OFF - 1);
    expect(edgesSliderValue(-5)).toBe(0);
    for (let p = 0; p <= EDGES_OFF; p++) expect(edgesSliderValue(edgesFromSlider(p))).toBe(p);
  });
});

describe("isProblemRow", () => {
  it("marks problem counts that are not zero", () => {
    expect(isProblemRow("Border edges", "482")).toBe(true);
    expect(isProblemRow("Inv. orientation", "6,354")).toBe(true);
    expect(isProblemRow("Holes", "1")).toBe(true);
    expect(isProblemRow("Non-manifold edges", "2,656")).toBe(true);
  });

  it("leaves zero, unknown and plain counts alone", () => {
    expect(isProblemRow("Holes", "0")).toBe(false);
    expect(isProblemRow("Holes", "?")).toBe(false);
    expect(isProblemRow("Triangles", "762,000")).toBe(false);
    expect(isProblemRow("Shells", "27")).toBe(false);
  });
});

describe("refreshOnAutoUpdate", () => {
  const status = {} as RepairStatus;
  it("computes the status when Auto-update goes on while it is unknown and nothing runs", () => {
    expect(refreshOnAutoUpdate(true, null, false)).toBe(true);
    expect(refreshOnAutoUpdate(true, status, false)).toBe(false);
    expect(refreshOnAutoUpdate(true, null, true)).toBe(false);
    expect(refreshOnAutoUpdate(false, null, false)).toBe(false);
  });
});

describe("StatusTab", () => {
  // Server rendering reads the repair store's initial state: no status yet, the default switches.
  const html = renderToStaticMarkup(<StatusTab />);
  const rows = [...html.matchAll(/<dt>(.*?)<\/dt><dd class="([^"]*)">(.*?)<\/dd>|<dt>(.*?)<\/dt><dd>(.*?)<\/dd>/g)].map((m) => [m[1] ?? m[4], m[3] ?? m[5]]);

  it("shows ? for closed, oriented and every statistic until the status is computed", () => {
    expect(rows).toEqual([
      ["Mesh is closed", "?"],
      ["Mesh is oriented", "?"],
      ["Edges", "?"],
      ["Triangles", "?"],
      ["Shells", "?"],
      ["Border edges", "?"],
      ["Inv. orientation", "?"],
      ["Holes", "?"],
      ["Non-manifold edges", "?"],
    ]);
    expect(html).toContain('<dd class="rp-mark is-unknown">?</dd>');
    expect(html).toContain("Not computed for the mesh as it is now. Click Update.");
  });

  it("has Update and Auto-update, which starts off", () => {
    expect(html).toMatch(/<button type="button" class="clip-btn" title="[^"]+">.*?Update<\/button>/);
    expect(html).toMatch(/<input type="checkbox"\/>Auto-update<\/label>/);
  });

  it("starts with every highlight on and Edges from off, in the designed order", () => {
    const labels = [...html.matchAll(/<input type="checkbox" checked=""\/><span class="rp-swatch[^"]*" aria-hidden="true"><\/span>(.*?)<\/label>/g)].map((m) => m[1]);
    expect(labels).toEqual(["Holes", "Triangles", "Degenerate faces", "Errors"]);
    const section = html.slice(html.indexOf(">Highlight<"));
    const order = ["Holes", "Triangles", "Edges from", "Degenerate faces", "Errors"].map((l) => section.indexOf(`>${l}<`));
    expect(order.every((at, i) => at > 0 && (i === 0 || at > order[i - 1]))).toBe(true);
    expect(html).toMatch(/aria-label="Edges from" aria-valuetext="off" min="0" max="180" step="1" value="180"/);
    expect(html).toContain('<output class="rp-edges-value is-off">off</output>');
  });
});

describe("ViewTab", () => {
  const html = renderToStaticMarkup(<ViewTab />);

  it("shows the Soon part switches disabled", () => {
    for (const label of ["Show textures", "Show transparent part"]) {
      expect(html).toContain(`<input type="checkbox" disabled="" readOnly=""/>${label}<span class="soon-pill">Soon</span>`);
    }
  });

  it("starts with slim faces hidden, their 10° slider idle, and the surface tolerance at 180°", () => {
    expect(html).toMatch(/<input type="checkbox"\/><span class="rp-swatch rp-swatch-slim" aria-hidden="true"><\/span>Colored/);
    expect(html).toMatch(/<input type="checkbox"\/><span class="rp-swatch rp-swatch-slim is-line" aria-hidden="true"><\/span>Highlight edges/);
    expect(html).toMatch(/aria-label="Smallest angle" aria-valuetext="10°" min="0" max="60" step="1" disabled="" value="10"/);
    expect(html).toMatch(/aria-label="Tolerance" aria-valuetext="180°" min="0" max="180" step="1" value="180"/);
    const titles = [...html.matchAll(/<h3 class="rp-title">(.*?)<\/h3>/g)].map((m) => m[1]);
    expect(titles).toEqual(["Part", "Show slim faces", "Surface selection"]);
  });
});
