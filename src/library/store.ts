// The Part library mode's state: the catalog from Rust, the open session (category, item, name and the values typed
// into the form), a part being made, the picked picture, and the tile pictures. src/library/session.ts is the only code
// that changes it.
import { create } from "zustand";
import type { LibraryCategory, LibraryItem, Picture } from "./ipc";

export type { LibraryCategory };

/** The categories in the order the library shows them, each with the item whose picture stands for it. */
export const CATEGORIES: { id: LibraryCategory; label: string; picture: string }[] = [
  { id: "primitive", label: "Primitive", picture: "sphere" },
  { id: "design", label: "Design", picture: "gear" },
  { id: "mathematical", label: "Mathematical", picture: "klein" },
  { id: "quality", label: "Quality", picture: "tensile" },
];

/** A field's value as the form holds it: text for numbers, texts and choices; true or false for toggles. */
export type FieldValue = string | boolean;

export interface LibrarySession {
  category: LibraryCategory;
  /** Id of the item picked in the tiles. */
  primitive: string;
  /** The new part's name. */
  name: string;
  /** True once the user typed a name: picking another item then keeps it. */
  nameEdited: boolean;
  /** The form's values per parameter key, as typed. */
  values: Record<string, FieldValue>;
  /** Triangles the picked item makes with the form's values; null while unknown (being built, or a value is wrong). */
  triangles: number | null;
}

/** Create part in progress. */
export interface LibraryBusy {
  job: number;
  cancelling: boolean;
  /** 0..1, from Rust's progress events. */
  fraction: number;
}

export interface LibraryState {
  /** Loaded once, the first time the library opens. */
  catalog: LibraryItem[] | null;
  /** Null outside the mode. */
  session: LibrarySession | null;
  busy: LibraryBusy | null;
  /** The picture picked for Bitmap, Heightmap and Picture sweep; null uses the built-in sample. Kept while the app runs. */
  picture: Picture | null;
  /** Tile pictures as image URLs, by item id. The picked tile's follows the form. */
  thumbs: Record<string, string>;
}

export const useLibrary = create<LibraryState>()(() => ({
  catalog: null,
  session: null,
  busy: null,
  picture: null,
  thumbs: {},
}));

export type { LibraryItem };
