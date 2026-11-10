//! `ZwiftLauncher.exe`: `ZwiftLauncher.exe GAME.exe ARGS...` starts
//! `GAME.exe ARGS...` as its child and waits for it.
//!
//! ZwiftApp.exe reads the image path of its parent process and refuses to run
//! unless that path contains "ZwiftLauncher.exe", "ZwiftHelper.exe" or
//! "devenv.exe": otherwise the game starts ZwiftLauncher.exe itself and exits,
//! or shows "Could not run Update process" (ERROR ZA1). Being *named*
//! ZwiftLauncher.exe is what satisfies the check, so the snap ships this
//! instead of Zwift's own launcher. That launcher only exists to run the
//! patcher, which the Rust launcher replaces with a manifest-based CDN sync.
//!
//! The alternative, spoofing the parent of ZwiftApp.exe with a
//! `PROC_THREAD_ATTRIBUTE_PARENT_PROCESS` attribute, needs a live
//! ZwiftLauncher.exe process to point at. Nothing creates that any more, so
//! this binary is the parent itself and stays alive for as long as the game
//! runs.
//!
//! The child's command line is rebuilt from this process's argv rather than
//! copied from `GetCommandLineW`: `wine start` re-quotes each argument, so the
//! raw line carries the `--token` JSON escaped, which the game cannot parse. See
//! [`crate::command_line`].
//!
//! Derived from bpauli/zwift-bootstrap (MIT OR Apache-2.0), which established
//! the parent-image check and this remedy.

use std::mem::{size_of, zeroed};

use windows::{
    core::{PCWSTR, PWSTR},
    Win32::{
        Foundation::CloseHandle,
        Storage::FileSystem::SearchPathW,
        System::Threading::{
            CreateProcessW, GetExitCodeProcess, WaitForSingleObject, INFINITE,
            PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, STARTUPINFOW,
        },
    },
};

use crate::command_line;

/// `SearchPathW` takes an explicit buffer length; the paths inside the prefix
/// are short, so this is only headroom against a silent truncation.
const PATH_BUFFER: usize = 1024;

/// Distinct non-zero exit codes, as in bpauli's shim, so a failure stays
/// diagnosable from the process exit status alone.
const EXIT_BAD_ARGS: i32 = 2;
const EXIT_NOT_FOUND: i32 = 3;
const EXIT_CANNOT_START: i32 = 4;

/// Run the child and return its exit code.
pub fn run() -> i32 {
    let Some((name, args)) = child_arguments() else {
        return EXIT_BAD_ARGS;
    };
    if name.len() >= PATH_BUFFER {
        return EXIT_BAD_ARGS;
    }
    let wide_name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();

    let mut exe = [0u16; PATH_BUFFER];
    // Wine leaves the current directory out of CreateProcessW's search path
    // here, so the child has to be resolved explicitly.
    let length = unsafe {
        SearchPathW(
            None,
            PCWSTR::from_raw(wide_name.as_ptr()),
            None,
            Some(&mut exe),
            None,
        )
    };
    if length == 0 || length as usize > exe.len() {
        return EXIT_NOT_FOUND;
    }

    let mut startup: STARTUPINFOW = unsafe { zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    let mut process: PROCESS_INFORMATION = unsafe { zeroed() };
    // CreateProcessW is allowed to rewrite the command line buffer in place,
    // which is fine: it is ours.
    let mut command: Vec<u16> = command_line::child(&name, &args)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let created = unsafe {
        CreateProcessW(
            PCWSTR::from_raw(exe.as_ptr()),
            PWSTR::from_raw(command.as_mut_ptr()),
            None,
            None,
            false,
            PROCESS_CREATION_FLAGS(0),
            None,
            None,
            &startup,
            &mut process,
        )
    };
    if created.is_err() {
        return EXIT_CANNOT_START;
    }

    // Stay alive for as long as the game does: this process is its parent, and
    // the game may re-check that.
    let mut code = 1u32;
    unsafe {
        WaitForSingleObject(process.hProcess, INFINITE);
        GetExitCodeProcess(process.hProcess, &mut code).ok();
        CloseHandle(process.hThread).ok();
        CloseHandle(process.hProcess).ok();
    }
    code as i32
}

/// This process's own image name and the child's command line, from argv.
///
/// `argv[0]` is our image and `argv[1]` the child's; the rest are the child's
/// arguments. Non-UTF-8 arguments are refused rather than mangled: wine derives
/// them from the launcher's own UTF-8 argv, so they can only change shape if
/// something upstream did.
fn child_arguments() -> Option<(String, Vec<String>)> {
    let mut arguments = std::env::args_os().skip(1);
    let name = arguments.next()?.into_string().ok()?;
    if name.is_empty() {
        return None;
    }
    let args = arguments
        .map(|argument| argument.into_string().ok())
        .collect::<Option<Vec<_>>>()?;
    Some((name, args))
}
