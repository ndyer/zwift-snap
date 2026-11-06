//! Update orchestration, independent of the UI and Wine process implementation.

use crate::version::version_ge;
use std::fmt;
use std::time::Duration;

/// Operations required by the update state machine.
///
/// A future Wine adapter supplies these operations and sends progress and log
/// messages to the Rust GUI. Failures must include their underlying diagnostics.
pub trait UpdateBackend {
    type Error: fmt::Display;

    fn latest_version(&mut self) -> Result<String, Self::Error>;
    fn current_version(&mut self) -> Result<String, Self::Error>;
    fn start_launcher(&mut self) -> Result<(), Self::Error>;
    fn launcher_running(&mut self) -> Result<bool, Self::Error>;
    fn stop_launcher(&mut self) -> Result<(), Self::Error>;
    fn sleep(&mut self, duration: Duration) -> Result<(), Self::Error>;
    fn progress(&mut self, percent: u8, message: &str);
    fn message(&mut self, message: &str);
}

#[derive(Debug, PartialEq, Eq)]
pub enum UpdateOutcome {
    UpToDate,
    Updated(String),
    /// An installed copy can still be started when the version check fails.
    CheckUnavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub struct UpdateError(pub String);

impl fmt::Display for UpdateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UpdateError {}

/// Check an existing installation. A failed version lookup is non-fatal.
pub fn update_if_needed<B: UpdateBackend>(
    backend: &mut B,
    iterations: u32,
    interval: Duration,
) -> Result<UpdateOutcome, UpdateError> {
    backend.progress(5, "Checking for Zwift updates");
    let latest = match backend.latest_version() {
        Ok(version) if !version.is_empty() && version != "none" => version,
        Ok(_) => {
            backend.message("Unable to check for Zwift updates: empty latest version; starting the installed version");
            return Ok(UpdateOutcome::CheckUnavailable);
        }
        Err(error) => {
            backend.message(&format!(
                "Unable to check for Zwift updates: {error}; starting the installed version"
            ));
            return Ok(UpdateOutcome::CheckUnavailable);
        }
    };
    let original = backend
        .current_version()
        .map_err(|error| UpdateError(format!("Unable to read installed version: {error}")))?;
    if version_ge(&original, &latest) {
        backend.message(&format!(
            "Zwift is up to date (installed {original}, latest {latest})"
        ));
        return Ok(UpdateOutcome::UpToDate);
    }
    update_using_launcher(backend, &latest, &original, iterations, interval, 10, 80)
}

/// Download or update the game using ZwiftLauncher.exe.
///
/// Once started, the launcher is stopped even when polling fails or times out.
pub fn update_using_launcher<B: UpdateBackend>(
    backend: &mut B,
    latest: &str,
    original: &str,
    iterations: u32,
    interval: Duration,
    percent_start: u8,
    percent_span: u8,
) -> Result<UpdateOutcome, UpdateError> {
    if latest.is_empty() || latest == "none" || original.is_empty() {
        return Err(UpdateError("Invalid version for Zwift update".into()));
    }
    if percent_start as u16 + percent_span as u16 > 100 {
        return Err(UpdateError("Invalid update progress range".into()));
    }
    let message = if original == "none" {
        format!("Downloading Zwift {latest}, this takes several minutes")
    } else {
        format!("Updating Zwift from version {original} to {latest}")
    };
    backend.progress(percent_start, &message);
    backend
        .start_launcher()
        .map_err(|error| UpdateError(format!("Failed to start Zwift launcher: {error}")))?;
    backend.message("Zwift launcher started using wine");

    let result = poll_update(
        backend,
        latest,
        original,
        iterations,
        interval,
        percent_start,
        percent_span,
    );
    let cleanup = backend.stop_launcher();
    match (result, cleanup) {
        (Ok(version), Ok(())) => {
            backend.message(&format!("Zwift updated to version {version}"));
            Ok(UpdateOutcome::Updated(version))
        }
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(error)) => Err(UpdateError(format!(
            "Failed to stop Zwift launcher: {error}"
        ))),
        (Err(error), Err(cleanup)) => Err(UpdateError(format!(
            "{error}; failed to stop Zwift launcher: {cleanup}"
        ))),
    }
}

fn poll_update<B: UpdateBackend>(
    backend: &mut B,
    latest: &str,
    original: &str,
    iterations: u32,
    interval: Duration,
    percent_start: u8,
    percent_span: u8,
) -> Result<String, UpdateError> {
    for counter in 1..=iterations {
        if !backend
            .launcher_running()
            .map_err(|error| UpdateError(format!("Unable to check Zwift launcher: {error}")))?
        {
            return Err(UpdateError(
                "Launcher exited unexpectedly, update did not complete".into(),
            ));
        }
        let percent = u64::from(percent_start)
            + u64::from(counter) * u64::from(percent_span) / u64::from(iterations);
        let elapsed = interval.as_secs().saturating_mul(u64::from(counter));
        let total = interval.as_secs().saturating_mul(u64::from(iterations));
        backend.progress(
            percent as u8,
            &format!(
                "Updating Zwift to version {latest}, waited {} of {} minutes",
                elapsed / 60,
                total / 60
            ),
        );
        backend
            .sleep(interval)
            .map_err(|error| UpdateError(format!("Update wait failed: {error}")))?;
        let current = backend
            .current_version()
            .map_err(|error| UpdateError(format!("Unable to read installed version: {error}")))?;
        if current != original {
            if version_ge(&current, latest) {
                return Ok(current);
            }
            return Err(UpdateError(format!(
                "Zwift updated to an older version than expected (Expected: {latest}, Actual: {current})"
            )));
        }
    }
    Err(UpdateError(format!(
        "Update timed out after {} seconds",
        interval.as_secs().saturating_mul(u64::from(iterations))
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Fake {
        latest: Result<String, &'static str>,
        versions: VecDeque<Result<String, &'static str>>,
        running: Result<bool, &'static str>,
        start: Result<(), &'static str>,
        stop: Result<(), &'static str>,
        starts: usize,
        stops: usize,
        progress: Vec<(u8, String)>,
        messages: Vec<String>,
    }

    impl Fake {
        fn new(versions: &[&str]) -> Self {
            Self {
                latest: Ok("1.124.0".into()),
                versions: versions.iter().map(|v| Ok((*v).into())).collect(),
                running: Ok(true),
                start: Ok(()),
                stop: Ok(()),
                starts: 0,
                stops: 0,
                progress: Vec::new(),
                messages: Vec::new(),
            }
        }
    }

    impl UpdateBackend for Fake {
        type Error = &'static str;

        fn latest_version(&mut self) -> Result<String, Self::Error> {
            self.latest.clone()
        }
        fn current_version(&mut self) -> Result<String, Self::Error> {
            self.versions.pop_front().expect("unexpected version read")
        }
        fn start_launcher(&mut self) -> Result<(), Self::Error> {
            self.starts += 1;
            self.start
        }
        fn launcher_running(&mut self) -> Result<bool, Self::Error> {
            self.running
        }
        fn stop_launcher(&mut self) -> Result<(), Self::Error> {
            self.stops += 1;
            self.stop
        }
        fn sleep(&mut self, _: Duration) -> Result<(), Self::Error> {
            Ok(())
        }
        fn progress(&mut self, percent: u8, message: &str) {
            self.progress.push((percent, message.into()));
        }
        fn message(&mut self, message: &str) {
            self.messages.push(message.into());
        }
    }

    const INTERVAL: Duration = Duration::from_secs(5);

    #[test]
    fn up_to_date_and_newer_versions_do_not_launch_updater() {
        for version in ["1.124.0", "1.125.0"] {
            let mut backend = Fake::new(&[version]);
            assert_eq!(
                update_if_needed(&mut backend, 60, INTERVAL).unwrap(),
                UpdateOutcome::UpToDate
            );
            assert_eq!((backend.starts, backend.stops), (0, 0));
        }
    }

    #[test]
    fn offline_check_is_explicitly_nonfatal() {
        let mut backend = Fake::new(&[]);
        backend.latest = Err("network unavailable");
        assert_eq!(
            update_if_needed(&mut backend, 60, INTERVAL).unwrap(),
            UpdateOutcome::CheckUnavailable
        );
        assert!(backend.messages[0].contains("network unavailable"));
        assert_eq!(backend.starts, 0);
    }

    #[test]
    fn updates_to_newer_than_advertised_and_cleans_up() {
        let mut backend = Fake::new(&["1.100.0", "1.100.0", "1.125.0"]);
        assert_eq!(
            update_if_needed(&mut backend, 4, INTERVAL).unwrap(),
            UpdateOutcome::Updated("1.125.0".into())
        );
        assert_eq!((backend.starts, backend.stops), (1, 1));
        assert_eq!(
            backend.progress.iter().map(|p| p.0).collect::<Vec<_>>(),
            [5, 10, 30, 50]
        );
    }

    #[test]
    fn fresh_install_download_uses_download_message() {
        let mut backend = Fake::new(&["1.124.0"]);
        assert_eq!(
            update_using_launcher(&mut backend, "1.124.0", "none", 2, INTERVAL, 90, 9).unwrap(),
            UpdateOutcome::Updated("1.124.0".into())
        );
        assert!(backend.progress[0].1.starts_with("Downloading Zwift"));
        assert_eq!(backend.progress[1].0, 94);
    }

    #[test]
    fn timeout_and_early_exit_clean_up() {
        let mut timeout = Fake::new(&["1.100.0", "1.100.0"]);
        let err = update_using_launcher(&mut timeout, "1.124.0", "1.100.0", 2, INTERVAL, 10, 80)
            .unwrap_err();
        assert!(err.0.contains("timed out after 10 seconds"));
        assert_eq!(timeout.stops, 1);
        let mut exited = Fake::new(&[]);
        exited.running = Ok(false);
        assert!(
            update_using_launcher(&mut exited, "1.124.0", "1.100.0", 2, INTERVAL, 10, 80)
                .unwrap_err()
                .0
                .contains("exited unexpectedly")
        );
        assert_eq!(exited.stops, 1);
    }

    #[test]
    fn older_update_and_poll_errors_are_not_success() {
        let mut older = Fake::new(&["1.101.0"]);
        assert!(
            update_using_launcher(&mut older, "1.124.0", "1.100.0", 2, INTERVAL, 10, 80)
                .unwrap_err()
                .0
                .contains("older version")
        );
        assert_eq!(older.stops, 1);
        let mut failed = Fake::new(&[]);
        failed.running = Err("tasklist failed");
        failed.stop = Err("cleanup failed");
        let error = update_using_launcher(&mut failed, "1.124.0", "1.100.0", 2, INTERVAL, 10, 80)
            .unwrap_err();
        assert!(error.0.contains("tasklist failed") && error.0.contains("cleanup failed"));
        assert_eq!(failed.stops, 1);
    }

    #[test]
    fn zero_iterations_and_bad_ranges_do_not_panic() {
        let mut backend = Fake::new(&[]);
        assert!(
            update_using_launcher(&mut backend, "1.124.0", "none", 0, INTERVAL, 10, 80)
                .unwrap_err()
                .0
                .contains("timed out")
        );
        assert_eq!(backend.stops, 1);
        assert!(
            update_using_launcher(&mut backend, "1.124.0", "none", 1, INTERVAL, 90, 20).is_err()
        );
        assert_eq!(backend.starts, 1);
    }
}
