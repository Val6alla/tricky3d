// Repair panel › Actions: one button per repair action, run on the working copy (on the selection when there is one)
// with the catalog's default parameters. Each run that changes the mesh is one inner undo step.
import { formatCount } from "../../info";
import { statusCount } from "../format";
import { actionSpec, defaultParams } from "../scripts";
import { runAction } from "../session";
import { useRepair } from "../store";
import type { ActionId, ActionSpec, Catalog } from "../types";

/** A button of the tab: the action it runs and its label here. */
export interface ActionButton {
  id: ActionId;
  label: string;
}

export const CLOSE_HOLES: ActionButton[] = [
  { id: "close_trivial_holes", label: "Trivial" },
  { id: "stitch_triangles", label: "Stitch triangles" },
  { id: "close_all_holes", label: "All" },
  { id: "close_defined_holes", label: "Defined" },
];

export const FIX_FLIPPED: ActionButton = { id: "fix_flipped_triangles", label: "Fix flipped triangles" };

export const SELF_INTERSECTIONS: ActionButton[] = [
  { id: "detect_self_intersections", label: "Detect" },
  { id: "split_off_self_intersections", label: "Split off" },
  { id: "remove_self_intersections", label: "Remove" },
];

export const MORE_ACTIONS: ActionButton[] = [
  { id: "remove_double_triangles", label: "Remove double triangles" },
  { id: "remove_degenerate_faces", label: "Remove degenerate faces" },
  { id: "wrap_part_surface", label: "Wrap part surface" },
  { id: "remove_problem_areas", label: "Remove problem areas" },
  { id: "split_non_oriented_edges", label: "Split non-oriented edges" },
];

/** Every button of the tab, in order. */
export const ALL_BUTTONS: ActionButton[] = [...CLOSE_HOLES, FIX_FLIPPED, ...SELF_INTERSECTIONS, ...MORE_ACTIONS];

/**
 * Whether a button can run now, and its tooltip: the action's hint, with the reason added when it cannot run (no
 * catalog yet, a Soon action, a selection it needs, a run going on).
 */
export function buttonState(spec: ActionSpec | undefined, busy: boolean, selected: number): { disabled: boolean; title: string } {
  if (!spec) return { disabled: true, title: "The repair actions are not loaded." };
  if (!spec.implemented) return { disabled: true, title: `${spec.label} is coming in a future version.` };
  if (spec.needsSelection && selected === 0) return { disabled: true, title: `${spec.hint} Select triangles first.` };
  if (busy) return { disabled: true, title: `${spec.hint} Wait for the current repair to finish.` };
  return { disabled: false, title: spec.hint };
}

/** The line over the buttons: what the actions work on. */
export function selectionNote(selected: number): string {
  if (selected === 0) return "No triangles selected: actions work on the whole part.";
  return `${formatCount(selected)} ${selected === 1 ? "triangle" : "triangles"} selected: actions work on the selection.`;
}

/** What the last Detect found, or null before it ran on the mesh as it is. */
export function intersectionNote(intersecting: Uint32Array | null): string | null {
  if (intersecting === null) return null;
  const n = intersecting.length;
  if (n === 0) return "Detect found no self-intersections.";
  return `Detect found ${formatCount(n)} self-intersecting ${n === 1 ? "triangle" : "triangles"}.`;
}

export function ActionsTab({ missing }: { missing: string | null }) {
  const catalog = useRepair((s) => s.catalog);
  const busy = useRepair((s) => s.busy !== null);
  const selected = useRepair((s) => s.session?.selectedCount ?? 0);
  const status = useRepair((s) => s.session?.status ?? null);
  const intersecting = useRepair((s) => s.session?.intersecting ?? null);
  const found = intersectionNote(intersecting);
  const button = (b: ActionButton) => <RunButton key={b.id} button={b} catalog={catalog} busy={busy} selected={selected} />;

  return (
    <div className="rp-body">
      {missing && <p className="rpanel-muted">{missing}</p>}
      <p className="rpanel-muted">{selectionNote(selected)}</p>
      <section className="rp-group" aria-label="Close holes">
        <h3 className="rp-title">Close holes</h3>
        <dl className="rpanel-info rp-info">
          <div>
            <dt>Border edges</dt>
            <dd className={status ? "rp-value" : undefined}>{statusCount(status?.borderEdges)}</dd>
          </div>
          <div>
            <dt>Holes</dt>
            <dd className={status ? "rp-value" : undefined}>{statusCount(status?.holes)}</dd>
          </div>
        </dl>
        <div className="rp-grid rp-grid-2">{CLOSE_HOLES.map(button)}</div>
      </section>
      <section className="rp-group" aria-label="Flipped triangles">
        <h3 className="rp-title">Flipped triangles</h3>
        <dl className="rpanel-info rp-info">
          <div>
            <dt>Inv. orientation</dt>
            <dd className={status ? "rp-value" : undefined}>{statusCount(status?.invertedTriangles)}</dd>
          </div>
        </dl>
        <div className="rp-grid">{button(FIX_FLIPPED)}</div>
      </section>
      <section className="rp-group" aria-label="Self-intersections">
        <h3 className="rp-title">Self-intersections</h3>
        <div className="rp-grid rp-grid-3">{SELF_INTERSECTIONS.map(button)}</div>
        {found && <p className="rpanel-muted">{found}</p>}
      </section>
      <section className="rp-group" aria-label="More actions">
        <h3 className="rp-title">More actions</h3>
        <div className="rp-grid">{MORE_ACTIONS.map(button)}</div>
      </section>
    </div>
  );
}

interface RunButtonProps {
  button: ActionButton;
  catalog: Catalog | null;
  busy: boolean;
  selected: number;
}

function RunButton({ button, catalog, busy, selected }: RunButtonProps) {
  const spec = catalog ? actionSpec(catalog, button.id) : undefined;
  const { disabled, title } = buttonState(spec, busy, selected);
  return (
    <button
      type="button"
      className="clip-btn rp-run"
      title={title}
      disabled={disabled}
      onClick={() => {
        if (spec) void runAction(spec.id, defaultParams(spec));
      }}
    >
      {button.label}
    </button>
  );
}
