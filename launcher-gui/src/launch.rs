//! Start Zwift with the Windows launcher as its parent process.

use crate::logging::Logger;
use crate::wine_update::Tools;
use std::io;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

pub struct LaunchConfig<'a> {
    pub wine_prefix: &'a Path,
    pub install_dir: &'a Path,
    pub runfromprocess: &'a Path,
    pub wait_for_window: &'a Path,
    pub tools: &'a Tools,
    /// Path to gamemoderun when enabled and available.
    pub gamemoderun: Option<&'a Path>,
    pub game_args: &'a [String],
    /// OAuth token file written by the GUI login; its JSON is passed to the
    /// game at launch time, after login has completed.
    pub token_file: Option<&'a Path>,
    pub startup_attempts: u32,
    pub startup_interval: Duration,
    pub game_poll_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOutcome {
    Visible,
    TimedOut,
    NotRequested,
}

fn wine(config: &LaunchConfig<'_>) -> Command {
    let mut command = Command::new(&config.tools.wine);
    command.env("WINEPREFIX", config.wine_prefix);
    command
}

fn command_output(logger: &Logger, command: &mut Command, name: &str) -> io::Result<Vec<u8>> {
    let output = logger.command_output(command)?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "{name} failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(output.stdout)
}

fn wine_directory(config: &LaunchConfig<'_>, logger: &Logger) -> io::Result<String> {
    let output = command_output(
        logger,
        Command::new(&config.tools.winepath)
            .env("WINEPREFIX", config.wine_prefix)
            .arg("-w")
            .arg(config.install_dir),
        "winepath",
    )?;
    let path = std::str::from_utf8(&output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?
        .trim();
    if path.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "winepath returned an empty path",
        ));
    }
    Ok(path.to_owned())
}

fn task_pid(config: &LaunchConfig<'_>, logger: &Logger, name: &str) -> io::Result<Option<u32>> {
    let output = command_output(
        logger,
        wine(config).args([
            "tasklist",
            "/fo",
            "list",
            "/fi",
            &format!("IMAGENAME eq {name}"),
        ]),
        "wine tasklist",
    )?;
    let text = std::str::from_utf8(&output)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    for line in text.lines() {
        if let Some(pid) = line.strip_prefix("PID:") {
            let pid = pid
                .trim()
                .parse::<u32>()
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            if pid == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "wine tasklist returned PID 0",
                ));
            }
            return Ok(Some(pid));
        }
    }
    Ok(None)
}

fn wait_for_task(config: &LaunchConfig<'_>, logger: &Logger, name: &str) -> io::Result<u32> {
    for attempt in 0..=config.startup_attempts {
        if let Some(pid) = task_pid(config, logger, name)? {
            return Ok(pid);
        }
        if attempt < config.startup_attempts {
            logger.cancellation().wait(config.startup_interval)?;
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("{name} did not start"),
    ))
}

fn run_command(command: &mut Command, logger: &Logger, label: &str) -> io::Result<()> {
    let status = logger.run_inherited_output(command)?;
    if !status.success() {
        return Err(io::Error::other(format!("{label} failed ({status})")));
    }
    Ok(())
}

fn stop_background_tasks(config: &LaunchConfig<'_>, logger: &Logger) {
    for task in ["ZwiftLauncher.exe", "ZwiftWindowsCrashHandler.exe"] {
        match logger.command_output(wine(config).args(["taskkill", "/f", "/im", task])) {
            Ok(output) if output.status.success() => {}
            Ok(output) => logger.warning(&format!(
                "Unable to stop {task} ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => logger.warning(&format!("Unable to stop {task}: {error}")),
        }
    }
}

/// Read the saved OAuth token and turn it into a `--token=<json>` argument.
///
/// The file is read at launch time (after login) rather than at startup so a
/// fresh token from the login dialog reaches the game. When a file is required,
/// a missing or invalid token must not silently launch an unauthenticated game.
fn token_arguments(token_file: Option<&Path>) -> io::Result<Vec<String>> {
    let Some(token_file) = token_file else {
        return Ok(Vec::new());
    };
    let text = std::fs::read_to_string(token_file).map_err(|error| {
        io::Error::other(format!(
            "Unable to read Zwift login token {}: {error}",
            token_file.display()
        ))
    })?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| {
        io::Error::other(format!(
            "Invalid Zwift login token in {}: {error}",
            token_file.display()
        ))
    })?;
    if value["access_token"].as_str().is_none_or(str::is_empty) {
        return Err(io::Error::other("Zwift login token has no access token"));
    }
    // Zwift expects the token JSON itself, compact, not a file path.
    let json = serde_json::to_string(&value)
        .map_err(|error| io::Error::other(format!("Unable to serialize login token: {error}")))?;
    Ok(vec![format!("--token={json}")])
}

/// Starts the game and optionally waits for its first window.
///
/// The caller closes its progress UI after this returns, then calls
/// [`wait_for_exit`]. A window timeout is not a launch failure.
pub fn start(
    config: &LaunchConfig<'_>,
    logger: &Logger,
    wait_for_window: bool,
    progress: &mut impl FnMut(u8, &str),
) -> io::Result<WindowOutcome> {
    let token_args = token_arguments(config.token_file)?;
    let directory = wine_directory(config, logger)?;
    progress(95, "Starting Zwift");
    run_command(
        wine(config).args([
            "start",
            "/d",
            &directory,
            "ZwiftLauncher.exe",
            "SilentLaunch",
        ]),
        logger,
        "Zwift launcher",
    )?;

    let result: io::Result<()> = (|| {
        let launcher_pid = wait_for_task(config, logger, "ZwiftLauncher.exe")?;
        logger.info("Starting Zwift using wine");
        let mut game = match config.gamemoderun {
            Some(wrapper) => {
                logger.info("Using gamemode");
                let mut cmd = Command::new(wrapper);
                cmd.arg(&config.tools.wine)
                    .env("WINEPREFIX", config.wine_prefix);
                cmd
            }
            None => {
                logger.info("Not using gamemode");
                wine(config)
            }
        };
        game.args(["start", "/d", &directory, "/unix"])
            .arg(config.runfromprocess)
            .arg(launcher_pid.to_string())
            .arg("ZwiftApp.exe")
            .args(config.game_args)
            .args(&token_args);
        run_command(&mut game, logger, "Zwift")?;
        wait_for_task(config, logger, "ZwiftApp.exe")?;
        Ok(())
    })();
    stop_background_tasks(config, logger);
    result?;
    logger.ok("Zwift started using wine");

    if !wait_for_window {
        return Ok(WindowOutcome::NotRequested);
    }
    progress(99, "Waiting for the Zwift window");
    let game_pid = task_pid(config, logger, "ZwiftApp.exe")?
        .ok_or_else(|| io::Error::other("Zwift exited before its window appeared"))?;
    let status = logger.run_command(
        wine(config)
            .arg(config.wait_for_window)
            .arg(game_pid.to_string()),
    )?;
    match status.code() {
        Some(0) => {
            logger.ok("Zwift window is visible");
            Ok(WindowOutcome::Visible)
        }
        Some(2) => {
            logger.warning("Timed out waiting for the Zwift window; leaving Zwift running");
            Ok(WindowOutcome::TimedOut)
        }
        _ => Err(io::Error::other(format!(
            "Unable to wait for the Zwift window ({status})"
        ))),
    }
}

/// Wait until Zwift exits; tasklist errors are returned rather than treated as exit.
pub fn wait_for_exit(config: &LaunchConfig<'_>, logger: &Logger) -> io::Result<()> {
    while task_pid(config, logger, "ZwiftApp.exe")?.is_some() {
        logger.cancellation().wait(config.game_poll_interval)?;
    }
    logger.info("Zwift closed, exiting");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    struct Fixture {
        dir: PathBuf,
        tools: Tools,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    fn write_executable(path: &Path, contents: &str) {
        let temporary = path.with_extension("script-tmp");
        fs::write(&temporary, contents).unwrap();
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(temporary, path).unwrap();
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let lock = crate::SCRIPT_TEST_LOCK.lock().unwrap();
            let dir =
                std::env::temp_dir().join(format!("zwift-launch-{name}-{}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let tools = Tools {
                wine: dir.join("wine"),
                winepath: dir.join("winepath"),
                wineserver: dir.join("wineserver"),
                wget: dir.join("wget"),
            };
            write_executable(&tools.winepath, "#!/bin/sh\nprintf 'C:\\\\Zwift\\n'\n");
            write_executable(
                &tools.wine,
                &format!(
                    r#"#!/bin/sh
set -eu
test "$WINEPREFIX" = '{0}/prefix'
printf '%s\n' "$*" >> '{0}/calls'
case "$1" in
    start)
        if [ "$4" = ZwiftLauncher.exe ]; then
            test ! -f '{0}/fail-launcher'
        else
            test "$4" = /unix
            test "$5" = '{0}/runfromprocess.exe'
            test "$6" = 4141
            test "$7" = ZwiftApp.exe
            test ! -f '{0}/fail-game'
        fi;;
    tasklist)
        test ! -f '{0}/fail-tasklist'
        case "$5" in
            *ZwiftLauncher.exe) test ! -f '{0}/missing-launcher' && printf 'PID: 4141\n' || true;;
            *ZwiftApp.exe)
                if [ -f '{0}/exit-after-start' ]; then
                    if [ -f '{0}/game-seen' ]; then exit 0; fi
                    : > '{0}/game-seen'
                fi
                test ! -f '{0}/game-exited' && printf 'PID: 777\n' || true;;
        esac;;
    taskkill) :;;
    '{0}/window.exe') test "$2" = 777; echo window-output; if [ -f '{0}/window-result' ]; then exit "$(cat '{0}/window-result')"; fi;;
    *) exit 99;;
esac
"#,
                    dir.display()
                ),
            );
            Self {
                dir,
                tools,
                _lock: lock,
            }
        }

        fn config(&self) -> LaunchConfig<'_> {
            LaunchConfig {
                wine_prefix: &self.dir,
                install_dir: &self.dir,
                runfromprocess: &self.dir,
                wait_for_window: &self.dir,
                tools: &self.tools,
                gamemoderun: None,
                game_args: &[],
                token_file: None,
                startup_attempts: 1,
                startup_interval: Duration::ZERO,
                game_poll_interval: Duration::ZERO,
            }
        }

        fn run(&self, window: bool) -> (io::Result<WindowOutcome>, Vec<(u8, String)>) {
            let prefix = self.dir.join("prefix");
            let runfromprocess = self.dir.join("runfromprocess.exe");
            let wait_for_window = self.dir.join("window.exe");
            let config = LaunchConfig {
                wine_prefix: &prefix,
                runfromprocess: &runfromprocess,
                wait_for_window: &wait_for_window,
                ..self.config()
            };
            let logger = Logger::new(self.dir.join("log"), 0, false);
            logger.init().unwrap();
            let mut events = Vec::new();
            let result = start(&config, &logger, window, &mut |p, m| {
                events.push((p, m.into()))
            });
            (result, events)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }

    #[test]
    fn starts_game_with_launcher_pid_and_waits_for_window() {
        let fixture = Fixture::new("success");
        let (result, events) = fixture.run(true);
        assert_eq!(result.unwrap(), WindowOutcome::Visible);
        assert_eq!(
            events.iter().map(|event| event.0).collect::<Vec<_>>(),
            [95, 99]
        );
        let calls = fs::read_to_string(fixture.dir.join("calls")).unwrap();
        assert!(calls.contains("ZwiftLauncher.exe SilentLaunch"));
        assert!(calls.contains("runfromprocess.exe 4141 ZwiftApp.exe"));
        assert!(calls.contains("taskkill /f /im ZwiftLauncher.exe"));
        assert!(calls.contains("window.exe 777"));
        assert!(fs::read_to_string(fixture.dir.join("log"))
            .unwrap()
            .contains("window-output"));
    }

    #[test]
    fn headless_skips_window_wait() {
        let fixture = Fixture::new("headless");
        let (result, events) = fixture.run(false);
        assert_eq!(result.unwrap(), WindowOutcome::NotRequested);
        assert_eq!(events.len(), 1);
        assert!(!fs::read_to_string(fixture.dir.join("calls"))
            .unwrap()
            .contains("window.exe"));
    }

    #[test]
    fn window_timeout_is_nonfatal_but_other_failures_are_reported() {
        for (name, code) in [("timeout", "2"), ("failure", "1")] {
            let fixture = Fixture::new(name);
            fs::write(fixture.dir.join("window-result"), code).unwrap();
            let (result, _) = fixture.run(true);
            if code == "2" {
                assert_eq!(result.unwrap(), WindowOutcome::TimedOut);
            } else {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("Unable to wait for the Zwift window"));
            }
        }
    }

    #[test]
    fn game_disappearing_before_window_is_an_error() {
        let fixture = Fixture::new("gone");
        fs::write(fixture.dir.join("exit-after-start"), "").unwrap();
        let (result, _) = fixture.run(true);
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("exited before its window"));
        assert!(!fs::read_to_string(fixture.dir.join("calls"))
            .unwrap()
            .contains("window.exe"));
    }

    #[test]
    fn startup_errors_stop_background_tasks() {
        for marker in ["missing-launcher", "fail-game", "fail-tasklist"] {
            let fixture = Fixture::new(marker);
            fs::write(fixture.dir.join(marker), "").unwrap();
            let (result, _) = fixture.run(true);
            assert!(result.is_err());
            let calls = fs::read_to_string(fixture.dir.join("calls")).unwrap();
            assert!(calls.contains("taskkill /f /im ZwiftLauncher.exe"));
            assert!(calls.contains("taskkill /f /im ZwiftWindowsCrashHandler.exe"));
            assert!(!calls.contains("window.exe"));
        }
    }

    #[test]
    fn gamemode_wraps_wine_and_forwards_game_arguments() {
        let fixture = Fixture::new("gamemode");
        let wrapper = fixture.dir.join("gamemoderun");
        write_executable(
            &wrapper,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}/wrapper-call'\nexec \"$@\"\n",
                fixture.dir.display()
            ),
        );
        let prefix = fixture.dir.join("prefix");
        let runfromprocess = fixture.dir.join("runfromprocess.exe");
        let window = fixture.dir.join("window.exe");
        let args = ["--test-argument".to_owned()];
        let config = LaunchConfig {
            wine_prefix: &prefix,
            runfromprocess: &runfromprocess,
            wait_for_window: &window,
            gamemoderun: Some(&wrapper),
            game_args: &args,
            ..fixture.config()
        };
        let logger = Logger::new(fixture.dir.join("log"), 0, false);
        logger.init().unwrap();
        assert_eq!(
            start(&config, &logger, false, &mut |_, _| {}).unwrap(),
            WindowOutcome::NotRequested
        );
        let call = fs::read_to_string(fixture.dir.join("wrapper-call")).unwrap();
        assert!(call.starts_with(&fixture.tools.wine.to_string_lossy().to_string()));
        assert!(call.contains("ZwiftApp.exe --test-argument"));
    }

    #[test]
    fn token_is_read_at_launch_and_forwarded_as_compact_json() {
        let fixture = Fixture::new("token");
        let prefix = fixture.dir.join("prefix");
        let runfromprocess = fixture.dir.join("runfromprocess.exe");
        let token_file = fixture.dir.join("token.json");
        let config = LaunchConfig {
            wine_prefix: &prefix,
            runfromprocess: &runfromprocess,
            token_file: Some(&token_file),
            ..fixture.config()
        };
        // Login writes the file after the launch configuration has been created.
        fs::write(&token_file, "{\n  \"access_token\": \"test-only\"\n}").unwrap();
        let logger = Logger::new(fixture.dir.join("log"), 0, false);
        logger.init().unwrap();
        start(&config, &logger, false, &mut |_, _| {}).unwrap();
        let calls = fs::read_to_string(fixture.dir.join("calls")).unwrap();
        assert!(calls.contains("ZwiftApp.exe --token={\"access_token\":\"test-only\"}"));
        assert!(!calls.contains(&format!("--token={}", token_file.display())));
    }

    #[test]
    fn required_token_errors_prevent_game_start() {
        let fixture = Fixture::new("invalid-token");
        let token_file = fixture.dir.join("token.json");
        let config = LaunchConfig {
            token_file: Some(&token_file),
            ..fixture.config()
        };
        let logger = Logger::new(fixture.dir.join("log"), 0, false);
        logger.init().unwrap();
        assert!(start(&config, &logger, false, &mut |_, _| {}).is_err());
        for contents in ["not JSON", "{}", "{\"access_token\":\"\"}"] {
            fs::write(&token_file, contents).unwrap();
            assert!(start(&config, &logger, false, &mut |_, _| {}).is_err());
        }
        assert!(!fixture.dir.join("calls").exists());
    }

    #[test]
    fn failed_launcher_command_does_not_start_game() {
        let fixture = Fixture::new("fail-launcher");
        fs::write(fixture.dir.join("fail-launcher"), "").unwrap();
        let (result, _) = fixture.run(true);
        assert!(result.unwrap_err().to_string().contains("Zwift launcher"));
        let calls = fs::read_to_string(fixture.dir.join("calls")).unwrap();
        assert!(!calls.contains("runfromprocess.exe"));
        assert!(!calls.contains("window.exe"));
    }

    #[test]
    fn exit_wait_distinguishes_tasklist_error_from_exit() {
        let fixture = Fixture::new("wait");
        let prefix = fixture.dir.join("prefix");
        let config = LaunchConfig {
            wine_prefix: &prefix,
            ..fixture.config()
        };
        let logger = Logger::new(fixture.dir.join("log"), 0, false);
        logger.init().unwrap();
        fs::write(fixture.dir.join("game-exited"), "").unwrap();
        wait_for_exit(&config, &logger).unwrap();
        fs::write(fixture.dir.join("fail-tasklist"), "").unwrap();
        assert!(wait_for_exit(&config, &logger).is_err());
    }
}
