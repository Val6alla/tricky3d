// Part library › the gallery over the view: a row of four large category buttons, each with a picture of one of its
// parts, and below it a grid of the category's tiles, each with its picture (src/library/thumbnails.ts) and name. A
// click picks a tile, a double-click creates the part. Items not available yet show as Soon and say why.
import { useLibrary, CATEGORIES } from "./store";
import { createPart, pickCategory, pickPrimitive } from "./session";
import { THUMB_HEIGHT, THUMB_WIDTH } from "./thumbnails";
import "./library.css";

export function LibraryBrowser() {
  const session = useLibrary((s) => s.session);
  const catalog = useLibrary((s) => s.catalog);
  const thumbs = useLibrary((s) => s.thumbs);
  if (!session || !catalog) return null;
  const items = catalog.filter((p) => p.category === session.category);
  const category = CATEGORIES.find((c) => c.id === session.category);

  return (
    <div className="lib-browser" role="region" aria-label="Part library">
      <div className="lib-categories" role="tablist" aria-label="Categories">
        {CATEGORIES.map((c) => {
          const picture = thumbs[c.picture];
          return (
            <button
              key={c.id}
              type="button"
              role="tab"
              aria-selected={c.id === session.category}
              className={`lib-category${c.id === session.category ? " is-selected" : ""}`}
              onClick={() => pickCategory(c.id)}
            >
              <span className="lib-category-picture" aria-hidden="true">
                {picture ? <img src={picture} alt="" /> : <span className="lib-thumb-wait" />}
              </span>
              <span className="lib-category-label">{c.label}</span>
            </button>
          );
        })}
      </div>
      <div className="lib-tiles" role="listbox" aria-label={`${category?.label ?? ""} parts`}>
        {items.map((p) => {
          const soon = p.soon !== null;
          const selected = p.id === session.primitive;
          return (
            <button
              key={p.id}
              type="button"
              role="option"
              aria-selected={selected}
              aria-disabled={soon || undefined}
              className={`lib-tile${selected ? " is-selected" : ""}${soon ? " is-soon" : ""}`}
              title={soon ? `${p.label} is not available yet: ${p.soon}` : `${p.description} Double-click to create it.`}
              onClick={() => pickPrimitive(p.id)}
              onDoubleClick={() => {
                if (soon) return;
                pickPrimitive(p.id);
                void createPart();
              }}
            >
              <span className="lib-thumb" style={{ width: THUMB_WIDTH, height: THUMB_HEIGHT }}>
                {soon ? (
                  <span className="soon-pill">Soon</span>
                ) : thumbs[p.id] ? (
                  <img src={thumbs[p.id]} alt="" width={THUMB_WIDTH} height={THUMB_HEIGHT} />
                ) : (
                  <span className="lib-thumb-wait" aria-hidden />
                )}
              </span>
              <span className="lib-tile-label">{p.label}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
