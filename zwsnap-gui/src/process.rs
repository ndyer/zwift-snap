//! Stream child process output without blocking either output pipe.

use crate::cancel::Cancellation;
use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Output as CommandOutput, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type Output = dyn Fn(&[u8]) -> io::Result<()> + Send + Sync;

/// Readers for commands whose descendants can outlive the immediate child.
#[derive(Default)]
pub struct OutputCapture {
    readers: Mutex<Vec<JoinHandle<()>>>,
    stopping: Arc<AtomicBool>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl OutputCapture {
    /// Wait for the immediate command, keeping inherited output pipes open.
    ///
    /// Call `finish` only after the descendants have been stopped.
    pub fn run(
        &self,
        command: &mut Command,
        output: Arc<Output>,
        cancellation: &Cancellation,
    ) -> io::Result<ExitStatus> {
        let mut readers = self.readers.lock().expect("capture mutex poisoned");
        cancellation.check()?;
        command.process_group(0);
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");
        for reader in [
            Box::new(stdout) as Box<dyn ReadFd>,
            Box::new(stderr) as Box<dyn ReadFd>,
        ] {
            let output = output.clone();
            let stopping = self.stopping.clone();
            let errors = self.errors.clone();
            let cancellation = cancellation.clone();
            readers.push(std::thread::spawn(move || {
                drain_inherited(reader, &*output, &stopping, &cancellation, &errors)
            }));
        }
        drop(readers);
        wait_cancellable(&mut child, cancellation)
    }

    /// Stop the readers after their descendants have been terminated, and report errors.
    pub fn finish(&self) -> io::Result<()> {
        let mut readers = self.readers.lock().expect("capture mutex poisoned");
        self.stopping.store(true, Ordering::Release);
        for reader in readers.drain(..) {
            if reader.join().is_err() {
                self.errors
                    .lock()
                    .expect("capture error mutex poisoned")
                    .push("output reader panicked".into());
            }
        }
        self.stopping.store(false, Ordering::Release);
        let errors =
            std::mem::take(&mut *self.errors.lock().expect("capture error mutex poisoned"));
        if errors.is_empty() {
            Ok(())
        } else {
            Err(io::Error::other(errors.join("; ")))
        }
    }
}

trait ReadFd: Read + AsRawFd + Send {}
impl<T: Read + AsRawFd + Send> ReadFd for T {}

fn drain_inherited(
    mut reader: Box<dyn ReadFd>,
    output: &Output,
    stopping: &AtomicBool,
    cancellation: &Cancellation,
    errors: &Mutex<Vec<String>>,
) {
    let mut buffer = [0; 8192];
    let mut forwarding_failed = false;
    let mut deadline = None;
    loop {
        if (stopping.load(Ordering::Acquire) || cancellation.is_cancelled()) && deadline.is_none() {
            deadline = Some(Instant::now() + Duration::from_secs(3));
        }
        if deadline.is_some_and(|end| Instant::now() >= end) {
            report_capture_error(errors, "output did not drain during cleanup".into());
            break;
        }
        let mut fd = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd points to one initialized pollfd for the duration of the call.
        let ready = unsafe { libc::poll(&mut fd, 1, 100) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            report_capture_error(errors, format!("output poll failed: {error}"));
            break;
        }
        if ready == 0 {
            continue;
        }
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(error) = output(&buffer[..n]) {
                    if !forwarding_failed {
                        report_capture_error(errors, format!("output forwarding failed: {error}"));
                        forwarding_failed = true;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                report_capture_error(errors, format!("output read failed: {error}"));
                break;
            }
        }
    }
}

fn wait_cancellable(
    child: &mut std::process::Child,
    cancellation: &Cancellation,
) -> io::Result<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if cancellation.wait(Duration::from_millis(50)).is_err() {
            let pgid = i32::try_from(child.id())
                .map_err(|_| io::Error::other("child process ID exceeds platform limit"))?;
            signal_group(pgid, libc::SIGTERM)?;
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline && group_exists(pgid)? {
                std::thread::sleep(Duration::from_millis(50));
            }
            if group_exists(pgid)? {
                signal_group(pgid, libc::SIGKILL)?;
            }
            let _ = child.wait()?;
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Launcher cancelled",
            ));
        }
    }
}

fn group_exists(pgid: i32) -> io::Result<bool> {
    // SAFETY: signal 0 only checks whether the process group exists.
    if unsafe { libc::kill(-pgid, 0) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(false),
        Some(libc::EPERM) => Ok(true),
        _ => Err(error),
    }
}

fn signal_group(pgid: i32, signal: i32) -> io::Result<()> {
    // SAFETY: pgid is the process group ID of the child we started.
    if unsafe { libc::kill(-pgid, signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}

fn report_capture_error(errors: &Mutex<Vec<String>>, error: String) {
    eprintln!("{error}");
    errors
        .lock()
        .expect("capture error mutex poisoned")
        .push(error);
}

/// Run a command, forwarding stdout and stderr as raw bytes to `output`.
///
/// The callback may be called from either reader thread. Both pipes are drained
/// even if forwarding fails, and the child is always reaped.
pub fn run_logged(
    command: &mut Command,
    output: &(impl Fn(&[u8]) -> io::Result<()> + Sync),
) -> io::Result<ExitStatus> {
    run_logged_cancellable(command, output, &Cancellation::default())
}

pub fn run_logged_cancellable(
    command: &mut Command,
    output: &(impl Fn(&[u8]) -> io::Result<()> + Sync),
    cancellation: &Cancellation,
) -> io::Result<ExitStatus> {
    cancellation.check()?;
    command.process_group(0);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let forwarding_error = Arc::new(Mutex::new(None));

    let wait_result = std::thread::scope(|scope| {
        let drain = |mut reader: Box<dyn ReadFd>| {
            let mut buffer = [0; 8192];
            let mut deadline = None;
            loop {
                if cancellation.is_cancelled() && deadline.is_none() {
                    deadline = Some(Instant::now() + Duration::from_secs(3));
                }
                if deadline.is_some_and(|end| Instant::now() >= end) {
                    break;
                }
                let mut fd = libc::pollfd {
                    fd: reader.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: fd points to one initialized pollfd for the duration of the call.
                let ready = unsafe { libc::poll(&mut fd, 1, 100) };
                if ready < 0 {
                    let error = io::Error::last_os_error();
                    if error.kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    let mut first = forwarding_error.lock().expect("error mutex poisoned");
                    if first.is_none() {
                        *first = Some(error);
                    }
                    break;
                }
                if ready == 0 {
                    continue;
                }
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        if let Err(error) = output(&buffer[..n]) {
                            let mut first = forwarding_error.lock().expect("error mutex poisoned");
                            if first.is_none() {
                                *first = Some(error);
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        let mut first = forwarding_error.lock().expect("error mutex poisoned");
                        if first.is_none() {
                            *first = Some(error);
                        }
                        break;
                    }
                }
            }
        };
        scope.spawn(move || drain(Box::new(stdout)));
        scope.spawn(move || drain(Box::new(stderr)));
        wait_cancellable(&mut child, cancellation)
    });

    let status = wait_result?;
    if let Some(error) = forwarding_error
        .lock()
        .expect("error mutex poisoned")
        .take()
    {
        return Err(error);
    }
    Ok(status)
}

/// Run a command and capture both streams while remaining responsive to cancellation.
pub fn run_output_cancellable(
    command: &mut Command,
    cancellation: &Cancellation,
) -> io::Result<CommandOutput> {
    cancellation.check()?;
    command.process_group(0);
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");
    let stdout_bytes = Arc::new(Mutex::new(Vec::new()));
    let stderr_bytes = Arc::new(Mutex::new(Vec::new()));
    let read_errors = Arc::new(Mutex::new(Vec::new()));

    let drain = |mut reader: Box<dyn ReadFd>, bytes: Arc<Mutex<Vec<u8>>>| {
        let mut buffer = [0; 8192];
        let mut deadline = None;
        loop {
            if cancellation.is_cancelled() && deadline.is_none() {
                deadline = Some(Instant::now() + Duration::from_secs(3));
            }
            if deadline.is_some_and(|end| Instant::now() >= end) {
                read_errors
                    .lock()
                    .expect("read error mutex poisoned")
                    .push("output did not drain after cancellation".to_string());
                break;
            }
            let mut fd = libc::pollfd {
                fd: reader.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: fd points to one initialized pollfd for the duration of the call.
            let ready = unsafe { libc::poll(&mut fd, 1, 100) };
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                read_errors
                    .lock()
                    .expect("read error mutex poisoned")
                    .push(error.to_string());
                break;
            }
            if ready == 0 {
                continue;
            }
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => bytes
                    .lock()
                    .expect("output buffer mutex poisoned")
                    .extend_from_slice(&buffer[..n]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    read_errors
                        .lock()
                        .expect("read error mutex poisoned")
                        .push(error.to_string());
                    break;
                }
            }
        }
    };
    let status = std::thread::scope(|scope| {
        let drain = &drain;
        let stdout_bytes = stdout_bytes.clone();
        let stderr_bytes = stderr_bytes.clone();
        scope.spawn(move || drain(Box::new(stdout), stdout_bytes));
        scope.spawn(move || drain(Box::new(stderr), stderr_bytes));
        wait_cancellable(&mut child, cancellation)
    })?;
    let errors = read_errors.lock().expect("read error mutex poisoned");
    if !errors.is_empty() {
        return Err(io::Error::other(errors.join("; ")));
    }
    let stdout = std::mem::take(&mut *stdout_bytes.lock().expect("output mutex poisoned"));
    let stderr = std::mem::take(&mut *stderr_bytes.lock().expect("output mutex poisoned"));
    Ok(CommandOutput {
        status,
        stdout,
        stderr,
    })
}

/// Cleanup must remain bounded even after the caller has requested cancellation.
pub fn run_output_timeout(command: &mut Command, timeout: Duration) -> io::Result<CommandOutput> {
    let cancellation = Cancellation::default();
    std::thread::scope(|scope| {
        let (finished, receiver) = std::sync::mpsc::channel::<()>();
        let timer_cancellation = &cancellation;
        let timer = scope.spawn(move || {
            if matches!(
                receiver.recv_timeout(timeout),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ) {
                timer_cancellation.cancel();
                true
            } else {
                false
            }
        });
        let result = run_output_cancellable(command, &cancellation);
        drop(finished);
        if timer.join().expect("cleanup timer panicked") {
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Cleanup command timed out",
            ))
        } else {
            result
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn inherited_output_outlives_immediate_command() {
        let dir = std::env::temp_dir().join(format!("zwift-pipes-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let capture = Arc::new(OutputCapture::default());
        let (chunks, received) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let worker_capture = capture.clone();
        let worker_dir = dir.clone();
        let worker = std::thread::spawn(move || {
            let result = worker_capture.run(
                Command::new("sh").env("PIPE_TEST_DIR", worker_dir).args([
                    "-c",
                    "(i=0; while [ ! -f \"$PIPE_TEST_DIR/release\" ]; do \
                         i=$((i + 1)); [ \"$i\" -lt 200 ] || exit 1; sleep 0.05; \
                         done; printf later; printf ' diagnostic' >&2) & printf initial; exit 7",
                ]),
                Arc::new(move |bytes| {
                    chunks
                        .send(bytes.to_vec())
                        .map_err(|error| io::Error::other(error.to_string()))
                }),
                &Cancellation::default(),
            );
            done.send(result).unwrap();
        });
        let status = completed.recv_timeout(Duration::from_secs(3));
        std::fs::write(dir.join("release"), "").unwrap();
        let status = status
            .expect("immediate command waited for the descendant")
            .unwrap();
        assert_eq!(status.code(), Some(7));
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut bytes = Vec::new();
        while !bytes.windows(5).any(|window| window == b"later")
            || !bytes.windows(10).any(|window| window == b"diagnostic")
        {
            let remaining = deadline.saturating_duration_since(Instant::now());
            bytes.extend(
                received
                    .recv_timeout(remaining)
                    .expect("missing descendant output"),
            );
        }
        assert!(bytes.windows(7).any(|window| window == b"initial"));
        capture.finish().unwrap();
        worker.join().unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn inherited_output_reports_forwarding_errors_after_draining() {
        let capture = OutputCapture::default();
        let status = capture
            .run(
                Command::new("sh").args(["-c", "head -c 262144 /dev/zero"]),
                Arc::new(|_| Err(io::Error::other("log write failed"))),
                &Cancellation::default(),
            )
            .unwrap();
        assert!(status.success());
        assert!(capture
            .finish()
            .unwrap_err()
            .to_string()
            .contains("log write failed"));
        capture.finish().unwrap();
    }

    #[test]
    fn cancellation_terminates_the_child_process_group() {
        let dir = std::env::temp_dir().join(format!("zwift-cancel-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let pid_file = dir.join("child.pid");
        let cancellation = Cancellation::default();
        let worker_cancellation = cancellation.clone();
        let worker_pid_file = pid_file.clone();
        let worker = std::thread::spawn(move || {
            run_output_cancellable(
                Command::new("sh")
                    .env("CHILD_PID_FILE", worker_pid_file)
                    .args(["-c", "sleep 30 & echo $! > \"$CHILD_PID_FILE\"; wait"]),
                &worker_cancellation,
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        while !pid_file.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let child_pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("child process did not start")
            .trim()
            .parse()
            .unwrap();
        cancellation.cancel();
        assert_eq!(
            worker.join().unwrap().unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        // SAFETY: signal 0 only checks whether the process still exists.
        assert_eq!(unsafe { libc::kill(child_pid, 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn captures_both_pipes_and_preserves_exit_status() {
        let bytes = Mutex::new(Vec::new());
        let status = run_logged(
            Command::new("sh").args(["-c", "printf stdout; printf stderr >&2; exit 7"]),
            &|chunk| {
                bytes.lock().unwrap().extend_from_slice(chunk);
                Ok(())
            },
        )
        .unwrap();
        let bytes = bytes.into_inner().unwrap();
        assert_eq!(status.code(), Some(7));
        assert!(bytes.windows(6).any(|chunk| chunk == b"stdout"));
        assert!(bytes.windows(6).any(|chunk| chunk == b"stderr"));
    }

    #[test]
    fn cleanup_timeout_terminates_an_unresponsive_command() {
        let started = Instant::now();
        let error = run_output_timeout(
            Command::new("sh").args(["-c", "exec sleep 30"]),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
        let output = run_output_timeout(
            Command::new("sh").args(["-c", "printf cleaned"]),
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"cleaned");
    }

    #[test]
    fn drains_large_streams_when_forwarding_fails() {
        let result = run_logged(
            Command::new("sh").args([
                "-c",
                "head -c 262144 /dev/zero & head -c 262144 /dev/zero >&2; wait",
            ]),
            &|_| Err(io::Error::other("log write failed")),
        );
        assert_eq!(result.unwrap_err().to_string(), "log write failed");
    }
}
