// Part library › the gallery over the view: the four categories, and in Primitive a tile per primitive with its picture
// (src/library/thumbnails.ts). A click picks a tile, a double-click creates the part.
import { useLibrary, CATEGORIES } from "./store";
import { createPart, pickCategory, pickPrimitive } from "./session";
import { THUMB_HEIGHT, THUMB_WIDTH } from "./thumbnails";
import "./library.css";

export function LibraryBrowser() {
  const session = useLibrary((s) => s.session);
  const catalog = useLibrary((s) => s.catalog);
  const thumbs = useLibrary((s) => s.thumbs);
  if (!session || !catalog) return null;
  const category = CATEGORIES.find((c) => c.id === session.category);

  return (
    <div className="lib-browser" role="region" aria-label="Part library">
      <div className="lib-categories" role="tablist" aria-label="Categories">
        {CATEGORIES.map((c) => (
          <button
            key={c.id}
            type="button"
            role="tab"
            aria-selected={c.id === session.category}
            className={`lib-category${c.id === session.category ? " is-selected" : ""}`}
            onClick={() => pickCategory(c.id)}
          >
            {c.label}
            {!c.ready && <span className="soon-pill">Soon</span>}
          </button>
        ))}
      </div>
      {category?.ready ? (
        <div className="lib-tiles" role="listbox" aria-label="Primitives">
          {catalog.map((p) => (
            <button
              key={p.id}
              type="button"
              role="option"
              aria-selected={p.id === session.primitive}
              className={`lib-tile${p.id === session.primitive ? " is-selected" : ""}`}
              title={`${p.label}. Double-click to create it.`}
              onClick={() => pickPrimitive(p.id)}
              onDoubleClick={() => {
                pickPrimitive(p.id);
                void createPart();
              }}
            >
              <span className="lib-thumb" style={{ width: THUMB_WIDTH, height: THUMB_HEIGHT }}>
                {thumbs[p.id] ? <img src={thumbs[p.id]} alt="" width={THUMB_WIDTH} height={THUMB_HEIGHT} /> : <span className="lib-thumb-wait" aria-hidden />}
              </span>
              <span className="lib-tile-label">{p.label}</span>
            </button>
          ))}
        </div>
      ) : (
        <p className="lib-soon">{category?.label} parts are coming in a future version.</p>
      )}
    </div>
  );
}
