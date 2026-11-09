use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

/// The version the fixture's game and CDN both report.
const VERSION: &str = "1.2.0";

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "zwift-entrypoint-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let fixture = Self { root };
        fs::create_dir_all(fixture.root.join("bin")).unwrap();
        // The snap's own ZwiftLauncher.exe (the windows-helpers part), which the
        // launcher copies into the game directory before starting it.
        fs::write(fixture.root.join("bin/ZwiftLauncher.exe"), "shim").unwrap();
        let game = fixture
            .root
            .join("common/.wine/drive_c/Program Files (x86)/Zwift");
        fs::create_dir_all(&game).unwrap();
        for name in ["ZwiftApp.exe", "ZwiftLauncher.exe"] {
            fs::write(game.join(name), "").unwrap();
        }
        fixture.write_wine_prerequisites();
        // The installed version, as the game records it. `parse_ver_cur` requires a
        // `manifest` attribute even when the manifest file itself is never read,
        // which is the case here: the served version matches, so the sync stops at
        // the version check.
        fs::write(
            game.join("Zwift_ver_cur.xml"),
            format!(
                "<Zwift version=\"{VERSION}\" sversion=\"{VERSION} x\" manifest=\"manifest.xml\"/>\n"
            ),
        )
        .unwrap();
        // The CDN the launcher syncs against, standing in for cdn.zwift.com: it
        // serves the same version, so the update check finds the install current
        // and transfers nothing. Nothing here can reach the network.
        let cdn = fixture.root.join("cdn");
        fs::create_dir(&cdn).unwrap();
        fs::write(
            cdn.join("Zwift_ver_cur.xml"),
            format!(
                "<Zwift version=\"{VERSION}\" sversion=\"{VERSION} x\" manifest=\"manifest.xml\"/>\n"
            ),
        )
        .unwrap();
        let data = fixture
            .root
            .join("common/.wine/drive_c/users/tester/AppData/Local/Zwift");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("prefs.xml"), "<ZWIFT><CONFIG/></ZWIFT>").unwrap();
        fixture.script("winepath", "printf 'C:\\\\Zwift\\n'");
        fixture.script("wineserver", "echo \"$1\" >> \"$TEST_ROOT/cleanup\"");
        fixture.script(
            "wine",
            r#"printf '%s\n' "$*" >> "$TEST_ROOT/commands"
case "$1" in
start) test "$4" = ZwiftLauncher.exe; test "$5" = ZwiftApp.exe
    if [ -n "${TEST_HANG_START:-}" ]; then touch "$TEST_ROOT/waiting"; exec sleep 30; fi
    printf 'child output\n'; printf 'child diagnostic\n' >&2;;
tasklist) case "$5" in
    *ZwiftApp.exe) if [ ! -f "$TEST_ROOT/seen" ]; then
        touch "$TEST_ROOT/seen"; printf 'PID: 42\n'
    fi;;
esac;;
taskkill|reg.exe) :;;
*) exit 1;;
esac"#,
        );
        fixture
    }

    fn game_dir(&self) -> PathBuf {
        self.root
            .join("common/.wine/drive_c/Program Files (x86)/Zwift")
    }

    /// Fabricate the game's graphics profile files, so the launcher can write them.
    /// Tests that do not call this keep the launcher on the "no profile files" path
    /// and never try to measure the (absent) GPU.
    fn write_graphics_profiles(&self) {
        let configs = self.game_dir().join("data/configs");
        fs::create_dir_all(&configs).unwrap();
        for profile in ["basic", "medium", "high", "ultra"] {
            fs::write(configs.join(format!("{profile}.txt")), profile).unwrap();
        }
    }

    /// The content of all four graphics profile files, in `Profile::ALL` order.
    fn graphics_contents(&self) -> [String; 4] {
        ["basic", "medium", "high", "ultra"].map(|profile| {
            fs::read_to_string(self.game_dir().join(format!("data/configs/{profile}.txt"))).unwrap()
        })
    }

    /// Fabricate what the Wine prerequisites leave behind, so the launcher sees the
    /// prefix as installed. Kept in step with the sentinels in
    /// `launcher-gui/src/install.rs` (`WINETRICKS_PREREQUISITES`).
    fn write_wine_prerequisites(&self) {
        let prefix = self.root.join("common/.wine");
        let path = prefix.join("drive_c/windows/system32/d3dcompiler_47.dll");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn script(&self, name: &str, body: &str) {
        let path = self.root.join("bin").join(name);
        fs::write(&path, format!("#!/bin/sh\nset -eu\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_zwift-gui"));
        command
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.root.join("bin").display()),
            )
            .env("SNAP", &self.root)
            .env("SNAP_USER_COMMON", self.root.join("common"))
            .env("TEST_ROOT", &self.root)
            .env("ZWIFT_NO_GUI", "1")
            .env("ZWIFT_DISPLAY_MODE", "800x600")
            .env("ZWIFT_POLL_INTERVAL", "0")
            .env(
                "ZWIFT_CDN_BASE",
                format!("file://{}", self.root.join("cdn").display()),
            );
        command
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.root.join(name)).unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).unwrap();
    }
}

fn wait(mut child: Child) -> Output {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("launcher did not finish in time");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn output(command: &mut Command) -> Output {
    wait(
        command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    )
}

#[test]
fn missing_preferences_are_created_before_launching() {
    let fixture = Fixture::new();
    let prefs = fixture
        .root
        .join("common/.wine/drive_c/users/tester/AppData/Local/Zwift/prefs.xml");
    fs::remove_file(&prefs).unwrap();
    fs::remove_dir(prefs.parent().unwrap()).unwrap();
    let result = output(&mut fixture.command());
    assert!(result.status.success(), "{result:?}");
    assert!(fs::read_to_string(&prefs)
        .unwrap()
        .contains("<USER_RESOLUTION_PREF>800x600</USER_RESOLUTION_PREF>"));
}

#[test]
fn terminal_entrypoint_launches_logs_configures_and_cleans_up() {
    let fixture = Fixture::new();
    fs::write(fixture.root.join("common/zwift.log"), "previous").unwrap();
    let result = output(fixture.command().args(["--", "--example=hello world"]));
    assert!(result.status.success(), "{result:?}");
    assert_eq!(fixture.read("common/zwift.log.1"), "previous");
    let log = fixture.read("common/zwift.log");
    assert!(log.contains("Checking for Zwift updates"));
    assert!(log.contains("child output"));
    assert!(log.contains("child diagnostic"));
    assert!(log.contains("Zwift closed, exiting"));
    let commands = fixture.read("commands");
    assert!(commands.contains("start /d C:\\Zwift ZwiftLauncher.exe ZwiftApp.exe"));
    assert!(commands.contains("--example=hello world"));
    assert!(!commands.contains("taskkill /f /im ZwiftLauncher.exe"));
    assert_eq!(
        fs::read_to_string(fixture.game_dir().join("ZwiftLauncher.exe")).unwrap(),
        "shim"
    );
    assert!(fixture
        .read("common/.wine/drive_c/users/tester/AppData/Local/Zwift/prefs.xml")
        .contains("<USER_RESOLUTION_PREF>800x600</USER_RESOLUTION_PREF>"));
    assert_eq!(fixture.read("cleanup"), "-k\n-w\n");
}

#[test]
fn invalid_configuration_never_starts_wine() {
    let fixture = Fixture::new();
    let result = output(fixture.command().env("ZWIFT_DISPLAY_MODE", "oops"));
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Invalid display mode"));
    assert!(!fixture.root.join("commands").exists());
    assert!(!fixture.root.join("cleanup").exists());
}

#[test]
fn start_failure_is_logged_and_returns_failure_after_cleanup() {
    let fixture = Fixture::new();
    fixture.script("wine", "printf 'start failed\\n' >&2; exit 7");
    let result = output(&mut fixture.command());
    assert!(!result.status.success());
    let log = fixture.read("common/zwift.log");
    assert!(log.contains("start failed"));
    assert!(log.contains("Failed to start Zwift"));
    assert_eq!(fixture.read("cleanup"), "-k\n-w\n");
}

/// A SIGTERM while the launcher is waiting on the game must kill the child it
/// started and still run the shutdown cleanup.
///
/// This used to hang a `wget` stub, because the download ran as a child process.
/// The download is now in-process, so the interesting child is the game itself.
#[test]
fn sigterm_terminates_the_game_child_and_still_runs_cleanup() {
    let fixture = Fixture::new();
    let child = fixture
        .command()
        .env("TEST_HANG_START", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !fixture.root.join("waiting").exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    let ready = fixture.root.join("waiting").exists();
    // SAFETY: this PID belongs to the launcher child started by this test.
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGTERM) }, 0);
    let result = wait(child);
    let log = fs::read_to_string(fixture.root.join("common/zwift.log")).unwrap_or_default();
    assert!(ready, "the game did not start:\n{log}");
    assert!(!result.status.success());
    assert!(log.contains("Launcher cancelled"));
    assert_eq!(fixture.read("cleanup"), "-k\n-w\n");
}

#[cfg(feature = "gtk-ui")]
#[test]
fn unavailable_display_falls_back_to_terminal() {
    let fixture = Fixture::new();
    let result = output(
        fixture
            .command()
            .env("ZWIFT_NO_GUI", "0")
            .env("GDK_BACKEND", "x11")
            .env("DISPLAY", "invalid:98765"),
    );
    assert!(result.status.success(), "{result:?}");
    assert!(fixture
        .read("common/zwift.log")
        .contains("using terminal output"));
    assert_eq!(fixture.read("cleanup"), "-k\n-w\n");
}

#[test]
fn a_forced_graphics_profile_is_written_into_every_profile_file() {
    let fixture = Fixture::new();
    fixture.write_graphics_profiles();
    let result = output(fixture.command().env("ZWIFT_GRAPHICS_PROFILE", "ultra"));
    assert!(result.status.success(), "{result:?}");
    // All four files hold the forced profile's content, so it does not matter
    // which one the game's GPU table picks.
    assert_eq!(fixture.graphics_contents(), ["ultra"; 4]);
    // The game's own content is saved before being overwritten.
    for profile in ["basic", "medium", "high", "ultra"] {
        assert_eq!(
            fixture.read(&format!("common/graphics/{profile}.txt")),
            profile
        );
    }
    assert_eq!(fixture.read("common/graphics/applied"), "ultra\n");
    assert!(fixture
        .read("common/zwift.log")
        .contains("Graphics profile: ultra (set with ZWIFT_GRAPHICS_PROFILE)"));
}

#[test]
fn the_default_graphics_setting_restores_the_game_files() {
    let fixture = Fixture::new();
    fixture.write_graphics_profiles();
    let result = output(fixture.command().env("ZWIFT_GRAPHICS_PROFILE", "high"));
    assert!(result.status.success(), "{result:?}");
    assert_eq!(fixture.graphics_contents(), ["high"; 4]);
    // The fake wine reports ZwiftApp.exe only once per fixture.
    fs::remove_file(fixture.root.join("seen")).unwrap();
    let result = output(fixture.command().env("ZWIFT_GRAPHICS_PROFILE", "default"));
    assert!(result.status.success(), "{result:?}");
    assert_eq!(
        fixture.graphics_contents(),
        ["basic", "medium", "high", "ultra"]
    );
    assert!(!fixture.root.join("common/graphics/applied").exists());
    assert!(fixture
        .read("common/zwift.log")
        .contains("Graphics profile: game default (left to the game)"));
}

#[test]
fn invalid_graphics_profile_never_starts_wine() {
    let fixture = Fixture::new();
    let result = output(fixture.command().env("ZWIFT_GRAPHICS_PROFILE", "fastest"));
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Unknown graphics profile"));
    assert!(!fixture.root.join("commands").exists());
    assert!(!fixture.root.join("cleanup").exists());
}
