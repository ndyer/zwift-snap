//! Launcher lifecycle and user decisions, independent of the GUI toolkit.

use crate::cancel::Cancellation;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    Install,
    Update,
}

/// The UI decides whether to retry a failed install or launch after a failed update.
pub trait LauncherUi {
    fn failure(&mut self, failure: Failure, detail: &str) -> bool;
    fn game_ready(&mut self);
    /// Called before the game is started. Return `false` to abort the launch.
    /// The default permits launching without login (terminal and preview runs).
    fn login_required(&mut self) -> bool {
        true
    }
}

/// Operations performed by the launcher in lifecycle order.
pub trait LauncherBackend {
    type Error: fmt::Display;

    fn prepare(&mut self) -> Result<(), Self::Error>;
    /// Whether a complete Zwift installation is present, including the Wine
    /// prerequisites. A `false` result makes the lifecycle take the install path.
    fn installed(&mut self) -> Result<bool, Self::Error>;
    fn install(&mut self) -> Result<(), Self::Error>;
    fn update(&mut self) -> Result<(), Self::Error>;
    fn configure_display(&mut self) -> Result<(), Self::Error>;
    fn start(&mut self) -> Result<(), Self::Error>;
    fn wait_for_exit(&mut self) -> Result<(), Self::Error>;
    fn cleanup(&mut self) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct LauncherError(pub String);

impl fmt::Display for LauncherError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for LauncherError {}

/// Run the launcher and attempt cleanup even if preparation or startup fails.
pub fn run<B: LauncherBackend, U: LauncherUi>(
    backend: &mut B,
    ui: &mut U,
) -> Result<(), LauncherError> {
    run_cancellable(backend, ui, &Cancellation::default())
}

/// Run a lifecycle with cooperative cancellation and unconditional cleanup.
pub fn run_cancellable<B: LauncherBackend, U: LauncherUi>(
    backend: &mut B,
    ui: &mut U,
    cancellation: &Cancellation,
) -> Result<(), LauncherError> {
    let result = run_inner(backend, ui, cancellation);
    match (result, backend.cleanup()) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(LauncherError(format!("Cleanup failed: {error}"))),
        (Err(error), Err(cleanup)) => {
            Err(LauncherError(format!("{error}; cleanup failed: {cleanup}")))
        }
    }
}

fn run_inner<B: LauncherBackend, U: LauncherUi>(
    backend: &mut B,
    ui: &mut U,
    cancellation: &Cancellation,
) -> Result<(), LauncherError> {
    let check = || {
        cancellation
            .check()
            .map_err(|error| LauncherError(error.to_string()))
    };
    check()?;
    backend
        .prepare()
        .map_err(|error| LauncherError(format!("Launcher preparation failed: {error}")))?;
    check()?;
    if !backend
        .installed()
        .map_err(|error| LauncherError(format!("Installation check failed: {error}")))?
    {
        loop {
            check()?;
            match backend.install() {
                Ok(()) => break,
                Err(error) => {
                    check()?;
                    if !ui.failure(Failure::Install, &error.to_string()) {
                        check()?;
                        return Err(LauncherError(format!("Zwift installation failed: {error}")));
                    }
                }
            }
        }
    } else if let Err(error) = backend.update() {
        check()?;
        if !ui.failure(Failure::Update, &error.to_string()) {
            check()?;
            return Err(LauncherError(format!("Zwift update failed: {error}")));
        }
    }
    check()?;
    if !backend
        .installed()
        .map_err(|error| LauncherError(format!("Installation check failed: {error}")))?
    {
        return Err(LauncherError(
            "ZwiftApp.exe or ZwiftLauncher.exe is missing".into(),
        ));
    }
    backend.configure_display().map_err(|error| {
        LauncherError(format!("Failed to configure Zwift display mode: {error}"))
    })?;
    check()?;
    if !ui.login_required() {
        check()?;
        return Err(LauncherError(
            "Zwift login is required before launching".into(),
        ));
    }
    check()?;
    backend
        .start()
        .map_err(|error| LauncherError(format!("Failed to start Zwift: {error}")))?;
    check()?;
    ui.game_ready();
    let result = backend
        .wait_for_exit()
        .map_err(|error| LauncherError(format!("Failed while waiting for Zwift: {error}")));
    result?;
    check()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct Fake {
        calls: Vec<&'static str>,
        installed: VecDeque<bool>,
        installs: VecDeque<Result<(), &'static str>>,
        update: Result<(), &'static str>,
        start: Result<(), &'static str>,
        cleanup: Result<(), &'static str>,
    }

    impl Fake {
        fn new(installed: &[bool]) -> Self {
            Self {
                calls: Vec::new(),
                installed: installed.iter().copied().collect(),
                installs: VecDeque::new(),
                update: Ok(()),
                start: Ok(()),
                cleanup: Ok(()),
            }
        }
    }

    impl LauncherBackend for Fake {
        type Error = &'static str;
        fn prepare(&mut self) -> Result<(), Self::Error> {
            self.calls.push("prepare");
            Ok(())
        }
        fn installed(&mut self) -> Result<bool, Self::Error> {
            self.calls.push("installed");
            Ok(self.installed.pop_front().unwrap())
        }
        fn install(&mut self) -> Result<(), Self::Error> {
            self.calls.push("install");
            self.installs.pop_front().unwrap()
        }
        fn update(&mut self) -> Result<(), Self::Error> {
            self.calls.push("update");
            self.update
        }
        fn configure_display(&mut self) -> Result<(), Self::Error> {
            self.calls.push("display");
            Ok(())
        }
        fn start(&mut self) -> Result<(), Self::Error> {
            self.calls.push("start");
            self.start
        }
        fn wait_for_exit(&mut self) -> Result<(), Self::Error> {
            self.calls.push("wait");
            Ok(())
        }
        fn cleanup(&mut self) -> Result<(), Self::Error> {
            self.calls.push("cleanup");
            self.cleanup
        }
    }

    #[derive(Default)]
    struct Ui {
        decisions: VecDeque<bool>,
        failures: Vec<Failure>,
        ready: bool,
    }
    impl LauncherUi for Ui {
        fn failure(&mut self, failure: Failure, _: &str) -> bool {
            self.failures.push(failure);
            self.decisions.pop_front().unwrap()
        }
        fn game_ready(&mut self) {
            self.ready = true;
        }
    }

    #[test]
    fn fresh_install_retries_then_starts_without_updating_again() {
        let mut backend = Fake::new(&[false, true]);
        backend.installs.extend([Err("failed"), Ok(())]);
        let mut ui = Ui {
            decisions: [true].into(),
            ..Ui::default()
        };
        run(&mut backend, &mut ui).unwrap();
        assert_eq!(
            backend.calls,
            [
                "prepare",
                "installed",
                "install",
                "install",
                "installed",
                "display",
                "start",
                "wait",
                "cleanup"
            ]
        );
        assert_eq!(ui.failures, [Failure::Install]);
        assert!(ui.ready);
    }

    #[test]
    fn failed_update_can_launch_anyway_or_quit() {
        for launch in [true, false] {
            let mut backend = Fake::new(&[true, true]);
            backend.update = Err("offline");
            let mut ui = Ui {
                decisions: [launch].into(),
                ..Ui::default()
            };
            assert_eq!(run(&mut backend, &mut ui).is_ok(), launch);
            assert_eq!(ui.ready, launch);
            assert_eq!(backend.calls.last(), Some(&"cleanup"));
            assert_eq!(backend.calls.contains(&"start"), launch);
        }
    }

    #[test]
    fn missing_game_is_never_launched_even_after_successful_install() {
        let mut backend = Fake::new(&[false, false]);
        backend.installs.push_back(Ok(()));
        let mut ui = Ui::default();
        assert!(run(&mut backend, &mut ui)
            .unwrap_err()
            .0
            .contains("missing"));
        assert_eq!(backend.calls.last(), Some(&"cleanup"));
        assert!(!backend.calls.contains(&"start"));
    }

    #[test]
    fn cleanup_failure_is_reported_alongside_start_failure() {
        let mut backend = Fake::new(&[true, true]);
        backend.start = Err("start error");
        backend.cleanup = Err("cleanup error");
        let mut ui = Ui::default();
        let error = run(&mut backend, &mut ui).unwrap_err();
        assert!(error.0.contains("start error") && error.0.contains("cleanup error"));
        assert!(!ui.ready);
    }
}
