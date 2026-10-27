//! The files the window may have read or written. The file commands take a path from the webview, so
//! script running there (a bug, or something worse) could otherwise read or overwrite any file the
//! user can. They only accept paths the user chose: picked in one of the app's own file dialogs
//! (commands::pick_files, pick_open_file, pick_save_file) or dropped on the window. Picking or dropping
//! a file to read never lets the window write it; only a save pick, or a 3MF picked in the File › Open
//! dialog and then opened as a project, does. Grants last until the app quits and are never stored.
//!
//! Paths are compared exactly as the dialog or the drop returned them. The window gets that text and
//! sends the same text back, so nothing may rewrite it on the way: no canonicalizing, and no Unicode
//! normalization (macOS file names often hold "й" as "и" plus a combining mark).

use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use crate::formats::display_name;

#[derive(Default)]
pub struct Grants {
    sets: Mutex<Sets>,
}

#[derive(Default)]
struct Sets {
    read: HashSet<PathBuf>,
    write: HashSet<PathBuf>,
    /// 3MFs picked in the File › Open dialog, whose title Rust sets: may become writable.
    projects: HashSet<PathBuf>,
}

impl Grants {
    fn sets(&self) -> MutexGuard<'_, Sets> {
        // The sets stay valid even if a thread panicked while holding the lock.
        self.sets
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A file picked in an open dialog or dropped on the window.
    pub fn allow_read(&self, path: impl Into<PathBuf>) {
        self.sets().read.insert(path.into());
    }

    /// A 3MF picked in the File › Open dialog: readable now, writable once it opened as a project.
    pub fn allow_project(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let mut sets = self.sets();
        sets.read.insert(path.clone());
        sets.projects.insert(path);
    }

    /// After `path` opened as a project: Save writes it back, but only if it was picked as a project.
    /// A file the user merely imported or dropped stays read-only even when it is a valid 3MF.
    pub fn adopt_project(&self, path: &Path) {
        let mut sets = self.sets();
        if sets.projects.contains(path) {
            sets.write.insert(path.to_path_buf());
        }
    }

    /// A file picked in a save dialog.
    pub fn allow_write(&self, path: impl Into<PathBuf>) {
        self.sets().write.insert(path.into());
    }

    /// True when some granted file lives in `folder`, so a save dialog may open there. Any other folder
    /// the window names is not touched at all: even a look at a network path reaches out to that host.
    pub fn knows_folder(&self, folder: &Path) -> bool {
        let sets = self.sets();
        sets.read
            .iter()
            .chain(&sets.write)
            .any(|p| p.parent() == Some(folder))
    }

    pub fn check_read(&self, path: &Path) -> Result<(), String> {
        if self.sets().read.contains(path) {
            return Ok(());
        }
        Err(format!(
            "Could not open \"{}\": tricky3D only opens files you pick in a file dialog or drop on its window.",
            display_name(path)
        ))
    }

    /// Accepts a granted path, or a granted file name with ".`extension`" added in the same folder: the
    /// window adds the format's extension when the name typed in the save dialog does not end in it.
    /// `verb` starts the message.
    pub fn check_write(&self, path: &Path, extension: &str, verb: &str) -> Result<(), String> {
        let sets = self.sets();
        let extended = |granted: &PathBuf| {
            let (Some(name), Some(want)) = (granted.file_name(), path.file_name()) else {
                return false;
            };
            let mut name = OsString::from(name);
            name.push(".");
            name.push(extension);
            !extension.is_empty() && want == name && granted.parent() == path.parent()
        };
        if sets.write.contains(path) || sets.write.iter().any(extended) {
            return Ok(());
        }
        Err(format!(
            "Could not {verb} \"{}\": tricky3D only writes to files you pick in a save dialog.",
            display_name(path)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_granted_paths_pass() {
        let grants = Grants::default();
        let picked = Path::new("C:/models/part.stl");
        assert!(grants.check_read(picked).is_err());
        grants.allow_read(picked);
        assert!(grants.check_read(picked).is_ok());
        assert!(grants.check_read(Path::new("C:/models/other.stl")).is_err());
        assert!(grants.check_read(Path::new("C:/models")).is_err());
        // Reading does not let the window write.
        assert!(grants.check_write(picked, "stl", "write").is_err());
    }

    #[test]
    fn writes_allow_the_picked_name_with_the_format_extension_added() {
        let grants = Grants::default();
        grants.allow_write("C:/out/part");
        grants.allow_write("C:/out/project.3mf");
        let ok = |p: &str, ext: &str| grants.check_write(Path::new(p), ext, "write").is_ok();
        assert!(ok("C:/out/part", "stl"));
        assert!(ok("C:/out/part.stl", "stl"));
        assert!(ok("C:/out/project.3mf", "3mf"));
        assert!(!ok("C:/out/part.obj", "stl"));
        assert!(!ok("C:/out/part.", ""));
        // A file inside a folder named like the grant is not the grant with an extension.
        assert!(!ok("C:/out/part/.stl", "stl"));
        assert!(!ok("C:/elsewhere/part.stl", "stl"));
        let refused = grants
            .check_write(Path::new("C:/Windows/win.ini"), "stl", "save")
            .unwrap_err();
        assert_eq!(
            refused,
            "Could not save \"win.ini\": tricky3D only writes to files you pick in a save dialog."
        );
        // Writing does not let the window read.
        assert!(grants.check_read(Path::new("C:/out/project.3mf")).is_err());
    }

    #[test]
    fn only_a_project_pick_becomes_writable_when_opened() {
        let grants = Grants::default();
        let imported = Path::new("C:/models/plate.3mf");
        let project = Path::new("C:/jobs/plate.3mf");
        grants.allow_read(imported);
        grants.allow_project(project);
        assert!(grants.check_read(project).is_ok());
        assert!(grants.check_write(project, "3mf", "save").is_err());
        grants.adopt_project(imported);
        grants.adopt_project(project);
        assert!(grants.check_write(imported, "3mf", "save").is_err());
        assert!(grants.check_write(project, "3mf", "save").is_ok());
    }

    #[test]
    fn save_dialogs_only_open_in_folders_of_granted_files() {
        let grants = Grants::default();
        assert!(!grants.knows_folder(Path::new("C:/jobs")));
        grants.allow_read("C:/jobs/plate.stl");
        assert!(grants.knows_folder(Path::new("C:/jobs")));
        assert!(!grants.knows_folder(Path::new("//host/share")));
    }
}
