//! Windows-side helpers the snap runs under wine: `ZwiftLauncher.exe` (a
//! pass-through parent for the game) and `wait-for-window.exe` (waits for the
//! game's window to appear).
//!
//! The win32 parts only build for Windows targets, so the logic that does not
//! need the API — command line splitting and the window predicate — stays in
//! platform-independent modules that `cargo test` covers on the Linux build
//! host. A PE binary cannot run there, so those unit tests are the only local
//! check; the snap's smoke and pairing tests exercise the real binaries.

pub mod command_line;
pub mod window;

#[cfg(windows)]
pub mod shim;
#[cfg(windows)]
pub mod wait;

/// Where a helper binary lands on a host that cannot run it.
///
/// The crate is a snap build input, so `cargo clippy` and `cargo test` on the
/// Linux build host still build the binary targets, which only exist for
/// Windows.
#[cfg(not(windows))]
pub fn not_windows() -> ! {
    eprintln!("this helper is built for Windows and runs under wine");
    std::process::exit(1);
}
