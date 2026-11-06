//! Fresh Wine prefix and Zwift installation.

use crate::logging::Logger;
use crate::update::{update_using_launcher, UpdateBackend};
use crate::wine_update::{Tools, WineUpdate};
use std::collections::HashSet;
use std::fs;
use std::io::{self, ErrorKind};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

const SETUP_URL: &str = "https://cdn.zwift.com/app/ZwiftSetup.exe";
pub const EXPECTED_DRIVE_C_BYTES: u64 = 8_537_613_774;

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
    let status = logger.run_inherited_output(command)?;
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
/// thread while phase updates arrive from the calling thread.
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

    for (percent, message, verb) in [
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
    ] {
        phase(percent, message);
        run(
            Command::new(config.winetricks)
                .env("WINEPREFIX", config.wine_prefix)
                .args(["-q", verb]),
            logger,
            &format!("winetricks {verb}"),
        )?;
    }

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
                    "echo \"winetricks $2\" >> '{0}/calls'\ntest ! -f '{0}/fail-winetricks'\n\
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
                "winetricks dotnet48",
                "winetricks d3dcompiler_47",
                "winetricks webview2",
                "winetricks renderer=vulkan",
                "download",
                "setup",
                "server",
                "start",
                "taskkill",
                "taskkill",
                "server"
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
