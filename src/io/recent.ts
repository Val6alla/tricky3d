// File › Recent files: the last ten files opened, imported or saved, newest first. Rust keeps the list
// (src-tauri/src/recent.rs) and adds to it itself, so only files the user picked are on it; it drops files that are
// gone each time the list is read. The window keeps a copy for the menu, read again whenever the File menu opens.
import { isTauri } from "@tauri-apps/api/core";
import { create } from "zustand";
import { File, FileBox } from "lucide-react";
import type { Feature } from "../features";
import { fileName, folderOf, openRecentFile } from "./files";
import { grantRecent, recentFiles, type RecentFile } from "./ipc";

export const useRecent = create<{ files: RecentFile[] }>(() => ({ files: [] }));

/** Reads the list again (the File menu does as it opens). Keeps the old copy when Rust cannot answer. */
export async function refreshRecent(): Promise<void> {
  if (!isTauri()) return;
  try {
    useRecent.setState({ files: await recentFiles() });
  } catch {
    // The menu shows the list it had.
  }
}

/** Opens a recent file as File › Open would, then reads the list again (it moved to the top, or dropped off). */
export async function openRecent(path: string): Promise<void> {
  await openRecentFile(path, grantRecent);
  await refreshRecent();
}

/**
 * The submenu's rows: each file by name (its folder too when two share a name), its full path as the hint; a single
 * row saying so when the list is empty.
 */
export function recentRows(files: readonly RecentFile[], open: (path: string) => void = (p) => void openRecent(p)): Feature[] {
  if (files.length === 0) {
    return [
      {
        id: "file.recent.none",
        tab: "file",
        group: "Recent files",
        label: "No recent files",
        icon: File,
        status: "ready",
        hint: "Files you open, import or save show here.",
      },
    ];
  }
  const names = files.map((f) => fileName(f.path));
  return files.map((f, i): Feature => {
    const twice = names.filter((n) => n === names[i]).length > 1;
    const folder = folderOf(f.path).replace(/[\\/]+$/, "");
    return {
      id: `file.recent.${i}`,
      tab: "file",
      group: "Recent files",
      label: twice ? `${names[i]} (${fileName(folder) || folder})` : names[i],
      icon: f.kind === "project" ? FileBox : File,
      status: "ready",
      run: () => open(f.path),
      hint: f.path,
    };
  });
}
