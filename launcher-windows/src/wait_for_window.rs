//! `wait-for-window.exe`. The work is in [`zwift_windows_helpers::wait`], which
//! only exists for Windows targets.
//!
//! This stays a console binary on purpose: it writes diagnostics to stderr on
//! failure, which the launcher captures into the log.

#[cfg(windows)]
fn main() {
    std::process::exit(zwift_windows_helpers::wait::run());
}

#[cfg(not(windows))]
fn main() {
    zwift_windows_helpers::not_windows()
}
