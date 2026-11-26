//! A finer system timer during calls (Windows).
//!
//! The voice leaves on a 20 ms timer. Windows wakes timers at the system
//! timer resolution, 15.6 ms unless a program asks for less; Discord,
//! browsers and games ask for 1 ms, for the whole system. Without one of them
//! open, Rico's timer fired late and the voice left in small bursts, heard
//! as lag (on consoles above all). While a call is open Rico asks for 1 ms
//! itself, as Discord does, and gives it back when the call ends.
//!
//! The only `unsafe` code of Rico, approved by the owner (2026-11-25): two
//! calls into `winmm.dll` through `windows-sys`.

/// While alive, the system timer resolution is 1 ms (on Windows; nothing
/// elsewhere). Dropping it gives it back.
#[derive(Debug)]
pub struct FineTimer {
    active: bool,
}

/// The period asked for, in milliseconds.
#[cfg(windows)]
const PERIOD_MS: u32 = 1;

impl FineTimer {
    pub fn new() -> Self {
        #[cfg(windows)]
        {
            // SAFETY: timeBeginPeriod takes a plain integer and touches no
            // memory of ours; it is balanced by exactly one timeEndPeriod in
            // Drop, and only when it succeeded (`active`).
            let result = unsafe { windows_sys::Win32::Media::timeBeginPeriod(PERIOD_MS) };
            let active = result == windows_sys::Win32::Media::TIMERR_NOERROR;
            if active {
                tracing::info!("voice: system timer at {PERIOD_MS} ms during the call");
            } else {
                tracing::warn!("voice: the system timer could not be made finer (code {result})");
            }
            Self { active }
        }
        #[cfg(not(windows))]
        Self { active: false }
    }

    /// The finer timer is in effect.
    pub const fn active(&self) -> bool {
        self.active
    }
}

impl Default for FineTimer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for FineTimer {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.active {
            // SAFETY: as in `new`: an integer argument, no memory; called
            // once for the one successful timeBeginPeriod.
            unsafe {
                windows_sys::Win32::Media::timeEndPeriod(PERIOD_MS);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_finer_timer_is_given_back() {
        let timer = FineTimer::new();
        assert_eq!(timer.active(), cfg!(windows));
        drop(timer);
        // Twice in a row is fine too (Windows counts them).
        let a = FineTimer::new();
        let b = FineTimer::new();
        drop((a, b));
    }
}
