//! End-to-end check of the screensaver inhibitor against a fake session service.
//!
//! This is opt-in: a normal session only owns these names while the desktop is
//! actually running, so `tests/inhibit.sh` starts a fake
//! `org.freedesktop.ScreenSaver` or `org.gnome.SessionManager` on a private
//! session bus and points the test at its recording through
//! `ZWIFT_INHIBIT_SERVICE_LOG`. Without it the test does nothing, which keeps
//! `cargo test` and CI free of a D-Bus service dependency.
//!
//! The point of the check is the marshalling: the arguments of `Inhibit` are
//! positional and the reply is a single `u` inside a tuple, and getting either
//! wrong only shows up as the desktop quietly ignoring the inhibitor.

use zwsnap_gui::display::DisplayMode;
use zwsnap_gui::inhibit::ScreenInhibitor;
use zwsnap_gui::logging::Logger;

/// The reason the launcher passes to the desktop, and which the fake records.
const REASON: &str = "Zwift is running fullscreen";

#[test]
fn acquires_and_releases_the_session_inhibitor() {
    let Ok(recording) = std::env::var("ZWIFT_INHIBIT_SERVICE_LOG") else {
        eprintln!("ZWIFT_INHIBIT_SERVICE_LOG is not set: run tests/inhibit.sh instead");
        return;
    };
    let backend = std::env::var("ZWIFT_INHIBIT_BACKEND").unwrap_or("freedesktop".to_string());

    let dir = std::env::temp_dir().join(format!("zwift-inhibit-{}-{backend}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let logger = Logger::new(dir.join("launcher.log"), 0, false);
    logger.init().unwrap();

    let mut inhibitor = ScreenInhibitor::acquire(&DisplayMode::Fullscreen, &logger);
    assert!(
        inhibitor.is_active(),
        "the fullscreen session was not inhibited"
    );
    inhibitor.release(&logger);

    // Both calls are synchronous, so the fake's recording is complete here.
    let seen = std::fs::read_to_string(&recording).unwrap();
    let expected = match backend.as_str() {
        // No toplevel window to name, and the logout plus idle flags.
        "gnome" => [
            format!("gnome-inhibit Zwift | 0 | {REASON} | 12"),
            "gnome-uninhibit 77".to_string(),
        ],
        _ => [
            format!("freedesktop-inhibit Zwift | {REASON}"),
            "freedesktop-uninhibit 41".to_string(),
        ],
    };
    for line in &expected {
        assert!(seen.contains(line.as_str()), "missing {line:?} in:\n{seen}");
    }

    let _ = std::fs::remove_dir_all(&dir);
}
