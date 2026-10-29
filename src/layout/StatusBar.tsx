import { useBoolean } from "../boolean/store";
import { BOOLEAN_TOOLS, EDIT_TOOLS, REPAIR_TOOLS, VIEW_TOOLS, measureStatus } from "../features";
import { labelStatus } from "../label/session";
import { useLabel } from "../label/store";
import { useMeasure } from "../measure/store";
import { formatCount } from "../info";
import { useRepair } from "../repair/store";
import { useStore, type Vec3 } from "../store";
import { useUi, type Task } from "../ui";
import "./layout.css";

/** Up to two decimals, without trailing zeros: 200, 200.5, 200.25. */
function formatMm(v: number): string {
  return String(Math.round(v * 100) / 100);
}

function formatPlatform([x, y, z]: Vec3): string {
  return `${formatMm(x)}×${formatMm(y)}×${formatMm(z)} mm`;
}

/** Repair mode's selection count: "1,204 of 762,014 triangles selected". */
export function triangleSelectionText(selected: number, total: number): string {
  return `${formatCount(selected)} of ${formatCount(total)} ${total === 1 ? "triangle" : "triangles"} selected`;
}

/** Busy indicator for running imports, exports and saves: the oldest task, its progress, and how many more. */
function TaskStatus({ tasks }: { tasks: Task[] }) {
  const [first] = tasks;
  const percent = first.progress === null ? null : Math.round(first.progress * 100);
  return (
    <span className="statusbar-task" role="status">
      <span className="statusbar-spinner" aria-hidden="true" />
      <span className="statusbar-task-label" title={first.label}>
        {first.label}
      </span>
      {/* Hidden from screen readers so they announce the task once, not every percent. */}
      {percent !== null && (
        <span className="statusbar-task-percent" aria-hidden="true">
          {percent}%
        </span>
      )}
      {tasks.length > 1 && <span className="statusbar-task-more">+{tasks.length - 1} more</span>}
    </span>
  );
}

/**
 * Platform size, the tool and its hint (or the hovered entry's), running tasks, and the selection count. In repair
 * mode the tool is the repair select tool and the count is of the working copy's triangles; in Boolean operations the
 * tool is the mode's.
 */
export function StatusBar() {
  const platform = useStore((s) => s.platform);
  const partTool = useStore((s) => s.tool);
  const selected = useStore((s) => s.selection.length);
  const total = useStore((s) => s.parts.length);
  const repairTool = useRepair((r) => (r.session ? r.selectTool : null));
  const editTool = useRepair((r) => (r.session ? r.editTool : null));
  const booleanTool = useBoolean((b) => (b.session ? b.tool : null));
  const selectedTriangles = useRepair((r) => r.session?.selectedCount ?? 0);
  const triangles = useRepair((r) => r.session?.selection.length ?? 0);
  const hoverHint = useUi((s) => s.hoverHint);
  const viewTool = useUi((s) => s.viewTool);
  // Re-renders as the tool and the clicks change; measureStatus reads the rest itself.
  useMeasure((m) => [m.session?.tool, m.session?.picks.length, m.session?.points.length].join());
  const measureTool = measureStatus(useMeasure.getState());
  // Label generation: what the next click does.
  useLabel((l) => `${l.session?.tool}:${l.session?.kind}`);
  const labelTool = labelStatus();
  const tasks = useUi((s) => s.tasks);
  const tool = viewTool ? VIEW_TOOLS[viewTool] : labelTool ? labelTool : measureTool ? measureTool : editTool ? EDIT_TOOLS[editTool] : repairTool ? REPAIR_TOOLS[repairTool] : booleanTool ? BOOLEAN_TOOLS[booleanTool] : partTool;
  const hint = hoverHint ?? tool.hint;

  return (
    <footer className="statusbar">
      <span className="statusbar-platform" title="Platform size">
        {formatPlatform(platform)}
      </span>
      <span className="statusbar-sep" aria-hidden="true" />
      <span className="statusbar-tool">{tool.name}</span>
      <span className="statusbar-hint" title={hint}>
        {hint}
      </span>
      {tasks.length > 0 && <TaskStatus tasks={tasks} />}
      {repairTool ? (
        <span className="statusbar-selection">{triangleSelectionText(selectedTriangles, triangles)}</span>
      ) : (
        total > 0 && (
          <span className="statusbar-selection">
            {selected} of {total} selected
          </span>
        )
      )}
    </footer>
  );
}
