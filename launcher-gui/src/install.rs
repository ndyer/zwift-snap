//! Fresh Wine prefix and Zwift installation.

use crate::logging::{Logger, OutputObserver};
use crate::update::{update_using_launcher, UpdateBackend};
use crate::wine_update::{Tools, WineUpdate};
use std::collections::HashSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const SETUP_URL: &str = "https://cdn.zwift.com/app/ZwiftSetup.exe";
pub const EXPECTED_DRIVE_C_BYTES: u64 = 8_537_613_774;

/// The Wine prerequisites, installed by a single winetricks invocation.
///
/// winetricks runs every command-line verb in one process, in order, and exits
/// non-zero on the first failing verb. One call therefore installs all of them and
/// still stops the install before Zwift is downloaded, while separate calls would
/// start the script, probe Wine and initialise the prefix once per verb. The order
/// matters: `dotnet48` selects the Windows version the later verbs expect.
///
/// The percentage is the terminal-mode progress of each verb; the GUI replaces it
/// with the percentage of the sampled prefix size.
const WINETRICKS_PREREQUISITES: [(u8, &str, &str); 4] = [
    (
        8,
        "Installing .NET Framework 4.8, this takes several minutes",
        "dotnet48",
    ),
    (
        55,
        "Installing the Direct3D shader compiler",
        "d3dcompiler_47",
    ),
    (60, "Installing WebView2", "webview2"),
    (
        75,
        "Enabling the Vulkan renderer for video routes",
        "renderer=vulkan",
    ),
];

/// Title shown until winetricks announces its first verb, and kept as a fallback
/// when its output cannot be read.
const WINETRICKS_PREREQUISITES_MESSAGE: &str = "Installing the wine prerequisites (.NET Framework 4.8, the Direct3D shader compiler, WebView2 and the Vulkan renderer), this takes several minutes";

/// The line winetricks prints (to stderr) before running each verb.
const WINETRICKS_VERB_MARKER: &str = "Executing w_do_call ";

/// Phase messages are independent of sampled percentages during a fresh install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallProgress {
    Phase { percent: u8, message: String },
    Sample(u8),
}

pub struct InstallConfig<'a> {
    pub wine_prefix: &'a Path,
    pub user_common: &'a Path,
    pub install_dir: &'a Path,
    pub wineboot: &'a Path,
    pub winetricks: &'a Path,
    pub tools: &'a Tools,
    pub download_iterations: u32,
    pub poll_interval: Duration,
    /// Set to zero to disable sampling, for example when running without a GUI.
    pub expected_drive_c_bytes: u64,
    pub sample_interval: Duration,
}

fn run(command: &mut Command, logger: &Logger, label: &str) -> io::Result<()> {
    finish(logger.run_inherited_output(command)?, label)
}

/// Like `run`, but observes the child's output while it runs.
fn run_observed(
    command: &mut Command,
    logger: &Logger,
    label: &str,
    observer: OutputObserver,
) -> io::Result<()> {
    finish(
        logger.run_inherited_output_observed(command, observer)?,
        label,
    )
}

fn finish(status: ExitStatus, label: &str) -> io::Result<()> {
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{label} failed ({status})")))
    }
}

fn missing_ok_remove_file(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Remove only installer-owned caches; never follow a directory symlink.
pub fn cleanup_artifacts(prefix: &Path, common: &Path, cache: &Path) -> io::Result<()> {
    missing_ok_remove_file(&common.join("ZwiftSetup.exe"))?;
    remove_artifact(&cache.join("winetricks"))?;
    if let Some(user) = resolve_wine_user(prefix)? {
        let temp = user.join("AppData/Local/Temp");
        let relative = temp.strip_prefix(prefix).map_err(io::Error::other)?;
        let mut ancestor = prefix.to_path_buf();
        for component in relative.components() {
            ancestor.push(component);
            match fs::symlink_metadata(&ancestor) {
                Ok(metadata) if metadata.is_symlink() => {
                    return Err(io::Error::other(format!(
                        "Refusing to clean Wine Temp through symlink {}",
                        ancestor.display()
                    )));
                }
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            }
        }
        match fs::symlink_metadata(&temp) {
            Ok(metadata) if metadata.is_symlink() => {
                return Err(io::Error::other(
                    "Refusing to clean a symlinked Wine Temp directory",
                ));
            }
            Ok(_) => {
                for entry in fs::read_dir(temp)? {
                    remove_artifact(&entry?.path())?;
                }
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn remove_artifact(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// Find the first Wine user other than Public, matching the shell launcher.
pub fn resolve_wine_user(prefix: &Path) -> io::Result<Option<std::path::PathBuf>> {
    let users = prefix.join("drive_c/users");
    let entries = match fs::read_dir(&users) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut candidates = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_name() != "Public" && entry.path().is_dir() {
            candidates.push(entry.path());
        }
    }
    candidates.sort();
    Ok(candidates.into_iter().next())
}

/// Replace Wine user directory symlinks with private directories without touching their targets.
pub fn make_user_dirs_private(prefix: &Path, logger: &Logger) -> io::Result<()> {
    if let Some(user) = resolve_wine_user(prefix)? {
        for entry in fs::read_dir(user)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                let path = entry.path();
                let target = fs::read_link(&path)?;
                logger.info(&format!(
                    "Keeping {} private in the wine prefix instead of linking to {}",
                    entry.file_name().to_string_lossy(),
                    target.display()
                ));
                fs::remove_file(&path)?;
                fs::create_dir(&path)?;
            }
        }
    }
    Ok(())
}

/// Apparent size like `du -sb`, without following symlinks or counting hardlinks twice.
fn apparent_size(path: &Path, seen: &mut HashSet<(u64, u64)>) -> io::Result<u64> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    if !seen.insert((metadata.dev(), metadata.ino())) {
        return Ok(0);
    }
    let mut size = metadata.len();
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            size = size.saturating_add(apparent_size(&entry?.path(), seen)?);
        }
    }
    Ok(size)
}

fn sample_percent(bytes: u64, expected: u64, previous: u8) -> u8 {
    let percent = (u128::from(bytes) * 99 / u128::from(expected)).min(99) as u8;
    percent.max(previous)
}

/// Install Wine prerequisites, the Zwift launcher, and the game.
///
/// The progress callback must be thread-safe: size samples arrive from a worker
/// thread and winetricks phase updates from an output reader thread, while the
/// remaining phase updates arrive from the calling thread.
pub fn install(
    config: &InstallConfig<'_>,
    logger: &Logger,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> io::Result<()> {
    if !config.wine_prefix.is_absolute()
        || !config.user_common.is_absolute()
        || !config.install_dir.is_absolute()
    {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "Wine prefix, snap data and Zwift installation paths must be absolute",
        ));
    }
    if config.expected_drive_c_bytes > 0 && config.sample_interval.is_zero() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            "install progress sample interval must be nonzero",
        ));
    }
    let sampled = std::sync::Mutex::new(0);
    let report = |event| {
        let mut sampled = sampled.lock().expect("install progress lock poisoned");
        let event = match event {
            InstallProgress::Sample(percent) => {
                *sampled = percent;
                InstallProgress::Sample(percent)
            }
            InstallProgress::Phase { message, .. } if config.expected_drive_c_bytes > 0 => {
                InstallProgress::Phase {
                    percent: *sampled,
                    message,
                }
            }
            event => event,
        };
        progress(event);
    };
    let progress = &report;
    std::thread::scope(|scope| {
        let (stop, receiver) = mpsc::channel();
        let (ready, ready_receiver) = mpsc::sync_channel(0);
        let sampler = if config.expected_drive_c_bytes > 0 {
            Some(scope.spawn(move || {
                let mut previous = 0;
                let mut milestone = 0;
                let mut ready = Some(ready);
                loop {
                    match apparent_size(&config.wine_prefix.join("drive_c"), &mut HashSet::new()) {
                        Ok(bytes) => {
                            previous =
                                sample_percent(bytes, config.expected_drive_c_bytes, previous);
                            progress(InstallProgress::Sample(previous));
                            if previous / 10 > milestone {
                                milestone = previous / 10;
                                logger.info(&format!("Install progress: {previous}%"));
                            }
                        }
                        Err(error) => {
                            logger.warning(&format!("Unable to sample install progress: {error}"))
                        }
                    }
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(());
                    }
                    match receiver.recv_timeout(config.sample_interval) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                }
            }))
        } else {
            None
        };
        if sampler.is_some() {
            ready_receiver
                .recv()
                .expect("install progress sampler failed to start");
        }
        let result = install_steps(config, logger, progress);
        drop(stop);
        if let Some(sampler) = sampler {
            sampler.join().expect("install progress sampler panicked");
        }
        result
    })
}

/// Derives per-verb progress messages from winetricks' own output.
///
/// winetricks offers no other progress signal for the prerequisites, so the
/// install dialog's per-verb titles are read from the `Executing w_do_call <verb>`
/// lines it prints while it works. Every one of those lines comes from its single
/// stderr stream, so their order is preserved even though the two output reader
/// threads race with each other.
struct WinetricksProgress {
    pending: Vec<u8>,
    next: usize,
}

impl WinetricksProgress {
    fn new() -> Self {
        Self {
            pending: Vec::new(),
            next: 0,
        }
    }

    /// Consumes a chunk of output and returns the phases it completed.
    ///
    /// Output arrives in arbitrary chunks, so a trailing partial line is kept until
    /// the rest of it has been read.
    fn feed(&mut self, bytes: &[u8]) -> Vec<(u8, &'static str)> {
        self.pending.extend_from_slice(bytes);
        let mut phases = Vec::new();
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=end).collect::<Vec<u8>>();
            line.pop();
            let line = String::from_utf8_lossy(&line);
            if let Some(verb) = line
                .trim_end_matches('\r')
                .strip_prefix(WINETRICKS_VERB_MARKER)
            {
                self.report(verb, &mut phases);
            }
        }
        phases
    }

    /// Reports each configured verb once, in the configured order.
    ///
    /// winetricks also announces the nested verbs it runs for a prerequisite (for
    /// example `dotnet40` inside `dotnet48`), so only the configured verbs count.
    /// Anything already reported, out of order or unknown is ignored; a verb that
    /// was missed entirely is skipped rather than reported late.
    fn report(&mut self, verb: &str, phases: &mut Vec<(u8, &'static str)>) {
        for (index, (percent, message, expected)) in
            WINETRICKS_PREREQUISITES.iter().enumerate().skip(self.next)
        {
            if verb == *expected {
                self.next = index + 1;
                phases.push((*percent, *message));
                return;
            }
        }
    }
}

/// Install the Wine prerequisites with a single winetricks invocation.
fn install_prerequisites(
    config: &InstallConfig<'_>,
    logger: &Logger,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> io::Result<()> {
    progress(InstallProgress::Phase {
        percent: WINETRICKS_PREREQUISITES[0].0,
        message: WINETRICKS_PREREQUISITES_MESSAGE.to_string(),
    });

    let verbs: Vec<&str> = WINETRICKS_PREREQUISITES
        .iter()
        .map(|(_, _, verb)| *verb)
        .collect();
    let label = format!("winetricks {}", verbs.join(" "));
    let mut command = Command::new(config.winetricks);
    command
        .env("WINEPREFIX", config.wine_prefix)
        // -q only selects unattended installs. It does not silence the verb markers
        // the titles below are read from; that is WINETRICKS_SUPER_QUIET.
        .arg("-q")
        .args(&verbs);

    let (sender, receiver) = mpsc::channel();
    let sender = Mutex::new(sender);
    let parser = Mutex::new(WinetricksProgress::new());
    let done = Arc::new(AtomicBool::new(false));
    let observer = {
        let done = Arc::clone(&done);
        Arc::new(move |bytes: &[u8]| {
            if done.load(Ordering::Acquire) {
                return;
            }
            let phases = parser
                .lock()
                .expect("winetricks progress lock poisoned")
                .feed(bytes);
            let sender = sender.lock().expect("winetricks sender lock poisoned");
            for phase in phases {
                let _ = sender.send(phase);
            }
        })
    };

    std::thread::scope(|scope| {
        let reporter_done = Arc::clone(&done);
        let reporter = scope.spawn(move || {
            loop {
                match receiver.recv_timeout(Duration::from_millis(50)) {
                    Ok((percent, message)) => progress(InstallProgress::Phase {
                        percent,
                        message: message.to_string(),
                    }),
                    // Drain everything winetricks sent before stopping: the markers
                    // arrive just before it exits.
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if reporter_done.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        let result = run_observed(&mut command, logger, &label, observer);
        // Stop the reporter before returning: it holds the only borrow of the
        // progress callback, and the caller emits the next phase right after.
        done.store(true, Ordering::Release);
        reporter
            .join()
            .expect("winetricks progress reporter panicked");
        result
    })
}

fn install_steps(
    config: &InstallConfig<'_>,
    logger: &Logger,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> io::Result<()> {
    let phase = |percent, message: &str| {
        progress(InstallProgress::Phase {
            percent,
            message: message.to_string(),
        })
    };
    phase(2, "Setting up wine");
    run(
        Command::new(config.wineboot)
            .env("WINEPREFIX", config.wine_prefix)
            .env("WINEDLLOVERRIDES", "mscoree,mshtml=;winemenubuilder.exe=d")
            .arg("-u"),
        logger,
        "wineboot",
    )?;
    make_user_dirs_private(config.wine_prefix, logger)?;
    install_prerequisites(config, logger, progress)?;

    let setup = config.user_common.join("ZwiftSetup.exe");
    let setup_log = config.user_common.join("ZwiftSetup.log");
    phase(80, "Downloading Zwift");
    run(
        Command::new(&config.tools.wget)
            .arg("-O")
            .arg(&setup)
            .arg(SETUP_URL),
        logger,
        "Zwift installer download",
    )?;
    phase(88, "Installing Zwift");
    missing_ok_remove_file(&setup_log)?;
    let windows_log = format!("Z:{}", setup_log.to_string_lossy().replace('/', "\\"));
    let setup_result = run(
        Command::new(&config.tools.wine)
            .env("WINEPREFIX", config.wine_prefix)
            .arg(&setup)
            .args([
                "/SP-",
                "/VERYSILENT",
                "/SUPPRESSMSGBOXES",
                "/NORESTART",
                "/NOCANCEL",
            ])
            .arg(format!("/LOG={windows_log}")),
        logger,
        "Zwift installer",
    );
    match fs::read(&setup_log) {
        Ok(bytes) if !bytes.is_empty() => {
            logger.info("Zwift installer log:");
            logger.write_process_output(&bytes)?;
        }
        Ok(_) => logger.warning(&format!(
            "The Zwift installer wrote no log ({})",
            setup_log.display()
        )),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            logger.warning(&format!(
                "The Zwift installer wrote no log ({})",
                setup_log.display()
            ));
        }
        Err(error) => {
            return Err(io::Error::other(format!(
                "Unable to read installer log: {error}"
            )))
        }
    }
    setup_result?;
    let status = run(
        Command::new(&config.tools.wineserver)
            .env("WINEPREFIX", config.wine_prefix)
            .arg("-k"),
        logger,
        "wineserver -k",
    );
    if let Err(error) = status {
        logger.warning(&format!(
            "Unable to stop wineserver after installation: {error}"
        ));
    }
    if !config.install_dir.join("ZwiftLauncher.exe").is_file() {
        return Err(io::Error::other(
            "The installer finished but the Zwift launcher is not installed",
        ));
    }
    let mut update_progress = |percent, message: &str| phase(percent, message);
    let mut backend = WineUpdate {
        install_dir: config.install_dir,
        wine_prefix: Some(config.wine_prefix),
        tools: config.tools,
        logger,
        on_progress: &mut update_progress,
    };
    let latest = backend.latest_version().map_err(|error| {
        io::Error::other(format!("Unable to retrieve the latest Zwift version number, Zwift cannot be downloaded: {error}"))
    })?;
    let current = backend.current_version()?;
    logger.info(&format!(
        "Zwift launcher installed, downloading Zwift {latest}"
    ));
    update_using_launcher(
        &mut backend,
        &latest,
        &current,
        config.download_iterations,
        config.poll_interval,
        90,
        9,
    )
    .map_err(io::Error::other)?;
    if !config.install_dir.join("ZwiftApp.exe").is_file()
        || !config.install_dir.join("ZwiftLauncher.exe").is_file()
    {
        return Err(io::Error::other(
            "The launcher finished but Zwift is not installed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::Mutex;

    struct Fixture {
        dir: std::path::PathBuf,
        tools: Tools,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let lock = crate::SCRIPT_TEST_LOCK.lock().unwrap();
            let dir =
                std::env::temp_dir().join(format!("zwift-install-{name}-{}", std::process::id()));
            fs::create_dir(&dir).unwrap();
            let tools = Tools {
                wine: dir.join("wine"),
                winepath: dir.join("winepath"),
                wineserver: dir.join("wineserver"),
                wget: dir.join("wget"),
            };
            fs::create_dir_all(dir.join("prefix/drive_c/users/tester")).unwrap();
            fs::create_dir(dir.join("common")).unwrap();
            fs::create_dir(dir.join("hostdocs")).unwrap();
            fs::write(dir.join("hostdocs/keep"), "unchanged").unwrap();
            symlink(
                dir.join("hostdocs"),
                dir.join("prefix/drive_c/users/tester/Documents"),
            )
            .unwrap();
            let script = |path: &Path, body: &str| {
                fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            };
            script(&dir.join("wineboot"), &format!(
                "test \"$WINEDLLOVERRIDES\" = 'mscoree,mshtml=;winemenubuilder.exe=d'\ntest \"$WINEPREFIX\" = '{0}/prefix'\necho wineboot >> '{0}/calls'",
                dir.display()
            ));
            script(
                &dir.join("winetricks"),
                &format!(
                    "test \"$1\" = -q\nshift\necho \"winetricks $*\" >> '{0}/calls'\n\
                     for verb in \"$@\"; do echo \"Executing w_do_call $verb\" >&2; done\n\
                     test ! -f '{0}/fail-winetricks'\n\
                     if [ -f '{0}/grow-prefix' ]; then dd if=/dev/zero of='{0}/prefix/drive_c/growth' bs=4096 count=16 2>/dev/null; sleep 0.2; fi",
                    dir.display()
                ),
            );
            script(&tools.wget, &format!(
                "if [ \"$1\" = -O ]; then echo download >> '{0}/calls'; : > \"$2\"; else test ! -f '{0}/fail-version'; printf '<Zwift sversion=\"1.124.0 current\"/>'; fi",
                dir.display()
            ));
            script(&tools.winepath, "printf 'C:\\\\Zwift\\n'");
            script(
                &tools.wineserver,
                &format!("echo server >> '{}/calls'", dir.display()),
            );
            script(&tools.wine, &format!(
                "case \"$1\" in\n\
                 start) echo start >> '{0}/calls'; printf '<Zwift sversion=\"1.125.0 current\"/>' > '{0}/game/Zwift_ver_cur.xml'; if [ ! -f '{0}/missing-game' ]; then : > '{0}/game/ZwiftApp.exe'; fi;;\n\
                 tasklist) echo 'PID: 1';;\n\
                 taskkill) echo taskkill >> '{0}/calls';;\n\
                 *) echo setup >> '{0}/calls'; test \"$2\" = /SP-; test \"$3\" = /VERYSILENT; test \"$4\" = /SUPPRESSMSGBOXES; test \"$5\" = /NORESTART; test \"$6\" = /NOCANCEL; case \"$7\" in /LOG=Z:*) ;; *) exit 1 ;; esac; echo 'installer diagnostic' > '{0}/common/ZwiftSetup.log'; test ! -f '{0}/fail-setup'; mkdir -p '{0}/game'; : > '{0}/game/ZwiftLauncher.exe';;\n\
                 esac",
                dir.display()
            ));
            Self {
                dir,
                tools,
                _lock: lock,
            }
        }

        fn run(&self, sample_interval: Duration) -> (io::Result<()>, Vec<InstallProgress>) {
            let prefix = self.dir.join("prefix");
            let common = self.dir.join("common");
            let game = self.dir.join("game");
            let wineboot = self.dir.join("wineboot");
            let winetricks = self.dir.join("winetricks");
            let logger = Logger::new(self.dir.join("zwift.log"), 0, false);
            logger.init().unwrap();
            let config = InstallConfig {
                wine_prefix: &prefix,
                user_common: &common,
                install_dir: &game,
                wineboot: &wineboot,
                winetricks: &winetricks,
                tools: &self.tools,
                download_iterations: 2,
                poll_interval: Duration::ZERO,
                expected_drive_c_bytes: 102_400,
                sample_interval,
            };
            let events = Mutex::new(Vec::new());
            let result = install(&config, &logger, &|event| {
                events.lock().unwrap().push(event)
            });
            (result, events.into_inner().unwrap())
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.dir).unwrap();
        }
    }

    #[test]
    fn fresh_install_executes_steps_and_downloads_game() {
        let fixture = Fixture::new("success");
        let (result, events) = fixture.run(Duration::from_secs(30));
        result.unwrap();
        let calls = fs::read_to_string(fixture.dir.join("calls")).unwrap();
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            [
                "wineboot",
                "winetricks dotnet48 d3dcompiler_47 webview2 renderer=vulkan",
                "download",
                "setup",
                "server",
                "start",
                "taskkill",
                "taskkill",
                "server"
            ]
        );
        let messages: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                InstallProgress::Phase { message, .. } => Some(message.as_str()),
                InstallProgress::Sample(_) => None,
            })
            .collect();
        assert_eq!(
            &messages[..7],
            [
                "Setting up wine",
                WINETRICKS_PREREQUISITES_MESSAGE,
                "Installing .NET Framework 4.8, this takes several minutes",
                "Installing the Direct3D shader compiler",
                "Installing WebView2",
                "Enabling the Vulkan renderer for video routes",
                "Downloading Zwift",
            ]
        );
        assert!(events
            .iter()
            .any(|e| matches!(e, InstallProgress::Sample(_))));
        assert!(events.iter().any(|e| matches!(
            e, InstallProgress::Phase { message, .. } if message.contains("Downloading Zwift")
        )));
        assert!(fs::read_to_string(fixture.dir.join("zwift.log"))
            .unwrap()
            .contains("installer diagnostic"));
        let documents = fixture.dir.join("prefix/drive_c/users/tester/Documents");
        assert!(documents.is_dir() && !documents.is_symlink());
        assert_eq!(
            fs::read_to_string(fixture.dir.join("hostdocs/keep")).unwrap(),
            "unchanged"
        );
    }

    #[test]
    fn failure_stops_before_downloading_and_sampler_terminates() {
        let fixture = Fixture::new("winetricks");
        fs::write(fixture.dir.join("fail-winetricks"), "").unwrap();
        let (result, _) = fixture.run(Duration::from_secs(30));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("winetricks dotnet48"));
        assert!(!fs::read_to_string(fixture.dir.join("calls"))
            .unwrap()
            .contains("download"));
    }

    #[test]
    fn installer_failure_still_captures_installer_log() {
        let fixture = Fixture::new("setup");
        fs::write(fixture.dir.join("fail-setup"), "").unwrap();
        let (result, _) = fixture.run(Duration::from_secs(30));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Zwift installer failed"));
        assert!(fs::read_to_string(fixture.dir.join("zwift.log"))
            .unwrap()
            .contains("installer diagnostic"));
        assert!(!fs::read_to_string(fixture.dir.join("calls"))
            .unwrap()
            .contains("start"));
    }

    #[test]
    fn sampler_is_monotonic_and_capped() {
        assert_eq!(sample_percent(0, 100, 0), 0);
        assert_eq!(sample_percent(50, 100, 0), 49);
        assert_eq!(sample_percent(1, 100, 49), 49);
        assert_eq!(sample_percent(u64::MAX, 1, 49), 99);
    }

    #[test]
    fn winetricks_progress_follows_verb_markers_across_chunks() {
        let mut progress = WinetricksProgress::new();
        // a marker split over two reads is held back until the line is complete
        assert!(progress.feed(b"Executing w_do_call dotne").is_empty());
        assert_eq!(
            progress.feed(b"t48\nExecuting w_do_call remove_mono\r\n"),
            [(
                8,
                "Installing .NET Framework 4.8, this takes several minutes"
            )]
        );
        // nested verbs of a prerequisite and unknown verbs are not in the list
        assert!(progress
            .feed(b"Executing w_do_call dotnet40\nExecuting w_do_call bogus\n")
            .is_empty());
        // an already reported verb is not repeated
        assert!(progress.feed(b"Executing w_do_call dotnet48\n").is_empty());
        // a missed verb is skipped rather than reported out of order
        assert_eq!(
            progress.feed(b"Executing w_do_call renderer=vulkan\n"),
            [(75, "Enabling the Vulkan renderer for video routes")]
        );
        assert!(progress
            .feed(b"Executing w_do_call d3dcompiler_47\n")
            .is_empty());
        // unrelated output and a trailing partial line are ignored
        assert!(progress
            .feed(b"Executing winetricks\nExecuting w_do_call web")
            .is_empty());
    }

    #[test]
    fn cleanup_removes_artifacts_without_following_symlinks() {
        let fixture = Fixture::new("cleanup");
        let prefix = fixture.dir.join("prefix");
        let temp = prefix.join("drive_c/users/tester/AppData/Local/Temp");
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("discard"), "temporary").unwrap();
        std::os::unix::fs::symlink(fixture.dir.join("hostdocs"), temp.join("link")).unwrap();
        let cache = fixture.dir.join("cache");
        fs::create_dir_all(cache.join("winetricks")).unwrap();
        fs::write(cache.join("winetricks/download"), "").unwrap();
        fs::write(fixture.dir.join("ZwiftSetup.exe"), "").unwrap();
        cleanup_artifacts(&prefix, &fixture.dir, &cache).unwrap();
        assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
        assert!(!cache.join("winetricks").exists());
        assert!(!fixture.dir.join("ZwiftSetup.exe").exists());
        assert!(fixture.dir.join("hostdocs/keep").exists());
        fs::remove_dir(&temp).unwrap();
        std::os::unix::fs::symlink(fixture.dir.join("hostdocs"), &temp).unwrap();
        assert!(cleanup_artifacts(&prefix, &fixture.dir, &cache).is_err());
        assert!(fixture.dir.join("hostdocs/keep").exists());
    }

    #[test]
    fn zero_sampling_interval_is_rejected_before_start() {
        let fixture = Fixture::new("interval");
        let (result, _) = fixture.run(Duration::ZERO);
        assert_eq!(result.unwrap_err().kind(), ErrorKind::InvalidInput);
        assert!(!fixture.dir.join("calls").exists());
    }

    #[test]
    fn resolves_and_privatizes_an_existing_prefix() {
        let fixture = Fixture::new("existing-prefix");
        let prefix = fixture.dir.join("prefix");
        fs::create_dir(prefix.join("drive_c/users/Public")).unwrap();
        let logger = Logger::new(fixture.dir.join("log"), 0, false);
        logger.init().unwrap();
        assert_eq!(
            resolve_wine_user(&prefix).unwrap(),
            Some(prefix.join("drive_c/users/tester"))
        );
        make_user_dirs_private(&prefix, &logger).unwrap();
        make_user_dirs_private(&prefix, &logger).unwrap();
        assert!(prefix.join("drive_c/users/tester/Documents").is_dir());
        assert!(!prefix.join("drive_c/users/tester/Documents").is_symlink());
        assert_eq!(
            fs::read_to_string(fixture.dir.join("hostdocs/keep")).unwrap(),
            "unchanged"
        );
        assert_eq!(
            resolve_wine_user(&fixture.dir.join("no-prefix")).unwrap(),
            None
        );
    }

    #[test]
    fn fresh_install_fails_if_version_lookup_fails() {
        let fixture = Fixture::new("version");
        fs::write(fixture.dir.join("fail-version"), "").unwrap();
        let (result, _) = fixture.run(Duration::from_secs(30));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Zwift cannot be downloaded"));
        assert!(!fs::read_to_string(fixture.dir.join("calls"))
            .unwrap()
            .contains("start"));
    }

    #[test]
    fn updated_version_without_game_binary_is_not_success() {
        let fixture = Fixture::new("missing-game");
        fs::write(fixture.dir.join("missing-game"), "").unwrap();
        let (result, _) = fixture.run(Duration::from_secs(30));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Zwift is not installed"));
    }

    #[test]
    fn sampler_observes_prefix_growth_and_stops_on_completion() {
        let fixture = Fixture::new("growth");
        fs::write(fixture.dir.join("grow-prefix"), "").unwrap();
        let (result, events) = fixture.run(Duration::from_millis(20));
        result.unwrap();
        let samples: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                InstallProgress::Sample(percent) => Some(*percent),
                _ => None,
            })
            .collect();
        assert!(samples.len() > 2);
        assert!(samples.last().unwrap() > samples.first().unwrap());
        assert!(samples.windows(2).all(|pair| pair[0] <= pair[1]));
        assert!(samples.iter().all(|percent| *percent <= 99));
        let percentages: Vec<_> = events
            .iter()
            .map(|event| match event {
                InstallProgress::Sample(percent) | InstallProgress::Phase { percent, .. } => {
                    *percent
                }
            })
            .collect();
        assert!(percentages.windows(2).all(|pair| pair[0] <= pair[1]));
    }
}
