//! Rust-owned launcher logging and child-process output.
//!
//! The logger writes `zwift.log` itself and hands every message to registered
//! sinks, so a GUI log view needs no tailing of the file.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::{Arc, Mutex};

use crate::cancel::Cancellation;
use crate::process::{run_logged_cancellable, run_output_cancellable, OutputCapture};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Info,
    Ok,
    Warning,
    Error,
    Debug,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Info => "info",
            Kind::Ok => "ok",
            Kind::Warning => "warning",
            Kind::Error => "error",
            Kind::Debug => "debug",
        }
    }

    fn symbol(self) -> &'static str {
        match self {
            Kind::Info => "*",
            Kind::Ok => "✓",
            Kind::Warning => "!",
            Kind::Error => "✗",
            Kind::Debug => "◉",
        }
    }

    fn color(self) -> &'static str {
        match self {
            Kind::Info => "\x1b[0;34m",
            Kind::Ok => "\x1b[0;32m",
            Kind::Warning => "\x1b[0;33m",
            Kind::Error => "\x1b[0;31m",
            Kind::Debug => "\x1b[0;37m",
        }
    }

    /// Minimum VERBOSITY at which the console shows this kind.
    fn min_verbosity(self) -> u8 {
        match self {
            Kind::Info => 1,
            Kind::Debug => 3,
            Kind::Ok | Kind::Warning | Kind::Error => 0,
        }
    }
}

/// Line written to zwift.log: `[HH:MM:SS] kind: message`.
pub fn format_file_line(time: &str, kind: Kind, msg: &str) -> String {
    format!("[{time}] {}: {msg}\n", kind.name())
}

/// Console line: `[snap|*] message`, with the time added from VERBOSITY 2 on.
pub fn format_console_line(
    time: &str,
    kind: Kind,
    msg: &str,
    verbosity: u8,
    color: bool,
) -> Option<String> {
    if verbosity < kind.min_verbosity() {
        return None;
    }
    let stamp = if verbosity >= 2 {
        format!("{time}|")
    } else {
        String::new()
    };
    let line = format!("[snap|{stamp}{}] {msg}", kind.symbol());
    Some(if color {
        format!("{}{line}\x1b[0m", kind.color())
    } else {
        line
    })
}

/// Local time as HH:MM:SS.
pub fn local_time() -> String {
    // SAFETY: time and localtime_r only write to the locals passed in.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
    }
}

type Sink = Box<dyn Fn(Kind, &str) + Send>;
type RawSink = Box<dyn Fn(&[u8]) + Send>;

struct RawOutput {
    path: PathBuf,
    sinks: Mutex<Vec<RawSink>>,
    write: Mutex<()>,
}

impl RawOutput {
    fn write(&self, bytes: &[u8]) -> std::io::Result<()> {
        let _guard = self.write.lock().expect("raw output mutex poisoned");
        OpenOptions::new()
            .append(true)
            .open(&self.path)?
            .write_all(bytes)?;
        std::io::stdout().lock().write_all(bytes)?;
        for sink in self.sinks.lock().expect("raw sink mutex poisoned").iter() {
            sink(bytes);
        }
        Ok(())
    }
}

pub struct Logger {
    path: PathBuf,
    verbosity: u8,
    color: bool,
    sinks: Mutex<Vec<Sink>>,
    raw: Arc<RawOutput>,
    capture: OutputCapture,
    cancellation: Cancellation,
}

impl Logger {
    pub fn new(path: impl Into<PathBuf>, verbosity: u8, color: bool) -> Self {
        Self::with_cancellation(path, verbosity, color, Cancellation::default())
    }

    pub fn with_cancellation(
        path: impl Into<PathBuf>,
        verbosity: u8,
        color: bool,
        cancellation: Cancellation,
    ) -> Self {
        let path = path.into();
        Logger {
            path: path.clone(),
            verbosity,
            color,
            sinks: Mutex::new(Vec::new()),
            raw: Arc::new(RawOutput {
                path,
                sinks: Mutex::new(Vec::new()),
                write: Mutex::new(()),
            }),
            capture: OutputCapture::default(),
            cancellation,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }

    pub fn run_command(&self, command: &mut Command) -> std::io::Result<ExitStatus> {
        let raw = self.raw.clone();
        run_logged_cancellable(command, &|bytes| raw.write(bytes), &self.cancellation)
    }

    pub fn command_output(&self, command: &mut Command) -> std::io::Result<std::process::Output> {
        run_output_cancellable(command, &self.cancellation)
    }

    /// Creates the log directory, keeps the previous log as `zwift.log.1`, starts an empty log.
    pub fn init(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        if self.path.is_file() {
            let mut backup = self.path.clone().into_os_string();
            backup.push(".1");
            fs::rename(&self.path, backup)?;
        }
        fs::File::create(&self.path).map(|_| ())
    }

    /// Registers a receiver of every message, for example the GUI log view.
    pub fn add_sink(&self, sink: impl Fn(Kind, &str) + Send + 'static) {
        self.sinks.lock().unwrap().push(Box::new(sink));
    }

    /// Registers a receiver for unformatted child-process output.
    pub fn add_raw_sink(&self, sink: impl Fn(&[u8]) + Send + 'static) {
        self.raw.sinks.lock().unwrap().push(Box::new(sink));
    }

    /// Forward raw process output to the log, terminal, and registered sinks.
    pub fn write_process_output(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.raw.write(bytes)
    }

    /// Keep output from a launched program's descendants alive until cleanup.
    pub fn run_inherited_output(&self, command: &mut Command) -> std::io::Result<ExitStatus> {
        let raw = self.raw.clone();
        self.capture.run(
            command,
            Arc::new(move |bytes| raw.write(bytes)),
            &self.cancellation,
        )
    }

    /// Finish inherited output after stopping the Wine server.
    pub fn finish_inherited_output(&self) -> std::io::Result<()> {
        self.capture.finish()
    }

    pub fn log(&self, kind: Kind, msg: &str) {
        let time = local_time();
        // Like msgbox, only an existing log file is appended to.
        if self.path.is_file() {
            if let Err(error) = OpenOptions::new()
                .append(true)
                .open(&self.path)
                .and_then(|mut file| file.write_all(format_file_line(&time, kind, msg).as_bytes()))
            {
                eprintln!("Unable to write log {}: {error}", self.path.display());
            }
        }
        if let Some(line) = format_console_line(&time, kind, msg, self.verbosity, self.color) {
            if kind == Kind::Error {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
        for sink in self.sinks.lock().unwrap().iter() {
            sink(kind, msg);
        }
    }

    pub fn info(&self, msg: &str) {
        self.log(Kind::Info, msg);
    }
    pub fn ok(&self, msg: &str) {
        self.log(Kind::Ok, msg);
    }
    pub fn warning(&self, msg: &str) {
        self.log(Kind::Warning, msg);
    }
    pub fn error(&self, msg: &str) {
        self.log(Kind::Error, msg);
    }
    pub fn debug(&self, msg: &str) {
        self.log(Kind::Debug, msg);
    }

    /// Appends raw output of a child process (the `run_logged` + `tee -a` role).
    pub fn append_raw(&self, line: &str) {
        if let Ok(mut f) = OpenOptions::new().append(true).open(&self.path) {
            let _ = writeln!(f, "{line}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zwsnap-gui-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn file_line_format() {
        assert_eq!(
            format_file_line("12:34:56", Kind::Info, "hi"),
            "[12:34:56] info: hi\n"
        );
    }

    #[test]
    fn console_respects_verbosity() {
        assert_eq!(format_console_line("t", Kind::Info, "m", 0, false), None);
        assert_eq!(
            format_console_line("t", Kind::Info, "m", 1, false).unwrap(),
            "[snap|*] m"
        );
        assert_eq!(
            format_console_line("t", Kind::Ok, "m", 0, false).unwrap(),
            "[snap|✓] m"
        );
        assert_eq!(
            format_console_line("t", Kind::Ok, "m", 2, false).unwrap(),
            "[snap|t|✓] m"
        );
        assert_eq!(format_console_line("t", Kind::Debug, "m", 2, false), None);
        assert_eq!(
            format_console_line("t", Kind::Debug, "m", 3, false).unwrap(),
            "[snap|t|◉] m"
        );
    }

    #[test]
    fn console_color() {
        let line = format_console_line("t", Kind::Error, "m", 1, true).unwrap();
        assert_eq!(line, "\x1b[0;31m[snap|✗] m\x1b[0m");
    }

    #[test]
    fn init_rotates_previous_log() {
        let dir = tmp("rotate");
        let path = dir.join("sub/zwift.log");
        let logger = Logger::new(&path, 0, false);
        logger.init().unwrap();
        logger.ok("first");
        logger.init().unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "");
        assert!(fs::read_to_string(dir.join("sub/zwift.log.1"))
            .unwrap()
            .contains("ok: first"));
    }

    #[test]
    fn log_writes_file_and_sinks_but_not_without_file() {
        let dir = tmp("sink");
        let path = dir.join("zwift.log");
        let logger = Logger::new(&path, 0, false);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        logger.add_sink(move |k, m| s.lock().unwrap().push((k, m.to_string())));

        logger.ok("before init");
        assert!(!path.exists());

        logger.init().unwrap();
        logger.append_raw("raw wine output");
        logger.ok("started");
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("raw wine output\n["));
        assert!(text.trim_end().ends_with("ok: started"));
        assert_eq!(seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn raw_output_reaches_file_and_sink_without_reformatting() {
        let dir = tmp("raw");
        let path = dir.join("zwift.log");
        let logger = Logger::new(&path, 0, false);
        logger.init().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        logger.add_raw_sink(move |bytes| sink.lock().unwrap().extend_from_slice(bytes));
        logger.write_process_output(b"a\0b\n").unwrap();
        assert_eq!(fs::read(path).unwrap(), b"a\0b\n");
        assert_eq!(*seen.lock().unwrap(), b"a\0b\n");
    }
}
