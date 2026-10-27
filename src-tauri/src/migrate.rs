//! One-time move from the app's old name. The app was called Meshbench, with the identifier
//! app.meshbench.desktop; as tricky3D it is app.tricky3d.desktop, and the operating system keeps
//! each identifier's files in folders of their own. On the first start under the new identifier,
//! before the window opens, this copies what the old folders hold into the new ones: the repair
//! scripts the user saved (repair-scripts.json in the config folder) and the window's local
//! storage, which holds the view settings and panel widths (src/settingsRename.ts then moves them
//! to their new keys).
//!
//! It only copies: the old folders stay as they are, and a new folder that exists already is left
//! alone, so once the app has run under the new name nothing is copied again. Each new folder is
//! put together under a temporary name and renamed into place when complete, so a crash halfway
//! leaves no half-copied folder, and the next start copies again.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tauri::{AppHandle, Manager, Runtime};

pub const OLD_IDENTIFIER: &str = "app.meshbench.desktop";
pub const IDENTIFIER: &str = "app.tricky3d.desktop";

/// One folder to carry over: `parts` (paths relative to the folder; "" is all of it) of
/// `base/OLD_IDENTIFIER` go to `base/IDENTIFIER`.
#[derive(Debug, PartialEq)]
pub struct Carry {
    pub base: PathBuf,
    pub parts: &'static [&'static str],
}

/// What to copy, given the parents of the app's own folders: `config` and `local_data` are where
/// Tauri's app_config_dir and app_local_data_dir live (the folders named after the identifier sit
/// inside them), and `home` is the user's home folder.
pub fn plan(config: &Path, local_data: &Path, home: &Path) -> Vec<Carry> {
    // The config folder holds only the repair scripts.
    let mut plan = vec![Carry {
        base: config.to_path_buf(),
        parts: &[""],
    }];
    if cfg!(windows) {
        // WebView2's profile is in the local data folder; only its local storage is needed.
        plan.push(Carry {
            base: local_data.to_path_buf(),
            parts: &["EBWebView/Default/Local Storage"],
        });
    } else if cfg!(target_os = "macos") {
        // The config and local data folders are the same one. WebKit keeps the window's storage
        // in a folder of its own, named after the identifier too.
        plan.push(Carry {
            base: home.join("Library/WebKit"),
            parts: &[""],
        });
    } else {
        // WebKitGTK's data lives in the local data folder.
        plan.push(Carry {
            base: local_data.to_path_buf(),
            parts: &[""],
        });
    }
    plan
}

/// Copies the old app's folders when the app runs under its release identifier for the first
/// time. Problems are reported on stderr and never stop the app from starting.
pub fn from_old_name<R: Runtime>(app: &AppHandle<R>) {
    // Builds with another identifier (development checks) never touch the user's folders.
    if app.config().identifier != IDENTIFIER {
        return;
    }
    let path = app.path();
    let parent = |dir: tauri::Result<PathBuf>| dir.ok()?.parent().map(Path::to_path_buf);
    let (Some(config), Some(local_data), Ok(home)) = (
        parent(path.app_config_dir()),
        parent(path.app_local_data_dir()),
        path.home_dir(),
    ) else {
        return;
    };
    for carry in plan(&config, &local_data, &home) {
        let (old, new) = (carry.base.join(OLD_IDENTIFIER), carry.base.join(IDENTIFIER));
        if let Err(e) = copy_once(&old, &new, carry.parts) {
            eprintln!(
                "tricky3d: could not copy {} to {}: {e}",
                old.display(),
                new.display()
            );
        }
    }
}

/// Copies `parts` of `old` into a new folder `new`, unless `new` exists already. Returns whether
/// anything was copied. A file that cannot be read (say, one the old app holds locked while it
/// runs) is left out; the folder is still put in place.
pub fn copy_once(old: &Path, new: &Path, parts: &[&str]) -> io::Result<bool> {
    if new.exists() || !old.is_dir() {
        return Ok(false);
    }
    let mut staging = new.as_os_str().to_owned();
    staging.push(".copying");
    let staging = PathBuf::from(staging);
    // Left by a start that stopped while copying.
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    for part in parts {
        let from = old.join(part);
        if from.is_dir() {
            copy_tree(&from, &staging.join(part))?;
        } else if from.is_file() {
            let to = staging.join(part);
            if let Some(dir) = to.parent() {
                fs::create_dir_all(dir)?;
            }
            copy_file(&from, &to);
        }
    }
    if !staging.exists() {
        return Ok(false);
    }
    fs::rename(&staging, new)?;
    Ok(true)
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        // Symbolic links are left out: copying them could reach outside the folder.
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            copy_file(&entry.path(), &target);
        }
    }
    Ok(())
}

fn copy_file(from: &Path, to: &Path) {
    if let Err(e) = fs::copy(from, to) {
        eprintln!("tricky3d: left out {}: {e}", from.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join("tricky3d-tests")
            .join(format!("migrate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap()
    }

    #[test]
    fn copies_the_old_folder_once_and_leaves_it_as_it_was() {
        let base = temp("whole");
        let (old, new) = (base.join(OLD_IDENTIFIER), base.join(IDENTIFIER));
        write(&old.join("repair-scripts.json"), "{\"scripts\": []}");
        write(&old.join("deep/er/file.bin"), "x");

        assert!(copy_once(&old, &new, &[""]).unwrap());
        assert_eq!(read(&new.join("repair-scripts.json")), "{\"scripts\": []}");
        assert_eq!(read(&new.join("deep/er/file.bin")), "x");
        assert_eq!(read(&old.join("repair-scripts.json")), "{\"scripts\": []}");
        assert!(!base.join(format!("{IDENTIFIER}.copying")).exists());

        // Once the new folder exists, nothing is copied into it again, and nothing in it changes.
        write(&new.join("repair-scripts.json"), "{\"scripts\": [1]}");
        write(&old.join("later.txt"), "later");
        assert!(!copy_once(&old, &new, &[""]).unwrap());
        assert_eq!(read(&new.join("repair-scripts.json")), "{\"scripts\": [1]}");
        assert!(!new.join("later.txt").exists());
    }

    #[test]
    fn copies_only_the_parts_asked_for() {
        let base = temp("parts");
        let (old, new) = (base.join(OLD_IDENTIFIER), base.join(IDENTIFIER));
        let storage = "EBWebView/Default/Local Storage";
        write(&old.join(storage).join("leveldb/000003.log"), "settings");
        write(&old.join("EBWebView/Default/Cache/data_0"), "cache");

        assert!(copy_once(&old, &new, &[storage]).unwrap());
        assert_eq!(
            read(&new.join(storage).join("leveldb/000003.log")),
            "settings"
        );
        assert!(!new.join("EBWebView/Default/Cache").exists());
    }

    #[test]
    fn a_copy_cut_short_is_done_again() {
        let base = temp("crash");
        let (old, new) = (base.join(OLD_IDENTIFIER), base.join(IDENTIFIER));
        write(&old.join("repair-scripts.json"), "mine");
        let staging = base.join(format!("{IDENTIFIER}.copying"));
        write(&staging.join("half.tmp"), "half");

        assert!(copy_once(&old, &new, &[""]).unwrap());
        assert_eq!(read(&new.join("repair-scripts.json")), "mine");
        assert!(!new.join("half.tmp").exists());
        assert!(!staging.exists());
    }

    #[test]
    fn nothing_to_copy_creates_nothing() {
        let base = temp("none");
        let (old, new) = (base.join(OLD_IDENTIFIER), base.join(IDENTIFIER));
        assert!(!copy_once(&old, &new, &[""]).unwrap());
        fs::create_dir_all(&old).unwrap();
        assert!(!copy_once(&old, &new, &["EBWebView/Default/Local Storage"]).unwrap());
        assert!(!new.exists());
    }

    #[test]
    fn the_plan_covers_scripts_and_window_storage() {
        let (config, local, home) = (Path::new("cfg"), Path::new("local"), Path::new("home"));
        let plan = plan(config, local, home);
        assert_eq!(
            plan[0],
            Carry {
                base: config.into(),
                parts: &[""]
            }
        );
        let storage = &plan[1];
        if cfg!(windows) {
            assert_eq!(storage.base, local);
            assert_eq!(storage.parts, &["EBWebView/Default/Local Storage"]);
        } else if cfg!(target_os = "macos") {
            assert_eq!(storage.base, home.join("Library/WebKit"));
        } else {
            assert_eq!(storage.base, local);
        }
    }
}
