//! Full speed for repair runs. Windows power-throttles a process whose window is not in front: it
//! moves the threads onto the efficiency cores (EcoQoS), and a repair the user starts and then
//! leaves for another app takes two to three times as long. While a `FullSpeed` guard lives, the
//! process opts out of execution-speed throttling; when the last guard goes, the process gets back
//! the throttling state it had before. Elsewhere than Windows the guard does nothing.

use std::sync::{Mutex, MutexGuard};

/// The process opts out of power throttling while one of these lives.
pub struct FullSpeed(());

/// Guards alive, and the state to restore when the last goes (None: nothing to restore).
static ACTIVE: Mutex<(usize, Option<platform::State>)> = Mutex::new((0, None));

fn active() -> MutexGuard<'static, (usize, Option<platform::State>)> {
    // The count stays valid even if a thread panicked while holding the lock.
    ACTIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl FullSpeed {
    /// Turns throttling off for the whole process (the first guard does; later ones only count).
    pub fn start() -> FullSpeed {
        let mut active = active();
        if active.0 == 0 {
            active.1 = platform::full_speed();
        }
        active.0 += 1;
        FullSpeed(())
    }
}

impl Drop for FullSpeed {
    fn drop(&mut self) {
        let mut active = active();
        active.0 -= 1;
        if active.0 == 0
            && let Some(before) = active.1.take()
        {
            platform::restore(before);
        }
    }
}

#[cfg(windows)]
mod platform {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
        ProcessPowerThrottling, SetProcessInformation,
    };

    /// The process's power throttling state: which policies it controls, and which of those are on.
    pub struct State {
        control: u32,
        on: u32,
    }

    fn set(control: u32, on: u32) -> bool {
        let state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: control,
            StateMask: on,
        };
        // SAFETY: the pseudo handle of the current process needs no closing, and `state` is a
        // PROCESS_POWER_THROTTLING_STATE of the size passed, as ProcessPowerThrottling expects.
        unsafe {
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                &state as *const _ as *const core::ffi::c_void,
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            ) != 0
        }
    }

    /// What the process has now; None on Windows before 11, which cannot read it back.
    fn current() -> Option<State> {
        let mut state = PROCESS_POWER_THROTTLING_STATE {
            Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            ControlMask: 0,
            StateMask: 0,
        };
        // SAFETY: as in `set`, with `state` written by the call.
        let read = unsafe {
            GetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                &mut state as *mut _ as *mut core::ffi::c_void,
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            ) != 0
        };
        read.then_some(State {
            control: state.ControlMask,
            on: state.StateMask,
        })
    }

    /// Takes control of execution speed and turns its throttling off; the state before, or None
    /// when Windows refused the change.
    pub fn full_speed() -> Option<State> {
        // A process that never set its state leaves every policy to the system.
        let before = current().unwrap_or(State { control: 0, on: 0 });
        let control = before.control | PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        let on = before.on & !PROCESS_POWER_THROTTLING_EXECUTION_SPEED;
        set(control, on).then_some(before)
    }

    pub fn restore(before: State) {
        // Nothing to do on failure: the run is over either way.
        set(before.control, before.on);
    }

    /// Execution-speed throttling as the process has it: Some(None) while the system decides,
    /// None when Windows cannot say.
    #[cfg(test)]
    pub fn throttled() -> Option<Option<bool>> {
        let s = current()?;
        Some(
            (s.control & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0)
                .then_some(s.on & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0),
        )
    }
}

#[cfg(not(windows))]
mod platform {
    /// Nothing to restore.
    pub type State = ();

    pub fn full_speed() -> Option<State> {
        None
    }

    pub fn restore(_: State) {}
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn throttling_is_off_while_a_guard_lives_and_restored_after_the_last() {
        // Tests run in one process; this is the only one touching its throttling state.
        let Some(before) = platform::throttled() else {
            eprintln!("this Windows cannot read the throttling state back; nothing to check");
            return;
        };
        let first = FullSpeed::start();
        assert_eq!(platform::throttled(), Some(Some(false)));
        let second = FullSpeed::start();
        drop(first);
        assert_eq!(
            platform::throttled(),
            Some(Some(false)),
            "a run is still going"
        );
        drop(second);
        assert_eq!(platform::throttled(), Some(before));
    }
}
