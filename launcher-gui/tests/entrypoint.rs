use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

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
        let game = fixture
            .root
            .join("common/.wine/drive_c/Program Files (x86)/Zwift");
        fs::create_dir_all(&game).unwrap();
        for name in ["ZwiftApp.exe", "ZwiftLauncher.exe"] {
            fs::write(game.join(name), "").unwrap();
        }
        fs::write(
            game.join("Zwift_ver_cur.xml"),
            "<Zwift sversion=\"1.2.0 x\"/>",
        )
        .unwrap();
        let data = fixture
            .root
            .join("common/.wine/drive_c/users/tester/AppData/Local/Zwift");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("prefs.xml"), "<ZWIFT><CONFIG/></ZWIFT>").unwrap();
        fixture.script("wget", "printf '<Zwift sversion=\"1.2.0 x\"/>'");
        fixture.script("winepath", "printf 'C:\\\\Zwift\\n'");
        fixture.script("wineserver", "echo \"$1\" >> \"$TEST_ROOT/cleanup\"");
        fixture.script(
            "wine",
            r#"printf '%s\n' "$*" >> "$TEST_ROOT/commands"
case "$1" in
start) printf 'child output\n'; printf 'child diagnostic\n' >&2;;
tasklist) case "$5" in
    *ZwiftLauncher.exe) printf 'PID: 41\n';;
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
            .env("ZWIFT_POLL_INTERVAL", "0");
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
    assert!(fixture.read("commands").contains("--example=hello world"));
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

#[test]
fn sigterm_cancels_the_real_worker_and_still_runs_cleanup() {
    let fixture = Fixture::new();
    fixture.script("wget", "touch \"$TEST_ROOT/waiting\"; exec sleep 30");
    let child = fixture
        .command()
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
    assert!(ready, "worker did not start");
    assert!(!result.status.success());
    assert!(fixture
        .read("common/zwift.log")
        .contains("Launcher cancelled"));
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
