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
//!   verifies, so at most the pool size (never more than `jobs`) files are staged at
//!   once, not the whole plan. A full first install is several GiB and must not need
//!   twice that.
//!
//! With [`Adaptive`] the pool also grows and shrinks while it runs: it measures the
//! throughput of each window as a *rate* (bytes per second, over the window's real
//! elapsed time rather than its raw byte total), smooths it with an exponential
//! moving average, adds a worker while the rate still improves, and trims the count
//! back once several windows in a row decline. Measuring a rate is what makes this stable:
//! a window that merely lasted longer, or a stalled patch of the plan, can no longer
//! be misread as the link slowing down, so the count settles where the link is
//! saturated instead of being guessed up front or ratcheting down toward one.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::cancel::Cancellation;
use crate::cdn::{Expect, Fetcher};
use crate::logging::Logger;
use crate::manifest::{FileEntry, RelPath};

/// A transfer-progress callback: bytes completed, the total to complete, and the
/// transfer rate in bytes per second when the downloader has measured one. Only the
/// adaptive controller measures a rate, and its workers repeat the last one on their
/// per-file ticks, so the UI can show it beside the bar throughout.
pub type ByteProgress<'a> = &'a (dyn Fn(u64, u64, Option<f64>) + Sync);

/// Where a plan's files come from and go.
pub struct DownloadConfig<'a> {
    /// The manifest's `folder` attribute: the path segment the files live under.
    pub folder: &'a str,
    /// The directory the verified files are placed in.
    pub install_dir: &'a Path,
    /// A scratch directory, ideally on the same filesystem as `install_dir`.
    pub staging_dir: &'a Path,
    /// The ceiling on parallel transfers, and the fixed count when `adaptive` is
    /// `None`. One is always used, however small the value.
    pub jobs: usize,
    /// Grow the number of transfers until the link saturates, instead of running a
    /// fixed pool of `jobs`. `None` is the fixed pool.
    pub adaptive: Option<Adaptive>,
}

/// Parameters for the adaptive transfer count.
///
/// The count starts at `start`, grows by one while a window's throughput improves,
/// and is trimmed back once several windows in a row decline: the additive-increase,
/// multiplicative-decrease response to a saturated link. It never exceeds the
/// [`DownloadConfig::jobs`] ceiling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Adaptive {
    /// Transfers to begin with. Clamped to `1..=jobs`.
    pub start: usize,
    /// How long each throughput sample covers.
    pub window: Duration,
}

/// Transfers to start with: enough to hide one connection's latency without
/// flooding a slow link before the first measurement.
const DEFAULT_START: usize = 4;

impl Default for Adaptive {
    fn default() -> Self {
        Adaptive {
            start: DEFAULT_START,
            window: Duration::from_secs(1),
        }
    }
}

/// Download every file in `files` into `config.install_dir`.
///
/// `progress` is called with bytes completed, the total to complete, and the current
/// transfer rate when one is known, from the worker threads as well as from the
/// calling thread. On the first failure the remaining files are skipped and that
/// error is returned; files already placed are verified content, so a later run only
/// re-fetches what is still missing.
pub fn download(
    fetcher: &dyn Fetcher,
    config: &DownloadConfig<'_>,
    files: &[(RelPath, FileEntry)],
    logger: &Logger,
    progress: ByteProgress<'_>,
) -> Result<usize, String> {
    if files.is_empty() {
        return Ok(0);
    }
    let total: u64 = files
        .iter()
        .map(|(_, entry)| entry.length)
        .fold(0, u64::saturating_add);
    progress(0, total, None);

    // A shared cursor into `files`, so workers pull work without a channel.
    let next = AtomicUsize::new(0);
    let done = AtomicU64::new(0);
    // Network bytes consumed during the current sampling window, summed by the
    // workers through `on_bytes` and drained by the controller each window.
    let bytes = AtomicU64::new(0);
    // The most recent rate the controller measured, as `f64` bits, so a worker's
    // per-file tick can repeat it instead of dropping the display back to a bare
    // percentage between windows. NaN until the first window, meaning "unknown".
    let last_rate = AtomicU64::new(f64::NAN.to_bits());
    let failure: Mutex<Option<String>> = Mutex::new(None);
    let cancellation = logger.cancellation();

    // The pool can never be larger than the plan. A fixed pool runs at the ceiling;
    // an adaptive one starts lower and grows toward it.
    let max = config.jobs.max(1).min(files.len());
    let adapt = match &config.adaptive {
        Some(adaptive) if max > 1 && !adaptive.window.is_zero() => Some(adaptive),
        _ => None,
    };
    let start = adapt.map_or(max, |adaptive| adaptive.start.clamp(1, max));
    let pool = Pool::new(start);
    // The largest transfer count seen, for the closing log line.
    let peak = AtomicUsize::new(start);

    std::thread::scope(|scope| {
        for id in 0..max {
            // Borrow rather than capture the shared state: a `&AtomicU64` and the
            // likes are `Copy`, so the `move` closure only has to own its `id`.
            let next = &next;
            let done = &done;
            let bytes = &bytes;
            let last_rate = &last_rate;
            let failure = &failure;
            let cancellation = &cancellation;
            let pool = &pool;
            scope.spawn(move || {
                run_worker(
                    id,
                    fetcher,
                    config,
                    files,
                    total,
                    next,
                    done,
                    bytes,
                    last_rate,
                    failure,
                    cancellation,
                    pool,
                    progress,
                )
            });
        }
        // Drive the transfer count while the workers run. This runs on the calling
        // thread rather than one of its own, so its exit and the scope's join are the
        // same point and no worker can be left waiting for it.
        if let Some(adaptive) = adapt {
            let window = adaptive.window;
            let mut controller = Controller::new(start, max);
            loop {
                // A cancelled launcher makes this return at once; stop sampling and
                // let the workers notice the cancellation themselves.
                let started = Instant::now();
                if cancellation.wait(window).is_err() || pool.is_closing() {
                    break;
                }
                let elapsed = started.elapsed().as_secs_f64();
                let now = bytes.swap(0, Ordering::Relaxed);
                if elapsed <= 0.0 {
                    continue;
                }
                // Bytes over the window's *real* duration, so an uneven or stalled
                // window is compared on the same footing as any other.
                let rate = now as f64 / elapsed;
                let completed = done.load(Ordering::Relaxed);
                // Remember the rate for the workers' own ticks, and publish it with
                // the bytes so the UI can show the transfer speed.
                last_rate.store(rate.to_bits(), Ordering::Relaxed);
                progress(completed, total, Some(rate));
                let before = controller.desired;
                // Near the end of the plan there is too little left to keep the pool
                // busy, so throughput falls for reasons that say nothing about the
                // link: hold instead of reading the tail as a loss.
                let wanted = if total.saturating_sub(completed) < tail_bytes(rate, elapsed) {
                    before
                } else {
                    controller.observe(rate)
                };
                if wanted != before {
                    logger.debug(&format!(
                        "Download concurrency {before} -> {wanted} ({}) using {} bytes in {:.2}s",
                        crate::stats::rate(rate),
                        now,
                        elapsed
                    ));
                    peak.fetch_max(wanted, Ordering::Relaxed);
                    pool.set_desired(wanted);
                }
            }
        }
        // Release any workers the controller parked, so the queue drains to the end
        // even though growth has stopped. A no-op for a fixed pool.
        pool.set_desired(max);
    });

    if let Some(error) = failure
        .into_inner()
        .expect("download failure lock poisoned")
    {
        return Err(error);
    }
    logger.info(&format!(
        "Downloaded {} files ({} bytes), up to {} parallel transfers",
        files.len(),
        total,
        peak.load(Ordering::Relaxed)
    ));
    Ok(files.len())
}

/// One pool worker: claim the next file and fetch it, until the plan is claimed or
/// something fails.
///
/// A worker with an `id` at or above the pool's current size parks until the
/// controller admits it again; the id is why the pool can shrink without aborting a
/// transfer that is already in flight.
#[allow(clippy::too_many_arguments)]
fn run_worker(
    id: usize,
    fetcher: &dyn Fetcher,
    config: &DownloadConfig<'_>,
    files: &[(RelPath, FileEntry)],
    total: u64,
    next: &AtomicUsize,
    done: &AtomicU64,
    bytes: &AtomicU64,
    last_rate: &AtomicU64,
    failure: &Mutex<Option<String>>,
    cancellation: &Cancellation,
    pool: &Pool,
    progress: ByteProgress<'_>,
) {
    let on_bytes = |delta: u64| {
        bytes.fetch_add(delta, Ordering::Relaxed);
    };
    loop {
        if !pool.wait_turn(id) {
            break;
        }
        // The launcher used to download with a child `wget`, which died with the
        // launcher; the transfer now runs in-process, so check between files
        // instead. A single file is the granularity: a several-hundred-megabyte
        // asset finishes before this is noticed.
        if let Err(error) = cancellation.check() {
            failure
                .lock()
                .expect("download failure lock poisoned")
                .get_or_insert_with(|| error.to_string());
            pool.close();
            break;
        }
        if failure
            .lock()
            .expect("download failure lock poisoned")
            .is_some()
        {
            pool.close();
            break;
        }
        let index = next.fetch_add(1, Ordering::Relaxed);
        let Some((path, entry)) = files.get(index) else {
            // Every index is claimed, so no worker will find work again: let any
            // parked worker exit rather than wait for a turn that never comes.
            pool.close();
            break;
        };
        if let Err(error) = fetch_one(fetcher, config, path, entry, &on_bytes) {
            // Another worker may have failed first; keep that error.
            failure
                .lock()
                .expect("download failure lock poisoned")
                .get_or_insert(error);
            pool.close();
            break;
        }
        let completed = done.fetch_add(entry.length, Ordering::Relaxed) + entry.length;
        // Repeat the controller's last rate, so a file finishing between windows does
        // not momentarily drop the rate from the progress display.
        progress(
            completed,
            total,
            tick_rate(last_rate.load(Ordering::Relaxed)),
        );
    }
}

/// How many transfers the pool is running now, and a way to wait for a turn.
///
/// Workers with an `id` below `desired` run; the rest wait. Lowering `desired`
/// therefore retires the highest workers as they finish, without interrupting a
/// transfer that is already in flight.
struct Pool {
    state: Mutex<PoolState>,
    changed: Condvar,
}

struct PoolState {
    desired: usize,
    /// Set once no further files will be claimed (the plan is claimed, or a
    /// failure), so waiting workers stop instead of parking.
    closing: bool,
}

impl Pool {
    fn new(desired: usize) -> Pool {
        Pool {
            state: Mutex::new(PoolState {
                desired,
                closing: false,
            }),
            changed: Condvar::new(),
        }
    }

    /// Block until worker `id` may run or the pool is closing. Returns `false` when
    /// the pool is closing and the worker must stop.
    fn wait_turn(&self, id: usize) -> bool {
        let mut state = self.state.lock().expect("download pool lock poisoned");
        while id >= state.desired && !state.closing {
            state = self
                .changed
                .wait(state)
                .expect("download pool lock poisoned");
        }
        !state.closing
    }

    fn set_desired(&self, desired: usize) {
        self.state
            .lock()
            .expect("download pool lock poisoned")
            .desired = desired;
        self.changed.notify_all();
    }

    fn close(&self) {
        self.state
            .lock()
            .expect("download pool lock poisoned")
            .closing = true;
        self.changed.notify_all();
    }

    fn is_closing(&self) -> bool {
        self.state
            .lock()
            .expect("download pool lock poisoned")
            .closing
    }
}

/// The smoothed throughput must rise by this fraction of the previous window to
/// add a transfer.
const GROWTH_FRACTION: f64 = 0.05;

/// Decode the rate a worker's per-file tick should report from the bits the
/// controller last stored. `None` until the first window has measured a rate, so a
/// file that finishes before then does not invent one.
fn tick_rate(bits: u64) -> Option<f64> {
    let rate = f64::from_bits(bits);
    rate.is_finite().then_some(rate)
}

/// The smoothed throughput must fall by this fraction to count as a decline.
const DECLINE_FRACTION: f64 = 0.10;

/// Consecutive declining windows before the transfer count is trimmed back.
const DECLINE_RUN: u32 = 4;

/// How much of the pool survives a cutback, as a fraction (`DECLINE_KEEP / 4`): a
/// quarter given up rather than half, so one bad patch costs far less additive
/// growth to recover from.
const DECLINE_KEEP: usize = 3;

/// The weight the newest window carries in the smoothed throughput.
const SMOOTHING: f64 = 0.4;

/// The adaptive transfer-count controller: additive increase, multiplicative
/// decrease, but driven by a smoothed *rate* and only after a run of declines.
///
/// Measuring a rate (bytes per second, over each window's real elapsed time) instead
/// of the window's raw byte total removes the old rule's main failure: a window that
/// merely lasted longer, or one the plan stalled through, no longer looks like a
/// loss. Smoothing the rate and requiring several windows in a row to decline removes
/// the other: a single noisy dip no longer shrinks the pool, so the count cannot
/// ratchet itself down toward one.
struct Controller {
    /// The transfer count to run for the next window.
    desired: usize,
    /// The ceiling: the plan's file count, or the configured `jobs`.
    max: usize,
    /// The smoothed throughput in bytes per second, or `None` until the first window.
    smoothed: Option<f64>,
    /// Windows in a row whose smoothed throughput fell by at least [`DECLINE_FRACTION`].
    declines: u32,
    /// Set for the single window after a change, to absorb the slow start of the
    /// connections that change opened.
    settle: bool,
}

impl Controller {
    /// A controller starting at `start` transfers, never above `max`.
    fn new(start: usize, max: usize) -> Controller {
        let max = max.max(1);
        Controller {
            desired: start.clamp(1, max),
            max,
            smoothed: None,
            declines: 0,
            settle: false,
        }
    }

    /// Fold one window's `rate` (bytes per second) into the average and return the
    /// transfer count to run for the next window.
    fn observe(&mut self, rate: f64) -> usize {
        let smoothed = match self.smoothed {
            Some(previous) => SMOOTHING * rate + (1.0 - SMOOTHING) * previous,
            None => rate,
        };
        let previous = self.smoothed.replace(smoothed);

        // The window right after a change is mostly its new connections' slow start,
        // not the link's steady state, so it only feeds the average.
        if self.settle {
            self.settle = false;
            return self.desired;
        }

        let Some(previous) = previous else {
            // Nothing to compare the first window against; if bytes moved, spend the
            // next window trying one more transfer.
            return if smoothed > 0.0 {
                self.change(self.desired + 1)
            } else {
                self.desired
            };
        };
        if previous <= 0.0 {
            return self.desired;
        }

        let change = smoothed / previous - 1.0;
        if change >= GROWTH_FRACTION {
            // Still buying throughput: add one transfer and clear any decline run.
            self.declines = 0;
            self.change(self.desired + 1)
        } else if change <= -DECLINE_FRACTION {
            // Worse, but only give transfers up once the fall is sustained.
            self.declines += 1;
            if self.declines >= DECLINE_RUN {
                self.declines = 0;
                self.change((self.desired * DECLINE_KEEP / 4).max(1))
            } else {
                self.desired
            }
        } else {
            // Flat: the link is saturated, or the wobble is inside the noise band.
            self.declines = 0;
            self.desired
        }
    }

    /// Adopt `desired` (clamped to `1..=max`), arming the settle window only when it
    /// actually moves.
    fn change(&mut self, desired: usize) -> usize {
        let desired = desired.clamp(1, self.max);
        if desired != self.desired {
            self.desired = desired;
            self.settle = true;
        }
        self.desired
    }
}

/// The byte count below which a plan's tail distorts throughput: about two windows'
/// worth at the current rate. Too little is left to keep the pool busy, so the
/// controller holds rather than reading the shrinking remainder as the link slowing.
fn tail_bytes(rate: f64, window_secs: f64) -> u64 {
    (rate * window_secs * 2.0).max(0.0) as u64
}

/// Fetch one file into staging, stamp its modification time, and move it into place.
fn fetch_one(
    fetcher: &dyn Fetcher,
    config: &DownloadConfig<'_>,
    path: &RelPath,
    entry: &FileEntry,
    on_bytes: &(dyn Fn(u64) + Sync),
) -> Result<(), String> {
    let staged = config.staging_dir.join(path.as_str());
    let url = fetcher.file_url(config.folder, path);
    let expect = Expect {
        length: entry.length,
        checksum: entry.checksum,
    };
    fetcher.fetch_file_with_progress(&url, &staged, expect, on_bytes)?;
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

        fn fetch_file_with_progress(
            &self,
            url: &str,
            dest: &Path,
            expect: Expect,
            on_bytes: &(dyn Fn(u64) + Sync),
        ) -> Result<(), String> {
            // Report the bytes up front, so the adaptive controller has something to
            // sample even though the fake is instantaneous.
            let key = url.splitn(3, '/').nth(2).unwrap_or_default();
            if let Some(bytes) = self.files.get(key) {
                on_bytes(bytes.len() as u64);
            }
            self.fetch_file(url, dest, expect)
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
            adaptive: None,
        };
        let logger = &test_logger(&install);
        let progress = |_, _, _| {};
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
            adaptive: None,
        };
        let logger = &test_logger(&install);
        let progress = |_, _, _| {};
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
            adaptive: None,
        };
        let logger = &test_logger(&install);
        let progress = |_, _, _| {};
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
            adaptive: None,
        };
        let logger = &test_logger(&install);
        let progress = |_, _, _| {};
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
            adaptive: None,
        };
        let logger = &test_logger(&install);
        let progress = |_, _, _| {};
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
            adaptive: None,
        };
        let logger = &test_logger(&install);
        let seen = Mutex::new(Vec::new());
        let progress = |done: u64, total: u64, _rate: Option<f64>| {
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
            adaptive: None,
        };
        let logger = test_logger(&install);
        let fake =
            Fake::new(&[("F/a", a), ("F/b", b)]).with_cancellation(logger.cancellation().clone());
        let progress = |_, _, _| {};
        let error = download(&fake, &config, &plan, &logger, &progress).unwrap_err();
        // The launcher used to download with a child `wget` that died with it; the
        // in-process transfer has to notice the same cancellation itself.
        assert!(error.contains("Launcher cancelled"), "{error}");
        assert_eq!(fake.fetches.load(Ordering::Relaxed), 1);
        // The file that did arrive is verified, so it stays; the rest is refetched.
        assert_eq!(fs::read(install.join("a")).unwrap(), a);
        assert!(!install.join("b").exists());
    }

    #[test]
    fn the_controller_grows_while_throughput_improves() {
        let mut controller = Controller::new(2, 8);
        // Doubling the rate every window keeps driving the count up, one transfer at
        // a time (a window is absorbed after each change, see the settle test below),
        // until it reaches the ceiling and stays there.
        let mut rate = 100.0;
        for _ in 0..24 {
            controller.observe(rate);
            rate *= 2.0;
        }
        assert_eq!(controller.desired, 8);
    }

    #[test]
    fn the_controller_holds_a_flat_rate() {
        let mut controller = Controller::new(4, 16);
        // The first window with traffic spends one more transfer; the next is the
        // settle window for that change.
        controller.observe(1_000.0);
        controller.observe(1_000.0);
        let settled = controller.desired;
        // Flat throughput is saturated: it must not keep growing, nor shrink.
        for _ in 0..10 {
            controller.observe(1_000.0);
        }
        assert_eq!(controller.desired, settled);
    }

    #[test]
    fn the_controller_trims_only_after_a_sustained_decline() {
        let mut controller = Controller::new(8, 16);
        controller.observe(1_000.0);
        controller.observe(1_000.0);
        let steady = controller.desired;
        assert!(steady > 8);

        // A single slow window is smoothed away and must not shrink the pool.
        controller.observe(10.0);
        assert_eq!(controller.desired, steady);

        // Only a sustained run of declines is a real loss of throughput, and even
        // then the cutback is a quarter, not a half.
        controller.observe(10.0);
        controller.observe(10.0);
        controller.observe(10.0);
        assert_eq!(controller.desired, (steady * DECLINE_KEEP / 4).max(1));
    }

    #[test]
    fn the_controller_never_trims_below_one() {
        let mut controller = Controller::new(2, 16);
        // Run the count up on a healthy window, then pull the rate down in a long,
        // steep slide. Repeated cutbacks reach the floor of one transfer and stop
        // there rather than falling to zero.
        controller.observe(1_000_000.0);
        for step in 0..50 {
            controller.observe(1_000_000.0 / (step as f64 + 2.0));
        }
        assert_eq!(controller.desired, 1);
    }

    #[test]
    fn worker_ticks_repeat_the_last_measured_rate() {
        // Until the controller has measured a window there is no rate to repeat.
        assert_eq!(tick_rate(f64::NAN.to_bits()), None);
        // Afterwards, a worker's tick carries the same rate the controller last did.
        assert_eq!(tick_rate(8_400_000.0f64.to_bits()), Some(8_400_000.0));
    }

    #[test]
    fn the_tail_guard_scales_with_the_rate() {
        // About two windows' worth of bytes: at 1 MB/s over a one-second window, try
        // to leave 2 MB, below which the controller stops adapting.
        assert_eq!(tail_bytes(1_000_000.0, 1.0), 2_000_000);
        assert_eq!(tail_bytes(0.0, 1.0), 0);
    }

    #[test]
    fn an_adaptive_run_finishes_every_file() {
        let (install, staging) = scratch("adaptive");
        // Enough files that the pool has work across several sampling windows.
        let payload: Vec<(String, Vec<u8>)> = (0..16)
            .map(|i| (format!("F/f{i}"), format!("file number {i}").into_bytes()))
            .collect();
        let files: Vec<(&str, &[u8])> = payload
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
            .collect();
        let fake = Fake::new(&files);
        let plan: Vec<(RelPath, FileEntry)> = payload
            .iter()
            .map(|(path, bytes)| {
                (
                    RelPath::parse(path.trim_start_matches("F/")).unwrap(),
                    entry(bytes),
                )
            })
            .collect();
        let config = DownloadConfig {
            folder: "F",
            install_dir: &install,
            staging_dir: &staging,
            jobs: 8,
            adaptive: Some(Adaptive {
                start: 2,
                window: Duration::from_millis(1),
            }),
        };
        let logger = &test_logger(&install);
        let progress = |_, _, _| {};
        let count = download(&fake, &config, &plan, logger, &progress).unwrap();
        assert_eq!(count, 16);
        for (path, bytes) in &payload {
            let name = path.trim_start_matches("F/");
            assert_eq!(fs::read(install.join(name)).unwrap(), *bytes);
        }
    }
}
