import { FEATURES, effectiveStatus, isHidden, type Feature, type TabId } from "../features";

export interface RibbonGroup {
  name: string;
  /** The group's visible entries in order; menu children stay inside their menu. */
  entries: Feature[];
  columns: Feature[][];
  /** The single large button the group shrinks to when the strip is too narrow. */
  button: Feature;
}

/** A tab's groups in order of first appearance, with hidden entries and empty groups dropped. */
export function groupsFor(tab: TabId, hideSoon: boolean): RibbonGroup[] {
  const groups = new Map<string, Feature[]>();
  for (const f of FEATURES) {
    // Menu children live in their menu, not in the group strip.
    if (f.tab !== tab || f.parent) continue;
    const list = groups.get(f.group) ?? [];
    if (!isHidden(f, hideSoon)) list.push(f);
    groups.set(f.group, list);
  }
  return [...groups]
    .filter(([, entries]) => entries.length > 0)
    .map(([name, entries]) => ({ name, entries, columns: columnsOf(entries), button: groupButton(tab, name, entries) }));
}

/** Large buttons take a column each; runs of small buttons stack up to three per column. */
export function columnsOf(items: Feature[]): Feature[][] {
  const columns: Feature[][] = [];
  for (const f of items) {
    const last = columns[columns.length - 1];
    if (f.size !== "large" && last && last[0].size !== "large" && last.length < 3) last.push(f);
    else columns.push([f]);
  }
  return columns;
}

const isSoon = (f: Feature) => effectiveStatus(f) === "soon";

/** Not in FEATURES: it only opens the group's entries, and is "soon" when all of them are. */
function groupButton(tab: TabId, name: string, entries: Feature[]): Feature {
  const status = entries.every(isSoon) ? "soon" : "ready";
  return { id: `${tab}.group.${name}`, tab, group: name, label: name, icon: entries[0].icon, status, size: "large" };
}

/**
 * Names of the groups to collapse so the strip fits in `room` px, given each group's width when
 * expanded and when collapsed. Groups with the largest share of "soon" entries go first, ties
 * right to left. If even all collapsed is too wide, all collapse and the strip scrolls.
 */
export function collapsedGroups(groups: RibbonGroup[], expanded: number[], collapsed: number[], room: number): Set<string> {
  const share = groups.map((g) => g.entries.filter(isSoon).length / g.entries.length);
  const order = groups.map((_, i) => i).sort((a, b) => share[b] - share[a] || b - a);
  const out = new Set<number>();
  const width = () => groups.reduce((sum, _, i) => sum + (out.has(i) ? collapsed[i] : expanded[i]), 0);
  for (const i of order) {
    if (width() <= room) break;
    out.add(i);
  }
  // Collapsing a wide group can free room to show narrower ones with the same share again. Never one
  // with a larger share: that would show "soon" groups while a group with ready entries is collapsed.
  const tier = Math.min(...[...out].map((i) => share[i]));
  for (const i of [...out].reverse()) {
    if (share[i] !== tier) continue;
    out.delete(i);
    if (width() > room) out.add(i);
  }
  return new Set([...out].map((i) => groups[i].name));
}
