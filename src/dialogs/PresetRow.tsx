// Manage settings: pick a saved preset to load it; save the dialog's settings as a new preset or over the picked one;
// delete it. The Pack dialog, the analysis dialogs and the mesh tools share it, each with its own store and scope.
import { useEffect, useId, useState } from "react";
import { errorMessage } from "../io/files";
import { toast } from "../ui";
import { presetNames, presetsWith, presetsWithout, type PresetMap, type PresetStore, type PresetValues } from "./presets";

/** Loaded once per store while the app runs, so a second dialog opens with the list at once. */
const cache = new Map<PresetStore, PresetMap>();

export function PresetRow({
  store,
  scope,
  values,
  onLoad,
  title = "Manage settings",
}: {
  store: PresetStore;
  /** The packing method, the analysis or the mesh tool the presets belong to. */
  scope: string;
  values: PresetValues;
  onLoad(saved: PresetValues): void;
  title?: string;
}) {
  const [presets, setPresets] = useState<PresetMap | null>(() => cache.get(store) ?? null);
  const [picked, setPicked] = useState("");
  const [naming, setNaming] = useState<string | null>(null);
  const id = useId();
  const mine = presets?.[scope] ?? {};
  const names = presetNames(presets, scope);

  useEffect(() => {
    let live = true;
    store
      .load()
      .then((p) => {
        cache.set(store, p);
        if (live) setPresets(p);
      })
      .catch((err: unknown) => {
        if (!live) return;
        setPresets({});
        toast(errorMessage(err, `Could not read your ${store.what}`), "error");
      });
    return () => {
      live = false;
    };
  }, [store]);
  // Another scope has other presets.
  useEffect(() => setPicked(""), [scope]);

  async function keep(next: PresetMap, pick: string) {
    try {
      await store.save(next);
      cache.set(store, next);
      setPresets(next);
      setPicked(pick);
    } catch (err) {
      toast(errorMessage(err, `Could not save your ${store.what}`), "error");
    }
  }

  function addPreset() {
    const name = (naming ?? "").trim();
    if (!name || !presets) return;
    setNaming(null);
    void keep(presetsWith(presets, scope, name, values), name);
  }

  return (
    <section className="dlg-section" aria-label={title}>
      <h3 className="dlg-section-title">{title}</h3>
      {naming === null ? (
        <div className="arr-presets">
          <select
            id={id}
            aria-label="Preset"
            className="dlg-input dlg-select"
            value={picked}
            disabled={!presets}
            onChange={(e) => {
              setPicked(e.target.value);
              if (e.target.value && mine[e.target.value]) onLoad(mine[e.target.value]);
            }}
          >
            <option value="">New setting</option>
            {names.map((n) => (
              <option key={n} value={n}>
                {n}
              </option>
            ))}
          </select>
          <button type="button" className="dlg-btn" disabled={!presets} onClick={() => setNaming("")} title="Save these settings as a new preset.">
            Save as…
          </button>
          <button
            type="button"
            className="dlg-btn"
            disabled={!presets || !picked}
            onClick={() => presets && void keep(presetsWith(presets, scope, picked, values), picked)}
            title="Save these settings over the picked preset."
          >
            Save
          </button>
          <button
            type="button"
            className="dlg-btn"
            disabled={!presets || !picked}
            onClick={() => presets && void keep(presetsWithout(presets, scope, picked), "")}
            title="Delete the picked preset."
          >
            Delete
          </button>
        </div>
      ) : (
        <div className="arr-presets">
          <input
            aria-label="Preset name"
            className="dlg-input"
            type="text"
            maxLength={80}
            placeholder="Name of the preset"
            value={naming}
            onChange={(e) => setNaming(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                e.stopPropagation();
                addPreset();
              }
            }}
          />
          <button type="button" className="dlg-btn" disabled={!naming.trim()} onClick={addPreset}>
            Add
          </button>
          <button type="button" className="dlg-btn" onClick={() => setNaming(null)}>
            Back
          </button>
        </div>
      )}
    </section>
  );
}
