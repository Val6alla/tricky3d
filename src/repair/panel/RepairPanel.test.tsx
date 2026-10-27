import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { formatDegrees, clamp } from "./controls";
import { REPAIR_TABS, RepairPanel, catalogNote } from "./RepairPanel";

describe("catalogNote", () => {
  it("is empty once the catalog is loaded", () => {
    expect(catalogNote(true, true, false)).toBeNull();
    expect(catalogNote(true, false, true)).toBeNull();
  });

  it("says why the actions are missing", () => {
    expect(catalogNote(false, false, false)).toBe("Repair needs the tricky3D desktop app.");
    expect(catalogNote(false, true, false)).toBe("Loading the repair actions…");
    expect(catalogNote(false, true, true)).toBe("The repair actions could not be loaded. Close repair mode and open it again to retry.");
  });
});

describe("controls", () => {
  it("show degrees whole, or with the one decimal they have", () => {
    expect(formatDegrees(10)).toBe("10");
    expect(formatDegrees(7.5)).toBe("7.5");
    expect(formatDegrees(7.25)).toBe("7.3");
    expect(formatDegrees(0)).toBe("0");
  });

  it("keep values in range", () => {
    expect(clamp(5, 0, 10)).toBe(5);
    expect(clamp(-1, 0, 10)).toBe(0);
    expect(clamp(11, 0, 10)).toBe(10);
  });
});

describe("RepairPanel", () => {
  const html = renderToStaticMarkup(<RepairPanel />);

  it("has the five tabs in the designed order, Status first and selected", () => {
    expect(REPAIR_TABS.map((t) => t.label)).toEqual(["Status", "Actions", "Repair scripts", "Shells", "View"]);
    const tabs = [...html.matchAll(/<button ([^>]*role="tab"[^>]*)>(.*?)<\/button>/g)];
    expect(tabs.map((t) => t[2])).toEqual(["Status", "Actions", "Repair scripts", "Shells", "View"]);
    expect(tabs[0][1]).toContain('aria-selected="true"');
    expect(tabs[0][1]).toContain('tabindex="0"');
    for (const t of tabs.slice(1)) {
      expect(t[1]).toContain('aria-selected="false"');
      expect(t[1]).toContain('tabindex="-1"');
    }
  });

  it("shows only the selected tab's panel, the Status tab", () => {
    const panels = [...html.matchAll(/<div [^>]*role="tabpanel"[^>]*>/g)];
    expect(panels).toHaveLength(1);
    expect(html).toContain("Mesh is closed");
  });

  it("ends with Apply repair and Run repair script", () => {
    const footer = html.slice(html.indexOf('<div class="rp-footer">'));
    expect(footer).toMatch(/^<div class="rp-footer"><button type="button" class="clip-btn clip-btn-primary"[^>]*>.*?Apply repair<\/button><button type="button" class="clip-btn"[^>]*>.*?Run repair script<\/button><\/div><\/div>$/);
  });
});
