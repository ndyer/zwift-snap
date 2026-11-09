//! `ZwiftLauncher.exe`. The work is in [`zwift_windows_helpers::shim`], which
//! only exists for Windows targets.

// Zwift's own launcher is a GUI binary; matching it keeps a console window from
// flashing up next to the game.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    std::process::exit(zwift_windows_helpers::shim::run());
}

#[cfg(not(windows))]
fn main() {
    zwift_windows_helpers::not_windows()
}
