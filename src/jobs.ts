// Job numbers for every Rust run that can be cancelled: repair mode and batch repair, Boolean operations, platform
// statistics, the Part library, Generate shell, labels, arranging and packing. They all cancel through repair_cancel,
// and Rust keeps a cancel that arrives before its run has started only for a number above every one started so far
// (src-tauri/src/jobs.rs RepairJobs). One counter for all of them keeps that true whichever feature ran last.

let last = 0;

/** A job number above every one handed out before. */
export function nextJob(): number {
  last += 1;
  return last;
}
