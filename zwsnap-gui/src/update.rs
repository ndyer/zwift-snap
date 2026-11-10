//! The update check: bring an installed game up to the version the CDN serves.
//!
//! Nothing here runs Zwift's own launcher or installer. The version the CDN
//! advertises is compared with the version files in the install directory, and the
//! difference is fetched directly (see [`crate::sync`]), which is what
//! [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap) does and what
//! removes the installer, the patcher and the polling loop that used to sit here.

use crate::cdn::{Fetcher, Remote};
use crate::logging::Logger;
use crate::manifest::VerCur;
use crate::sync::{self, Outcome, SyncConfig};
use crate::version::Version;
use std::path::Path;

/// The percentage reported while the served version file is read.
const PERCENT_CHECK: u8 = 5;
/// The percentage reported once the transfer starts.
const PERCENT_DOWNLOAD_START: u8 = 10;
/// The percentage span the transfer fills, so a large download ends near 90 %.
const PERCENT_DOWNLOAD_SPAN: u8 = 80;

/// The message the launcher shows while it reads the served version file.
pub const CHECK_MESSAGE: &str = "Checking for Zwift updates";

/// The message the launcher shows while it transfers the game.
pub const DOWNLOAD_MESSAGE: &str = "Downloading Zwift";

/// What an update check found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateOutcome {
    /// The installation already carries the served version.
    UpToDate(Version),
    /// The installation was brought up to the served version.
    Updated(Version),
    /// The CDN could not be reached. What is installed can still be started, so this
    /// is reported but is not a failure.
    CheckUnavailable,
}

/// Where an update reads from and writes to.
pub struct UpdateConfig<'a> {
    /// The directory holding `Zwift_ver_cur.xml` and the game files.
    pub install_dir: &'a Path,
    /// Scratch space for in-flight transfers, on `install_dir`'s filesystem.
    pub staging_dir: &'a Path,
    /// Parallel transfers.
    pub jobs: usize,
    /// The CDN root, or a `file://` directory in the tests.
    pub cdn_base: &'a str,
}

/// Bring an installed game up to the version the CDN serves.
///
/// Only the version file lookup is allowed to fail softly: an unreachable CDN yields
/// [`UpdateOutcome::CheckUnavailable`] and the launch continues with what is
/// installed. A transfer that fails midway is an error, because the installation is
/// then a mix of two versions; the sync writes the version files last, so a later run
/// diffs again and finishes the job.
pub fn update_if_needed(
    config: &UpdateConfig<'_>,
    logger: &Logger,
    progress: &(dyn Fn(u8, &str) + Sync),
) -> Result<UpdateOutcome, String> {
    progress(PERCENT_CHECK, CHECK_MESSAGE);
    let fetcher = Remote::new(config.cdn_base);
    let Some(ver_cur) = check(&fetcher, logger)? else {
        return Ok(UpdateOutcome::CheckUnavailable);
    };
    let sync_config = SyncConfig {
        install_dir: config.install_dir,
        staging_dir: config.staging_dir,
        jobs: config.jobs,
    };
    let reporting = |done: u64, total: u64| {
        progress(
            PERCENT_DOWNLOAD_START + percent_of(done, total, PERCENT_DOWNLOAD_SPAN),
            DOWNLOAD_MESSAGE,
        );
    };
    match sync::sync_version(&fetcher, &ver_cur, &sync_config, logger, &reporting) {
        Ok(Outcome::Current { version }) => Ok(UpdateOutcome::UpToDate(version)),
        Ok(Outcome::Updated { version, .. }) => Ok(UpdateOutcome::Updated(version)),
        Err(error) => Err(error),
    }
}

/// Read the served version file. A transport failure is a warning, not an error.
fn check(fetcher: &dyn Fetcher, logger: &Logger) -> Result<Option<VerCur>, String> {
    match fetcher.fetch_ver_cur() {
        Ok(ver_cur) => Ok(Some(ver_cur)),
        Err(error) => {
            logger.warning(&format!(
                "Unable to check for Zwift updates ({error}); starting the installed version"
            ));
            Ok(None)
        }
    }
}

/// The share of `span` that `done` of `total` bytes represents.
fn percent_of(done: u64, total: u64, span: u8) -> u8 {
    if total == 0 {
        return 0;
    }
    let percent = u128::from(done) * u128::from(span) / u128::from(total);
    percent.min(u128::from(span)) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{local_manifest, local_version};
    use crate::test_support::{write_cdn, SERVER_VERSION};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    /// A `file://` CDN plus the directories an update writes into.
    struct Fixture {
        root: PathBuf,
        install: PathBuf,
        staging: PathBuf,
        logger: Logger,
        progress: Arc<Mutex<Vec<(u8, String)>>>,
    }

    fn fixture(name: &str, files: &[(&str, &str)]) -> Fixture {
        let root = std::env::temp_dir().join(format!("zwift-update-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("cdn")).unwrap();
        write_cdn(&root.join("cdn"), SERVER_VERSION, files);
        let install = root.join("install");
        let staging = root.join("staging");
        fs::create_dir_all(&install).unwrap();
        let logger = Logger::new(root.join("zwift.log"), 0, false);
        logger.init().unwrap();
        Fixture {
            root,
            install,
            staging,
            logger,
            progress: Arc::new(Mutex::new(Vec::new())),
        }
    }

    impl Fixture {
        fn cdn_base(&self) -> String {
            format!("file://{}", self.root.join("cdn").display())
        }

        fn run(&self) -> Result<UpdateOutcome, String> {
            let recorded = Arc::clone(&self.progress);
            update_if_needed(
                &UpdateConfig {
                    install_dir: &self.install,
                    staging_dir: &self.staging,
                    jobs: 2,
                    cdn_base: &self.cdn_base(),
                },
                &self.logger,
                &move |percent, message| {
                    recorded
                        .lock()
                        .unwrap()
                        .push((percent, message.to_string()));
                },
            )
        }

        fn log(&self) -> String {
            fs::read_to_string(self.root.join("zwift.log")).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn server_version() -> Version {
        Version::parse(SERVER_VERSION).unwrap()
    }

    #[test]
    fn an_outdated_install_is_synced_to_the_served_version() {
        let fixture = fixture(
            "outdated",
            &[("ZwiftApp.exe", "game"), ("data/a.txt", "asset")],
        );
        assert_eq!(
            fixture.run().unwrap(),
            UpdateOutcome::Updated(server_version())
        );
        assert_eq!(
            fs::read_to_string(fixture.install.join("data/a.txt")).unwrap(),
            "asset"
        );
        assert_eq!(local_version(&fixture.install), server_version());
        assert_eq!(local_manifest(&fixture.install).files().len(), 2);
        // Staging is emptied as each verified file is placed in the install directory.
        assert!(!fixture.staging.join("data/a.txt").exists());
    }

    #[test]
    fn a_synced_install_is_up_to_date_and_transfers_nothing() {
        let fixture = fixture("current", &[("ZwiftApp.exe", "game")]);
        assert!(matches!(fixture.run().unwrap(), UpdateOutcome::Updated(_)));
        let file = fixture.install.join("ZwiftApp.exe");
        fs::write(&file, "changed by hand").unwrap();
        let version = fs::read(fixture.install.join("Zwift_ver_cur.xml")).unwrap();
        fixture.progress.lock().unwrap().clear();

        let outcome = fixture.run().unwrap();

        // The version file matches, so no manifest is read and nothing is fetched.
        assert_eq!(outcome, UpdateOutcome::UpToDate(server_version()));
        assert_eq!(
            fs::read_to_string(&file).unwrap(),
            "changed by hand",
            "an up-to-date install must not be rewritten"
        );
        assert_eq!(
            fs::read(fixture.install.join("Zwift_ver_cur.xml")).unwrap(),
            version
        );
        assert!(fixture
            .progress
            .lock()
            .unwrap()
            .iter()
            .all(|(_, message)| message != DOWNLOAD_MESSAGE));
    }

    #[test]
    fn an_unreachable_cdn_keeps_the_installed_game_and_warns() {
        let fixture = fixture("offline", &[("ZwiftApp.exe", "game")]);
        fs::remove_file(fixture.root.join("cdn/Zwift_ver_cur.xml")).unwrap();
        assert_eq!(fixture.run().unwrap(), UpdateOutcome::CheckUnavailable);
        assert!(!fixture.install.join("ZwiftApp.exe").exists());
        let log = fixture.log();
        assert!(log.contains("Unable to check for Zwift updates"), "{log}");
        assert!(log.contains("starting the installed version"), "{log}");
    }

    #[test]
    fn download_failures_are_fatal_and_leave_the_version_files_alone() {
        let fixture = fixture(
            "broken",
            &[("ZwiftApp.exe", "game"), ("data/a.txt", "asset")],
        );
        fs::remove_file(fixture.root.join("cdn/v1/data/a.txt")).unwrap();
        let error = fixture.run().unwrap_err();
        assert!(error.contains("data"), "{error}");
        // The version files are written last, so a failed run leaves the install
        // looking outdated and the next run fetches what is missing.
        assert!(!fixture.install.join("Zwift_ver_cur.xml").exists());
        assert_eq!(local_version(&fixture.install), Version::ZERO);
    }

    #[test]
    fn progress_is_monotonic_and_never_exceeds_the_download_span() {
        let fixture = fixture(
            "progress",
            &[
                ("ZwiftApp.exe", "game"),
                ("data/a.txt", "asset"),
                ("data/b.txt", "more"),
            ],
        );
        assert!(matches!(fixture.run().unwrap(), UpdateOutcome::Updated(_)));
        let progress = fixture.progress.lock().unwrap().clone();
        assert_eq!(progress.first().unwrap().1, CHECK_MESSAGE);
        let percentages: Vec<u8> = progress.iter().map(|(percent, _)| *percent).collect();
        assert!(
            percentages.windows(2).all(|pair| pair[0] <= pair[1]),
            "{percentages:?}"
        );
        assert!(
            percentages.iter().all(|percent| *percent <= 90),
            "{percentages:?}"
        );
    }

    #[test]
    fn a_zero_length_plan_reports_no_progress_but_never_panics() {
        assert_eq!(percent_of(0, 0, PERCENT_DOWNLOAD_SPAN), 0);
        assert_eq!(
            percent_of(5, 5, PERCENT_DOWNLOAD_SPAN),
            PERCENT_DOWNLOAD_SPAN
        );
        assert_eq!(
            percent_of(50, 10, PERCENT_DOWNLOAD_SPAN),
            PERCENT_DOWNLOAD_SPAN
        );
    }
}
