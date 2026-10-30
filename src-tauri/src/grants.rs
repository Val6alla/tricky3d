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
//!
//! Folders, for where a save dialog opens, are compared as folders instead (`folder_key`): the same
//! folder may come back spelled another way, with "/" for "\", a trailing separator, another letter
//! case or, on Windows, a short 8.3 name ("C:\Users\RUNNER~1" for "C:\Users\runneradmin").

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
    /// Files picked in the Export dialog for one file per part: their folders take the parts' files.
    each: HashSet<PathBuf>,
    /// The folders of every file above, and those an opened project names as its parts' sources
    /// (local drives only), as `folder_key` spells them.
    folders: HashSet<PathBuf>,
}

/// The `folder_key` of the folder `path` is in, if it names one.
fn folder_of(path: &Path) -> Option<PathBuf> {
    path.parent()
        .filter(|f| f.components().count() > 0)
        .map(folder_key)
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
        let path = path.into();
        // Looked up before taking the lock: it may ask the disk for the folder's long name.
        let folder = folder_of(&path);
        let mut sets = self.sets();
        sets.folders.extend(folder);
        sets.read.insert(path);
    }

    /// A 3MF picked in the File › Open dialog: readable now, writable once it opened as a project.
    pub fn allow_project(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let folder = folder_of(&path);
        let mut sets = self.sets();
        sets.folders.extend(folder);
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
        let path = path.into();
        let folder = folder_of(&path);
        let mut sets = self.sets();
        sets.folders.extend(folder);
        sets.write.insert(path);
    }

    /// A file picked in the Export dialog for one file per part. The file itself is not written; its
    /// folder takes one file per part, and `check_each` lets that export through. The folder is the
    /// last one used from then on, so later save dialogs may open there.
    pub fn allow_each(&self, path: impl Into<PathBuf>) {
        let path = path.into();
        let folder = folder_of(&path);
        let mut sets = self.sets();
        sets.folders.extend(folder);
        sets.each.insert(path);
    }

    /// The folders of the files an opened project's parts were imported from, read from the project
    /// by Rust. Only absolute paths on a local drive are kept.
    pub fn remember_source_folders(&self, sources: &[PathBuf]) {
        let folders: Vec<PathBuf> = sources
            .iter()
            .filter_map(|source| source.parent())
            .filter(|folder| is_local_folder(folder))
            .map(folder_key)
            .collect();
        self.sets().folders.extend(folders);
    }

    /// True when some granted file lives in `folder`, or an opened project named it as a source
    /// folder, however either is spelled (`folder_key`), so a save dialog may open there.
    pub fn knows_folder(&self, folder: &Path) -> bool {
        let key = folder_key(folder);
        self.sets().folders.contains(&key)
    }

    /// The first of `candidates` (in order of preference) that `knows_folder` and that still exists,
    /// for a save dialog to open in. Only known folders are checked for existence; of any other, at
    /// most a long name is looked up on a local fixed disk (`folder_key`), since even checking
    /// whether a network path exists reaches out to that host.
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

/// One spelling per folder, so two paths of the same folder compare equal: built from its components,
/// so "/" or "\" and trailing separators do not matter, and on Windows with every short 8.3 name
/// made long and in one letter case, as NTFS ignores case. Only a folder on a local fixed disk is
/// looked up for its long names (one that is gone keeps the names it has); any other, a share or a
/// mapped network drive among them, is compared as written, so a path the window names never makes
/// tricky3D reach out to a network host. Elsewhere letter case counts: a Linux or macOS volume may
/// tell "Models" from "models".
pub(crate) fn folder_key(folder: &Path) -> PathBuf {
    let written: PathBuf = folder.components().collect();
    #[cfg(windows)]
    {
        let path = if win::on_fixed_disk(&written) {
            win::long_name(&written).map_or(written, |long| long.components().collect())
        } else {
            written
        };
        match path.to_str() {
            Some(text) => PathBuf::from(text.chars().map(win::upper).collect::<String>()),
            None => path,
        }
    }
    #[cfg(not(windows))]
    written
}

#[cfg(windows)]
pub(crate) mod win {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::{Component, Path, PathBuf, Prefix};

    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLongPathNameW};
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_FIXED;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str().encode_wide().chain([0]).collect()
    }

    /// True when `path` starts with the letter of a fixed disk in this computer ("C:\"), not a
    /// network, removable or optical drive.
    pub(super) fn on_fixed_disk(path: &Path) -> bool {
        let Some(Component::Prefix(prefix)) = path.components().next() else {
            return false;
        };
        let Prefix::Disk(letter) = prefix.kind() else {
            return false;
        };
        let root = [u16::from(letter), u16::from(b':'), u16::from(b'\\'), 0];
        // SAFETY: a NUL-terminated wide string that lives through the call.
        unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_FIXED }
    }

    /// `path` with every short 8.3 name made long; None when it does not exist or cannot be read.
    pub(super) fn long_name(path: &Path) -> Option<PathBuf> {
        call(path, GetLongPathNameW)
    }

    /// `path` with its short 8.3 names, where the volume keeps them (tests make such paths).
    #[cfg(test)]
    pub(crate) fn short_name(path: &Path) -> Option<PathBuf> {
        call(
            path,
            windows_sys::Win32::Storage::FileSystem::GetShortPathNameW,
        )
    }

    type PathCall = unsafe extern "system" fn(*const u16, *mut u16, u32) -> u32;

    /// Runs a call that writes a path into a buffer and says the size it needs when it is too small.
    fn call(path: &Path, f: PathCall) -> Option<PathBuf> {
        let wide = wide(path);
        let mut buf = vec![0u16; 260];
        for _ in 0..2 {
            let len = u32::try_from(buf.len()).ok()?;
            // SAFETY: `wide` is NUL-terminated and `buf` holds `len` units; both outlive the call.
            let n = unsafe { f(wide.as_ptr(), buf.as_mut_ptr(), len) } as usize;
            if n == 0 {
                return None;
            }
            if n < buf.len() {
                buf.truncate(n);
                return Some(PathBuf::from(OsString::from_wide(&buf)));
            }
            // Too small: `n` is the size it needs.
            buf.resize(n, 0);
        }
        None
    }

    /// The letter as NTFS compares names: its upper case, where that is a single letter.
    pub(super) fn upper(c: char) -> char {
        let mut up = c.to_uppercase();
        match (up.next(), up.next()) {
            (Some(u), None) => u,
            _ => c,
        }
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
    fn a_known_folder_is_known_however_it_is_spelled() {
        let grants = Grants::default();
        if cfg!(windows) {
            grants.allow_read(r"C:\Models\Kits\gun.stl");
            for same in [
                r"C:\Models\Kits",
                r"C:\Models\Kits\",
                "C:/Models/Kits/",
                r"c:\models\KITS",
                r"C:\Models\.\Kits",
            ] {
                assert!(grants.knows_folder(Path::new(same)), "{same}");
            }
            assert!(!grants.knows_folder(Path::new(r"C:\Models")));
            assert!(!grants.knows_folder(Path::new(r"C:\Models\Kits2")));
            // A share is compared as written, without asking the network, in any letter case.
            grants.allow_read(r"\\host\share\parts\gun.stl");
            assert!(grants.knows_folder(Path::new(r"\\HOST\Share\parts\")));
        } else {
            grants.allow_read("/models/Kits/gun.stl");
            for same in [
                "/models/Kits",
                "/models/Kits/",
                "/models//Kits",
                "/models/./Kits",
            ] {
                assert!(grants.knows_folder(Path::new(same)), "{same}");
            }
            // Letter case counts where the volume may tell names apart by it.
            assert!(!grants.knows_folder(Path::new("/models/kits")));
        }
    }

    #[test]
    fn a_folder_picked_for_one_file_per_part_is_known_from_then_on() {
        let grants = Grants::default();
        let picked = if cfg!(windows) {
            r"C:\out\parts\gun.stl"
        } else {
            "/out/parts/gun.stl"
        };
        assert!(!grants.knows_folder(Path::new(picked).parent().unwrap()));
        grants.allow_each(picked);
        assert!(grants.knows_folder(Path::new(picked).parent().unwrap()));
    }

    /// A folder named by its short 8.3 name is the folder of its long name, both ways round. Windows
    /// runners have such names in their temp folder ("C:\Users\RUNNER~1"); on a volume that keeps no
    /// short names both spellings are the same and the test still checks the long one.
    #[cfg(windows)]
    #[test]
    fn short_8_3_names_name_the_same_folder() {
        let root = crate::test_dir::temp_dir("short-names");
        let long = root.join("a folder with a long name");
        std::fs::create_dir_all(&long).unwrap();
        let short = win::short_name(&long).unwrap();
        let long = win::long_name(&long).unwrap();

        let grants = Grants::default();
        grants.allow_read(long.join("gun.stl"));
        assert!(grants.knows_folder(&short), "{short:?} is {long:?}");
        assert_eq!(grants.dialog_folder([&short]), Some(short.clone()));
        let grants = Grants::default();
        grants.allow_read(short.join("gun.stl"));
        assert!(grants.knows_folder(&long), "{long:?} is {short:?}");
        assert_eq!(folder_key(&short), folder_key(&long));
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
        let root = crate::test_dir::temp_dir("dialog-folder");
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
    }
}
