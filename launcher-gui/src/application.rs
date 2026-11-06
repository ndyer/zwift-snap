//! Snap entry point configuration and wiring for the real launcher.

use crate::cancel::Cancellation;
use crate::display::DisplayMode;
use crate::install::{InstallConfig, InstallProgress, EXPECTED_DRIVE_C_BYTES};
use crate::launch::LaunchConfig;
use crate::logging::Logger;
use crate::orchestration::{self, Failure, LauncherUi};
use crate::runtime::{Runtime, DEFAULT_UPDATE_ITERATIONS};
use crate::wine_update::Tools;
use std::ffi::OsString;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub struct Config {
    pub snap: PathBuf,
    pub common: PathBuf,
    pub prefix: PathBuf,
    pub log: PathBuf,
    pub cache: PathBuf,
    pub display: DisplayMode,
    pub no_gui: bool,
    pub no_gamemode: bool,
    pub disable_egl: bool,
    pub verbosity: u8,
    pub color: bool,
    pub poll_interval: Duration,
    pub download_iterations: u32,
    pub expected_bytes: u64,
    pub sample_interval: Duration,
}

impl Config {
    pub fn from_env() -> io::Result<Self> {
        Self::read(|name| std::env::var_os(name))
    }

    fn read(env: impl Fn(&str) -> Option<OsString>) -> io::Result<Self> {
        let value = |name: &str| env(name).filter(|value| !value.is_empty());
        let required = |name: &str| {
            value(name).map(PathBuf::from).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} is not set; run this launcher inside the snap"),
                )
            })
        };
        let number = |name: &str, default: u64| -> io::Result<u64> {
            match value(name) {
                None => Ok(default),
                Some(value) => value
                    .to_str()
                    .and_then(|text| text.parse().ok())
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidInput,
                            format!("{name} must be a nonnegative integer"),
                        )
                    }),
            }
        };
        let flag = |name, default| -> io::Result<bool> {
            match number(name, default)? {
                0 => Ok(false),
                1 => Ok(true),
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{name} must be 0 or 1"),
                )),
            }
        };
        let snap = required("SNAP")?;
        let common = required("SNAP_USER_COMMON")?;
        let prefix = value("WINEPREFIX")
            .map(PathBuf::from)
            .unwrap_or_else(|| common.join(".wine"));
        let log = value("ZWIFT_LOG")
            .map(PathBuf::from)
            .unwrap_or_else(|| common.join("zwift.log"));
        let cache = value("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| common.join(".cache"));
        for path in [&snap, &common, &prefix, &log, &cache] {
            if !path.is_absolute() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Launcher path must be absolute: {}", path.display()),
                ));
            }
        }
        let display = match value("ZWIFT_DISPLAY_MODE") {
            None => DisplayMode::Fullscreen,
            Some(mode) => DisplayMode::parse(mode.to_str().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "ZWIFT_DISPLAY_MODE is not UTF-8",
                )
            })?)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
        };
        let sample_interval = Duration::from_secs(number("ZWIFT_PROGRESS_INTERVAL", 5)?);
        if sample_interval.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "ZWIFT_PROGRESS_INTERVAL must be greater than zero",
            ));
        }
        Ok(Self {
            snap,
            common,
            prefix,
            log,
            cache,
            display,
            no_gui: flag("ZWIFT_NO_GUI", 0)?,
            no_gamemode: flag("ZWIFT_NO_GAMEMODE", 1)?,
            disable_egl: flag("WINE_DISABLE_EGL", 0)?,
            verbosity: u8::try_from(number("VERBOSITY", 1)?)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
            color: flag("COLORED_OUTPUT", 0)?,
            poll_interval: Duration::from_secs(number("ZWIFT_POLL_INTERVAL", 5)?),
            download_iterations: u32::try_from(number("ZWIFT_DOWNLOAD_ITERATIONS", 240)?)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
            expected_bytes: number("ZWIFT_EXPECTED_DRIVE_C_BYTES", EXPECTED_DRIVE_C_BYTES)?,
            sample_interval,
        })
    }

    /// Called before GTK or any worker threads are started.
    pub fn prepare_environment(&self) -> io::Result<()> {
        std::fs::create_dir_all(&self.common)?;
        std::env::set_var("WINEPREFIX", &self.prefix);
        if std::env::var_os("XDG_CACHE_HOME").is_none_or(|value| value.is_empty()) {
            std::env::set_var("XDG_CACHE_HOME", self.common.join(".cache"));
        }
        if std::env::var("ZWIFT_BLUETOOTH_DEBUG").as_deref() == Ok("1") {
            let mut debug = std::env::var_os("WINEDEBUG").unwrap_or_default();
            if !debug.is_empty() {
                debug.push(",");
            }
            debug.push("+bluetooth,+winebth");
            std::env::set_var("WINEDEBUG", debug);
        }
        if std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland") {
            link_wayland_socket()?;
            std::env::remove_var("DISPLAY");
        } else if !self.disable_egl && std::path::Path::new("/dev/nvidiactl").exists() {
            std::env::set_var("__EGL_VENDOR_LIBRARY_FILENAMES", "");
        }
        Ok(())
    }
}

fn link_wayland_socket() -> io::Result<()> {
    let Some(display) = std::env::var_os("WAYLAND_DISPLAY").filter(|value| !value.is_empty())
    else {
        return Ok(());
    };
    let display = PathBuf::from(display);
    if display.is_absolute() {
        return Ok(());
    }
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") else {
        return Ok(());
    };
    // SAFETY: getuid has no preconditions or side effects.
    let host = PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })).join(&display);
    let local = PathBuf::from(runtime).join(display);
    if local.exists() || !host.exists() {
        return Ok(());
    }
    if local
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.is_symlink())
    {
        std::fs::remove_file(&local)?;
    }
    std::os::unix::fs::symlink(host, local)
}

pub fn run(
    config: &Config,
    logger: &Logger,
    ui: &mut impl LauncherUi,
    graphical: bool,
    progress: &(impl Fn(InstallProgress) + Sync),
    args: &[String],
) -> Result<(), orchestration::LauncherError> {
    let install_dir = config.prefix.join("drive_c/Program Files (x86)/Zwift");
    let tools = Tools::default();
    let wineboot = PathBuf::from("wineboot");
    let winetricks = PathBuf::from("winetricks");
    let runfromprocess = config.snap.join("bin/runfromprocess-rs.exe");
    let window = config.snap.join("bin/wait-for-window.exe");
    let gamemode = (!config.no_gamemode)
        .then(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|dir| dir.join("gamemoderun"))
                    .find(|path| {
                        use std::os::unix::fs::PermissionsExt;
                        path.metadata().is_ok_and(|metadata| {
                            metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
                        })
                    })
            })
        })
        .flatten();
    let mut runtime = Runtime {
        install: InstallConfig {
            wine_prefix: &config.prefix,
            user_common: &config.common,
            install_dir: &install_dir,
            wineboot: &wineboot,
            winetricks: &winetricks,
            tools: &tools,
            download_iterations: config.download_iterations,
            poll_interval: config.poll_interval,
            expected_drive_c_bytes: if graphical { config.expected_bytes } else { 0 },
            sample_interval: config.sample_interval,
        },
        launch: LaunchConfig {
            wine_prefix: &config.prefix,
            install_dir: &install_dir,
            runfromprocess: &runfromprocess,
            wait_for_window: &window,
            tools: &tools,
            gamemoderun: gamemode.as_deref(),
            game_args: args,
            startup_attempts: 20,
            startup_interval: Duration::from_millis(100),
            game_poll_interval: config.poll_interval,
        },
        logger,
        cache: &config.cache,
        display_mode: config.display.clone(),
        wait_for_window: graphical,
        disable_egl: config.disable_egl
            && std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland"),
        update_iterations: DEFAULT_UPDATE_ITERATIONS,
        progress,
    };
    orchestration::run_cancellable(&mut runtime, ui, logger.cancellation())
}

struct TerminalUi<'a>(&'a Logger);

impl LauncherUi for TerminalUi<'_> {
    fn failure(&mut self, kind: Failure, detail: &str) -> bool {
        self.0.error(detail);
        kind == Failure::Update
    }

    fn game_ready(&mut self) {}
}

pub fn launch(config: Config, args: Vec<String>) -> io::Result<()> {
    config.prepare_environment()?;
    let cancellation = Cancellation::default();
    let logger = Arc::new(Logger::with_cancellation(
        &config.log,
        config.verbosity,
        config.color || std::io::stdout().is_terminal(),
        cancellation.clone(),
    ));
    logger.init()?;
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGHUP,
    ])?;
    let signal_handle = signals.handle();
    let signal_worker = std::thread::spawn(move || {
        if signals.forever().next().is_some() {
            cancellation.cancel();
        }
    });
    let result = launch_ui(config, args, logger);
    signal_handle.close();
    signal_worker
        .join()
        .map_err(|_| io::Error::other("Signal handler thread panicked"))?;
    result
}

fn launch_ui(config: Config, args: Vec<String>, logger: Arc<Logger>) -> io::Result<()> {
    #[cfg(feature = "gtk-ui")]
    if !config.no_gui
        && (std::env::var_os("DISPLAY").is_some_and(|value| !value.is_empty())
            || std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty()))
    {
        match gtk::init() {
            Ok(()) => {
                return crate::gui::launch(
                    config.display.clone(),
                    logger.cancellation().clone(),
                    move |mut ui| {
                        let log_ui = ui.clone();
                        logger.add_sink(move |kind, message| {
                            log_ui.log(format!("{}: {message}\n", kind.name()))
                        });
                        let log_ui = ui.clone();
                        logger.add_raw_sink(move |bytes| {
                            log_ui.log(String::from_utf8_lossy(bytes).into_owned())
                        });
                        let progress_ui = ui.clone();
                        let progress = |event: InstallProgress| {
                            if let InstallProgress::Phase { ref message, .. } = event {
                                logger.info(message);
                            }
                            progress_ui.progress(event);
                        };
                        let result = run(&config, &logger, &mut ui, true, &progress, &args);
                        if let Err(error) = &result {
                            logger.error(&error.to_string());
                        }
                        result.map_err(|error| error.to_string())
                    },
                );
            }
            Err(error) => logger.warning(&format!(
                "Unable to initialize GTK: {error}; using terminal output"
            )),
        }
    }
    let progress = |event| {
        if let InstallProgress::Phase { message, .. } = event {
            logger.info(&message);
        }
    };
    let result = run(
        &config,
        &logger,
        &mut TerminalUi(&logger),
        false,
        &progress,
        &args,
    );
    if let Err(error) = &result {
        logger.error(&error.to_string());
    }
    result.map_err(|error| io::Error::other(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(values: &[(&str, &str)]) -> io::Result<Config> {
        Config::read(|name| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn defaults_match_the_shell_launcher() {
        let config = config(&[("SNAP", "/snap/zwift/x1"), ("SNAP_USER_COMMON", "/data")]).unwrap();
        assert_eq!(config.prefix, PathBuf::from("/data/.wine"));
        assert_eq!(config.log, PathBuf::from("/data/zwift.log"));
        assert_eq!(config.display, DisplayMode::Fullscreen);
        assert!(config.no_gamemode);
        assert!(!config.no_gui);
        assert_eq!(config.download_iterations, 240);
        assert_eq!(config.poll_interval, Duration::from_secs(5));
    }

    #[test]
    fn overrides_are_validated_without_mutating_the_environment() {
        let config = config(&[
            ("SNAP", "/snap/zwift/x1"),
            ("SNAP_USER_COMMON", "/data"),
            ("WINEPREFIX", "/prefix"),
            ("ZWIFT_DISPLAY_MODE", "800x600"),
            ("ZWIFT_NO_GUI", "1"),
            ("ZWIFT_POLL_INTERVAL", "0"),
        ])
        .unwrap();
        assert_eq!(config.prefix, PathBuf::from("/prefix"));
        assert_eq!(
            config.display,
            DisplayMode::Windowed {
                width: 800,
                height: 600
            }
        );
        assert!(config.no_gui);
        assert_eq!(config.poll_interval, Duration::ZERO);
        for (name, value) in [
            ("WINEPREFIX", "relative"),
            ("ZWIFT_DISPLAY_MODE", "invalid"),
            ("ZWIFT_PROGRESS_INTERVAL", "0"),
            ("ZWIFT_DOWNLOAD_ITERATIONS", "-1"),
            ("ZWIFT_NO_GUI", "2"),
        ] {
            assert!(
                super::tests::config(&[
                    ("SNAP", "/snap/zwift/x1"),
                    ("SNAP_USER_COMMON", "/data"),
                    (name, value),
                ])
                .is_err(),
                "{name}={value}"
            );
        }
        assert!(super::tests::config(&[]).is_err());
    }
}
