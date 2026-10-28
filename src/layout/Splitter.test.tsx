import type { ReactElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// The handle's own pointer and key handlers, called as the browser would. React's state hooks are stand-ins, so
// Splitter can be called as a plain function and its element's handlers run without a page: one call is one render,
// and its handlers share that render's refs, as they do in the app.
vi.mock("react", async (importOriginal) => ({
  ...(await importOriginal<typeof import("react")>()),
  useState: <T,>(init: T | (() => T)) => [typeof init === "function" ? (init as () => T)() : init, () => {}],
  useRef: <T,>(current: T) => ({ current }),
}));

import { RIGHT_PANEL_SIZE } from "./RightPanel";
import { SIDEBAR_SIZE } from "./Sidebar";
import { dragWidth, keyWidth, loadWidth, Splitter, usePanelWidth, type PanelSize } from "./Splitter";

let stored: Map<string, string>;

beforeEach(() => {
  stored = new Map();
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => stored.get(k) ?? null,
    setItem: (k: string, v: string) => void stored.set(k, v),
  });
});

afterEach(() => vi.unstubAllGlobals());

interface HandleProps {
  onPointerDown(e: object): void;
  onPointerMove(e: object): void;
  onPointerUp(): void;
  onDoubleClick(): void;
  onKeyDown(e: object): void;
  "aria-valuemin": number;
  "aria-valuemax": number;
  "aria-valuenow": number;
}

/** One render of the handle on a panel `width` wide; `resized` collects the widths it asks for. */
function handle(edge: "left" | "right", size: PanelSize, width: number, resized: number[]) {
  return (Splitter({ edge, size, width, onResize: (w) => resized.push(w), label: "Resize" }) as ReactElement<HandleProps>).props;
}

const pointer = (clientX: number) => ({ button: 0, clientX, pointerId: 1, preventDefault() {}, currentTarget: { setPointerCapture() {} } });
const key = (k: string) => ({ key: k, preventDefault() {} });

describe("the panels' size rules", () => {
  it("let the parts sidebar be 180 to 480 px wide, 260 at first, and the right panel 200 to 480, 280 at first", () => {
    expect(SIDEBAR_SIZE).toEqual({ storageKey: "tricky3d.sidebarWidth", min: 180, max: 480, initial: 260 });
    expect(RIGHT_PANEL_SIZE).toEqual({ storageKey: "tricky3d.rightPanelWidth", min: 200, max: 480, initial: 280 });
  });
});

describe("dragging a splitter", () => {
  it("widens a panel when dragged away from it and narrows it when dragged towards it, on either side", () => {
    // The sidebar's handle is on its right edge, the right panel's on its left edge.
    expect(dragWidth("right", SIDEBAR_SIZE, 260, 40)).toBe(300);
    expect(dragWidth("right", SIDEBAR_SIZE, 260, -40)).toBe(220);
    expect(dragWidth("left", RIGHT_PANEL_SIZE, 280, -40)).toBe(320);
    expect(dragWidth("left", RIGHT_PANEL_SIZE, 280, 40)).toBe(240);
  });

  it("stops at the panel's limits, and comes back from them as soon as the pointer does", () => {
    expect(dragWidth("right", SIDEBAR_SIZE, 260, 5000)).toBe(480);
    expect(dragWidth("right", SIDEBAR_SIZE, 260, -5000)).toBe(180);
    expect(dragWidth("left", RIGHT_PANEL_SIZE, 280, -5000)).toBe(480);
    expect(dragWidth("left", RIGHT_PANEL_SIZE, 280, 5000)).toBe(200);

    const resized: number[] = [];
    const h = handle("right", SIDEBAR_SIZE, 260, resized);
    h.onPointerDown(pointer(100));
    h.onPointerMove(pointer(900));
    h.onPointerMove(pointer(-900));
    h.onPointerMove(pointer(150));
    expect(resized).toEqual([480, 180, 310]);
    h.onPointerUp();
  });

  it("keeps the width it ends on for next time, writing it once when the drag ends", () => {
    const resized: number[] = [];
    const h = handle("left", RIGHT_PANEL_SIZE, 280, resized);
    h.onPointerDown(pointer(1000));
    h.onPointerMove(pointer(950));
    h.onPointerMove(pointer(900));
    // Live updates during the drag leave storage alone.
    expect(stored.size).toBe(0);
    h.onPointerUp();
    expect(resized).toEqual([330, 380]);
    expect(stored.get("tricky3d.rightPanelWidth")).toBe("380");
    // A second end (the lost pointer capture after pointer up) writes nothing more.
    stored.clear();
    h.onPointerUp();
    expect(stored.size).toBe(0);
    // The next start of the app opens the panel at that width.
    stored.set("tricky3d.rightPanelWidth", "380");
    expect(usePanelWidth(RIGHT_PANEL_SIZE)[0]).toBe(380);
  });

  it("writes nothing for a drag that ends where it started, or for another button than the left", () => {
    const resized: number[] = [];
    let h = handle("right", SIDEBAR_SIZE, 260, resized);
    h.onPointerDown(pointer(100));
    h.onPointerMove(pointer(140));
    h.onPointerMove(pointer(100));
    h.onPointerUp();
    expect(stored.size).toBe(0);
    h = handle("right", SIDEBAR_SIZE, 260, resized);
    h.onPointerDown({ ...pointer(100), button: 2 });
    h.onPointerMove(pointer(200));
    expect(resized).toEqual([300, 260]);
  });

  it("steps 16 px with the arrow keys, within the limits, and keeps each step", () => {
    expect(keyWidth("right", SIDEBAR_SIZE, 260, "ArrowRight")).toBe(276);
    expect(keyWidth("right", SIDEBAR_SIZE, 260, "ArrowLeft")).toBe(244);
    expect(keyWidth("left", RIGHT_PANEL_SIZE, 280, "ArrowLeft")).toBe(296);
    expect(keyWidth("left", RIGHT_PANEL_SIZE, 280, "ArrowRight")).toBe(264);
    expect(keyWidth("right", SIDEBAR_SIZE, 470, "ArrowRight")).toBe(480);
    expect(keyWidth("left", RIGHT_PANEL_SIZE, 205, "ArrowRight")).toBe(200);

    const resized: number[] = [];
    handle("left", RIGHT_PANEL_SIZE, 280, resized).onKeyDown(key("ArrowLeft"));
    expect(resized).toEqual([296]);
    expect(stored.get("tricky3d.rightPanelWidth")).toBe("296");
    handle("left", RIGHT_PANEL_SIZE, 280, resized).onKeyDown(key("ArrowUp"));
    expect(resized).toEqual([296]);
  });

  it("goes back to the panel's first width on a double-click, and keeps that", () => {
    const resized: number[] = [];
    handle("right", SIDEBAR_SIZE, 410, resized).onDoubleClick();
    expect(resized).toEqual([260]);
    expect(stored.get("tricky3d.sidebarWidth")).toBe("260");
  });

  it("tells assistive technology its limits and the width now", () => {
    const h = handle("right", SIDEBAR_SIZE, 300.4, []);
    expect([h["aria-valuemin"], h["aria-valuemax"], h["aria-valuenow"]]).toEqual([180, 480, 300]);
  });
});

describe("a kept width", () => {
  it("is read back within the panel's limits, and the first width stands in for nothing or junk", () => {
    expect(loadWidth(SIDEBAR_SIZE)).toBe(260);
    stored.set("tricky3d.sidebarWidth", "333");
    expect(loadWidth(SIDEBAR_SIZE)).toBe(333);
    // A width kept from a build with other limits, or edited by hand.
    stored.set("tricky3d.sidebarWidth", "9999");
    expect(loadWidth(SIDEBAR_SIZE)).toBe(480);
    stored.set("tricky3d.sidebarWidth", "20");
    expect(loadWidth(SIDEBAR_SIZE)).toBe(180);
    stored.set("tricky3d.sidebarWidth", "wide");
    expect(loadWidth(SIDEBAR_SIZE)).toBe(260);
    // Each panel keeps its own.
    stored.set("tricky3d.sidebarWidth", "300");
    expect(loadWidth(RIGHT_PANEL_SIZE)).toBe(280);
  });

  it("falls back to the first width when storage cannot be read, and a drag still works without it", () => {
    vi.stubGlobal("localStorage", {
      getItem: () => {
        throw new Error("denied");
      },
      setItem: () => {
        throw new Error("denied");
      },
    });
    expect(loadWidth(RIGHT_PANEL_SIZE)).toBe(280);
    const resized: number[] = [];
    const h = handle("left", RIGHT_PANEL_SIZE, 280, resized);
    h.onPointerDown(pointer(500));
    h.onPointerMove(pointer(480));
    expect(() => h.onPointerUp()).not.toThrow();
    expect(resized).toEqual([300]);
  });
});
