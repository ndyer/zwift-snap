//! Zwift graphics profile written into the game's own `data/configs/*.txt`.
//!
//! Zwift ships four profile files and picks one from its internal GPU table, which
//! rates many cards below what they can render. The launcher can instead measure
//! the machine (`gpu_test`) and write the matching profile's content into *all
//! four* files, so whichever file the game picks gives the same result.
//!
//! The game's own content is saved first, next to the Wine prefix in
//! `<common>/graphics/`, because writing over the files destroys it. The saved
//! copies are also the source of what gets written, and `applied` records which
//! profile was forced so a file replaced behind this module's back - by the
//! launcher's own sync of a new game version - is recognised and that file's
//! original is refreshed.
//!
//! `ZWIFT_GRAPHICS_PROFILE` selects the behaviour: `auto` (default) measures,
//! `basic`/`medium`/`high`/`ultra` force a profile, and `default` restores the
//! game's own files so the game's GPU table decides again.

use crate::gpu_test::{self, GpuTest};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Frame-time limits of the measured workload, in milliseconds.
///
/// Calibrated against `gpu_test`'s workload: the highest profile whose limit the
/// median frame time meets is used. Changing that workload invalidates these.
pub const MAX_FRAME_MS_FOR_60_FPS: [(Profile, f64); 3] = [
    (Profile::Ultra, 7.25),
    (Profile::High, 7.84),
    (Profile::Medium, 8.88),
];

/// The name of the file recording which profile was last forced.
const APPLIED_FILE: &str = "applied";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Profile {
    Basic,
    Medium,
    High,
    Ultra,
}

impl Profile {
    pub const ALL: [Profile; 4] = [
        Profile::Basic,
        Profile::Medium,
        Profile::High,
        Profile::Ultra,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Profile::Basic => "basic",
            Profile::Medium => "medium",
            Profile::High => "high",
            Profile::Ultra => "ultra",
        }
    }

    /// The game file this profile is read from and written to.
    pub fn relative_path(self) -> PathBuf {
        PathBuf::from("data/configs").join(format!("{}.txt", self.name()))
    }

    /// The highest profile whose frame-time limit the measurement meets.
    pub fn for_frame_time(frame_ms: f64) -> Profile {
        MAX_FRAME_MS_FOR_60_FPS
            .iter()
            .find(|(_, limit)| frame_ms <= *limit)
            .map_or(Profile::Basic, |(profile, _)| *profile)
    }

    fn parse(word: &str) -> Option<Profile> {
        Profile::ALL
            .into_iter()
            .find(|profile| profile.name() == word)
    }
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// What `ZWIFT_GRAPHICS_PROFILE` asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// Measure at every launch.
    Auto,
    Fixed(Profile),
    /// Leave the game's own files alone and let its GPU table decide.
    GameDefault,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SettingError(pub String);

impl fmt::Display for SettingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SettingError {}

impl Setting {
    pub fn parse(word: &str) -> Result<Setting, SettingError> {
        match word {
            "auto" => Ok(Setting::Auto),
            "default" => Ok(Setting::GameDefault),
            _ => Profile::parse(word)
                .map(Setting::Fixed)
                .ok_or_else(|| SettingError(format!("Unknown graphics profile: {word}. Use auto, basic, medium, high, ultra or default."))),
        }
    }

    /// Setting from `ZWIFT_GRAPHICS_PROFILE`; empty counts as unset.
    pub fn from_env() -> Result<Setting, SettingError> {
        match std::env::var("ZWIFT_GRAPHICS_PROFILE") {
            Ok(value) if !value.is_empty() => Setting::parse(&value),
            _ => Ok(Setting::Auto),
        }
    }
}

impl fmt::Display for Setting {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Setting::Auto => f.write_str("auto"),
            Setting::Fixed(profile) => profile.fmt(f),
            Setting::GameDefault => f.write_str("default"),
        }
    }
}

/// What the launcher did, for the log.
#[derive(Debug, Clone, PartialEq)]
pub enum Choice {
    Fixed(Profile),
    Measured {
        profile: Profile,
        test: GpuTest,
    },
    /// Each profile file holds the game's own content.
    GameDefault {
        reason: String,
    },
}

impl Choice {
    fn forced_profile(&self) -> Option<Profile> {
        match self {
            Choice::Fixed(profile) | Choice::Measured { profile, .. } => Some(*profile),
            Choice::GameDefault { .. } => None,
        }
    }
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Choice::Fixed(profile) => {
                write!(f, "{profile} (set with ZWIFT_GRAPHICS_PROFILE)")
            }
            Choice::Measured { profile, test } => write!(
                f,
                "{profile} (GPU test {:.2} ms on {})",
                test.frame_ms, test.renderer
            ),
            Choice::GameDefault { reason } => write!(f, "game default ({reason})"),
        }
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            Err(error)
        }
    }
}

fn read_applied(originals: &Path) -> Option<Profile> {
    let text = fs::read_to_string(originals.join(APPLIED_FILE)).ok()?;
    Profile::parse(text.trim())
}

/// Saves the game's own profile files and returns their content by profile.
///
/// A file only holds the game's content when it is not what this module last
/// wrote: with `applied` set, anything that differs from that profile's copy was
/// replaced by an update and is the new original.
fn refresh_originals(
    install_dir: &Path,
    originals: &Path,
) -> io::Result<BTreeMap<Profile, Vec<u8>>> {
    fs::create_dir_all(originals)?;
    let applied = read_applied(originals);
    let written = applied
        .map(|profile| originals.join(format!("{}.txt", profile.name())))
        .and_then(|path| fs::read(path).ok());
    let mut saved = BTreeMap::new();
    for profile in Profile::ALL {
        let file = install_dir.join(profile.relative_path());
        let copy = originals.join(format!("{}.txt", profile.name()));
        let disk = match fs::read(&file) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            // A single unreadable file only excludes that profile.
            Err(_) => None,
        };
        let is_original = match (&written, &disk) {
            (Some(written), Some(disk)) => disk != written,
            (None, Some(_)) => true,
            _ => false,
        };
        if is_original {
            if let Some(disk) = &disk {
                if !fs::read(&copy).is_ok_and(|current| &current == disk) {
                    write_atomically(&copy, disk)?;
                }
            }
        }
        if let Ok(bytes) = fs::read(&copy) {
            saved.insert(profile, bytes);
        }
    }
    Ok(saved)
}

/// Applies `setting`, measuring the machine when it is `Auto`.
pub fn apply(install_dir: &Path, originals: &Path, setting: Setting) -> io::Result<Choice> {
    apply_with(install_dir, originals, setting, gpu_test::run)
}

/// `apply` with the measurement injected, so tests need no GPU.
pub fn apply_with(
    install_dir: &Path,
    originals: &Path,
    setting: Setting,
    measure: impl FnOnce() -> Result<GpuTest, String>,
) -> io::Result<Choice> {
    let saved = refresh_originals(install_dir, originals)?;
    if saved.is_empty() {
        return Ok(Choice::GameDefault {
            reason: "the installed Zwift has no graphics profile files".into(),
        });
    }
    let choice = match setting {
        Setting::GameDefault => Choice::GameDefault {
            reason: "left to the game".into(),
        },
        Setting::Fixed(profile) => Choice::Fixed(profile),
        Setting::Auto => match measure() {
            Ok(test) => {
                let profile = Profile::for_frame_time(test.frame_ms);
                Choice::Measured { profile, test }
            }
            Err(reason) => Choice::GameDefault {
                reason: format!("GPU test failed: {reason}"),
            },
        },
    };
    // Forcing a profile without a saved copy of it would write nothing anywhere.
    let choice = match choice.forced_profile() {
        Some(profile) if !saved.contains_key(&profile) => Choice::GameDefault {
            reason: format!("no saved copy of {}", profile.relative_path().display()),
        },
        _ => choice,
    };

    for profile in Profile::ALL {
        let bytes = match choice.forced_profile() {
            Some(forced) => saved.get(&forced),
            None => saved.get(&profile),
        };
        let Some(bytes) = bytes else { continue };
        let file = install_dir.join(profile.relative_path());
        if !fs::read(&file).is_ok_and(|current| &current == bytes) {
            write_atomically(&file, bytes)?;
        }
    }
    match choice.forced_profile() {
        Some(profile) => write_atomically(
            &originals.join(APPLIED_FILE),
            format!("{profile}\n").as_bytes(),
        )?,
        None => match fs::remove_file(originals.join(APPLIED_FILE)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        },
    }
    Ok(choice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(0);

    struct Fixture {
        root: PathBuf,
        install: PathBuf,
        originals: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "zwift-graphics-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            let install = root.join("install");
            let originals = root.join("graphics");
            fs::create_dir_all(install.join("data/configs")).unwrap();
            for profile in Profile::ALL {
                fs::write(install.join(profile.relative_path()), profile.name()).unwrap();
            }
            Self {
                root,
                install,
                originals,
            }
        }

        fn contents(&self) -> [String; 4] {
            Profile::ALL.map(|profile| {
                fs::read_to_string(self.install.join(profile.relative_path())).unwrap()
            })
        }

        fn saved(&self, profile: Profile) -> String {
            fs::read_to_string(self.originals.join(format!("{}.txt", profile.name()))).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn test() -> GpuTest {
        GpuTest {
            renderer: "Test GPU".into(),
            frame_ms: 7.5,
        }
    }

    #[test]
    fn the_frame_time_picks_the_highest_profile_whose_limit_it_meets() {
        for (frame_ms, profile) in [
            (0.5, Profile::Ultra),
            (7.25, Profile::Ultra),
            (7.26, Profile::High),
            (7.84, Profile::High),
            (7.85, Profile::Medium),
            (8.88, Profile::Medium),
            (8.89, Profile::Basic),
            (100.0, Profile::Basic),
        ] {
            assert_eq!(Profile::for_frame_time(frame_ms), profile, "{frame_ms} ms");
        }
    }

    #[test]
    fn parse_and_print_the_setting_words() {
        for (word, setting) in [
            ("auto", Setting::Auto),
            ("basic", Setting::Fixed(Profile::Basic)),
            ("medium", Setting::Fixed(Profile::Medium)),
            ("high", Setting::Fixed(Profile::High)),
            ("ultra", Setting::Fixed(Profile::Ultra)),
            ("default", Setting::GameDefault),
        ] {
            assert_eq!(Setting::parse(word), Ok(setting));
            assert_eq!(setting.to_string(), word);
        }
        for bad in ["", "Ultra", "fast", "ultra high"] {
            let error = Setting::parse(bad).unwrap_err().to_string();
            assert!(error.contains(&format!(": {bad}.")), "{error}");
        }
    }

    #[test]
    fn auto_writes_the_measured_profile_into_all_four_and_saves_the_originals() {
        let fixture = Fixture::new();
        let choice = apply_with(&fixture.install, &fixture.originals, Setting::Auto, || {
            Ok(test())
        })
        .unwrap();
        assert_eq!(choice.to_string(), "high (GPU test 7.50 ms on Test GPU)");
        assert_eq!(fixture.contents(), ["high"; 4]);
        for profile in Profile::ALL {
            assert_eq!(fixture.saved(profile), profile.name());
        }
        assert_eq!(
            fs::read_to_string(fixture.originals.join(APPLIED_FILE)).unwrap(),
            "high\n"
        );
    }

    #[test]
    fn a_failed_test_restores_the_game_files_and_a_second_run_is_idempotent() {
        let fixture = Fixture::new();
        apply_with(&fixture.install, &fixture.originals, Setting::Auto, || {
            Ok(test())
        })
        .unwrap();
        let choice = apply_with(&fixture.install, &fixture.originals, Setting::Auto, || {
            Err("no libEGL".into())
        })
        .unwrap();
        assert_eq!(
            choice.to_string(),
            "game default (GPU test failed: no libEGL)"
        );
        assert_eq!(fixture.contents(), ["basic", "medium", "high", "ultra"]);
        assert!(!fixture.originals.join(APPLIED_FILE).exists());
        // Restoring uses the saved copies, never the files it just wrote.
        for profile in Profile::ALL {
            assert_eq!(fixture.saved(profile), profile.name());
        }
        apply_with(
            &fixture.install,
            &fixture.originals,
            Setting::GameDefault,
            || panic!("measured a fixed setting"),
        )
        .unwrap();
        assert_eq!(fixture.contents(), ["basic", "medium", "high", "ultra"]);
    }

    #[test]
    fn a_fixed_setting_does_not_measure() {
        let fixture = Fixture::new();
        let choice = apply_with(
            &fixture.install,
            &fixture.originals,
            Setting::Fixed(Profile::Medium),
            || panic!("measured a fixed setting"),
        )
        .unwrap();
        assert_eq!(choice, Choice::Fixed(Profile::Medium));
        assert_eq!(fixture.contents(), ["medium"; 4]);
    }

    #[test]
    fn an_update_refreshes_the_saved_original_of_that_file_only() {
        let fixture = Fixture::new();
        apply_with(
            &fixture.install,
            &fixture.originals,
            Setting::Fixed(Profile::High),
            || panic!("measured a fixed setting"),
        )
        .unwrap();
        // A new game version replaces one profile file.
        fs::write(
            fixture.install.join(Profile::Basic.relative_path()),
            "basic 2",
        )
        .unwrap();
        apply_with(
            &fixture.install,
            &fixture.originals,
            Setting::Fixed(Profile::Ultra),
            || panic!("measured a fixed setting"),
        )
        .unwrap();
        assert_eq!(fixture.saved(Profile::Basic), "basic 2");
        for profile in [Profile::Medium, Profile::High, Profile::Ultra] {
            assert_eq!(fixture.saved(profile), profile.name());
        }
        assert_eq!(fixture.contents(), ["ultra"; 4]);
    }

    #[test]
    fn a_missing_profile_file_disables_the_choice_that_needs_it() {
        let fixture = Fixture::new();
        fs::remove_file(fixture.install.join(Profile::Ultra.relative_path())).unwrap();
        let choice = apply_with(
            &fixture.install,
            &fixture.originals,
            Setting::Fixed(Profile::Ultra),
            || panic!("measured a fixed setting"),
        )
        .unwrap();
        assert_eq!(
            choice,
            Choice::GameDefault {
                reason: "no saved copy of data/configs/ultra.txt".into()
            }
        );
        for profile in [Profile::Basic, Profile::Medium, Profile::High] {
            let file = fixture.install.join(profile.relative_path());
            assert_eq!(fs::read_to_string(file).unwrap(), profile.name());
        }
        assert!(!fixture
            .install
            .join(Profile::Ultra.relative_path())
            .exists());
        assert!(!fixture.originals.join(APPLIED_FILE).exists());
    }

    #[test]
    fn a_jar_without_profile_files_is_left_alone() {
        let fixture = Fixture::new();
        for profile in Profile::ALL {
            fs::remove_file(fixture.install.join(profile.relative_path())).unwrap();
        }
        let choice = apply_with(
            &fixture.install,
            &fixture.originals,
            Setting::Fixed(Profile::High),
            || panic!("measured a fixed setting"),
        )
        .unwrap();
        assert_eq!(
            choice,
            Choice::GameDefault {
                reason: "the installed Zwift has no graphics profile files".into()
            }
        );
    }
}
