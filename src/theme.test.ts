// @ts-expect-error The app's tsconfig has no Node types; vitest runs this file in Node.
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// The light theme's readable text, measured with the WCAG formula against the surfaces it sits on.

const css = (path: string) => readFileSync(`src/${path}`, "utf8").replace(/\r\n/g, "\n");
const theme = css("theme.css");
const ribbon = css("ribbon/ribbon.css");
const menus = css("menus/menus.css");
const layout = css("layout/layout.css");

/** The custom properties of one block of theme.css: the first `:root {` (light) or the one inside the dark media query. */
function tokens(css: string, dark: boolean): Map<string, string> {
  const start = dark ? css.indexOf(":root", css.indexOf("prefers-color-scheme: dark")) : css.indexOf(":root");
  const block = css.slice(start, css.indexOf("\n}", start));
  return new Map([...block.matchAll(/(--[\w-]+):\s*([^;]+);/g)].map((m) => [m[1], m[2].trim()]));
}

function rgb(hex: string): number[] {
  return [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
}

/** `rgba(r, g, b, a)` over an opaque `under`. */
function over(colour: string, under: string): number[] {
  const m = /rgba\((\d+),\s*(\d+),\s*(\d+),\s*([\d.]+)\)/.exec(colour);
  if (!m) return rgb(colour);
  const a = Number(m[4]);
  return rgb(under).map((u, i) => Math.round(Number(m[i + 1]) * a + u * (1 - a)));
}

function luminance(c: number[]): number {
  const [r, g, b] = c.map((v) => v / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function contrast(a: number[], b: number[]): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

/** The declarations of `selector`'s first rule. */
function rule(css: string, selector: string): string {
  const at = css.indexOf(`${selector} {`);
  expect(at, selector).toBeGreaterThanOrEqual(0);
  return css.slice(at, css.indexOf("}", at));
}

const light = tokens(theme, false);
const dark = tokens(theme, true);
const t = (name: string) => light.get(name) ?? "";
const SURFACES = ["--surface", "--surface-2", "--surface-3", "--bg"];

describe("light theme contrast", () => {
  it("warning text reads at 4.5:1 or more on every surface", () => {
    for (const s of SURFACES) expect(contrast(rgb(t("--warn")), rgb(t(s))), s).toBeGreaterThanOrEqual(4.5);
  });

  it("an unselected contextual tab reads at 4.5:1 or more on its tint, also on hover", () => {
    const tint = over(t("--accent-soft"), t("--bg"));
    expect(contrast(rgb(t("--accent-strong")), tint)).toBeGreaterThanOrEqual(4.5);
    expect(contrast(rgb(t("--accent-strong-hover")), tint)).toBeGreaterThanOrEqual(4.5);
    expect(rule(ribbon, ".rb-tab.is-contextual")).toContain("var(--accent-strong)");
    expect(rule(ribbon, ".rb-tab.is-contextual:hover")).toContain("var(--accent-strong-hover)");
  });

  it("small grey text that must be read uses --text-subtle, 4.5:1 or more on the panels", () => {
    for (const s of ["--surface", "--surface-2"]) expect(contrast(rgb(t("--text-subtle")), rgb(t(s))), s).toBeGreaterThanOrEqual(4.5);
    expect(rule(menus, ".menu-shortcut")).toContain("var(--text-subtle)");
    expect(rule(layout, ".statusbar-task-more")).toContain("var(--text-subtle)");
    expect(rule(layout, ".rpanel-info dd")).toContain("var(--text-subtle)");
  });

  it("leaves the dark theme's colours as they were", () => {
    expect(dark.get("--warn")).toBe("#e0a84a");
    expect(dark.get("--accent-strong")).toBe(dark.get("--accent"));
    expect(dark.get("--accent-strong-hover")).toBe(dark.get("--accent-hover"));
    // --text-subtle is the dark theme's --text-faint, so text moved to it looks the same there.
    expect(dark.get("--text-subtle")).toBe(dark.get("--text-faint"));
  });
});
