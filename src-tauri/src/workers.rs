//! The threads that do the heavy work: imports, exports, cuts, splits, checks and repairs. They
//! leave one CPU core free and run below normal priority, so the computer stays responsive while
//! a long repair runs: the window, the user's other programs and the system all go first.
//!
//! A job runs on a thread of its own (`run`), and its parallel loops on rayon's global pool, which
//! `init` sizes to `count()` threads. While a loop runs, the job's thread waits for the pool, so a
//! job never keeps more than `count()` threads busy.

/// Threads heavy work may keep busy: one fewer than the logical CPUs, and at least one.
pub fn count() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1))
}

/// Sizes rayon's global pool, which every parallel loop in src/repair and src/ops uses, to
/// `count()` threads below normal priority. Call once, before any job starts.
pub fn init() {
    let built = rayon::ThreadPoolBuilder::new()
        .num_threads(count())
        .thread_name(|i| format!("worker {i}"))
        .start_handler(|_| lower_priority())
        .build_global();
    if let Err(e) = built {
        eprintln!("tricky3d: could not set up the worker threads: {e}");
    }
}

/// Runs `work` on a new thread below normal priority and waits for it. The thread ends with the
/// job, so its priority never needs restoring.
pub fn run<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let stopped =
        |e: &dyn std::fmt::Display| format!("The background task stopped unexpectedly ({e}).");
    std::thread::Builder::new()
        .name("job".into())
        .spawn(move || {
            lower_priority();
            work()
        })
        .map_err(|e| stopped(&e))?
        .join()
        .map_err(|_| stopped(&"it panicked"))?
}

/// Lowers the calling thread to below normal priority, for good: call it only on threads that
/// do nothing but heavy work. Failing to lower it changes nothing else.
pub fn lower_priority() {
    platform::lower_priority();
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };

    pub fn lower_priority() {
        // SAFETY: the pseudo handle of the current thread needs no closing.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }
}

#[cfg(target_os = "macos")]
mod platform {
    pub fn lower_priority() {
        // The utility class is below the default class threads start in. Unlike the background
        // class it still runs on the performance cores, so a repair keeps its speed when nothing
        // else wants them.
        // SAFETY: changes only the calling thread's own QoS class.
        unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0);
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod platform {
    /// The niceness the `nice` command gives by default.
    const NICENESS: libc::c_int = 10;

    pub fn lower_priority() {
        // On Linux the niceness belongs to each thread, and PRIO_PROCESS with 0 sets the calling
        // thread's, not the whole process's (setpriority(2), "Bugs").
        // SAFETY: a plain system call about the calling thread.
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, NICENESS);
        }
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    pub fn lower_priority() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heavy_work_leaves_a_core_free() {
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
        assert_eq!(count(), (cores - 1).max(1));
    }

    #[test]
    fn a_job_runs_on_its_own_thread_and_returns_its_result() {
        let caller = std::thread::current().id();
        let (thread, name) = run(|| {
            let t = std::thread::current();
            Ok((t.id(), t.name().map(str::to_string)))
        })
        .unwrap();
        assert_ne!(thread, caller);
        assert_eq!(name.as_deref(), Some("job"));
        assert_eq!(
            run(|| Err::<(), _>("no".to_string())),
            Err("no".to_string())
        );
    }

    #[test]
    fn a_job_that_panics_ends_with_a_message_and_the_next_job_runs() {
        use rayon::prelude::*;
        let stopped = Err("The background task stopped unexpectedly (it panicked).".to_string());
        assert_eq!(
            run(|| -> Result<(), String> { panic!("a bug in a job") }),
            stopped
        );
        // A panic in a parallel loop, as repairs run them, reaches the job's thread the same way.
        let parallel = run(|| {
            (0..64)
                .into_par_iter()
                .for_each(|i| assert_ne!(i, 40, "a bug in a parallel loop"));
            Ok(())
        });
        assert_eq!(parallel, stopped);
        assert_eq!(run(|| Ok(7)), Ok(7));
    }

    /// `run` turns a panic into a message only when panics unwind. Tests always unwind, whatever
    /// the profile says, so the test above cannot see a release profile with `panic = "abort"`,
    /// which would close the installed app instead: this checks the profile itself.
    #[test]
    fn release_builds_unwind() {
        let manifest = include_str!("../Cargo.toml").replace("\r\n", "\n");
        let release = manifest
            .split("[profile.release]\n")
            .nth(1)
            .expect("Cargo.toml has a release profile");
        let section = release.split("\n[").next().unwrap_or_default();
        assert!(
            !section.lines().any(|l| l.trim_start().starts_with("panic")),
            "the release profile must leave panic at its default, unwind:\n{section}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn jobs_run_below_normal_priority() {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        // SAFETY: reads the calling thread's priority.
        let priority = run(|| Ok(unsafe { GetThreadPriority(GetCurrentThread()) })).unwrap();
        assert_eq!(priority, THREAD_PRIORITY_BELOW_NORMAL);
    }
}
