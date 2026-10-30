//! Folders for tests that write files, the unit tests here and the integration tests in tests/.
//! Each is a fresh folder in the system temp folder's `tricky3d-tests`, removed with everything in
//! it when its guard is dropped: when the test ends, and also when it fails, since a panic unwinds
//! through the drop. `tricky3d-tests` goes with the last one, so a full test run leaves nothing.
//! This is the only place tests may call `std::env::temp_dir()` (checked below).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// A fresh, empty folder for one test, removed again when this is dropped.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
        // Only succeeds once the folder is empty: the last test out removes it.
        if let Some(parent) = self.0.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

/// The folder all test folders go in.
fn root() -> PathBuf {
    std::env::temp_dir().join("tricky3d-tests")
}

/// A new folder named after `name`, the process and a counter, so tests running at the same time
/// never share one.
pub fn temp_dir(name: &str) -> TempDir {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = root().join(format!(
        "{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    // Another test's drop can remove `tricky3d-tests` between creating it and creating `dir` in
    // it; creating both again then succeeds.
    let mut tries = 0;
    while let Err(e) = std::fs::create_dir_all(&dir) {
        tries += 1;
        assert!(tries < 5, "could not create {}: {e}", dir.display());
    }
    TempDir(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_goes_when_its_test_ends_also_when_it_fails() {
        let kept = temp_dir("guard-keeps");
        let dir = temp_dir("guard");
        std::fs::create_dir_all(dir.join("deep/er")).unwrap();
        std::fs::write(dir.join("deep/er/file.bin"), "x").unwrap();
        let path = dir.to_path_buf();
        drop(dir);
        assert!(!path.exists());

        let failed = std::panic::catch_unwind(|| {
            let dir = temp_dir("guard-panics");
            std::fs::write(dir.join("half.tmp"), "half").unwrap();
            panic!("a failing test (on purpose)");
        });
        assert!(failed.is_err());
        let left: Vec<_> = std::fs::read_dir(root())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with(&format!("guard-panics-{}-", std::process::id())))
            .collect();
        assert!(left.is_empty(), "left behind: {left:?}");
        // A folder still in use keeps `tricky3d-tests`.
        assert!(kept.exists());
    }

    /// Every test that writes files gets its folder here, so none can leave one behind.
    #[test]
    fn no_test_makes_temp_folders_of_its_own() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut offenders = Vec::new();
        let mut stack = vec![manifest.join("src"), manifest.join("tests")];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "rs")
                    && path != manifest.join("src").join("test_dir.rs")
                {
                    let text = std::fs::read_to_string(&path).unwrap();
                    if text.contains(concat!("env::", "temp_dir()")) {
                        offenders.push(path);
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use test_dir::temp_dir instead: {offenders:?}"
        );
    }
}
