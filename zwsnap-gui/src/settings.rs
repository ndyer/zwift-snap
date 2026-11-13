//! User-facing settings, stored in `$SNAP_USER_COMMON/settings.json`.
//!
//! The file is seeded on first run from a read-only template shipped in the snap
//! (`$SNAP/etc/zwift/settings.json`), then edited either by hand or through the
//! Settings dialog. Snap upgrades never clobber the user's copy: the template is
//! only read when the user file does not yet exist, and missing or invalid fields
//! in a stale user file fall back to the built-in defaults so a future snap can
//! add a key without breaking an old file.

use crate::display::DisplayMode;
use crate::graphics::{self, Setting as GraphicsSetting};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The name of the settings file, inside both `$SNAP_USER_COMMON` and the
/// read-only template location `$SNAP/etc/zwift/`.
pub const FILE_NAME: &str = "settings.json";

/// User-editable settings. `Display`/`GraphicsSetting` are shared with the rest of
/// the launcher, so a value here behaves exactly like the environment variable it
/// replaced.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub display: DisplayMode,
    pub graphics: GraphicsSetting,
    pub disable_egl: bool,
    pub bluetooth_debug: bool,
    pub verbosity: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            display: DisplayMode::Fullscreen,
            graphics: GraphicsSetting::Auto,
            disable_egl: false,
            bluetooth_debug: false,
            verbosity: 1,
        }
    }
}

impl Settings {
    /// Loads `path`, falling back to defaults for anything missing or unreadable.
    /// Errors are reported on stderr (no logger exists yet when this runs) but
    /// never fail the launch.
    pub fn load(path: &Path) -> Settings {
        let mut settings = Settings::default();
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return settings,
            Err(error) => {
                eprintln!("warning: cannot read {}: {error}", path.display());
                return settings;
            }
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("warning: {} is not valid JSON: {error}", path.display());
                return settings;
            }
        };
        let Some(object) = value.as_object() else {
            eprintln!("warning: {} must be a JSON object", path.display());
            return settings;
        };
        if let Some(display) = object.get("display_mode") {
            match display.as_str().map(DisplayMode::parse) {
                Some(Ok(mode)) => settings.display = mode,
                Some(Err(error)) => {
                    eprintln!(
                        "warning: ignoring invalid display_mode in {}: {error}",
                        path.display()
                    )
                }
                None => eprintln!(
                    "warning: ignoring non-string display_mode in {}",
                    path.display()
                ),
            }
        }
        if let Some(graphics) = object.get("graphics_profile") {
            match graphics.as_str().map(GraphicsSetting::parse) {
                Some(Ok(setting)) => settings.graphics = setting,
                Some(Err(error)) => {
                    eprintln!(
                        "warning: ignoring invalid graphics_profile in {}: {error}",
                        path.display()
                    )
                }
                None => eprintln!(
                    "warning: ignoring non-string graphics_profile in {}",
                    path.display()
                ),
            }
        }
        if let Some(disable_egl) = object.get("disable_egl") {
            match disable_egl.as_bool() {
                Some(value) => settings.disable_egl = value,
                None => eprintln!(
                    "warning: ignoring non-boolean disable_egl in {}",
                    path.display()
                ),
            }
        }
        if let Some(bluetooth_debug) = object.get("bluetooth_debug") {
            match bluetooth_debug.as_bool() {
                Some(value) => settings.bluetooth_debug = value,
                None => eprintln!(
                    "warning: ignoring non-boolean bluetooth_debug in {}",
                    path.display()
                ),
            }
        }
        if let Some(verbosity) = object.get("verbosity").and_then(serde_json::Value::as_u64) {
            if verbosity <= u64::from(u8::MAX) {
                settings.verbosity = verbosity as u8;
            } else {
                eprintln!(
                    "warning: ignoring out-of-range verbosity in {}",
                    path.display()
                );
            }
        }
        settings
    }

    /// Writes the settings atomically (temp file + rename) as prettified JSON,
    /// owner-only (0600).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let value = serde_json::json!({
            "display_mode": self.display.to_string(),
            "graphics_profile": self.graphics.to_string(),
            "disable_egl": self.disable_egl,
            "bluetooth_debug": self.bluetooth_debug,
            "verbosity": self.verbosity,
        });
        let text = serde_json::to_string_pretty(&value)
            .map_err(|error| io::Error::other(error.to_string()))?;
        write_atomic(path, text.as_bytes())
    }
}

/// Copies the read-only template into `$SNAP_USER_COMMON/settings.json` on first
/// run and returns that path. If the user file already exists it is left alone;
/// if the template is missing (e.g. running outside the snap) the file is simply
/// not created and the returned path is still where it would live.
pub fn ensure_initialized(snap: &Path, common: &Path) -> io::Result<PathBuf> {
    let path = common.join(FILE_NAME);
    if path.exists() {
        return Ok(path);
    }
    let template = snap.join("etc/zwift").join(FILE_NAME);
    if template.is_file() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(&template, &path)?;
        make_private(&path)?;
    }
    Ok(path)
}

/// Writes `data` to `path` atomically, with owner-only permissions.
fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().as_bytes().to_vec();
    tmp.extend_from_slice(b".tmp");
    let tmp_path = PathBuf::from(std::ffi::OsStr::from_bytes(&tmp));
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)?;
        file.write_all(data)?;
        file.sync_all()?;
    }
    fs::rename(&tmp_path, path)
}

fn make_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Whether the given snap interface plug is connected, via `snapctl`. Returns
/// `None` when `snapctl` is unavailable (e.g. the launcher is not running inside
/// the snap), so the caller can distinguish "unknown" from "no".
pub fn snap_is_connected(plug: &str) -> Option<bool> {
    let status = std::process::Command::new("snapctl")
        .args(["is-connected", plug])
        .status()
        .ok()?;
    Some(status.success())
}

/// The result of measuring the GPU with the same workload the launcher uses to
/// pick a graphics profile.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuProbe {
    pub renderer: String,
    pub frame_ms: f64,
    pub profile: graphics::Profile,
}

/// Measures the GPU and maps the result to a graphics profile. `Err` carries the
/// reason the measurement failed (no GPU access, software renderer, etc.).
pub fn probe_gpu() -> Result<GpuProbe, String> {
    let test = crate::gpu_test::run()?;
    let profile = graphics::Profile::for_frame_time(test.frame_ms);
    Ok(GpuProbe {
        renderer: test.renderer,
        frame_ms: test.frame_ms,
        profile,
    })
}

/// Deletes the whole wine prefix. Refuses to remove a path that does not look like
/// a wine prefix (its final component must be `.wine`), so a misconfiguration
/// cannot erase unrelated data.
pub fn purge_wine_prefix(prefix: &Path) -> io::Result<()> {
    if prefix.file_name().and_then(|name| name.to_str()) != Some(".wine") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("refusing to remove {}: not a wine prefix", prefix.display()),
        ));
    }
    fs::remove_dir_all(prefix)
}

/// Deletes the installed game and its per-user data, keeping the wine prefix (and
/// thus registry/configuration state) so the game is re-downloaded on next start.
pub fn purge_game_data(prefix: &Path) -> io::Result<()> {
    let install = prefix.join("drive_c/Program Files (x86)/Zwift");
    if install.is_dir() {
        fs::remove_dir_all(&install)?;
    }
    let users = prefix.join("drive_c/users");
    let entries = match fs::read_dir(&users) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries.flatten() {
        for relative in ["AppData/Local/Zwift", "Documents/Zwift"] {
            let dir = entry.path().join(relative);
            if dir.is_dir() {
                fs::remove_dir_all(&dir)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graphics::Profile;
    use std::fs;

    fn fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zwsnap-gui-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_round_trip_through_save_and_load() {
        let dir = fixture("defaults");
        let path = dir.join("settings.json");
        let settings = Settings::default();
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded, settings);
    }

    #[test]
    fn load_is_lenient_about_garbage_and_missing_keys() {
        let dir = fixture("lenient");
        let path = dir.join("settings.json");

        // Not valid JSON.
        fs::write(&path, "not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());

        // Missing keys fall back to defaults, present keys apply.
        fs::write(&path, r#"{"display_mode":"800x600","bogus":"x"}"#).unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(
            loaded.display,
            DisplayMode::Windowed {
                width: 800,
                height: 600
            }
        );
        assert_eq!(loaded.graphics, GraphicsSetting::Auto);
        assert_eq!(loaded.verbosity, 1);

        // Invalid field values are ignored individually.
        fs::write(
            &path,
            r#"{"display_mode":"oops","graphics_profile":"ultra",
                "disable_egl":true,"bluetooth_debug":true,"verbosity":3}"#,
        )
        .unwrap();
        let loaded = Settings::load(&path);
        assert_eq!(loaded.display, DisplayMode::Fullscreen);
        assert_eq!(loaded.graphics, GraphicsSetting::Fixed(Profile::Ultra));
        assert!(loaded.disable_egl);
        assert!(loaded.bluetooth_debug);
        assert_eq!(loaded.verbosity, 3);

        // A JSON array is not an object.
        fs::write(&path, r#"[]"#).unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }

    #[test]
    fn ensure_initialized_copies_template_once() {
        let dir = fixture("init");
        let snap = dir.join("snap");
        let common = dir.join("common");
        let template_dir = snap.join("etc/zwift");
        fs::create_dir_all(&template_dir).unwrap();
        let template = template_dir.join(FILE_NAME);
        fs::write(&template, r#"{"verbosity":3}"#).unwrap();

        let path = ensure_initialized(&snap, &common).unwrap();
        assert_eq!(path, common.join(FILE_NAME));
        assert_eq!(Settings::load(&path).verbosity, 3);

        // A subsequent call must not clobber a user edit.
        fs::write(&path, r#"{"verbosity":4}"#).unwrap();
        let path = ensure_initialized(&snap, &common).unwrap();
        assert_eq!(Settings::load(&path).verbosity, 4);
    }

    #[test]
    fn ensure_initialized_without_template_returns_path() {
        let dir = fixture("init-empty");
        let snap = dir.join("snap");
        let common = dir.join("common");
        let path = ensure_initialized(&snap, &common).unwrap();
        assert_eq!(path, common.join(FILE_NAME));
        assert!(!path.exists());
    }

    #[test]
    fn purge_wine_prefix_refuses_non_prefix() {
        let dir = fixture("purge-prefix");
        assert!(purge_wine_prefix(dir.as_path()).is_err());

        let prefix = dir.join(".wine");
        fs::create_dir_all(prefix.join("drive_c")).unwrap();
        purge_wine_prefix(&prefix).unwrap();
        assert!(!prefix.exists());
    }

    #[test]
    fn purge_game_data_keeps_prefix() {
        let dir = fixture("purge-game");
        let prefix = dir.join(".wine");
        let install = prefix.join("drive_c/Program Files (x86)/Zwift");
        let user = prefix.join("drive_c/users/tester");
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(user.join("AppData/Local/Zwift")).unwrap();
        fs::create_dir_all(user.join("Documents/Zwift")).unwrap();
        fs::write(prefix.join("drive_c/keep.txt"), "keep").unwrap();

        purge_game_data(&prefix).unwrap();
        assert!(!install.exists());
        assert!(!user.join("AppData/Local/Zwift").exists());
        assert!(!user.join("Documents/Zwift").exists());
        assert!(prefix.join("drive_c/keep.txt").exists());
    }
}
