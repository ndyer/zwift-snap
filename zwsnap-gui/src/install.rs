//! Fresh Wine prefix and Zwift installation.

use crate::cdn::{Expect, Remote};
use crate::download::Adaptive;
use crate::launch;
use crate::logging::Logger;
use crate::sync::{self, SyncConfig};
use crate::wine_tools::Tools;
use std::fs;
use std::io::{self, ErrorKind};
use std::path::Path;
use std::process::{Command, ExitStatus};

/// Where the Direct3D shader compiler's redistributable is downloaded from.
///
/// This replaces winetricks' `d3dcompiler_47` verb: the launcher installs the
/// prerequisite itself, so the snap no longer ships winetricks or any of the Wine
/// internals its verbs assumed. The directory is the standalone SDK installer
/// Microsoft still serves; tests point `InstallConfig::prerequisites_base` at a
/// `file://` fixture instead.
pub const D3DCOMPILER_BASE: &str =
    "https://download.microsoft.com/download/B/0/C/B0C80BA3-8AD6-4958-810B-6882485230B5/standalonesdk/Installers";

/// The 64-bit `d3dcompiler_47` cabinet inside [`D3DCOMPILER_BASE`].
const D3DCOMPILER_CAB: &str = "61d57a7a82309cd161a854a6f4619e52.cab";

/// Length and CRC-32 of the cabinet. They pin the download the same way a manifest
/// entry pins a game file, so a truncated or substituted transfer never becomes a
/// prefix that cannot run the game. Taken from the file Microsoft serves.
const D3DCOMPILER_CAB_LENGTH: u64 = 1_776_364;
const D3DCOMPILER_CAB_CRC32: u32 = 0x63cd_3009;

/// What the shipped cabinet must look like, for [`InstallConfig::prerequisites_expect`].
pub fn d3dcompiler_expect() -> Expect {
    Expect {
        length: D3DCOMPILER_CAB_LENGTH,
        checksum: Some(D3DCOMPILER_CAB_CRC32),
    }
}

/// The DLL inside the cabinet. `cabextract` lists it (and a second payload) by this
/// name and stores it without an extension, so it is renamed on install.
const D3DCOMPILER_CAB_DLL: &str = "fil3585cb2ea5db13cc0838f8d06b5c9679";

/// Where the extracted compiler is installed, relative to the Wine prefix. Wine
/// resolves `d3dcompiler_47` from `system32`, so the native DLL replaces the
/// builtin in place.
pub const D3DCOMPILER_DLL: &str = "drive_c/windows/system32/d3dcompiler_47.dll";

/// Proof that the native compiler and its `native` override are installed, as a
/// path relative to the Wine prefix.
///
/// The DLL itself cannot be the proof: `wineboot` seeds `system32` with Wine's own
/// builtin `d3dcompiler_47.dll`, so a prefix that has never run this step already
/// has a file at [`D3DCOMPILER_DLL`]. This sibling marker is written by the
/// launcher only once the native DLL is in place and the override imported, so it
/// is the reliable proof [`missing_wine_prerequisite`] needs.
pub const D3DCOMPILER_PROOF: &str = "drive_c/windows/system32/d3dcompiler_47.dll.native";

/// A Wine prerequisite that can be detected from the filesystem.
struct Prerequisite {
    /// Progress message shown while it is installed.
    message: &'static str,
    /// A path relative to the Wine prefix that the launcher itself writes once the
    /// prerequisite is in place, used by `missing_wine_prerequisite`.
    proof: &'static str,
}

/// The Wine prerequisites that leave a file behind.
///
/// `.NET Framework 4.8` and `WebView2` are deliberately absent: both existed for
/// Zwift's own `ZwiftLauncher.exe`, a .NET launcher hosting a WebView2 UI. The
/// launcher shim replaces that program and nothing runs it any more, so neither is
/// installed. The game itself is a native 64-bit binary and needs neither.
///
/// The Vulkan renderer (winetricks' `renderer=vulkan`) is installed too but is not
/// listed here: it is a registry value that cannot be probed without running Wine.
const WINE_PREREQUISITES: [Prerequisite; 1] = [Prerequisite {
    message: "Installing the Direct3D shader compiler",
    proof: D3DCOMPILER_PROOF,
}];

/// Progress message for the registry-only Vulkan prerequisite.
const VULKAN_MESSAGE: &str = "Enabling the Vulkan renderer for video routes";

/// Progress message for the Zwift game transfer.
const GAME_MESSAGE: &str = "Downloading Zwift";

/// `d3dcompiler_47` is loaded by the game instead of Wine's builtin, so the DLL is
/// overridden to `native`. The leading `*` matches the form winetricks wrote.
const DLLOVERRIDE_REG: &str =
    "[HKEY_CURRENT_USER\\Software\\Wine\\DllOverrides]\r\n\"*d3dcompiler_47\"=\"native\"\r\n";

/// The registry value winetricks' `winetricks_set_wined3d_var renderer vulkan` wrote.
const VULKAN_REG: &str =
    "[HKEY_CURRENT_USER\\Software\\Wine\\Direct3D]\r\n\"renderer\"=\"vulkan\"\r\n";

/// One named step of the launcher's work. Every step but [`InstallTask::UpdateCheck`]
/// gets a progress-bar tile of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallTask {
    /// Creating and updating the Wine prefix (`wineboot`).
    WineSetup,
    /// The native `d3dcompiler_47` the game loads.
    D3dCompiler,
    /// Selecting Wine's Vulkan Direct3D renderer.
    Vulkan,
    /// Reading the version the CDN serves, to decide what to transfer.
    ///
    /// A check has no measurable fraction and is over in a moment, so it draws no
    /// bar: it only names the status line. It is an update's first step and never
    /// part of a fresh install.
    UpdateCheck,
    /// The Zwift game itself, fetched from the CDN.
    Game,
}

impl InstallTask {
    /// The label shown beside the task's progress bar, and in the status line.
    pub fn label(self) -> &'static str {
        match self {
            InstallTask::WineSetup => "Setting up Wine",
            InstallTask::D3dCompiler => WINE_PREREQUISITES[0].message,
            InstallTask::Vulkan => VULKAN_MESSAGE,
            InstallTask::UpdateCheck => crate::update::CHECK_MESSAGE,
            InstallTask::Game => GAME_MESSAGE,
        }
    }
}

/// How far along one [`InstallTask`] is. A task with no measurable fraction pulses
/// while [`TaskState::Running`] and completes with [`TaskState::Done`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// In progress with no measurable fraction; the bar pulses.
    Running,
    /// In progress with a measured percentage, 0 to 99.
    Fraction(u8),
    /// Finished successfully.
    Done,
}

/// Progress updates sent from the installer to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallProgress {
    /// A per-task update for one of the install bars.
    Task { task: InstallTask, state: TaskState },
    /// A message shown while the game starts, once the install bars are gone.
    Launch { message: String },
}

pub struct InstallConfig<'a> {
    pub wine_prefix: &'a Path,
    pub user_common: &'a Path,
    pub install_dir: &'a Path,
    /// Scratch space for in-flight transfers, on `user_common`'s filesystem so a
    /// verified file can be renamed into place.
    pub staging_dir: &'a Path,
    /// The launcher shim that replaces the game's own `ZwiftLauncher.exe`.
    pub launcher_shim: &'a Path,
    pub wineboot: &'a Path,
    /// Where the Wine prerequisites' redistributable is downloaded from, or a
    /// `file://` directory in tests.
    pub prerequisites_base: &'a str,
    /// What the cabinet at `prerequisites_base` must look like. Production uses
    /// [`d3dcompiler_expect`]; tests serve a small fixture.
    pub prerequisites_expect: Expect,
    pub tools: &'a Tools,
    /// The CDN root the game is fetched from, or a `file://` directory in tests.
    pub cdn_base: &'a str,
    /// Parallel transfers.
    pub jobs: usize,
    /// Grow the transfer count until the link saturates; `None` is a fixed pool of
    /// `jobs`.
    pub adaptive: Option<Adaptive>,
}

fn run(command: &mut Command, logger: &Logger, label: &str) -> io::Result<()> {
    finish(logger.run_inherited_output(command)?, label)
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

/// Remove only launcher-owned scratch; never follow a directory symlink.
pub fn cleanup_artifacts(prefix: &Path, common: &Path, staging: &Path) -> io::Result<()> {
    // A finished run leaves the staging directory empty; an interrupted one can hold
    // partial transfers, which the next run refetches from scratch anyway.
    remove_artifact(staging)?;
    // The prerequisite download and extraction scratch, if a run was interrupted
    // between fetching the cabinet and installing the DLL.
    remove_artifact(&common.join("prerequisites"))?;
    // A prefix installed by an older launcher still holds the downloaded installer,
    // which nothing runs any more. It is a hundred megabytes, so reclaim it.
    missing_ok_remove_file(&common.join("ZwiftSetup.exe"))?;
    missing_ok_remove_file(&common.join("ZwiftSetup.log"))?;
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

/// The proof path of the first Wine prerequisite that is not installed, if any.
///
/// The registry-only Vulkan prerequisite has no proof path and is not probed: it
/// cannot be observed from the filesystem, so a fresh install always rewrites it.
pub fn missing_wine_prerequisite(prefix: &Path) -> Option<&'static str> {
    WINE_PREREQUISITES.iter().find_map(|prerequisite| {
        let proof = prerequisite.proof;
        (!prefix.join(proof).exists()).then_some(proof)
    })
}

/// Whether every Wine prerequisite that can be probed from the filesystem is installed.
pub fn wine_prerequisites_installed(prefix: &Path) -> bool {
    missing_wine_prerequisite(prefix).is_none()
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

/// Install Wine prerequisites, the Zwift launcher, and the game.
///
/// `wineboot` runs first because the game and its prerequisites are written inside
/// the prefix it creates. The prerequisite track and the game transfer then run
/// concurrently on scoped threads, so the progress callback is called from more
/// than one thread and must be thread-safe. Both tracks are always joined; the
/// prerequisite track's error takes precedence.
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
    install_wine_prefix(config, logger, progress)?;
    std::thread::scope(|scope| {
        let prerequisites = scope.spawn(|| install_prerequisites(config, logger, progress));
        let game = scope.spawn(|| install_game(config, logger, progress));
        let prerequisites_result = prerequisites.join().expect("prerequisite track panicked");
        let game_result = game.join().expect("game track panicked");
        prerequisites_result.and(game_result)
    })
}

/// A per-task progress event, for brevity at the call sites.
fn task(task: InstallTask, state: TaskState) -> InstallProgress {
    InstallProgress::Task { task, state }
}

/// The percentage of a byte transfer, capped below 100 until [`TaskState::Done`].
fn fraction(done: u64, total: u64) -> TaskState {
    let percent = if total == 0 {
        0
    } else {
        (u128::from(done) * 100 / u128::from(total)).min(99) as u8
    };
    TaskState::Fraction(percent)
}

/// Create the Wine prefix and lock down its per-user directories.
fn install_wine_prefix(
    config: &InstallConfig<'_>,
    logger: &Logger,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> io::Result<()> {
    progress(task(InstallTask::WineSetup, TaskState::Running));
    run(
        Command::new(config.wineboot)
            .env("WINEPREFIX", config.wine_prefix)
            .env("WINEDLLOVERRIDES", "mscoree,mshtml=;winemenubuilder.exe=d")
            .arg("-u"),
        logger,
        "wineboot",
    )?;
    make_user_dirs_private(config.wine_prefix, logger)?;
    progress(task(InstallTask::WineSetup, TaskState::Done));
    Ok(())
}

/// Install the Wine prerequisites a fresh prefix needs.
///
/// The two steps share the prefix's registry, so they run one after the other.
fn install_prerequisites(
    config: &InstallConfig<'_>,
    logger: &Logger,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> io::Result<()> {
    progress(task(InstallTask::D3dCompiler, TaskState::Running));
    install_d3dcompiler(config, logger)?;
    progress(task(InstallTask::D3dCompiler, TaskState::Done));
    progress(task(InstallTask::Vulkan, TaskState::Running));
    configure_vulkan_renderer(config, logger)?;
    progress(task(InstallTask::Vulkan, TaskState::Done));
    Ok(())
}

/// Download the Direct3D shader compiler, install it, and override it to `native`.
///
/// The DLL is fetched from Microsoft into a scratch directory under the snap data,
/// extracted with `cabextract`, renamed into `system32`, and then selected through
/// the same `HKCU\Software\Wine\DllOverrides` value winetricks wrote. The step is
/// skipped only when its own [`D3DCOMPILER_PROOF`] marker is present: the DLL on
/// its own says nothing, since `wineboot` leaves Wine's builtin there.
fn install_d3dcompiler(config: &InstallConfig<'_>, logger: &Logger) -> io::Result<()> {
    let proof = config.wine_prefix.join(WINE_PREREQUISITES[0].proof);
    if proof.is_file() {
        return Ok(());
    }
    let dll = config.wine_prefix.join(D3DCOMPILER_DLL);
    let scratch = config.user_common.join("prerequisites");
    remove_artifact(&scratch)?;
    fs::create_dir(&scratch)?;
    let result = (|| {
        let cab = scratch.join(D3DCOMPILER_CAB);
        let base = config.prerequisites_base.trim_end_matches('/');
        Remote::new(config.prerequisites_base)
            .fetch_file(
                &format!("{base}/{D3DCOMPILER_CAB}"),
                &cab,
                config.prerequisites_expect,
            )
            .map_err(io::Error::other)?;
        run(
            Command::new(&config.tools.cabextract)
                .arg("-q")
                .arg("-L")
                .arg("-F")
                .arg(D3DCOMPILER_CAB_DLL)
                .arg("-d")
                .arg(&scratch)
                .arg(&cab),
            logger,
            &format!("cabextract {D3DCOMPILER_CAB}"),
        )?;
        fs::rename(scratch.join(D3DCOMPILER_CAB_DLL), &dll)?;
        write_registry(
            config,
            logger,
            "d3dcompiler_47-override.reg",
            DLLOVERRIDE_REG,
        )?;
        // Written last, so the marker only appears once both the native DLL and its
        // override are in place; it records what it vouches for.
        fs::write(&proof, D3DCOMPILER_DLL)
    })();
    // The scratch holds the cabinet and any partially extracted payload; neither is
    // needed once the DLL is in place.
    remove_artifact(&scratch)?;
    result
}

/// Select the Vulkan renderer for Wine's Direct3D, as winetricks' `renderer=vulkan` did.
fn configure_vulkan_renderer(config: &InstallConfig<'_>, logger: &Logger) -> io::Result<()> {
    write_registry(config, logger, "vulkan-renderer.reg", VULKAN_REG)
}

/// Write a `REGEDIT4` file into the prefix and import it with `wine regedit`.
///
/// The file lives at the drive's root so the Windows path is a plain `C:\<name>`,
/// and is removed afterwards whether or not the import succeeded.
fn write_registry(
    config: &InstallConfig<'_>,
    logger: &Logger,
    name: &str,
    body: &str,
) -> io::Result<()> {
    let path = config.wine_prefix.join("drive_c").join(name);
    fs::write(&path, format!("REGEDIT4\r\n\r\n{body}"))?;
    let result = run(
        Command::new(&config.tools.wine)
            .env("WINEPREFIX", config.wine_prefix)
            .env("WINEDLLOVERRIDES", "winemenubuilder.exe=d")
            .arg("C:\\windows\\regedit.exe")
            .arg("/S")
            .arg(format!("C:\\{name}")),
        logger,
        &format!("wine regedit {name}"),
    );
    missing_ok_remove_file(&path)?;
    result
}

/// Fetch the game from the CDN and replace its own launcher with the shim.
fn install_game(
    config: &InstallConfig<'_>,
    logger: &Logger,
    progress: &(impl Fn(InstallProgress) + Sync),
) -> io::Result<()> {
    progress(task(InstallTask::Game, TaskState::Running));
    let fetcher = Remote::new(config.cdn_base);
    let sync_config = SyncConfig {
        install_dir: config.install_dir,
        staging_dir: config.staging_dir,
        jobs: config.jobs,
        adaptive: config.adaptive,
    };
    let reporting = |done: u64, total: u64| {
        progress(task(InstallTask::Game, fraction(done, total)));
    };
    sync::sync(&fetcher, &sync_config, logger, &reporting)
        .map_err(|error| io::Error::other(format!("Unable to download Zwift: {error}")))?;
    // The game directory's own ZwiftLauncher.exe is the downloaded patcher, which
    // nothing runs any more. Replacing it keeps `installed()` honest and leaves a
    // binary behind that does what the launcher does, rather than a stale patcher.
    launch::ensure_launcher_shim(config.install_dir, config.launcher_shim)?;
    if !config.install_dir.join("ZwiftApp.exe").is_file() {
        return Err(io::Error::other(
            "The Zwift download finished but ZwiftApp.exe is missing",
        ));
    }
    progress(task(InstallTask::Game, TaskState::Done));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{write_cdn, SERVER_VERSION};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::sync::Mutex;

    struct Fixture {
        dir: std::path::PathBuf,
        tools: Tools,
        /// A `file://` directory serving the prereq cabinet the launcher fetches.
        prerequisites_base: String,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            Self::with_cdn(
                name,
                &[("ZwiftApp.exe", "game"), ("data/Worlds/world.txt", "world")],
            )
        }

        fn with_cdn(name: &str, files: &[(&str, &str)]) -> Self {
            let lock = crate::SCRIPT_TEST_LOCK.lock().unwrap();
            let dir =
                std::env::temp_dir().join(format!("zwift-install-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir(&dir).unwrap();
            write_cdn(&dir.join("cdn"), SERVER_VERSION, files);
            fs::create_dir_all(dir.join("snap")).unwrap();
            fs::write(dir.join("snap/ZwiftLauncher.exe"), "shim").unwrap();
            let tools = Tools {
                wine: dir.join("wine"),
                winepath: dir.join("winepath"),
                wineserver: dir.join("wineserver"),
                cabextract: dir.join("cabextract"),
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
            // A stand-in for the prereq cabinet, served over `file://`. The launcher
            // verifies it against the expectation below, so its bytes are arbitrary.
            let cabinet = b"cabinet";
            fs::create_dir(dir.join("prerequisites")).unwrap();
            fs::write(dir.join("prerequisites").join(D3DCOMPILER_CAB), cabinet).unwrap();
            let script = |path: &Path, body: &str| {
                fs::write(path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            };
            // Like the real thing, wineboot creates the prefix and seeds `system32`
            // with Wine's own builtin `d3dcompiler_47.dll`. The launcher must still
            // install the native one, so this stub is what makes the fixture catch a
            // regression to probing the DLL's presence.
            script(&dir.join("wineboot"), &format!(
                "test \"$WINEDLLOVERRIDES\" = 'mscoree,mshtml=;winemenubuilder.exe=d'\ntest \"$WINEPREFIX\" = '{0}/prefix'\necho wineboot >> '{0}/calls'\n\
                 mkdir -p '{0}/prefix/drive_c/windows/system32'\n\
                 printf builtin > '{0}/prefix/drive_c/windows/system32/d3dcompiler_47.dll'",
                dir.display()
            ));
            // cabextract is invoked as `-q -L -F <dll> -d <dir> <cab>`; the arguments
            // are parsed rather than positional so the stub survives an order change.
            script(
                &dir.join("cabextract"),
                &format!(
                    "inner=''\nout=''\nwhile [ \"$#\" -gt 0 ]; do\n\
                     \tcase \"$1\" in\n\t\t-F) inner=\"$2\"; shift 2;;\n\t\t-d) out=\"$2\"; shift 2;;\n\t\t*) shift;;\n\tesac\ndone\n\
                     mkdir -p \"$out\"\nprintf dll > \"$out/$inner\"\n\
                     echo \"cabextract $inner\" >> '{0}/calls'\n\
                     test ! -f '{0}/fail-cabextract'",
                    dir.display()
                ),
            );
            // Wine itself only runs `regedit` for the prerequisites; the game is
            // downloaded by the launcher, so it needs no other wine process.
            script(
                &dir.join("wine"),
                &format!(
                    "test \"$1\" = 'C:\\windows\\regedit.exe'\necho \"wine regedit\" >> '{0}/calls'",
                    dir.display()
                ),
            );
            // Nothing in a fresh install may run wineserver any more: a call here is a
            // regression.
            script(
                &tools.wineserver,
                &format!("echo wineserver >> '{}/calls'\nexit 1", dir.display()),
            );
            let prerequisites_base = format!("file://{}", dir.join("prerequisites").display());
            Self {
                dir,
                tools,
                prerequisites_base,
                _lock: lock,
            }
        }

        fn run(&self) -> (io::Result<()>, Vec<InstallProgress>) {
            self.run_with_cdn(None)
        }

        fn run_with_cdn(&self, cdn_base: Option<&str>) -> (io::Result<()>, Vec<InstallProgress>) {
            let prefix = self.dir.join("prefix");
            let common = self.dir.join("common");
            let game = self.dir.join("game");
            let staging = common.join("download");
            let shim = self.dir.join("snap/ZwiftLauncher.exe");
            let wineboot = self.dir.join("wineboot");
            let cabinet = fs::read(self.dir.join("prerequisites").join(D3DCOMPILER_CAB)).unwrap();
            let logger = Logger::new(self.dir.join("zwift.log"), 0, false);
            logger.init().unwrap();
            let cdn_base = match cdn_base {
                Some(base) => base.to_string(),
                None => format!("file://{}", self.dir.join("cdn").display()),
            };
            let config = InstallConfig {
                wine_prefix: &prefix,
                user_common: &common,
                install_dir: &game,
                staging_dir: &staging,
                launcher_shim: &shim,
                wineboot: &wineboot,
                prerequisites_base: &self.prerequisites_base,
                prerequisites_expect: Expect {
                    length: cabinet.len() as u64,
                    checksum: Some(crc32fast::hash(&cabinet)),
                },
                tools: &self.tools,
                cdn_base: &cdn_base,
                jobs: 2,
                adaptive: None,
            };
            let events = Mutex::new(Vec::new());
            let result = install(&config, &logger, &|event| {
                events.lock().unwrap().push(event)
            });
            (result, events.into_inner().unwrap())
        }

        fn calls(&self) -> String {
            fs::read_to_string(self.dir.join("calls")).unwrap_or_default()
        }

        fn log(&self) -> String {
            fs::read_to_string(self.dir.join("zwift.log")).unwrap_or_default()
        }

        fn game(&self, name: &str) -> std::path::PathBuf {
            self.dir.join("game").join(name)
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
        let (result, events) = fixture.run();
        result.unwrap();
        // wineboot, then the prerequisite cabinet is extracted and its two registry
        // files imported. The game is fetched over the CDN, so no further wine process
        // runs.
        let calls = fixture.calls();
        assert_eq!(
            calls.lines().collect::<Vec<_>>(),
            [
                "wineboot",
                &format!("cabextract {D3DCOMPILER_CAB_DLL}"),
                "wine regedit",
                "wine regedit",
            ]
        );
        // Wine's builtin must have been overwritten by the native compiler (the
        // fixture's `cabextract` writes the bytes `dll`), and the launcher's marker
        // left behind as the proof of the override.
        let system32 = fixture.dir.join("prefix/drive_c/windows/system32");
        assert_eq!(
            fs::read_to_string(system32.join("d3dcompiler_47.dll")).unwrap(),
            "dll"
        );
        assert!(fixture.dir.join("prefix").join(D3DCOMPILER_PROOF).is_file());
        // Every task starts and every task finishes. The prerequisite and game
        // tracks run concurrently, so the events interleave and only the set matters.
        let mut started = Vec::new();
        let mut finished = Vec::new();
        for event in &events {
            if let InstallProgress::Task { task, state } = event {
                if !started.contains(task) {
                    started.push(*task);
                }
                if *state == TaskState::Done {
                    finished.push(*task);
                }
            }
        }
        for task in [
            InstallTask::WineSetup,
            InstallTask::D3dCompiler,
            InstallTask::Vulkan,
            InstallTask::Game,
        ] {
            assert!(
                started.contains(&task),
                "{task:?} never started: {events:?}"
            );
            assert!(
                finished.contains(&task),
                "{task:?} never finished: {events:?}"
            );
        }
        // The game transfer reports its own fraction as it goes.
        assert!(events.iter().any(|event| matches!(
            event,
            InstallProgress::Task {
                task: InstallTask::Game,
                state: TaskState::Fraction(_)
            }
        )));
        // The manifest's files landed in the game directory, the version file was
        // written, and the program's own ZwiftLauncher.exe was replaced by the shim.
        assert_eq!(
            fs::read_to_string(fixture.game("ZwiftApp.exe")).unwrap(),
            "game"
        );
        assert_eq!(
            fs::read_to_string(fixture.game("data/Worlds/world.txt")).unwrap(),
            "world"
        );
        assert_eq!(
            fs::read_to_string(fixture.game("ZwiftLauncher.exe")).unwrap(),
            "shim"
        );
        assert_eq!(
            crate::sync::local_version(&fixture.dir.join("game")),
            crate::version::Version::parse(SERVER_VERSION).unwrap()
        );
        let documents = fixture.dir.join("prefix/drive_c/users/tester/Documents");
        assert!(documents.is_dir() && !documents.is_symlink());
        assert_eq!(
            fs::read_to_string(fixture.dir.join("hostdocs/keep")).unwrap(),
            "unchanged"
        );
    }

    #[test]
    fn a_prefix_already_at_the_served_version_downloads_nothing() {
        // A restored game tree (or a reinstall triggered by a missing prerequisite
        // proof) leaves the game at the served version before the install runs.
        // Nothing must be transferred then: the local file is left untouched.
        let fixture = Fixture::new("preinstalled");
        let game = fixture.dir.join("game");
        fs::create_dir_all(game.join("data/Worlds")).unwrap();
        fs::write(game.join("ZwiftApp.exe"), "local copy").unwrap();
        fs::write(game.join("data/Worlds/world.txt"), "world").unwrap();
        fs::write(
            game.join("manifest.xml"),
            format!(
                "<Zwift version=\"{SERVER_VERSION}\" folder=\"v1\"><files>\
                 <file><path>ZwiftApp.exe</path><length>10</length>\
                 <modified>2024-01-02 03:04:05.000</modified><checksum>{}</checksum></file>\
                 <file><path>data\\Worlds\\world.txt</path><length>5</length>\
                 <modified>2024-01-02 03:04:05.000</modified><checksum>{}</checksum></file>\
                 </files></Zwift>",
                crc32fast::hash(b"local copy"),
                crc32fast::hash(b"world")
            ),
        )
        .unwrap();
        fs::write(
            game.join("Zwift_ver_cur.xml"),
            format!(
                "<Zwift version=\"{SERVER_VERSION}\" sversion=\"{SERVER_VERSION} (168455)\" \
                 manifest=\"manifest.xml\"/>"
            ),
        )
        .unwrap();
        let (result, _) = fixture.run();
        result.unwrap();
        assert_eq!(
            fs::read_to_string(fixture.game("ZwiftApp.exe")).unwrap(),
            "local copy"
        );
        assert!(
            fixture.log().contains("already installed"),
            "{}",
            fixture.log()
        );
        // The prerequisites still ran, and the shim was still installed.
        assert!(
            fixture.calls().contains("cabextract"),
            "{}",
            fixture.calls()
        );
        assert_eq!(
            fs::read_to_string(fixture.game("ZwiftLauncher.exe")).unwrap(),
            "shim"
        );
    }

    #[test]
    fn a_prerequisite_failure_fails_the_install() {
        // The prerequisite and game tracks run concurrently, so the game may still
        // have been fetched; the prefix track's error is the one that is returned.
        let fixture = Fixture::new("cabextract");
        fs::write(fixture.dir.join("fail-cabextract"), "").unwrap();
        let (result, _) = fixture.run();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains(&format!("cabextract {D3DCOMPILER_CAB}")));
    }

    #[test]
    fn a_transfer_failure_fails_the_install() {
        // The manifest lists a file the CDN cannot serve, which is what a half
        // populated cache or a truncated upload looks like.
        let fixture = Fixture::new("transfer");
        fs::remove_file(fixture.dir.join("cdn/v1/data/Worlds/world.txt")).unwrap();
        let (result, _) = fixture.run();
        let error = result.unwrap_err().to_string();
        assert!(error.contains("Unable to download Zwift"), "{error}");
        assert!(error.contains("world.txt"), "{error}");
        assert!(!fixture.game("ZwiftLauncher.exe").exists());
    }

    #[test]
    fn cleanup_removes_artifacts_without_following_symlinks() {
        let fixture = Fixture::new("cleanup");
        let prefix = fixture.dir.join("prefix");
        let temp = prefix.join("drive_c/users/tester/AppData/Local/Temp");
        fs::create_dir_all(&temp).unwrap();
        fs::write(temp.join("discard"), "temporary").unwrap();
        std::os::unix::fs::symlink(fixture.dir.join("hostdocs"), temp.join("link")).unwrap();
        let common = fixture.dir.join("common");
        let prerequisites = common.join("prerequisites");
        fs::create_dir_all(&prerequisites).unwrap();
        fs::write(prerequisites.join("partial"), "").unwrap();
        let staging = fixture.dir.join("common/download");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("partial"), "").unwrap();
        // A prefix installed by an older launcher still holds the downloaded installer.
        fs::write(fixture.dir.join("common/ZwiftSetup.exe"), "").unwrap();
        cleanup_artifacts(&prefix, &common, &staging).unwrap();
        assert_eq!(fs::read_dir(&temp).unwrap().count(), 0);
        assert!(!prerequisites.exists());
        assert!(!staging.exists());
        assert!(!fixture.dir.join("common/ZwiftSetup.exe").exists());
        assert!(fixture.dir.join("hostdocs/keep").exists());
        fs::remove_dir(&temp).unwrap();
        std::os::unix::fs::symlink(fixture.dir.join("hostdocs"), &temp).unwrap();
        assert!(cleanup_artifacts(&prefix, &common, &staging).is_err());
        assert!(fixture.dir.join("hostdocs/keep").exists());
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
        fs::remove_file(fixture.dir.join("cdn/Zwift_ver_cur.xml")).unwrap();
        let (result, _) = fixture.run();
        let error = result.unwrap_err().to_string();
        assert!(error.contains("Unable to download Zwift"), "{error}");
        assert!(error.contains("Zwift_ver_cur.xml"), "{error}");
        assert!(!fixture.game("ZwiftLauncher.exe").exists());
    }

    #[test]
    fn an_unwritable_cdn_base_fails_instead_of_installing_nothing() {
        let fixture = Fixture::new("bad-base");
        let base = format!("file://{}", fixture.dir.join("absent").display());
        let (result, _) = fixture.run_with_cdn(Some(&base));
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Unable to download Zwift"));
    }

    #[test]
    fn a_manifest_without_zwiftapp_is_not_success() {
        // The CDN can serve a manifest whose files are incomplete; the launcher must
        // not report a fresh install as done without the binary it launches.
        let fixture = Fixture::with_cdn("missing-game", &[("data/Worlds/world.txt", "world")]);
        let (result, _) = fixture.run();
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("ZwiftApp.exe is missing"));
    }

    #[test]
    fn wine_prerequisites_are_detected_from_their_proof_paths() {
        let dir = std::env::temp_dir().join(format!("zwift-prerequisites-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();

        // An empty prefix has no prerequisite at all, so the probed one is reported.
        assert_eq!(missing_wine_prerequisite(&dir), Some(D3DCOMPILER_PROOF));
        assert!(!wine_prerequisites_installed(&dir));

        // Wine's own builtin DLL is not the proof: wineboot puts one there before
        // the launcher ever runs, so it must not hide the missing prerequisite.
        let dll = dir.join(D3DCOMPILER_DLL);
        fs::create_dir_all(dll.parent().unwrap()).unwrap();
        fs::write(&dll, "wine builtin").unwrap();
        assert_eq!(missing_wine_prerequisite(&dir), Some(D3DCOMPILER_PROOF));
        assert!(!wine_prerequisites_installed(&dir));

        // The launcher's marker is the proof.
        let proof = dir.join(D3DCOMPILER_PROOF);
        fs::write(&proof, D3DCOMPILER_DLL).unwrap();
        assert!(wine_prerequisites_installed(&dir));

        // It is load-bearing: without it the prefix counts as incomplete again.
        fs::remove_file(&proof).unwrap();
        assert_eq!(missing_wine_prerequisite(&dir), Some(D3DCOMPILER_PROOF));
        assert!(!wine_prerequisites_installed(&dir));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_prefix_with_the_compiler_marker_skips_the_reinstall() {
        // The marker is the proof: with it present the prerequisite is left alone.
        // A marker written with a sentinel must survive unchanged, which shows the
        // step did not run and rewrite it.
        let fixture = Fixture::new("compiler-done");
        let proof = fixture.dir.join("prefix").join(D3DCOMPILER_PROOF);
        fs::create_dir_all(proof.parent().unwrap()).unwrap();
        fs::write(&proof, "sentinel").unwrap();
        let (result, _) = fixture.run();
        result.unwrap();
        assert!(
            !fixture.calls().contains("cabextract"),
            "{}",
            fixture.calls()
        );
        assert_eq!(fs::read_to_string(&proof).unwrap(), "sentinel");
    }
}
