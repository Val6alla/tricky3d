// Repair panel › View: how the working copy is drawn (slim faces) and the Surface selection tool's tolerance.
import { useStore } from "../../store";
import { useRepair } from "../store";
import { SliderField } from "./controls";

/** Slim faces: the smallest angle of a triangle is at most 60° (equilateral), so the slider ends there. */
export const SLIM_ANGLE_MAX = 60;
/** Surface selection crosses edges that bend by up to this many degrees; 180° crosses every edge. */
export const SURFACE_TOLERANCE_MAX = 180;

export function ViewTab() {
  const view = useRepair((s) => s.view);
  const hideSoon = useStore((s) => s.hideSoon);
  const setView = useRepair((s) => s.setView);
  const slim = view.slimColored || view.slimEdges;

  return (
    <div className="rp-body">
      {!hideSoon && (
        <section className="rp-group" aria-label="Part">
          <h3 className="rp-title">Part</h3>
          <div className="rp-checks">
            {["Show textures", "Show transparent part"].map((label) => (
              <label key={label} className="clip-check rp-check is-soon" title={`${label} is coming in a future version.`}>
                <input type="checkbox" disabled checked={false} readOnly />
                {label}
                <span className="soon-pill">Soon</span>
              </label>
            ))}
          </div>
        </section>
      )}
      <section className="rp-group" aria-label="Show slim faces">
        <h3 className="rp-title">Show slim faces</h3>
        <div className="rp-checks">
          <label className="clip-check rp-check" title="Draw triangles with a smallest angle below the limit in blue">
            <input type="checkbox" checked={view.slimColored} onChange={(e) => setView({ slimColored: e.target.checked })} />
            <span className="rp-swatch rp-swatch-slim" aria-hidden="true" />
            Colored
          </label>
          <label className="clip-check rp-check" title="Draw the edges of slim triangles">
            <input type="checkbox" checked={view.slimEdges} onChange={(e) => setView({ slimEdges: e.target.checked })} />
            <span className="rp-swatch rp-swatch-slim is-line" aria-hidden="true" />
            Highlight edges
          </label>
        </div>
        <SliderField
          label="Smallest angle"
          value={view.slimAngle}
          min={0}
          max={SLIM_ANGLE_MAX}
          step={1}
          unit="°"
          disabled={!slim}
          onChange={(slimAngle) => setView({ slimAngle })}
        />
        <p className="rpanel-muted">Slim faces are triangles whose smallest angle is below this.</p>
      </section>
      <section className="rp-group" aria-label="Surface selection">
        <h3 className="rp-title">Surface selection</h3>
        <SliderField
          label="Tolerance"
          value={view.surfaceTolerance}
          min={0}
          max={SURFACE_TOLERANCE_MAX}
          step={1}
          unit="°"
          onChange={(surfaceTolerance) => setView({ surfaceTolerance })}
        />
        <p className="rpanel-muted">The Surface tool grows across edges that bend by at most this angle; 180° takes the whole connected surface.</p>
      </section>
    </div>
  );
}
