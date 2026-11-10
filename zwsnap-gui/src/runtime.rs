//! Adapter connecting the launcher lifecycle to the Rust Wine implementation.

use crate::display::{configure_file, DisplayMode};
use crate::graphics::{self, Setting as GraphicsSetting};
use crate::install::{self, InstallConfig, InstallProgress};
use crate::launch::{self, LaunchConfig};
use crate::logging::Logger;
use crate::orchestration::LauncherBackend;
use crate::process::run_output_timeout;
use crate::update;
use std::io::{self, ErrorKind};
use std::process::Command;
use std::time::Duration;

pub struct Runtime<'a> {
    pub install: InstallConfig<'a>,
    pub launch: LaunchConfig<'a>,
    pub logger: &'a Logger,
    pub cache: &'a std::path::Path,
    pub display_mode: DisplayMode,
    pub graphics: GraphicsSetting,
    pub wait_for_window: bool,
    pub disable_egl: bool,
    pub progress: &'a (dyn Fn(InstallProgress) + Sync),
}

impl LauncherBackend for Runtime<'_> {
    type Error = io::Error;

    fn prepare(&mut self) -> io::Result<()> {
        if self.install.wine_prefix != self.launch.wine_prefix
            || self.install.install_dir != self.launch.install_dir
        {
            return Err(io::Error::new(
                ErrorKind::InvalidInput,
                "install and launch must use the same Wine prefix and Zwift installation",
            ));
        }
        if self.disable_egl {
            self.logger.info("Disabling EGL (using GLX instead)");
            let status = self.logger.run_inherited_output(
                Command::new(&self.install.tools.wine)
                    .env("WINEPREFIX", self.install.wine_prefix)
                    .args([
                        "reg.exe",
                        "add",
                        r"HKCU\Software\Wine\X11 Driver",
                        "/f",
                        "/v",
                        "UseEGL",
                        "/d",
                        "N",
                    ]),
            )?;
            if !status.success() {
                return Err(io::Error::other(format!(
                    "Failed to disable EGL ({status})"
                )));
            }
        }
        install::make_user_dirs_private(self.install.wine_prefix, self.logger)
    }

    fn installed(&mut self) -> io::Result<bool> {
        if !self.install.install_dir.join("ZwiftApp.exe").is_file()
            || !self.install.install_dir.join("ZwiftLauncher.exe").is_file()
        {
            return Ok(false);
        }
        // The game tree alone is not enough: a prefix without the Wine
        // prerequisites (for example one restored from a game-assets cache) cannot
        // run the game, so reinstall rather than trust the executables.
        if let Some(proof) = install::missing_wine_prerequisite(self.install.wine_prefix) {
            self.logger.warning(&format!(
                "The wine prefix is missing a winetricks prerequisite ({proof}), reinstalling Zwift"
            ));
            return Ok(false);
        }
        Ok(true)
    }

    fn install(&mut self) -> io::Result<()> {
        install::install(&self.install, self.logger, &|event| (self.progress)(event))
    }

    fn update(&mut self) -> io::Result<()> {
        let progress = self.progress;
        let config = update::UpdateConfig {
            install_dir: self.install.install_dir,
            staging_dir: self.install.staging_dir,
            jobs: self.install.jobs,
            cdn_base: self.install.cdn_base,
        };
        update::update_if_needed(&config, self.logger, &|percent, message| {
            progress(InstallProgress::Phase {
                percent,
                message: message.to_string(),
            })
        })
        .map(|_| ())
        .map_err(io::Error::other)
    }

    fn configure_display(&mut self) -> io::Result<()> {
        let user = install::resolve_wine_user(self.install.wine_prefix)?;
        let Some(user) = user else {
            self.logger
                .warning("Wine user directory not found; display mode cannot be set");
            return Ok(());
        };
        let prefs = user.join("AppData/Local/Zwift/prefs.xml");
        configure_file(&prefs, &self.display_mode)
            .map_err(|error| io::Error::other(error.to_string()))?;
        self.logger
            .info(&format!("Set Zwift display mode to {}", self.display_mode));
        Ok(())
    }

    fn configure_graphics(&mut self) -> io::Result<()> {
        // Never fail the launch over this: a profile the user did not get is
        // better than no game, and the game's own GPU table still applies.
        let originals = self.install.user_common.join("graphics");
        match graphics::apply(self.install.install_dir, &originals, self.graphics) {
            Ok(choice) => {
                self.logger.info(&format!("Graphics profile: {choice}"));
                Ok(())
            }
            Err(error) => {
                self.logger.warning(&format!(
                    "Failed to set the Zwift graphics profile ({error}); leaving it to the game"
                ));
                Ok(())
            }
        }
    }

    fn start(&mut self) -> io::Result<()> {
        let progress = self.progress;
        launch::start(
            &self.launch,
            self.logger,
            self.wait_for_window,
            &mut |percent, message| {
                progress(InstallProgress::Phase {
                    percent,
                    message: message.to_string(),
                })
            },
        )
        .map(|_| ())
    }

    fn wait_for_exit(&mut self) -> io::Result<()> {
        launch::wait_for_exit(&self.launch, self.logger)
    }

    fn cleanup(&mut self) -> io::Result<()> {
        self.logger.info("Stopping wine server");
        let mut errors = Vec::new();
        let output = run_output_timeout(
            Command::new(&self.install.tools.wineserver)
                .env("WINEPREFIX", self.install.wine_prefix)
                .arg("-k"),
            Duration::from_secs(10),
        );
        let server_stopped = match output {
            Ok(output) if output.status.success() => true,
            Ok(output) => {
                errors.push(format!(
                    "wineserver -k failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
                false
            }
            Err(error) => {
                errors.push(format!("wineserver -k: {error}"));
                false
            }
        };
        if server_stopped {
            let output = run_output_timeout(
                Command::new(&self.install.tools.wineserver)
                    .env("WINEPREFIX", self.install.wine_prefix)
                    .arg("-w"),
                Duration::from_secs(10),
            );
            match output {
                Ok(output) if output.status.success() => {}
                Ok(output) => errors.push(format!(
                    "wineserver -w failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(error) => errors.push(format!("wineserver -w: {error}")),
            }
        }
        if let Err(error) = self.logger.finish_inherited_output() {
            errors.push(format!("Process output cleanup: {error}"));
        }
        if let Err(error) = install::cleanup_artifacts(
            self.install.wine_prefix,
            self.install.user_common,
            self.install.staging_dir,
            self.cache,
        ) {
            errors.push(format!("Installer artifact cleanup: {error}"));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }
}

/// Default interval for the launch steps that poll for the game.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(feature = "gtk-ui"))]
    use crate::orchestration::{self, Failure, LauncherUi};
    use crate::wine_tools::Tools;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    fn script(path: &Path, contents: &str) {
        fs::write(path, format!("#!/bin/sh\nset -eu\n{contents}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Fabricate what the Wine prerequisites leave behind, so `installed()` is true.
    /// Kept in step with `install::WINETRICKS_PREREQUISITES`.
    fn write_wine_prerequisites(prefix: &Path) {
        let path = prefix.join("drive_c/windows/system32/d3dcompiler_47.dll");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[cfg(not(feature = "gtk-ui"))]
    struct Headless {
        ready: bool,
    }

    #[cfg(not(feature = "gtk-ui"))]
    impl LauncherUi for Headless {
        fn failure(&mut self, failure: Failure, _: &str) -> bool {
            failure == Failure::Update
        }
        fn game_ready(&mut self) {
            self.ready = true;
        }
    }

    #[test]
    fn runs_installed_game_and_cleans_up_with_stub_wine() {
        let _lock = crate::SCRIPT_TEST_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("zwift-runtime-{}", std::process::id()));
        // A cancelled earlier run may have left this behind under a reused pid.
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let prefix = dir.join("prefix");
        let common = dir.join("common");
        let game = dir.join("game");
        let user = prefix.join("drive_c/users/tester/AppData/Local/Zwift");
        fs::create_dir_all(&user).unwrap();
        fs::create_dir(&common).unwrap();
        fs::create_dir(&game).unwrap();
        write_wine_prerequisites(&prefix);
        for name in ["ZwiftLauncher.exe", "ZwiftApp.exe"] {
            fs::write(game.join(name), "").unwrap();
        }
        fs::write(
            game.join("Zwift_ver_cur.xml"),
            "<Zwift version=\"1.124.0\" sversion=\"1.124.0 x\" manifest=\"manifest.xml\"/>",
        )
        .unwrap();
        fs::write(user.join("prefs.xml"), "<ZWIFT><CONFIG/></ZWIFT>").unwrap();
        // A prefix installed by an older launcher; the cleanup reclaims it.
        fs::write(common.join("ZwiftSetup.exe"), "old setup").unwrap();
        // The game is up to date, so the update transfers nothing: serve the same
        // version from a local CDN.
        let cdn = dir.join("cdn");
        crate::test_support::write_cdn(
            &cdn,
            "1.124.0",
            &[("ZwiftApp.exe", ""), ("ZwiftLauncher.exe", "")],
        );
        // The snap's zwsnap-helpers part, copied into the game directory by
        // the launch step.
        let shim = dir.join("snap/bin/ZwiftLauncher.exe");
        fs::create_dir_all(shim.parent().unwrap()).unwrap();
        fs::write(&shim, "shim").unwrap();
        let tools = Tools {
            wine: dir.join("wine"),
            winepath: dir.join("winepath"),
            wineserver: dir.join("wineserver"),
        };
        script(&tools.winepath, "printf 'C:\\\\Zwift\\n'");
        script(
            &tools.wineserver,
            &format!(
                "if [ \"$1\" = -k ]; then echo stopped >> '{0}/calls'; fi\n\
                 if [ \"$1\" = -w ]; then\n\
                   i=0; while [ ! -f '{0}/game-done' ]; do\n\
                     i=$((i + 1)); [ \"$i\" -lt 200 ] || exit 1; sleep 0.05;\n\
                   done\n\
                 fi",
                dir.display()
            ),
        );
        script(
            &tools.wine,
            &format!(
                "case \"$1\" in\n\
                 start) (i=0; while [ ! -f '{0}/game-release' ]; do\n\
                      i=$((i + 1)); if [ \"$i\" -ge 200 ]; then : > '{0}/timed-out'; exit 1; fi; sleep 0.05;\n\
                    done; printf 'game descendant output\\n'; : > '{0}/game-done') & ;;\n\
                 tasklist) case \"$5\" in\n\
                   *ZwiftApp.exe) : > '{0}/game-release'; if [ ! -f '{0}/seen' ]; then : > '{0}/seen'; printf 'PID: 42\\n'; fi;;\n\
                 esac;;\n\
                 taskkill) :;;\n\
                 *) exit 1;;\n\
                 esac",
                dir.display()
            ),
        );
        let logger = Logger::new(dir.join("log"), 0, false);
        logger.init().unwrap();
        let streamed = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let streamed_sink = streamed.clone();
        logger.add_raw_sink(move |bytes| streamed_sink.lock().unwrap().extend_from_slice(bytes));
        let events = std::sync::Mutex::new(Vec::new());
        #[cfg(feature = "gtk-ui")]
        let (sender, receiver) = std::sync::mpsc::channel();
        #[cfg(feature = "gtk-ui")]
        let mut ui = crate::gui::WorkerUi::new(sender);
        #[cfg(feature = "gtk-ui")]
        let progress_ui = ui.clone();
        let progress = |event: InstallProgress| {
            #[cfg(feature = "gtk-ui")]
            progress_ui.progress(event.clone());
            events.lock().unwrap().push(event);
        };
        let window = dir.join("window.exe");
        let wineboot = dir.join("wineboot");
        let winetricks = dir.join("winetricks");
        let staging = common.join("download");
        let cdn_base = format!("file://{}", cdn.display());
        let install = InstallConfig {
            wine_prefix: &prefix,
            user_common: &common,
            install_dir: &game,
            staging_dir: &staging,
            launcher_shim: &shim,
            wineboot: &wineboot,
            winetricks: &winetricks,
            tools: &tools,
            cdn_base: &cdn_base,
            jobs: 2,
            expected_drive_c_bytes: 0,
            sample_interval: Duration::from_secs(1),
        };
        let launch = LaunchConfig {
            wine_prefix: &prefix,
            install_dir: &game,
            launcher_shim: &shim,
            wait_for_window: &window,
            tools: &tools,
            game_args: &[],
            token_file: None,
            startup_attempts: 1,
            startup_interval: Duration::ZERO,
            game_poll_interval: Duration::ZERO,
        };
        let mut runtime = Runtime {
            install,
            launch,
            logger: &logger,
            cache: &common,
            display_mode: DisplayMode::Windowed {
                width: 1280,
                height: 720,
            },
            graphics: GraphicsSetting::GameDefault,
            wait_for_window: false,
            disable_egl: false,
            progress: &progress,
        };
        #[cfg(not(feature = "gtk-ui"))]
        let mut ui = Headless { ready: false };
        #[cfg(not(feature = "gtk-ui"))]
        orchestration::run(&mut runtime, &mut ui).unwrap();
        #[cfg(not(feature = "gtk-ui"))]
        assert!(ui.ready);
        #[cfg(feature = "gtk-ui")]
        {
            let received = std::thread::scope(|scope| {
                scope.spawn(|| crate::gui::run_controller(&mut runtime, &mut ui));
                let mut received = Vec::new();
                loop {
                    let event = receiver
                        .recv()
                        .expect("controller stopped without a result");
                    if let crate::gui::Event::Failure {
                        ref detail,
                        ref decision,
                        ..
                    } = event
                    {
                        eprintln!("Unexpected runtime failure in test: {detail}");
                        decision.send(false).unwrap();
                    }
                    if let crate::gui::Event::LoginRequired { ref decision } = event {
                        decision.send(true).unwrap();
                    }
                    let finished = matches!(event, crate::gui::Event::Finished(_));
                    received.push(event);
                    if finished {
                        break;
                    }
                }
                received
            });
            assert!(received.iter().any(|event| matches!(
                event,
                crate::gui::Event::Progress(InstallProgress::Phase { percent: 95, .. })
            )));
            assert!(received
                .iter()
                .any(|event| matches!(event, crate::gui::Event::GameReady)));
            match received.last() {
                Some(crate::gui::Event::Finished(result)) => {
                    result.as_ref().expect("controller failed");
                }
                _ => panic!("controller stopped without a result"),
            }
        }
        assert!(fs::read_to_string(user.join("prefs.xml"))
            .unwrap()
            .contains("<USER_RESOLUTION_PREF>1280x720</USER_RESOLUTION_PREF>"));
        assert_eq!(fs::read_to_string(dir.join("calls")).unwrap(), "stopped\n");
        let output = fs::read_to_string(logger.path()).unwrap();
        assert!(output.contains("game descendant output"));
        let streamed = streamed.lock().unwrap();
        assert!(streamed
            .windows(b"game descendant output".len())
            .any(|window| window == b"game descendant output"));
        assert!(!dir.join("timed-out").exists());
        assert!(!common.join("ZwiftSetup.exe").exists());
        assert_eq!(
            fs::read_to_string(game.join("ZwiftLauncher.exe")).unwrap(),
            "shim"
        );
        assert!(events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, InstallProgress::Phase { percent: 95, .. })));
        fs::remove_dir_all(dir).unwrap();
    }
}
