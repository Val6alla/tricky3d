import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { SELECT_TOOL } from "../store";
import { StatusBar, triangleSelectionText } from "./StatusBar";

describe("StatusBar", () => {
  it("counts the working copy's selected triangles in repair mode", () => {
    expect(triangleSelectionText(1204, 762014)).toBe("1,204 of 762,014 triangles selected");
    expect(triangleSelectionText(0, 12)).toBe("0 of 12 triangles selected");
    expect(triangleSelectionText(1, 1)).toBe("1 of 1 triangle selected");
  });

  it("shows the Select tool and its hint outside repair mode", () => {
    // Server rendering reads each store's initial state: no parts, not repairing.
    const html = renderToStaticMarkup(<StatusBar />);
    expect(html).toContain(`<span class="statusbar-tool">${SELECT_TOOL.name}</span>`);
    expect(html).toContain(`title="${SELECT_TOOL.hint}">${SELECT_TOOL.hint}</span>`);
    expect(html).not.toContain("statusbar-selection");
  });
});
