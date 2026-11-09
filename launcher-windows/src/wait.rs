//! `wait-for-window.exe PID`: block until `PID` shows a real window, so the
//! launcher can tell "the game is starting" from "the window is up".
//!
//! This asks wine's own window API, so it works the same on X11 and Wayland.
//!
//! Exit codes: 0 the window is visible, 2 timed out (the launcher leaves the
//! game running), 1 an error the launcher reports as a failure.

use std::{
    mem::zeroed,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{CloseHandle, BOOL, HWND, LPARAM, RECT, WAIT_TIMEOUT},
    System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_ACCESS_RIGHTS},
    UI::WindowsAndMessaging::{
        EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    },
};

use crate::window::is_game_window;

/// `SYNCHRONIZE`: all `WaitForSingleObject` needs, and the minimum that works
/// on another user's process.
const SYNCHRONIZE: u32 = 0x0010_0000;

const TIMEOUT: Duration = Duration::from_secs(120);
const POLL: u32 = 100;

pub const EXIT_VISIBLE: i32 = 0;
const EXIT_ERROR: i32 = 1;
pub const EXIT_TIMEOUT: i32 = 2;

/// The process being waited for, read by the `EnumWindows` callback.
static TARGET: AtomicU32 = AtomicU32::new(0);
/// Set by the callback when it finds the window, which also stops the
/// enumeration.
static FOUND: AtomicBool = AtomicBool::new(false);

/// Wait for the game's window and return the exit code described in the module
/// documentation.
pub fn run() -> i32 {
    let pid = match std::env::args()
        .nth(1)
        .and_then(|pid| pid.parse::<u32>().ok())
    {
        Some(pid) if pid != 0 => pid,
        _ => {
            eprintln!("Usage: wait-for-window.exe PID");
            return EXIT_ERROR;
        }
    };
    TARGET.store(pid, Ordering::Relaxed);

    let process = match unsafe { OpenProcess(PROCESS_ACCESS_RIGHTS(SYNCHRONIZE), false, pid) } {
        Ok(process) => process,
        Err(error) => {
            eprintln!("Cannot open process {pid}: {error}");
            return EXIT_ERROR;
        }
    };

    let deadline = Instant::now() + TIMEOUT;
    loop {
        // EnumWindows reports an error whenever the callback stops it, so that
        // only counts as a failure when the window was not found.
        if let Err(error) = unsafe { EnumWindows(Some(find_window), LPARAM(0)) } {
            if !FOUND.load(Ordering::Relaxed) {
                eprintln!("Cannot enumerate windows: {error}");
                unsafe { CloseHandle(process).ok() };
                return EXIT_ERROR;
            }
        }
        if FOUND.load(Ordering::Relaxed) {
            unsafe { CloseHandle(process).ok() };
            return EXIT_VISIBLE;
        }

        // A short wait rather than a sleep, so a game that dies without ever
        // showing a window is noticed at once.
        let status = unsafe { WaitForSingleObject(process, POLL) };
        if status != WAIT_TIMEOUT {
            eprintln!("Process {pid} exited or cannot be waited on ({})", status.0);
            unsafe { CloseHandle(process).ok() };
            return EXIT_ERROR;
        }
        if Instant::now() >= deadline {
            eprintln!("Timed out waiting for process {pid} to show a window");
            unsafe { CloseHandle(process).ok() };
            return EXIT_TIMEOUT;
        }
    }
}

unsafe extern "system" fn find_window(window: HWND, _data: LPARAM) -> BOOL {
    let mut owner = 0u32;
    unsafe { GetWindowThreadProcessId(window, Some(&mut owner)) };

    let mut rect: RECT = unsafe { zeroed() };
    let measured = unsafe { GetWindowRect(window, &mut rect) }.is_ok();
    let matched = measured
        && is_game_window(
            owner,
            TARGET.load(Ordering::Relaxed),
            unsafe { IsWindowVisible(window) }.as_bool(),
            unsafe { IsIconic(window) }.as_bool(),
            rect.right - rect.left,
            rect.bottom - rect.top,
        );
    if matched {
        FOUND.store(true, Ordering::Relaxed);
        // Stop the enumeration.
        return BOOL(0);
    }
    BOOL(1)
}
