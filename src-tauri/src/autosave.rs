//! Autosave and recovery. While the app runs, the window sends the open project every few minutes
//! (File › Settings › Autosave); it is written as a project file into the app's own data folder
//! (`<local data>/autosave`), never over the user's file, and only the newest three are kept.
//!
//! A run holds a lock file in that folder for as long as it lasts (`session-<id>.lock`, locked
//! with the system's file lock) and deletes it when it ends normally. The system releases the lock
//! of a process that ends any other way, so at the next start a lock file that is still there but
//! no longer locked is a run that crashed: its newest autosave is offered for restoring. A lock
//! that is still held belongs to another tricky3D that is running, and is left alone.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

/// How many autosaves are kept.
pub const KEEP: usize = 3;

/// The autosave a crashed run left, as the window shows it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Recovery {
    /// The file's name in the autosave folder.
    pub id: String,
    /// When it was written, milliseconds since 1970.
    pub saved_at: u64,
}

pub struct Session {
    dir: PathBuf,
    id: String,
    /// Held open, locked, until the run ends.
    lock: Option<File>,
    /// Runs that crashed: their ids.
    crashed: Vec<String>,
    /// Autosaves written in this run.
    count: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The run id in a lock file's name, `session-<id>.lock`.
fn lock_id(name: &str) -> Option<&str> {
    name.strip_prefix("session-")?.strip_suffix(".lock")
}

/// The run id and number in an autosave's name, `autosave-<id>-<n>.3mf`.
fn autosave_of(name: &str) -> Option<(&str, u64)> {
    let rest = name.strip_prefix("autosave-")?.strip_suffix(".3mf")?;
    let (id, n) = rest.rsplit_once('-')?;
    Some((id, n.parse().ok()?))
}

impl Session {
    /// Starts this run's session in `dir` (made if missing): finds the runs that crashed, then
    /// takes a lock of its own.
    pub fn start(dir: &Path) -> io::Result<Session> {
        fs::create_dir_all(dir)?;
        let mut crashed = Vec::new();
        for entry in fs::read_dir(dir)?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = lock_id(&name) else {
                continue;
            };
            let Ok(file) = OpenOptions::new().read(true).write(true).open(entry.path()) else {
                continue;
            };
            // Locked by a run that is still going: another window of tricky3D.
            if file.try_lock().is_ok() {
                crashed.push(id.to_string());
            }
        }
        // A crashed run that saved nothing has nothing to offer, so the window never dismisses
        // it: its lock file goes now.
        let saved: Vec<String> = fs::read_dir(dir)?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                autosave_of(&name).map(|(id, _)| id.to_string())
            })
            .collect();
        crashed.retain(|id| {
            let offered = saved.contains(id);
            if !offered {
                let _ = fs::remove_file(dir.join(format!("session-{id}.lock")));
            }
            offered
        });
        // Start time, process id and a count: unique across runs and within one.
        static STARTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let id = format!("{}x{}x{n}", now_ms(), std::process::id());
        let path = dir.join(format!("session-{id}.lock"));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        lock.lock()?;
        Ok(Session {
            dir: dir.to_path_buf(),
            id,
            lock: Some(lock),
            crashed,
            count: 0,
        })
    }

    /// The newest autosave a crashed run left, if any.
    pub fn recovery(&self) -> Option<Recovery> {
        if self.crashed.is_empty() {
            return None;
        }
        let mut best: Option<(u64, String)> = None;
        for entry in fs::read_dir(&self.dir).ok()?.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some((id, _)) = autosave_of(&name) else {
                continue;
            };
            if !self.crashed.iter().any(|c| c == id) {
                continue;
            }
            let at = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            if best.as_ref().is_none_or(|(b, _)| at > *b) {
                best = Some((at, name));
            }
        }
        best.map(|(saved_at, id)| Recovery { id, saved_at })
    }

    /// The file of autosave `id`, if `id` names one in the folder (and nothing else).
    pub fn file(&self, id: &str) -> Option<PathBuf> {
        autosave_of(id)?;
        if id.contains(['/', '\\']) || id.contains("..") {
            return None;
        }
        let path = self.dir.join(id);
        path.is_file().then_some(path)
    }

    /// The crashed runs were dealt with (restored or not): their lock files go, so they are not
    /// offered again.
    pub fn dismiss(&mut self) {
        for id in self.crashed.drain(..) {
            let _ = fs::remove_file(self.dir.join(format!("session-{id}.lock")));
        }
    }

    /// Where the next autosave goes, and the number it takes.
    pub fn next_file(&mut self) -> PathBuf {
        self.count += 1;
        self.dir
            .join(format!("autosave-{}-{}.3mf", self.id, self.count))
    }

    /// Deletes all but the newest KEEP autosaves (by when they were written; the file names break
    /// ties). Files of other names are left alone.
    pub fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        let mut saves: Vec<(SystemTime, String)> = entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                autosave_of(&name)?;
                let at = e.metadata().and_then(|m| m.modified()).ok()?;
                Some((at, name))
            })
            .collect();
        saves.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| order(&b.1, &a.1)));
        for (_, name) in saves.into_iter().skip(KEEP) {
            let _ = fs::remove_file(self.dir.join(name));
        }
    }

    /// The run ends normally: its lock is released and deleted.
    pub fn end(&mut self) {
        if let Some(lock) = self.lock.take() {
            let _ = lock.unlock();
            drop(lock);
            let _ = fs::remove_file(self.dir.join(format!("session-{}.lock", self.id)));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.end();
    }
}

/// Autosave names in the order they were written: by run, then by number.
fn order(a: &str, b: &str) -> std::cmp::Ordering {
    match (autosave_of(a), autosave_of(b)) {
        (Some((ia, na)), Some((ib, nb))) => ia.cmp(ib).then(na.cmp(&nb)),
        _ => a.cmp(b),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole round: the window's project autosaved as the app writes it, the run crashes, the
    /// next run offers that autosave and opens it as the project it was.
    #[test]
    fn an_autosaved_project_survives_a_crash_and_restores() {
        use crate::bundle::{OutMesh, decode, encode};
        use serde_json::{Map, json};
        let dir = crate::test_dir::temp_dir("autosave-restore");
        let cube = crate::mesh::Mesh {
            positions: vec![
                0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0, 0.0, 0.0, 0.0, 10.0,
            ],
            indices: vec![0, 2, 1, 0, 1, 3, 1, 2, 3, 0, 3, 2],
        };
        let mut meta = Map::new();
        meta.insert("platform".into(), json!([200.0, 150.0, 100.0]));
        let mut info = Map::new();
        info.insert("name".into(), json!("Bracket"));
        let mut matrix = crate::mesh::IDENTITY;
        matrix[12] = 40.0;
        info.insert("matrix".into(), json!(matrix.to_vec()));
        info.insert("color".into(), json!("#ff8800"));
        let request = decode(&encode(
            &meta,
            &[OutMesh {
                info,
                positions: &cube.positions,
                indices: &cube.indices,
                edges: None,
            }],
        ))
        .unwrap();
        let mut run = Session::start(&dir).unwrap();
        let file = run.next_file();
        crate::jobs::save_project_to(&file, &request).unwrap();
        run.prune();
        // The crash: the system releases the lock, the lock file stays.
        drop(run.lock.take());
        std::mem::forget(run);
        let mut next = Session::start(&dir).unwrap();
        let offer = next.recovery().expect("the autosave is offered");
        let path = next.file(&offer.id).unwrap();
        let opened = decode(
            &crate::jobs::open_project(&path, &mut crate::progress::Progress::silent()).unwrap(),
        )
        .unwrap();
        assert_eq!(opened.meta["platform"], json!([200.0, 150.0, 100.0]));
        assert_eq!(opened.meshes.len(), 1);
        let part = &opened.meshes[0];
        assert_eq!(part.info["name"], json!("Bracket"));
        assert_eq!(part.info["color"], json!("#ff8800"));
        assert_eq!(part.info["matrix"][12], json!(40.0));
        assert_eq!(
            crate::mesh::stats(&part.mesh).volume,
            crate::mesh::stats(&cube).volume
        );
        next.dismiss();
        drop(next);
        assert_eq!(Session::start(&dir).unwrap().recovery(), None);
    }

    #[test]
    fn names_are_read_back() {
        assert_eq!(lock_id("session-17000.lock"), Some("17000"));
        assert_eq!(lock_id("session-1.txt"), None);
        assert_eq!(autosave_of("autosave-17000-3.3mf"), Some(("17000", 3)));
        assert_eq!(autosave_of("autosave-x.3mf"), None);
        assert_eq!(autosave_of("other-1-2.3mf"), None);
    }

    #[test]
    fn only_the_newest_three_are_kept_and_ids_stay_in_the_folder() {
        let dir = crate::test_dir::temp_dir("autosave-keep");
        let mut s = Session::start(&dir).unwrap();
        let mut written = Vec::new();
        for _ in 0..5 {
            let f = s.next_file();
            fs::write(&f, b"x").unwrap();
            written.push(f.file_name().unwrap().to_string_lossy().into_owned());
            s.prune();
        }
        fs::write(dir.join("notes.txt"), b"keep me").unwrap();
        s.prune();
        let mut left: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("autosave-"))
            .collect();
        left.sort_by(|a, b| order(a, b));
        assert_eq!(left, written[2..]);
        assert!(dir.join("notes.txt").is_file());
        assert!(s.file(&written[4]).is_some());
        assert!(s.file("../notes.txt").is_none());
        assert!(s.file("notes.txt").is_none());
        assert!(s.file(&written[0]).is_none(), "deleted");
    }

    #[test]
    fn a_crashed_run_is_offered_and_a_clean_or_running_one_is_not() {
        let dir = crate::test_dir::temp_dir("autosave-crash");
        // A run that ends normally: nothing to offer next time.
        {
            let mut clean = Session::start(&dir).unwrap();
            fs::write(clean.next_file(), b"clean").unwrap();
            clean.end();
        }
        let next = Session::start(&dir).unwrap();
        assert_eq!(next.recovery(), None);
        drop(next);
        // A run that crashes: its lock file stays, unlocked (forgetting the session leaves the
        // file, and closing the handle releases the lock, as the system does for a dead process).
        let mut crashed = Session::start(&dir).unwrap();
        let first = crashed.next_file();
        fs::write(&first, b"first").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newest = crashed.next_file();
        fs::write(&newest, b"newest").unwrap();
        let lock = crashed.lock.take().unwrap();
        drop(lock);
        std::mem::forget(crashed);
        // A run still going (another window) holds its lock: never offered.
        let running = Session::start(&dir).unwrap();
        let mut after = Session::start(&dir).unwrap();
        let offer = after.recovery().expect("the crashed run's autosave");
        assert_eq!(offer.id, newest.file_name().unwrap().to_string_lossy());
        assert_eq!(fs::read(after.file(&offer.id).unwrap()).unwrap(), b"newest");
        assert!(!after.crashed.contains(&running.id));
        // Once dealt with, it is not offered again.
        after.dismiss();
        drop(after);
        drop(running);
        let again = Session::start(&dir).unwrap();
        assert_eq!(again.recovery(), None);
    }

    /// A run that crashed before its first autosave has nothing to offer, so the window never
    /// dismisses it: the next start removes its lock file itself.
    #[test]
    fn a_crash_with_nothing_saved_leaves_no_lock_behind() {
        let dir = crate::test_dir::temp_dir("autosave-nothing");
        let locks = || {
            let mut names: Vec<String> = fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| lock_id(n).is_some())
                .collect();
            names.sort();
            names
        };
        let mut crashed = Session::start(&dir).unwrap();
        drop(crashed.lock.take());
        std::mem::forget(crashed);
        let next = Session::start(&dir).unwrap();
        assert_eq!(next.recovery(), None);
        assert_eq!(locks(), vec![format!("session-{}.lock", next.id)]);
    }
}
