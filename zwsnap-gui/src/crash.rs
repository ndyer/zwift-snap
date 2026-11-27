//! Best-effort crash backtrace for the launcher.
//!
//! The launcher runs inside a snap, where a fatal signal leaves no trace: the
//! kernel `core_pattern` pipes to apport, which ignores snaps, and confined
//! processes are not core-dumpable. A segfault would otherwise be invisible.
//!
//! [`install`] puts a fatal-signal handler in place that prints a backtrace to
//! stderr (which the launcher mirrors into its log), then re-raises the signal
//! with its default disposition so the exit status still reflects the crash.
//! It is always on: `zwift.log` is the user's to trim, and this runs only on a
//! fatal signal, never on the fast path.

use std::ffi::c_void;

extern "C" {
    fn backtrace(buffer: *mut *mut c_void, size: libc::c_int) -> libc::c_int;
    fn backtrace_symbols_fd(buffer: *const *mut c_void, size: libc::c_int, fd: libc::c_int);
}

/// Frames recorded from the faulting stack. Enough to walk past the signal
/// trampoline and the GTK/WebKit frames above the launcher's own.
const MAX_FRAMES: usize = 128;

extern "C" fn handler(signal: libc::c_int) {
    const HEADER: &[u8] = b"\n===== zwsnap-gui fatal signal =====\n";
    const BT: &[u8] = b"----- backtrace -----\n";
    const END: &[u8] = b"===== end backtrace =====\n";
    // Only async-signal-safe calls below: `write`, `backtrace_symbols_fd`,
    // `signal`, `raise`.
    unsafe {
        write_stderr(HEADER);
        let mut frames = [std::ptr::null_mut::<c_void>(); MAX_FRAMES];
        let count = backtrace(frames.as_mut_ptr(), MAX_FRAMES as libc::c_int);
        write_stderr(BT);
        if count > 0 {
            backtrace_symbols_fd(frames.as_ptr(), count, 2);
        }
        write_stderr(END);
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Write `bytes` to stderr, retrying short writes.
fn write_stderr(bytes: &[u8]) {
    let mut written = 0usize;
    while written < bytes.len() {
        let n = unsafe {
            libc::write(
                2,
                bytes[written..].as_ptr() as *const c_void,
                bytes.len() - written,
            )
        };
        if n <= 0 {
            break;
        }
        written += n as usize;
    }
}

/// Install the fatal-signal backtrace handler. Idempotent and cheap; call it
/// once at process start.
pub fn install() {
    unsafe {
        let action = handler as extern "C" fn(libc::c_int) as libc::sighandler_t;
        for signal in [
            libc::SIGSEGV,
            libc::SIGBUS,
            libc::SIGABRT,
            libc::SIGILL,
            libc::SIGFPE,
        ] {
            libc::signal(signal, action);
        }
    }
}
