/**
 * The app was called Meshbench before it became tricky3D, and its settings were saved under "meshbench." keys. On the
 * first start after the rename, src-tauri/src/migrate.rs copies the old window storage into the new app's folder, and
 * this moves each old setting to its "tricky3d." key, unless that key already has a value. main.tsx imports this
 * module first, so it runs before anything reads a setting. Running it again finds nothing left to move.
 */

const OLD_PREFIX = "meshbench.";
const NEW_PREFIX = "tricky3d.";

type KeyValueStorage = Pick<Storage, "length" | "key" | "getItem" | "setItem" | "removeItem">;

export function renameOldSettings(storage: KeyValueStorage): void {
  const old: string[] = [];
  for (let i = 0; i < storage.length; i++) {
    const key = storage.key(i);
    if (key?.startsWith(OLD_PREFIX)) old.push(key);
  }
  for (const key of old) {
    const value = storage.getItem(key);
    const renamed = NEW_PREFIX + key.slice(OLD_PREFIX.length);
    // The new key is written before the old one goes, so a crash in between loses nothing.
    if (value !== null && storage.getItem(renamed) === null) storage.setItem(renamed, value);
    storage.removeItem(key);
  }
}

try {
  if (typeof localStorage !== "undefined") renameOldSettings(localStorage);
} catch {
  // Storage can be unavailable; the app then starts with its default settings.
}
