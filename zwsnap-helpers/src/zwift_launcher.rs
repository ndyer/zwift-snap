//! `ZwiftLauncher.exe`. The work is in [`zwsnap_helpers::shim`], which
//! only exists for Windows targets.

// Zwift's own launcher is a GUI binary; matching it keeps a console window from
// flashing up next to the game.
#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    std::process::exit(zwsnap_helpers::shim::run());
}

#[cfg(not(windows))]
fn main() {
    zwsnap_helpers::not_windows()
}
