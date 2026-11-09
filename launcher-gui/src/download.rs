//! Parallel, verified file transfer.
//!
//! The download half of [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap)
//! (`src/sync.rs`, MIT OR Apache-2.0), simplified: a fixed pool of worker threads pulls
//! files off a shared queue, and each file is fetched into a staging path and only then
//! moved into place.
//!
//! Two properties matter:
//!
//! * **No partial file in the install directory.** A file is written under
//!   `staging_dir` and renamed into place only after [`Fetcher::fetch_file`] verified
//!   its length and checksum, so an interrupted run never leaves a truncated game file
//!   that a later diff would consider present.
//! * **Bounded extra disk.** Each worker moves its file out of staging as soon as it
//!   verifies, so at most `jobs` files are staged at once, not the whole plan. A full
//!   first install is several GiB and must not need twice that.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::cdn::{Expect, Fetcher};
use crate::logging::Logger;
use crate::manifest::{FileEntry, RelPath};

/// Where a plan's files come from and go.
pub struct DownloadConfig<'a> {
    /// The manifest's `folder` attribute: the path segment the files live under.
    pub folder: &'a str,
    /// The directory the verified files are placed in.
    pub install_dir: &'a Path,
    /// A scratch directory, ideally on the same filesystem as `install_dir`.
    pub staging_dir: &'a Path,
    /// Worker threads. One is always used, however small the value.
    pub jobs: usize,
}

/// Download every file in `files` into `config.install_dir`.
///
/// `progress` is called with bytes completed and the total to complete, from the
/// worker threads as well as from the calling thread. On the first failure the
/// remaining files are skipped and that error is returned; files already placed are
/// verified content, so a later run only re-fetches what is still missing.
pub fn download(
    fetcher: &dyn Fetcher,
    config: &DownloadConfig<'_>,
    files: &[(RelPath, FileEntry)],
    logger: &Logger,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<usize, String> {
    if files.is_empty() {
        return Ok(0);
    }
    let total: u64 = files
        .iter()
        .map(|(_, entry)| entry.length)
        .fold(0, u64::saturating_add);
    progress(0, total);

    // A shared cursor into `files`, so workers pull work without a channel.
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let workers = config.jobs.max(1).min(files.len());
    let cancellation = logger.cancellation();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    // The launcher used to download with a child `wget`, which died
                    // with the launcher; the transfer now runs in-process, so check
                    // between files instead. A single file is the granularity: a
                    // several-hundred-megabyte asset finishes before this is noticed.
                    if let Err(error) = cancellation.check() {
                        failure
                            .lock()
                            .expect("download failure lock poisoned")
                            .get_or_insert_with(|| error.to_string());
                        break;
                    }
                    if failure
                        .lock()
                        .expect("download failure lock poisoned")
                        .is_some()
                    {
                        break;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some((path, entry)) = files.get(index) else {
                        break;
                    };
                    if let Err(error) = fetch_one(fetcher, config, path, entry) {
                        let mut failure = failure.lock().expect("download failure lock poisoned");
                        // Another worker may have failed first; keep that error.
                        failure.get_or_insert(error);
                        break;
                    }
                    let completed = done.fetch_add(entry.length, Ordering::Relaxed) + entry.length;
                    progress(completed, total);
                }
            });
        }
    });

    if let Some(error) = failure
        .into_inner()
        .expect("download failure lock poisoned")
    {
        return Err(error);
    }
    logger.info(&format!(
        "Downloaded {} files ({} bytes) as {} parallel transfers",
        files.len(),
        total,
        workers
    ));
    Ok(files.len())
}

/// Fetch one file into staging, stamp its modification time, and move it into place.
fn fetch_one(
    fetcher: &dyn Fetcher,
    config: &DownloadConfig<'_>,
    path: &RelPath,
    entry: &FileEntry,
) -> Result<(), String> {
    let staged = config.staging_dir.join(path.as_str());
    let url = fetcher.file_url(config.folder, path);
    let expect = Expect {
        length: entry.length,
        checksum: entry.checksum,
    };
    fetcher.fetch_file(&url, &staged, expect)?;
    // The manifest's timestamp is the game's own bookkeeping: set it before the file
    // is visible under its real name, so nothing ever observes the wrong mtime.
    if let Some(modified) = entry.modified {
        set_modified(&staged, modified);
    }
    place(&staged, &config.install_dir.join(path.as_str()))
}

/// Set a file's modification time, ignoring platforms or filesystems that refuse.
fn set_modified(path: &Path, modified: SystemTime) {
    if let Ok(file) = fs::File::open(path) {
        let _ = file.set_times(fs::FileTimes::new().set_modified(modified));
    }
}

/// Move a verified file from staging into the install directory.
///
/// A rename is atomic and is the normal path. It fails across filesystems, which the
/// configuration avoids but cannot guarantee, so a copy is the fallback.
fn place(staged: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    match fs::rename(staged, dest) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(staged, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
            let _ = fs::remove_file(staged);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Manifest, VerCur};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicUsize;

    /// A CDN that serves a fixed map of path -> bytes, counts fetches, and can be
    /// told to fail one path or to cancel the launcher mid-transfer.
    struct Fake {
        files: BTreeMap<String, Vec<u8>>,
        fail: Option<String>,
        cancel_after_first: bool,
        cancellation: crate::cancel::Cancellation,
        fetches: AtomicUsize,
    }

    impl Fake {
        fn new(files: &[(&str, &[u8])]) -> Fake {
            Fake {
                files: files
                    .iter()
                    .map(|(path, bytes)| (path.to_string(), bytes.to_vec()))
                    .collect(),
                fail: None,
                cancel_after_first: false,
                cancellation: crate::cancel::Cancellation::default(),
                fetches: AtomicUsize::new(0),
            }
        }

        fn failing(mut self, path: &str) -> Fake {
            self.fail = Some(path.to_string());
            self
        }

        /// Cancel the launcher from the first fetch, exactly as a SIGTERM would.
        fn with_cancellation(mut self, cancellation: crate::cancel::Cancellation) -> Fake {
            self.cancel_after_first = true;
            self.cancellation = cancellation;
            self
        }
    }

    impl Fetcher for Fake {
        fn fetch_ver_cur(&self) -> Result<VerCur, String> {
            Err("not used".into())
        }

        fn fetch_manifest(
            &self,
            _name: &str,
            _checksum: Option<u32>,
        ) -> Result<(Manifest, Box<[u8]>), String> {
            Err("not used".into())
        }

        fn file_url(&self, folder: &str, path: &RelPath) -> String {
            format!("fake://{folder}/{}", path.as_str())
        }

        fn fetch_file(&self, url: &str, dest: &Path, expect: Expect) -> Result<(), String> {
            let fetched = self.fetches.fetch_add(1, Ordering::Relaxed);
            if self.cancel_after_first && fetched == 0 {
                self.cancellation.cancel();
            }
            // Strip the `fake://` scheme; the rest is the key `file_url` built.
            let key = url.splitn(3, '/').nth(2).unwrap_or_default().to_string();
            if self.fail.as_deref() == Some(key.as_str()) {
                return Err(format!("fake failure for {key}"));
            }
            let bytes = self
                .files
                .get(&key)
                .ok_or_else(|| format!("fake 404 for {key}"))?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(dest, bytes).unwrap();
            if !expect.matches(bytes.len() as u64, crc32fast::hash(bytes)) {
                // Like the real fetcher, never leave a rejected file behind.
                let _ = fs::remove_file(dest);
                return Err(format!("fake corruption for {key}"));
            }
            Ok(())
        }
    }

    fn scratch(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "zwift-download-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&root);
        let install = root.join("install");
        let staging = root.join("staging");
        fs::create_dir_all(&install).unwrap();
        fs::create_dir_all(&staging).unwrap();
        (install, staging)
    }

    /// A logger writing next to (not inside) the scratch install directory.
    fn test_logger(install: &Path) -> Logger {
        Logger::new(install.with_file_name("test.log"), 0, false)
    }

    fn entry(bytes: &[u8]) -> FileEntry {
        FileEntry {
            length: bytes.len() as u64,
            checksum: Some(crc32fast::hash(bytes)),
            modified: Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)),
        }
    }

    #[test]
    fn downloads_and_places_every_file() {
        let (install, staging) = scratch("happy");
        let fake = Fake::new(&[("F/a.bin", b"alpha"), ("F/b.bin", b"bravo bravo")]);
        let plan: Vec<(RelPath, FileEntry)> =
            [(r"a.bin", b"alpha" as &[u8]), (r"b.bin", b"bravo bravo")]
                .into_iter()
                .map(|(path, bytes)| (RelPath::parse(path).unwrap(), entry(bytes)))
                .collect();
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            jobs: 4,
        };
        let logger = &test_logger(&install);
        let progress = |_, _| {};
        let count = download(&fake, &config, &plan, logger, &progress).unwrap();
        assert_eq!(count, 2);
        assert_eq!(fs::read(install.join("a.bin")).unwrap(), b"alpha");
        assert_eq!(fs::read(install.join("b.bin")).unwrap(), b"bravo bravo");
        // Staging is drained as files are placed, so a large plan does not need
        // room for it twice over.
        assert_eq!(fs::read_dir(&staging).unwrap().count(), 0);
    }

    #[test]
    fn creates_nested_directories_and_keeps_the_manifest_mtime() {
        let (install, staging) = scratch("nested");
        let bytes = b"world";
        let fake = Fake::new(&[("F/data/Worlds/w.wad", bytes)]);
        let plan = vec![(RelPath::parse(r"data\Worlds\w.wad").unwrap(), entry(bytes))];
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            jobs: 2,
        };
        let logger = &test_logger(&install);
        let progress = |_, _| {};
        download(&fake, &config, &plan, logger, &progress).unwrap();
        let placed = install.join("data/Worlds/w.wad");
        assert_eq!(fs::read(&placed).unwrap(), bytes);
        let modified = fs::metadata(&placed).unwrap().modified().unwrap();
        assert_eq!(
            modified,
            SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)
        );
    }

    #[test]
    fn a_verification_failure_leaves_no_file_behind() {
        let (install, staging) = scratch("bad");
        let bytes = b"payload";
        // A plan that expects a different length than the fake serves: the fetch is
        // "verified" by the fake's own check, but the manifest disagrees, which is
        // exactly the case `place` must never make visible.
        let fake = Fake::new(&[("F/x.bin", bytes)]);
        let mut want = entry(bytes);
        want.length += 1;
        let plan = vec![(RelPath::parse("x.bin").unwrap(), want)];
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            jobs: 1,
        };
        let logger = &test_logger(&install);
        let progress = |_, _| {};
        let error = download(&fake, &config, &plan, logger, &progress).unwrap_err();
        assert!(error.contains("corruption"), "{error}");
        assert!(!install.join("x.bin").exists());
        assert!(!staging.join("x.bin").exists());
    }

    #[test]
    fn a_fetch_error_stops_the_pool_and_is_reported() {
        let (install, staging) = scratch("fail");
        let bytes = b"payload";
        let fake = Fake::new(&[("F/x.bin", bytes)]).failing("F/x.bin");
        let plan = vec![(RelPath::parse("x.bin").unwrap(), entry(bytes))];
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            jobs: 3,
        };
        let logger = &test_logger(&install);
        let progress = |_, _| {};
        let error = download(&fake, &config, &plan, logger, &progress).unwrap_err();
        assert!(error.contains("fake failure"), "{error}");
        assert!(!install.join("x.bin").exists());
    }

    #[test]
    fn an_empty_plan_is_a_no_op() {
        let (install, staging) = scratch("empty");
        let fake = Fake::new(&[]);
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            jobs: 8,
        };
        let logger = &test_logger(&install);
        let progress = |_, _| {};
        assert_eq!(download(&fake, &config, &[], logger, &progress).unwrap(), 0);
        assert_eq!(fake.fetches.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn progress_reaches_the_final_total() {
        let (install, staging) = scratch("progress");
        let a: &[u8] = b"aaa";
        let b: &[u8] = b"bbbb";
        let fake = Fake::new(&[("F/a", a), ("F/b", b)]);
        let plan = vec![
            (RelPath::parse("a").unwrap(), entry(a)),
            (RelPath::parse("b").unwrap(), entry(b)),
        ];
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            jobs: 2,
        };
        let logger = &test_logger(&install);
        let seen = Mutex::new(Vec::new());
        let progress = |done: u64, total: u64| {
            seen.lock().unwrap().push((done, total));
        };
        download(&fake, &config, &plan, logger, &progress).unwrap();
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.first(), Some(&(0, 7)));
        assert_eq!(seen.last(), Some(&(7, 7)));
        assert!(seen.iter().all(|(_, total)| *total == 7));
    }

    #[test]
    fn a_cancelled_launcher_stops_the_pool_between_files() {
        let (install, staging) = scratch("cancel");
        let a: &[u8] = b"aaa";
        let b: &[u8] = b"bbb";
        let plan = vec![
            (RelPath::parse("a").unwrap(), entry(a)),
            (RelPath::parse("b").unwrap(), entry(b)),
        ];
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            // One worker, so the order of the two files is fixed.
            jobs: 1,
        };
        let logger = test_logger(&install);
        let fake =
            Fake::new(&[("F/a", a), ("F/b", b)]).with_cancellation(logger.cancellation().clone());
        let progress = |_, _| {};
        let error = download(&fake, &config, &plan, &logger, &progress).unwrap_err();
        // The launcher used to download with a child `wget` that died with it; the
        // in-process transfer has to notice the same cancellation itself.
        assert!(error.contains("Launcher cancelled"), "{error}");
        assert_eq!(fake.fetches.load(Ordering::Relaxed), 1);
        // The file that did arrive is verified, so it stays; the rest is refetched.
        assert_eq!(fs::read(install.join("a")).unwrap(), a);
        assert!(!install.join("b").exists());
    }
}
