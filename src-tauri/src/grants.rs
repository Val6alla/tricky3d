//! The files the window may have read or written. The file commands take a path from the webview, so
//! script running there (a bug, or something worse) could otherwise read or overwrite any file the
//! user can. They only accept paths the user chose: picked in one of the app's own file dialogs
//! (commands::pick_files, pick_open_file, pick_save_file) or dropped on the window. Picking or dropping
//! a file to read never lets the window write it; only a save pick, or a 3MF picked in the File › Open
//! dialog and then opened as a project, does. An Export picked for one file per part lets the window
//! write mesh files named after its parts next to the file picked (jobs::export_each). Grants last
//! until the app quits and are never stored.
//!
//! Save dialogs may also open in a folder a project names as where its parts were imported from, so
//! Save and Export still open there after the project is reopened. Rust reads those from the file,
//! never from the window, and keeps only folders on a local drive: a project from elsewhere cannot
//! make a dialog reach out to a network host.
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
    /// Folders an opened project names as its parts' sources (local drives only).
    folders: HashSet<PathBuf>,
    /// Files picked in the Export dialog for one file per part: their folders take the parts' files.
    each: HashSet<PathBuf>,
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

    /// A file picked in the Export dialog for one file per part. The file itself is not written; its
    /// folder takes one file per part, and `check_each` lets that export through.
    pub fn allow_each(&self, path: impl Into<PathBuf>) {
        self.sets().each.insert(path.into());
    }

    /// The folders of the files an opened project's parts were imported from, read from the project
    /// by Rust. Only absolute paths on a local drive are kept.
    pub fn remember_source_folders(&self, sources: &[PathBuf]) {
        let mut sets = self.sets();
        for source in sources {
            if let Some(folder) = source.parent()
                && is_local_folder(folder)
            {
                sets.folders.insert(folder.to_path_buf());
            }
        }
    }

    /// True when some granted file lives in `folder`, or an opened project named it as a source
    /// folder, so a save dialog may open there. Any other folder the window names is not touched at
    /// all: even a look at a network path reaches out to that host.
    pub fn knows_folder(&self, folder: &Path) -> bool {
        let sets = self.sets();
        sets.folders.contains(folder)
            || sets
                .read
                .iter()
                .chain(&sets.write)
                .any(|p| p.parent() == Some(folder))
    }

    /// The first of `candidates` (in order of preference) that `knows_folder` and that still exists,
    /// for a save dialog to open in. Only known folders are looked at: nothing about any other folder
    /// is looked up, since even checking whether a network path exists reaches out to that host.
    pub fn dialog_folder<'p>(
        &self,
        candidates: impl IntoIterator<Item = &'p PathBuf>,
    ) -> Option<PathBuf> {
        candidates
            .into_iter()
            .find(|folder| {
                folder.components().count() > 0 && self.knows_folder(folder) && folder.is_dir()
            })
            .cloned()
    }

    /// Accepts a path picked for one file per part (`allow_each`).
    pub fn check_each(&self, path: &Path) -> Result<(), String> {
        if self.sets().each.contains(path) {
            return Ok(());
        }
        Err(format!(
            "Could not export to the folder of \"{}\": tricky3D only writes to folders you pick in the Export dialog.",
            display_name(path)
        ))
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

/// An absolute folder on a local drive: on Windows a drive letter ("C:\models"), never a share
/// ("\\host\share") or a device path; elsewhere any absolute path.
fn is_local_folder(folder: &Path) -> bool {
    if !folder.is_absolute() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::path::{Component, Prefix};
        matches!(
            folder.components().next(),
            Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_))
        )
    }
    #[cfg(not(windows))]
    {
        true
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
    fn one_file_per_part_needs_a_pick_for_it_and_writes_nothing_else() {
        let grants = Grants::default();
        let picked = Path::new("C:/out/gun.stl");
        assert!(grants.check_each(picked).is_err());
        grants.allow_write(picked);
        // A plain save pick is not a pick for one file per part.
        assert!(grants.check_each(picked).is_err());
        let each = Path::new("C:/out/parts/gun.stl");
        grants.allow_each(each);
        assert!(grants.check_each(each).is_ok());
        assert!(
            grants
                .check_each(Path::new("C:/out/parts/other.stl"))
                .is_err()
        );
        // Nor does it let the window write or read the file picked.
        assert!(grants.check_write(each, "stl", "write").is_err());
        assert!(grants.check_read(each).is_err());
    }

    #[test]
    fn save_dialogs_only_open_in_folders_of_granted_files() {
        let grants = Grants::default();
        assert!(!grants.knows_folder(Path::new("C:/jobs")));
        grants.allow_read("C:/jobs/plate.stl");
        assert!(grants.knows_folder(Path::new("C:/jobs")));
        assert!(!grants.knows_folder(Path::new("//host/share")));
    }

    #[test]
    fn a_project_teaches_the_source_folders_on_local_drives_only() {
        let grants = Grants::default();
        let local = if cfg!(windows) {
            r"C:\parts\gun.stl"
        } else {
            "/parts/gun.stl"
        };
        grants.remember_source_folders(&[
            PathBuf::from(local),
            PathBuf::from(r"\\host\share\gun.stl"),
            PathBuf::from("relative/gun.stl"),
        ]);
        assert!(grants.knows_folder(Path::new(local).parent().unwrap()));
        assert!(!grants.knows_folder(Path::new(r"\\host\share")));
        assert!(!grants.knows_folder(Path::new("relative")));
        // A known folder never lets the window read or write anything in it.
        assert!(grants.check_read(Path::new(local)).is_err());
        assert!(
            grants
                .check_write(Path::new(local), "stl", "write")
                .is_err()
        );
    }

    #[test]
    fn save_dialogs_open_in_the_source_folder_else_the_last_one_used_while_they_exist() {
        let root = std::env::temp_dir()
            .join("tricky3d-tests")
            .join(format!("dialog-folder-{}", std::process::id()));
        let source = root.join("imported from");
        let last = root.join("exported to");
        let unknown = root.join("never picked");
        for folder in [&source, &last, &unknown] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let grants = Grants::default();
        grants.allow_read(source.join("gun.stl"));
        grants.allow_write(last.join("stand.stl"));
        // The part's source folder first, even when the last export went elsewhere.
        assert_eq!(grants.dialog_folder([&source, &last]), Some(source.clone()));
        // A part that never came from a file: the last folder used.
        assert_eq!(grants.dialog_folder([&last]), Some(last.clone()));
        // A folder no file was picked in is never opened, nor looked at.
        assert_eq!(grants.dialog_folder([&unknown, &last]), Some(last.clone()));
        // The source folder is gone: the last folder used.
        std::fs::remove_dir_all(&source).unwrap();
        assert_eq!(grants.dialog_folder([&source, &last]), Some(last.clone()));
        // Neither is left: the system's choice.
        std::fs::remove_dir_all(&last).unwrap();
        assert_eq!(grants.dialog_folder([&source, &last]), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
