import { CONTEXT_MENU, FEATURES, childrenOf, featureById, isHidden, type Feature } from "../features";

/** A menu row: a feature, or "-" for a separator. */
export type MenuEntry = Feature | "-";

/** Puts a separator wherever the group changes, so a menu reads in sections. */
export function sectioned(features: Feature[]): MenuEntry[] {
  const out: MenuEntry[] = [];
  features.forEach((f, i) => {
    if (i > 0 && f.group !== features[i - 1].group) out.push("-");
    out.push(f);
  });
  return out;
}

/** Drops hidden entries, then any separator left at either end or next to another one. */
export function tidy(entries: MenuEntry[], hideSoon: boolean): MenuEntry[] {
  const out: MenuEntry[] = [];
  for (const e of entries) {
    if (e === "-") {
      if (out.length > 0 && out[out.length - 1] !== "-") out.push(e);
    } else if (!isHidden(e, hideSoon)) {
      out.push(e);
    }
  }
  if (out[out.length - 1] === "-") out.pop();
  return out;
}

export function submenuEntries(id: string): MenuEntry[] {
  return sectioned(childrenOf(id));
}

/** The File dropdown: every top-level "file" entry, one section per group. */
export function fileMenuEntries(): MenuEntry[] {
  return sectioned(FEATURES.filter((f) => f.tab === "file" && !f.parent));
}

export function contextMenuEntries(): MenuEntry[] {
  const out: MenuEntry[] = [];
  for (const id of CONTEXT_MENU) {
    const entry = id === "-" ? "-" : featureById(id);
    if (entry) out.push(entry);
  }
  return out;
}

/** The repair context menu (right-click on the working copy): every "repair-context" entry, one section per group. */
export function repairContextMenuEntries(): MenuEntry[] {
  return sectioned(FEATURES.filter((f) => f.tab === "repair-context" && !f.parent));
}
