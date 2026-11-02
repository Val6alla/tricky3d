//! File › Recent files: the last ten files opened, imported or saved, newest first, kept in
//! `recent-files.json` in the app's settings folder. Only Rust adds to the list, from the file
//! commands, after they read or wrote a file the user picked (src/grants.rs), so the window cannot
//! put a path of its own choosing on it; opening an entry grants that file again. A file that is
//! no longer there drops off the list the next time it is read.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const FILE: &str = "recent-files.json";
/// Entries kept.
pub const MAX: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A tricky3D project (.3mf), opened with File › Open or saved.
    Project,
    /// A mesh file, imported.
    Mesh,
}

/// `RecentFile` in src/io/recent.ts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: Kind,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    files: Vec<Entry>,
}

fn read(dir: &Path) -> Vec<Entry> {
    fs::read(dir.join(FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).ok())
        .map(|s| s.files)
        .unwrap_or_default()
}

fn write(dir: &Path, files: &[Entry]) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("Could not keep the recent files: {e}."))?;
    let stored = Stored {
        files: files.to_vec(),
    };
    let json = serde_json::to_vec_pretty(&stored).map_err(|e| e.to_string())?;
    crate::formats::write_atomic(&dir.join(FILE), |mut w| {
        use std::io::Write;
        w.write_all(&json)?;
        Ok(w)
    })
    .map_err(|e| format!("Could not keep the recent files: {e}."))
}

/// Puts `path` first (once), keeping at most MAX.
pub fn add(dir: &Path, path: &Path, kind: Kind) -> Result<(), String> {
    let mut files = read(dir);
    // Compared as paths: the same file written with "/" or "\" is one entry.
    files.retain(|e| e.path.as_path() != path);
    files.insert(
        0,
        Entry {
            path: path.to_path_buf(),
            kind,
        },
    );
    files.truncate(MAX);
    write(dir, &files)
}

/// The list, newest first, without the files that are no longer there (which also go from the
/// stored list).
pub fn list(dir: &Path) -> Vec<Entry> {
    let files = read(dir);
    let kept: Vec<Entry> = files.iter().filter(|e| e.path.is_file()).cloned().collect();
    if kept.len() != files.len() {
        let _ = write(dir, &kept);
    }
    kept
}

/// The entry for `path` when it is on the list and still there.
pub fn find(dir: &Path, path: &Path) -> Option<Entry> {
    list(dir).into_iter().find(|e| e.path.as_path() == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_once_ten_at_most_and_missing_files_drop_off() {
        let dir = crate::test_dir::temp_dir("recent");
        let settings = dir.join("settings");
        let files: Vec<PathBuf> = (0..12)
            .map(|k| {
                let f = dir.join(format!("part {k}.stl"));
                fs::write(&f, b"solid").unwrap();
                f
            })
            .collect();
        for f in &files {
            add(&settings, f, Kind::Mesh).unwrap();
        }
        let got = list(&settings);
        assert_eq!(got.len(), MAX);
        assert_eq!(got[0].path, files[11]);
        assert_eq!(got[9].path, files[2]);
        // Opening one again moves it to the top, once.
        add(&settings, &files[5], Kind::Mesh).unwrap();
        let got = list(&settings);
        assert_eq!(got[0].path, files[5]);
        assert_eq!(got.iter().filter(|e| e.path == files[5]).count(), 1);
        assert_eq!(got.len(), MAX);
        // A project saved over a mesh of the same path takes its kind.
        add(&settings, &files[3], Kind::Project).unwrap();
        assert_eq!(list(&settings)[0].kind, Kind::Project);
        // Deleted files drop off, from the stored list too.
        fs::remove_file(&files[11]).unwrap();
        fs::remove_file(&files[5]).unwrap();
        let got = list(&settings);
        assert_eq!(got.len(), MAX - 2);
        assert!(
            got.iter()
                .all(|e| e.path != files[11] && e.path != files[5])
        );
        assert_eq!(read(&settings).len(), MAX - 2);
        assert!(find(&settings, &files[3]).is_some());
        assert!(find(&settings, &files[11]).is_none());
        assert!(find(&settings, &dir.join("never opened.stl")).is_none());
        // A damaged list is an empty one.
        fs::write(settings.join(FILE), b"{not json").unwrap();
        assert!(list(&settings).is_empty());
    }
}
