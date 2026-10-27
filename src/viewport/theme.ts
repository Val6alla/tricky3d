/** CSS tokens (theme.css) the viewport reads at runtime. */
const TOKENS = {
  platform: "--vp-platform",
  gridMinor: "--vp-grid-minor",
  gridMajor: "--vp-grid-major",
  ruler: "--vp-ruler",
  selected: "--vp-selected",
  backface: "--vp-backface",
  openEdge: "--vp-open-edge",
  edge: "--vp-edge",
  axisX: "--vp-axis-x",
  axisY: "--vp-axis-y",
  axisZ: "--vp-axis-z",
  gizmoCentre: "--vp-gizmo-centre",
  repairHole: "--vp-repair-hole",
  repairError: "--vp-repair-error",
  repairDegenerate: "--vp-repair-degenerate",
  repairSelected: "--vp-repair-selected",
  repairSlim: "--vp-repair-slim",
  cutPoint: "--vp-cut-point",
  cutCurrent: "--vp-cut-current",
  cutLine: "--vp-cut-line",
  node: "--vp-node",
  booleanMain: "--vp-boolean-main",
  booleanMainSelected: "--vp-boolean-main-selected",
  booleanTool: "--vp-boolean-tool",
  booleanToolSelected: "--vp-boolean-tool-selected",
  surface: "--surface",
  surface2: "--surface-2",
  border: "--border",
  borderStrong: "--border-strong",
  text: "--text",
  textMuted: "--text-muted",
  accent: "--accent",
  font: "--font-ui",
} as const;

/** Token values as CSS strings, e.g. "#f28c28". */
export type VpTheme = Record<keyof typeof TOKENS, string>;

export function readTheme(): VpTheme {
  const style = getComputedStyle(document.documentElement);
  const theme = {} as VpTheme;
  for (const key of Object.keys(TOKENS) as (keyof VpTheme)[]) {
    theme[key] = style.getPropertyValue(TOKENS[key]).trim();
  }
  if (!theme.font) theme.font = "sans-serif";
  return theme;
}

/** Colour of world axis 0 (X), 1 (Y) or 2 (Z). */
export function axisColor(theme: VpTheme, axis: number): string {
  return [theme.axisX, theme.axisY, theme.axisZ][axis];
}

/** Calls `onChange` when the OS switches between light and dark. Returns the unsubscribe function. */
export function watchColorScheme(onChange: () => void): () => void {
  const query = window.matchMedia("(prefers-color-scheme: dark)");
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}
