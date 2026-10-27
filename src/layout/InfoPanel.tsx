import { useEffect, useMemo, useReducer, useState } from "react";
import { TriangleAlert } from "lucide-react";
import { computeAreasInBackground, infoRows, sameAreaWork, selectionText, type StatsLookup } from "../info";
import { cachedStats, loadStats } from "../meshCache";
import { targetIds, useStore, type AppState, type MeshData, type Part } from "../store";
import "./info.css";

/** Meshes whose stats could not be measured (plain browser, or Rust failed): shown as "—", not retried. */
const unavailable = new WeakSet<MeshData>();

const statsOf: StatsLookup = (mesh) => cachedStats(mesh) ?? (unavailable.has(mesh) ? null : undefined);

interface Slice {
  /** The selected parts, or all visible parts when nothing is selected. */
  targets: Part[];
  selected: number;
  total: number;
}

function readSlice(s: AppState): Slice {
  const ids = new Set(targetIds(s));
  return { targets: s.parts.filter((p) => ids.has(p.id)), selected: s.selection.length, total: s.parts.length };
}

/**
 * The store slice the panel shows, refreshed at most once per animation frame: a gizmo drag
 * replaces the moved parts on every pointer move, and the panel only needs to keep up with the screen.
 */
function useFrameSlice(): Slice {
  const [slice, setSlice] = useState(() => readSlice(useStore.getState()));
  useEffect(() => {
    let frame = 0;
    const refresh = () => {
      frame = 0;
      setSlice(readSlice(useStore.getState()));
    };
    const unsubscribe = useStore.subscribe((s, prev) => {
      if (!frame && (s.parts !== prev.parts || s.selection !== prev.selection)) frame = requestAnimationFrame(refresh);
    });
    // Catch a change between the first render and this subscription.
    frame = requestAnimationFrame(refresh);
    return () => {
      unsubscribe();
      cancelAnimationFrame(frame);
    };
  }, []);
  return slice;
}

export function InfoPanel() {
  const { targets, selected, total } = useFrameSlice();
  // Bumped when stats or a background area arrive; both live in caches outside React.
  const [version, bump] = useReducer((n: number) => n + 1, 0);

  // The heavy parts are cached: stats per mesh, bounds per Part object, exact areas per Part object
  // and per mesh + linear part. During a drag this redoes only what the new matrices affect.
  const info = useMemo(() => infoRows(targets, statsOf), [targets, version]);

  useEffect(() => {
    let live = true;
    for (const { mesh } of targets) {
      if (statsOf(mesh) !== undefined) continue;
      void loadStats(mesh).then((stats) => {
        if (!stats) unavailable.add(mesh);
        if (live) bump();
      });
    }
    return () => {
      live = false;
    };
  }, [targets]);

  // Exact areas that did not fit in the render budget are measured in slices. A move keeps the
  // meshes and linear parts, so the run carries on; a new linear part (scale, rotate) restarts it.
  const [measuring, setMeasuring] = useState(info.pendingArea);
  if (measuring !== info.pendingArea && !sameAreaWork(measuring, info.pendingArea)) setMeasuring(info.pendingArea);
  useEffect(() => {
    if (measuring.length === 0) return;
    return computeAreasInBackground(measuring, bump);
  }, [measuring]);

  return (
    <>
      <dl className="rpanel-info">
        {info.rows.map((row) => (
          <div key={row.label}>
            <dt>{row.label}</dt>
            <dd className={row.status === "value" ? "info-value" : undefined}>
              {row.warning && (
                <span className="info-warning" role="img" title={row.warning} aria-label={row.warning}>
                  <TriangleAlert size={13} aria-hidden="true" />
                </span>
              )}
              {row.text}
            </dd>
          </div>
        ))}
      </dl>
      <p className="rpanel-muted">{selectionText(selected, total)}</p>
    </>
  );
}
