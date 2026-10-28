import { Fragment, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type WheelEvent } from "react";
import { ChevronDown } from "lucide-react";
import {
  activate,
  blockedReason,
  effectiveStatus,
  hoverHintFor,
  isMenu,
  ribbonTabs,
  shownTab,
  tooltipFor,
  type Feature,
  type RibbonTab,
  type TabId,
} from "../features";
import { submenuEntries, type MenuEntry } from "../menus/entries";
import { Menu } from "../menus/Menu";
import { useBoolean } from "../boolean/store";
import { useLibrary } from "../library/store";
import { useMeasure } from "../measure/store";
import { useRepair } from "../repair/store";
import { useStore } from "../store";
import { useUi } from "../ui";
import { FileMenu } from "./FileMenu";
import { collapsedGroups, groupsFor, type RibbonGroup } from "./layout";
import "./ribbon.css";

const cx = (...names: (string | false)[]) => names.filter(Boolean).join(" ");

/**
 * The tab row and the active tab's group strip, rendered entirely from the feature registry. The selected tab lives
 * in the UI store (ribbonTab), so repair mode and Boolean operations can open their contextual tabs and go back to the
 * previous one.
 */
export function Ribbon() {
  const selected = useUi((s) => s.ribbonTab);
  const repairing = useRepair((r) => r.session !== null);
  const booleanOpen = useBoolean((b) => b.session !== null);
  const libraryOpen = useLibrary((l) => l.session !== null);
  const measureOpen = useMeasure((m) => m.session !== null);
  const hideSoon = useStore((s) => s.hideSoon);
  const tabs = ribbonTabs(repairing, booleanOpen, libraryOpen, measureOpen);
  const tab = shownTab(selected, tabs);
  const groups = useMemo(() => groupsFor(tab, hideSoon), [tab, hideSoon]);
  const { stripRef, collapsed } = useCollapsedGroups(groups);

  return (
    <header className="ribbon">
      <div className="rb-tabbar">
        <FileMenu />
        <span className="rb-tabbar-sep" aria-hidden />
        <TabList tabs={tabs} tab={tab} />
      </div>
      <div
        key={tab}
        ref={stripRef}
        id="rb-panel"
        className="rb-strip"
        role="tabpanel"
        aria-labelledby={`rb-tab-${tab}`}
        onWheel={scrollSideways}
      >
        {groups.map((g) =>
          collapsed.has(g.name) ? (
            <section key={g.name} className="rb-group is-collapsed" aria-label={g.name}>
              <RibbonButton f={g.button} entries={g.entries} />
            </section>
          ) : (
            <section key={g.name} className="rb-group" aria-label={g.name}>
              <div className="rb-group-body">
                {g.columns.map((column) => (
                  <div key={column[0].id} className="rb-col">
                    {column.map((f) => (f.dropdown ? <SplitButton key={f.id} f={f} /> : <RibbonButton key={f.id} f={f} />))}
                  </div>
                ))}
              </div>
              <div className="rb-group-label" aria-hidden>
                {g.name}
              </div>
            </section>
          ),
        )}
      </div>
    </header>
  );
}

const selectTab = (id: TabId) => useUi.getState().setRibbonTab(id);

/** The tab buttons, `tab` selected. A contextual tab comes after a separator, tinted and with its icon. */
export function TabList({ tabs, tab }: { tabs: readonly RibbonTab[]; tab: TabId }) {
  // Arrow keys move between tabs, as in any tab list.
  function onTabKey(e: KeyboardEvent<HTMLDivElement>) {
    const step = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (step === 0) return;
    e.preventDefault();
    const i = tabs.findIndex((t) => t.id === tab);
    const next = tabs[(i + step + tabs.length) % tabs.length].id;
    selectTab(next);
    document.getElementById(`rb-tab-${next}`)?.focus();
  }

  return (
    <div className="rb-tablist" role="tablist" aria-label="Ribbon" onKeyDown={onTabKey}>
      {tabs.map(({ id, label, contextual, icon: Icon }) => (
        <Fragment key={id}>
          {contextual && <span className="rb-tabbar-sep" aria-hidden />}
          <button
            id={`rb-tab-${id}`}
            type="button"
            role="tab"
            className={cx("rb-tab", contextual === true && "is-contextual", tab === id && "is-selected")}
            aria-selected={tab === id}
            aria-controls="rb-panel"
            tabIndex={tab === id ? 0 : -1}
            onClick={() => selectTab(id)}
          >
            {Icon && <Icon className="rb-tab-icon" size={13} strokeWidth={2} aria-hidden />}
            {label}
          </button>
        </Fragment>
      ))}
    </div>
  );
}

interface Widths {
  groups: RibbonGroup[];
  expanded?: number[];
  collapsed?: number[];
}

/**
 * Which groups to show collapsed so the strip fits the window, like an Office ribbon. Each group
 * list is measured twice, all expanded and then all collapsed; both passes happen in layout
 * effects, before the browser paints, so they never show. Resizing only redoes the arithmetic.
 */
function useCollapsedGroups(groups: RibbonGroup[]) {
  const stripRef = useRef<HTMLDivElement>(null);
  const [measured, setMeasured] = useState<Widths>({ groups });
  const [room, setRoom] = useState(Infinity);
  const widths: Widths = measured.groups === groups ? measured : { groups };

  useLayoutEffect(() => {
    const strip = stripRef.current;
    if (!strip || (widths.expanded && widths.collapsed)) return;
    const sizes = Array.from(strip.children, (el) => el.getBoundingClientRect().width);
    setMeasured(widths.expanded ? { ...widths, collapsed: sizes } : { ...widths, expanded: sizes });
  });

  useLayoutEffect(() => {
    const measure = () => {
      const strip = stripRef.current;
      if (!strip) return;
      const style = getComputedStyle(strip);
      setRoom(strip.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight));
    };
    measure();
    window.addEventListener("resize", measure);
    return () => window.removeEventListener("resize", measure);
  }, []);

  let collapsed: Set<string>;
  if (!widths.expanded) collapsed = new Set();
  else if (!widths.collapsed) collapsed = new Set(groups.map((g) => g.name));
  else collapsed = collapsedGroups(groups, widths.expanded, widths.collapsed, room);
  return { stripRef, collapsed };
}

/** A plain mouse wheel scrolls the group strip sideways when it is wider than the window. */
function scrollSideways(e: WheelEvent<HTMLDivElement>) {
  const strip = e.currentTarget;
  // Wheel events from an open dropdown bubble here through the React tree; leave those alone.
  if (!(e.target instanceof Node) || !strip.contains(e.target)) return;
  if (e.deltaX === 0 && strip.scrollWidth > strip.clientWidth) strip.scrollLeft += e.deltaY;
}

/**
 * `entries` turns the button into a dropdown of those entries (a collapsed group); menus get their children. A
 * blocked button (blockedReason) shows greyed out with the reason in its tooltip, and a click only shows the reason.
 */
export function RibbonButton({ f, entries }: { f: Feature; entries?: MenuEntry[] }) {
  const active = useStore((s) => f.isActive?.(s) ?? false);
  const repairActive = useRepair((r) => f.repairActive?.(r) ?? false);
  const booleanActive = useBoolean((b) => f.booleanActive?.(b) ?? false);
  const viewActive = useUi((u) => f.viewTool !== undefined && u.viewTool === f.viewTool);
  const measureActive = useMeasure((m) => f.measureActive?.(m) ?? false);
  const booleanOpen = useBoolean((b) => b.session !== null);
  // Ribbon buttons never act on a triangle under the cursor; that is the repair context menu's.
  const blocked = useRepair((r) => blockedReason(f, r, null, booleanOpen));
  const [menuAt, setMenuAt] = useState<HTMLElement | null>(null);
  const soon = effectiveStatus(f) === "soon";
  const submenu = entries !== undefined || isMenu(f);
  const large = f.size === "large";
  const toggle = f.isActive !== undefined || f.repairActive !== undefined || f.booleanActive !== undefined || f.viewTool !== undefined || f.measureActive !== undefined;
  const pressed = active || repairActive || booleanActive || viewActive || measureActive;
  const Icon = f.icon;

  return (
    <>
      <button
        type="button"
        className={cx(
          "rb-btn",
          large ? "rb-large" : "rb-small",
          soon && "is-soon",
          blocked !== null && "is-disabled",
          pressed && "is-active",
          menuAt !== null && "is-open",
        )}
        aria-label={f.label}
        aria-pressed={toggle ? pressed : undefined}
        aria-disabled={blocked !== null ? true : undefined}
        aria-haspopup={submenu ? "menu" : undefined}
        aria-expanded={submenu ? menuAt !== null : undefined}
        title={tooltipFor(f, blocked)}
        onClick={(e) => {
          if (menuAt) {
            setMenuAt(null);
            return;
          }
          // Runs a button, and toasts for any Soon or blocked one; a Soon menu still opens so its plans can be read.
          activate(f);
          if (submenu) setMenuAt(e.currentTarget);
        }}
        onMouseEnter={() => useUi.getState().setHoverHint(hoverHintFor(f, blocked))}
        onMouseLeave={() => useUi.getState().setHoverHint(null)}
      >
        <Icon className="rb-icon" size={large ? 22 : 16} strokeWidth={1.75} aria-hidden />
        <span className="rb-label">
          {f.label}
          {submenu && <ChevronDown className="rb-chevron" size={12} aria-hidden />}
        </span>
        {soon && <span className="soon-pill rb-pill">Soon</span>}
      </button>
      {menuAt && <Menu entries={entries ?? submenuEntries(f.id)} anchor={{ el: menuAt }} label={f.label} onClose={() => setMenuAt(null)} />}
    </>
  );
}

/**
 * A button with an arrow beside it (Undo, Redo): the button runs, and the arrow opens the rows `f.dropdown` builds at
 * that moment. A blocked button is greyed out as RibbonButton's are; the arrow still opens, so its list can say why.
 */
export function SplitButton({ f }: { f: Feature }) {
  const booleanOpen = useBoolean((b) => b.session !== null);
  const blocked = useRepair((r) => blockedReason(f, r, null, booleanOpen));
  const [menu, setMenu] = useState<{ at: HTMLElement; rows: MenuEntry[] } | null>(null);
  const Icon = f.icon;
  const hover = (hint: string | null) => useUi.getState().setHoverHint(hint);

  return (
    <div className={cx("rb-split", f.size === "large" ? "rb-split-large" : "rb-split-small")}>
      <button
        type="button"
        className={cx("rb-btn", f.size === "large" ? "rb-large" : "rb-small", "rb-split-main", blocked !== null && "is-disabled")}
        aria-label={f.label}
        aria-disabled={blocked !== null ? true : undefined}
        title={tooltipFor(f, blocked)}
        onClick={() => activate(f)}
        onMouseEnter={() => hover(hoverHintFor(f, blocked))}
        onMouseLeave={() => hover(null)}
      >
        <Icon className="rb-icon" size={f.size === "large" ? 22 : 16} strokeWidth={1.75} aria-hidden />
        <span className="rb-label">{f.label}</span>
      </button>
      <button
        type="button"
        className={cx("rb-btn", "rb-split-arrow", menu !== null && "is-open")}
        aria-label={`${f.label} several steps`}
        aria-haspopup="menu"
        aria-expanded={menu !== null}
        title={`${f.label} several steps`}
        onClick={(e) => setMenu(menu ? null : { at: e.currentTarget, rows: f.dropdown?.() ?? [] })}
        onMouseEnter={() => hover(`Pick how far to ${f.label.toLowerCase()}.`)}
        onMouseLeave={() => hover(null)}
      >
        <ChevronDown size={12} aria-hidden />
      </button>
      {menu && <Menu entries={menu.rows} anchor={{ el: menu.at }} label={f.label} onClose={() => setMenu(null)} />}
    </div>
  );
}
