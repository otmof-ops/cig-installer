// SPDX-FileCopyrightText: 2026 Jay Taylor (https://github.com/otmof-ops/cig-installer)
// SPDX-License-Identifier: FSL-1.1-ALv2
//! What the shim needs from the running process: surviving Ctrl+C while the program it started
//! handles it. (The self-update's rename of a running exe needs nothing beyond std.)

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{FALSE, TRUE};
use windows_sys::Win32::System::Console::{SetConsoleCtrlHandler, CTRL_BREAK_EVENT, CTRL_C_EVENT};

/// The handler the shim installs: Ctrl+C and Ctrl+Break are taken ("handled"), so the shim
/// carries on waiting while the child, which shares the console, answers them itself. Every
/// other event (the console closing, logoff, shutdown) keeps its default.
unsafe extern "system" fn swallow(event: u32) -> BOOL {
    if event == CTRL_C_EVENT || event == CTRL_BREAK_EVENT {
        TRUE
    } else {
        FALSE
    }
}

/// Make this process survive Ctrl+C and Ctrl+Break so a program it waits for handles them.
///
/// This installs a handler routine and does not set the "ignore Ctrl+C" flag
/// (`SetConsoleCtrlHandler(NULL, TRUE)`) because that flag is inherited by child processes,
/// and the child would then ignore Ctrl+C too. A handler is not inherited. Returns whether
/// the handler was installed.
pub fn swallow_ctrl_c() -> bool {
    // SAFETY: `swallow` has the `PHANDLER_ROUTINE` signature, never unwinds, touches no state
    // and lives for the whole process (it is a function item); the call itself takes no
    // pointers other than that routine.
    unsafe { SetConsoleCtrlHandler(Some(swallow), TRUE) != FALSE }
}
