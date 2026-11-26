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
//! With [`Adaptive`] the pool also grows and shrinks while it runs. Growth is a
//! probe: it adds one transfer and keeps it only if the mean rate over the windows
//! after the probe rose by at least [`MARGINAL_GAIN`] over the mean before it — a
//! search for the point where the link saturates, rather than a guess up front. It
//! shrinks on things a saturated link actually does: a probe that bought too little,
//! a spike in per-file time-to-first-byte, or the transport reporting pressure (HTTP
//! 429, 503, a reset connection). Judging each probe on a *mean* rate, not one window,
//! is what keeps this stable: the plan's file-size mix no longer looks like a change
//! in the link, and a quiet patch cannot ratchet the count down toward one.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crate::cancel::Cancellation;
use crate::cdn::{Expect, Fetcher, Pressure, TransferObserver};
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
/// The count starts at `start` and probes upward one transfer at a time, keeping a
/// transfer only while it buys throughput; it is trimmed back sooner, and harder, on
/// a probe that bought too little, a latency spike, or transport pressure. It never
/// exceeds the [`DownloadConfig::jobs`] ceiling.
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
    // The current sampling window's signals — bytes, per-file time-to-first-byte and
    // transport pressure — summed by the workers through the observer and drained by
    // the controller each window.
    let counters = WindowCounters::new();
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
            let counters = &counters;
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
                    counters,
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
                if elapsed <= 0.0 {
                    continue;
                }
                // Bytes over the window's *real* duration, so an uneven or stalled
                // window is compared on the same footing as any other.
                let sample = counters.drain(elapsed);
                let rate = sample.rate;
                let completed = done.load(Ordering::Relaxed);
                // Remember the rate for the workers' own ticks, and publish it with
                // the bytes so the UI can show the transfer speed.
                last_rate.store(rate.to_bits(), Ordering::Relaxed);
                progress(completed, total, Some(rate));
                let before = controller.desired;
                // Near the end of the plan there is too little left to keep the pool
                // busy, so throughput falls for reasons that say nothing about the
                // link: hold instead of reading the tail as a loss.
                let decision = if total.saturating_sub(completed) < tail_bytes(rate, elapsed) {
                    Decision {
                        desired: before,
                        reason: None,
                    }
                } else {
                    controller.observe(&sample)
                };
                if decision.desired != before {
                    logger.debug(&format!(
                        "Download concurrency {before} -> {} ({}; {}) using {} bytes in {:.2}s",
                        decision.desired,
                        decision.reason.unwrap_or("adaptive"),
                        crate::stats::rate(rate),
                        sample.bytes,
                        elapsed
                    ));
                    peak.fetch_max(decision.desired, Ordering::Relaxed);
                    pool.set_desired(decision.desired);
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

/// The shared counters a window's transfer signals are summed into.
///
/// Workers report into these through the [`TransferObserver`] implementation below;
/// the adaptive controller drains them once per sampling window. Keeping them in one
/// place means the transfer layer never learns the controller exists.
struct WindowCounters {
    bytes: AtomicU64,
    ttfb_nanos: AtomicU64,
    ttfb_count: AtomicU64,
    rate_limited: AtomicU64,
    unavailable: AtomicU64,
    reset: AtomicU64,
}

impl WindowCounters {
    fn new() -> WindowCounters {
        WindowCounters {
            bytes: AtomicU64::new(0),
            ttfb_nanos: AtomicU64::new(0),
            ttfb_count: AtomicU64::new(0),
            rate_limited: AtomicU64::new(0),
            unavailable: AtomicU64::new(0),
            reset: AtomicU64::new(0),
        }
    }

    /// Take everything accumulated since the last drain, as one window's signals.
    /// `elapsed` is the window's real duration in seconds, and is positive.
    fn drain(&self, elapsed: f64) -> Window {
        let bytes = self.bytes.swap(0, Ordering::Relaxed);
        let ttfb_count = self.ttfb_count.swap(0, Ordering::Relaxed);
        let ttfb_nanos = self.ttfb_nanos.swap(0, Ordering::Relaxed);
        Window {
            rate: bytes as f64 / elapsed,
            bytes,
            ttfb: (ttfb_count > 0).then(|| Duration::from_nanos(ttfb_nanos / ttfb_count)),
            rate_limited: self.rate_limited.swap(0, Ordering::Relaxed) as u32,
            unavailable: self.unavailable.swap(0, Ordering::Relaxed) as u32,
            reset: self.reset.swap(0, Ordering::Relaxed) as u32,
        }
    }
}

impl TransferObserver for WindowCounters {
    fn bytes(&self, delta: u64) {
        self.bytes.fetch_add(delta, Ordering::Relaxed);
    }

    fn ttfb(&self, elapsed: Duration) {
        self.ttfb_nanos
            .fetch_add(elapsed.as_nanos() as u64, Ordering::Relaxed);
        self.ttfb_count.fetch_add(1, Ordering::Relaxed);
    }

    fn pressure(&self, pressure: Pressure) {
        let counter = match pressure {
            Pressure::RateLimited => &self.rate_limited,
            Pressure::Unavailable => &self.unavailable,
            Pressure::Reset => &self.reset,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// One sampling window's worth of transfer signals.
struct Window {
    /// Throughput in bytes per second over the window's real elapsed time.
    rate: f64,
    /// Raw bytes transferred, for the log line.
    bytes: u64,
    /// Mean time-to-first-byte across the files that started in this window.
    ttfb: Option<Duration>,
    /// Files the server answered with HTTP 429.
    rate_limited: u32,
    /// Files the server answered with HTTP 503.
    unavailable: u32,
    /// Transfers whose connection was reset.
    reset: u32,
}

/// The controller's verdict for the next window.
struct Decision {
    desired: usize,
    /// Why the count changed, for the log line; `None` when it did not.
    reason: Option<&'static str>,
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
    counters: &WindowCounters,
    last_rate: &AtomicU64,
    failure: &Mutex<Option<String>>,
    cancellation: &Cancellation,
    pool: &Pool,
    progress: ByteProgress<'_>,
) {
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
        if let Err(error) = fetch_one(fetcher, config, path, entry, counters) {
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

/// The throughput gain a probe must buy to keep the transfer it added: the mean rate
/// over [`PROBE_WINDOWS`] windows after the probe, against the mean over
/// [`BASELINE_WINDOWS`] before it. Below this the link is saturated and the transfer
/// is given back.
const MARGINAL_GAIN: f64 = 0.05;

/// Windows averaged to estimate the steady rate a probe is compared against. One
/// window swings with the plan's file-size mix even on a steady link, so judging a
/// probe on a single window reads that noise as signal; a mean of several does not.
const BASELINE_WINDOWS: usize = 3;

/// Windows averaged, after the settle, to estimate the rate a probe bought. The
/// before and after means cover the same number of windows, so the comparison is
/// symmetric and neither side is noisier than the other.
const PROBE_WINDOWS: u32 = 3;

/// Windows swallowed after a probe opens its connection before measuring begins: the
/// first window is mostly the new connection's slow start, not the link's steady
/// state, so folding it into the mean would bias the gain down.
const PROBE_SETTLE: u32 = 1;

/// Decode the rate a worker's per-file tick should report from the bits the
/// controller last stored. `None` until the first window has measured a rate, so a
/// file that finishes before then does not invent one.
fn tick_rate(bits: u64) -> Option<f64> {
    let rate = f64::from_bits(bits);
    rate.is_finite().then_some(rate)
}

/// How much of the pool survives a pressure cutback, as a fraction
/// (`DECLINE_KEEP / 4`): a quarter given up, not half.
const DECLINE_KEEP: usize = 3;

/// A window's mean time-to-first-byte above this multiple of the baseline counts as
/// a latency spike: the link is queueing rather than flowing.
const TTFB_SPIKE_FACTOR: f64 = 2.0;

/// Consecutive spiking windows before the transfer count is eased back. A single
/// busy window is noise — the file-size mix alone swings the mean — so reacting to
/// each one would walk the pool down toward one on a healthy link. Only a sustained
/// rise counts.
const LATENCY_RUN: u32 = 3;

/// The weight the newest window carries in the time-to-first-byte baseline.
const TTFB_SMOOTHING: f64 = 0.3;

/// Windows to wait before probing again after a 429, which is the server explicitly
/// asking us to slow down.
const RATE_LIMIT_COOLDOWN: u32 = 4;

/// Windows to wait before probing again after a 503 or a reset connection.
const PRESSURE_COOLDOWN: u32 = 2;

/// Windows to wait before probing again after a latency spike.
const SPIKE_COOLDOWN: u32 = 3;

/// Windows to wait before probing again after a probe bought less than
/// [`MARGINAL_GAIN`]: the link is saturated, so stop pushing it.
const SATURATED_COOLDOWN: u32 = 8;

/// The adaptive transfer-count controller.
///
/// It grows the pool one transfer at a time and keeps a transfer only if it bought at
/// least [`MARGINAL_GAIN`] more throughput — a probe-and-commit search for the point
/// where the link saturates. The gain is judged on the *mean* rate over the windows on
/// each side of the probe ([`BASELINE_WINDOWS`] before, [`PROBE_WINDOWS`] after a
/// settle), not on one window, so the plan's file-size mix cannot pass for a real gain
/// or loss. It backs off a little sooner when the transfer layer reports a reason to: a
/// sustained per-file latency spike costs one transfer, and transport pressure (429,
/// 503, a reset connection) costs a quarter or more.
struct Controller {
    /// The transfer count to run for the next window.
    desired: usize,
    /// The ceiling: the plan's file count, or the configured `jobs`.
    max: usize,
    /// The mean of the last few window rates: the baseline a probe is compared against.
    recent: RateWindow,
    /// An in-flight probe, if any.
    probe: Option<Probe>,
    /// Windows remaining before another probe is allowed.
    cooldown: u32,
    /// The smoothed time-to-first-byte in seconds, spiking windows excluded.
    ttfb_baseline: Option<f64>,
    /// Consecutive spiking windows seen, reset by any calm or skipped window.
    spike_run: u32,
}

/// A transfer added to test whether it buys throughput.
#[derive(Clone, Copy)]
struct Probe {
    /// The mean steady-window rate before the transfer was added.
    baseline_rate: f64,
    /// Windows left to swallow before measuring, so the new connection's slow start is
    /// not mistaken for the link's steady state.
    settle_left: u32,
    /// The sum of the post-settle window rates measured so far.
    sum: f64,
    /// How many post-settle windows have been measured.
    samples: u32,
}

/// The mean of the last [`BASELINE_WINDOWS`] window rates.
///
/// A probe judges its gain against a mean rather than a single window, because one
/// window's rate swings with the plan's file-size mix even on a steady link.
struct RateWindow {
    samples: [f64; BASELINE_WINDOWS],
    next: usize,
    filled: usize,
}

impl RateWindow {
    fn new() -> RateWindow {
        RateWindow {
            samples: [0.0; BASELINE_WINDOWS],
            next: 0,
            filled: 0,
        }
    }

    fn push(&mut self, rate: f64) {
        self.samples[self.next] = rate;
        self.next = (self.next + 1) % BASELINE_WINDOWS;
        self.filled = (self.filled + 1).min(BASELINE_WINDOWS);
    }

    /// Whether a full window of samples has been seen, so the mean is a stable basis
    /// for a probe.
    fn is_full(&self) -> bool {
        self.filled == BASELINE_WINDOWS
    }

    /// The mean of the samples seen so far, or `None` before the first.
    fn mean(&self) -> Option<f64> {
        (self.filled > 0)
            .then(|| self.samples[..self.filled].iter().sum::<f64>() / self.filled as f64)
    }
}

impl Controller {
    /// A controller starting at `start` transfers, never above `max`.
    fn new(start: usize, max: usize) -> Controller {
        let max = max.max(1);
        Controller {
            desired: start.clamp(1, max),
            max,
            recent: RateWindow::new(),
            probe: None,
            cooldown: 0,
            ttfb_baseline: None,
            spike_run: 0,
        }
    }

    /// Fold one `window`'s signals in and decide the transfer count for the next.
    fn observe(&mut self, window: &Window) -> Decision {
        let rate = window.rate;
        self.recent.push(rate);
        if self.cooldown > 0 {
            self.cooldown -= 1;
        }

        // Transport pressure first: the server is telling us to slow down, and no
        // throughput reading matters until we have.
        if window.rate_limited > 0 {
            self.probe = None;
            self.spike_run = 0;
            self.cooldown = self.cooldown.max(RATE_LIMIT_COOLDOWN);
            // A 429 is explicit, so give up half the pool, not a quarter.
            return self.change(self.desired / 2, "429 rate limited");
        }
        if window.unavailable > 0 || window.reset > 0 {
            self.probe = None;
            self.spike_run = 0;
            self.cooldown = self.cooldown.max(PRESSURE_COOLDOWN);
            let reason = if window.unavailable > 0 {
                "503 unavailable"
            } else {
                "connection reset"
            };
            return self.change(self.desired * DECLINE_KEEP / 4, reason);
        }

        // A latency spike, but only a *sustained* one: the file-size mix swings the
        // mean TTFB window to window, so a single busy window is noise and reacting to
        // each would ratchet the pool down toward one. The window a change opened (its
        // settle window) is skipped too: a freshly opened connection's first byte is
        // dominated by its handshake, not by link queueing.
        if let Some(ttfb) = window.ttfb {
            let seconds = ttfb.as_secs_f64();
            let settling = self.probe.is_some_and(|probe| probe.settle_left > 0);
            if !settling && self.spike(Some(ttfb)) {
                self.spike_run += 1;
                if self.spike_run >= LATENCY_RUN {
                    self.spike_run = 0;
                    self.probe = None;
                    self.cooldown = self.cooldown.max(SPIKE_COOLDOWN);
                    // Adopt the elevated level as the new normal, so a level shift the
                    // link (not our concurrency) caused does not keep firing.
                    self.ttfb_baseline = Some(seconds);
                    return self.change(self.desired.saturating_sub(1), "latency spike");
                }
                // Still climbing toward a run: leave the baseline calm, so the next
                // window is compared against it rather than against the spike.
            } else {
                self.spike_run = 0;
                self.ttfb_baseline = Some(match self.ttfb_baseline {
                    Some(previous) => TTFB_SMOOTHING * seconds + (1.0 - TTFB_SMOOTHING) * previous,
                    None => seconds,
                });
            }
        }

        // Resolve an in-flight probe: swallow its settle, then average the windows it
        // opened before judging whether the extra transfer paid for itself.
        if let Some(probe) = self.probe.take() {
            if probe.settle_left > 0 {
                self.probe = Some(Probe {
                    settle_left: probe.settle_left - 1,
                    ..probe
                });
                return Decision {
                    desired: self.desired,
                    reason: None,
                };
            }
            let sum = probe.sum + rate;
            let samples = probe.samples + 1;
            if samples < PROBE_WINDOWS {
                self.probe = Some(Probe {
                    sum,
                    samples,
                    ..probe
                });
                return Decision {
                    desired: self.desired,
                    reason: None,
                };
            }
            let measured = sum / samples as f64;
            let gain = if probe.baseline_rate > 0.0 {
                measured / probe.baseline_rate - 1.0
            } else {
                0.0
            };
            if gain >= MARGINAL_GAIN {
                // The extra transfer paid off; keep it and allow another probe.
                return Decision {
                    desired: self.desired,
                    reason: None,
                };
            }
            // Marginal: give the transfer back and stop probing for a while.
            self.cooldown = self.cooldown.max(SATURATED_COOLDOWN);
            self.spike_run = 0;
            return self.change(self.desired.saturating_sub(1), "saturated");
        }

        // Otherwise probe for one more transfer, once a full baseline window and no
        // cooldown allow it.
        if self.cooldown == 0 && self.desired < self.max && self.recent.is_full() {
            if let Some(baseline_rate) = self.recent.mean() {
                self.probe = Some(Probe {
                    baseline_rate,
                    settle_left: PROBE_SETTLE,
                    sum: 0.0,
                    samples: 0,
                });
                return self.change(self.desired + 1, "probing");
            }
        }
        Decision {
            desired: self.desired,
            reason: None,
        }
    }

    /// Whether `ttfb` is a spike against the current baseline.
    fn spike(&self, ttfb: Option<Duration>) -> bool {
        match (ttfb, self.ttfb_baseline) {
            (Some(ttfb), Some(baseline)) if baseline > 0.0 => {
                ttfb.as_secs_f64() > baseline * TTFB_SPIKE_FACTOR
            }
            _ => false,
        }
    }

    /// Adopt `desired` (clamped to `1..=max`) and report why, when it moved.
    fn change(&mut self, desired: usize, reason: &'static str) -> Decision {
        let desired = desired.clamp(1, self.max);
        if desired != self.desired {
            self.desired = desired;
            Decision {
                desired,
                reason: Some(reason),
            }
        } else {
            Decision {
                desired,
                reason: None,
            }
        }
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
    observer: &dyn TransferObserver,
) -> Result<(), String> {
    let staged = config.staging_dir.join(path.as_str());
    let url = fetcher.file_url(config.folder, path);
    let expect = Expect {
        length: entry.length,
        checksum: entry.checksum,
    };
    fetcher.fetch_file_observed(&url, &staged, expect, observer)?;
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
    use std::time::Duration;

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

    /// A window carrying only a throughput rate.
    fn window(rate: f64) -> Window {
        Window {
            rate,
            bytes: 0,
            ttfb: None,
            rate_limited: 0,
            unavailable: 0,
            reset: 0,
        }
    }

    /// A window carrying a rate and a mean per-file time-to-first-byte.
    fn window_with_ttfb(rate: f64, ttfb: Duration) -> Window {
        Window {
            ttfb: Some(ttfb),
            ..window(rate)
        }
    }

    /// Drive the controller for `rounds` windows against a link whose throughput
    /// depends on the transfer count the controller currently wants, as a real link's
    /// does. The rate sampled for each window uses the count before that window's
    /// `observe`, so a probe sees the throughput its extra transfer actually bought.
    fn drive(controller: &mut Controller, rounds: usize, link: impl Fn(usize) -> f64) {
        for _ in 0..rounds {
            let rate = link(controller.desired);
            controller.observe(&window(rate));
        }
    }

    #[test]
    fn the_controller_climbs_a_link_that_still_improves() {
        let mut controller = Controller::new(2, 16);
        // Every extra transfer adds 25% of the base: well above MARGINAL_GAIN, so each
        // probe pays for itself and the count walks up to the ceiling.
        drive(&mut controller, 200, |n| {
            40_000_000.0 + 10_000_000.0 * n as f64
        });
        assert_eq!(controller.desired, 16);
    }

    #[test]
    fn the_controller_settles_a_saturated_link() {
        let mut controller = Controller::new(2, 16);
        // Throughput stops rising past six transfers: an extra one buys nothing, so
        // the controller should hover at the knee, well below the ceiling, not run to
        // it. The resting count is six; seven is the transient of the probe it keeps
        // trying and giving back. A single noisy-window judgement thrashes here instead.
        drive(&mut controller, 300, |n| {
            60_000_000.0_f64.min(10_000_000.0 * n as f64)
        });
        assert!(
            (6..=7).contains(&controller.desired),
            "desired = {}",
            controller.desired
        );
    }

    #[test]
    fn a_flat_link_never_grows() {
        let mut controller = Controller::new(2, 16);
        // A flat link buys nothing from an extra transfer, so every probe is given
        // back and the count never leaves its starting point.
        drive(&mut controller, 200, |_| 50_000_000.0);
        assert_eq!(controller.desired, 2);
    }

    #[test]
    fn a_rate_limit_halves_the_pool() {
        let mut controller = Controller::new(8, 16);
        let mut w = window(100.0);
        w.rate_limited = 1;
        let decision = controller.observe(&w);
        assert_eq!(decision.reason, Some("429 rate limited"));
        assert_eq!(controller.desired, 4);
    }

    #[test]
    fn a_503_or_a_reset_gives_up_a_quarter() {
        let mut controller = Controller::new(8, 16);
        let mut unavailable = window(100.0);
        unavailable.unavailable = 1;
        let decision = controller.observe(&unavailable);
        assert_eq!(decision.reason, Some("503 unavailable"));
        assert_eq!(controller.desired, 6);

        let mut controller = Controller::new(8, 16);
        let mut reset = window(100.0);
        reset.reset = 1;
        let decision = controller.observe(&reset);
        assert_eq!(decision.reason, Some("connection reset"));
        assert_eq!(controller.desired, 6);
    }

    #[test]
    fn pressure_never_trims_below_one() {
        let mut controller = Controller::new(4, 16);
        // A long run of 429s reaches the floor of one transfer and stops there
        // rather than falling to zero.
        for _ in 0..32 {
            let mut w = window(100.0);
            w.rate_limited = 1;
            controller.observe(&w);
        }
        assert_eq!(controller.desired, 1);
    }

    #[test]
    fn a_sustained_latency_spike_costs_one_transfer() {
        // `max == start`, so the controller never probes and only the spike moves it.
        let mut controller = Controller::new(4, 4);
        let baseline = Duration::from_millis(100);
        for _ in 0..5 {
            controller.observe(&window_with_ttfb(1_000.0, baseline));
        }
        assert_eq!(controller.desired, 4);
        // One spiking window is noise (the file-size mix swings the mean) and must not
        // move the count.
        let decision = controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(300)));
        assert_eq!(decision.reason, None);
        assert_eq!(controller.desired, 4);
        // Nor do two: only a run of LATENCY_RUN of them is a real loss of the link.
        let decision = controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(300)));
        assert_eq!(decision.reason, None);
        assert_eq!(controller.desired, 4);
        let decision = controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(300)));
        assert_eq!(decision.reason, Some("latency spike"));
        assert_eq!(controller.desired, 3);
        // The elevated level is adopted, so a window at the same latency is no longer
        // a spike: the count cannot ratchet down on a level shift it did not cause.
        let decision = controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(300)));
        assert_eq!(decision.reason, None);
        assert_eq!(controller.desired, 3);
    }

    #[test]
    fn a_latency_spike_does_not_raise_the_baseline_until_it_is_acted_on() {
        let mut controller = Controller::new(4, 4);
        for _ in 0..5 {
            controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(100)));
        }
        // A spiking window is not folded into the baseline, so the next spike still
        // reads as a spike rather than as the new normal, and the run can reach three.
        controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(500)));
        let decision = controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(500)));
        assert_eq!(decision.reason, None);
        let decision = controller.observe(&window_with_ttfb(1_000.0, Duration::from_millis(500)));
        assert_eq!(decision.reason, Some("latency spike"));
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
