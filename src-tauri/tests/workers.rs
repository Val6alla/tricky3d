//! The worker pool as the app sets it up at start (src/workers.rs): one thread fewer than the
//! logical CPUs, below normal priority. A test binary of its own, because rayon's global pool can
//! be set up only once per process.

use tricky3d_lib::workers;

#[test]
fn the_pool_leaves_a_core_free_and_runs_below_normal_priority() {
    workers::init();
    assert_eq!(rayon::current_num_threads(), workers::count());
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert_eq!(workers::count(), (cores - 1).max(1));

    // Every pool thread, asked at once, reports its own priority.
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, GetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
        };
        let priorities = rayon::broadcast(|_| {
            // SAFETY: reads the calling thread's priority.
            unsafe { GetThreadPriority(GetCurrentThread()) }
        });
        assert_eq!(priorities.len(), workers::count());
        assert!(
            priorities
                .iter()
                .all(|&p| p == THREAD_PRIORITY_BELOW_NORMAL),
            "{priorities:?}"
        );
    }
}
