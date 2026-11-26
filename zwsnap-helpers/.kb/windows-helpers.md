# Preface

The Windows binaries the snap ships and why they exist: the launcher shim the game
demands and the window-wait helper, plus how they are cross-compiled and kept in the
snap. Read this before changing `zwsnap-helpers/src/`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## What ships and why

The crate is cross-compiled to the Windows binaries the snap ships:

- `bin/ZwiftLauncher.exe` — the parent process `ZwiftApp.exe` demands.
- `bin/wait-for-window.exe` — reports when the game window is up.
- `bin/libusb0.dll` — the ANT+ shim that answers `ANT_DLL.dll`'s libusb-win32
  calls over the stick's serial device (compiled from `shim/libusb0.c` in C,
  not Rust).

The CDN manifest does not carry a `ZwiftLauncher.exe`, and `ZwiftApp.exe` only starts
when its parent's image path looks like Zwift's launcher, so the snap ships its own
shim (`src/shim.rs`) and copies it next to `ZwiftApp.exe` before a launch. Both
Rust helpers use Win32 rather than GTK: GDK can enumerate its own windows, not another
process's Wayland windows.

Zwift's ANT+ stack is `ANT_DLL.dll`, which is built against the libusb-win32 0.1
API — an API Wine has no driver for. Instead of bridging USB at the kernel level,
the snap ports `zwift-bootstrap`'s `libusb0.dll` shim, which implements just those
`usb_*` symbols and forwards them to the stick's Linux serial device
(`/dev/ttyUSB0`), and also hooks `CreateFileA/W` on the executable so nothing can
open the stick twice (Wine's `mountmgr` also surfaces it as a COM port, and
Zwift's serial probe opens every COM port it sees). See `shim/libusb0.c` for
`ANT_PORT` (device selection) and the baud/parity the stick expects.

## Cross-compilation and shipping

The crate is built by the `zwsnap-helpers` part, not snapcraft's Rust plugin: the
plugin builds for the host, while these must be Windows targets. The part uses
rustup's `x86_64-pc-windows-gnu` target. The two Rust binaries land in `bin/`.
`libusb0.dll` is compiled separately in the same part with `x86_64-w64-mingw32-gcc`
(the `gcc-mingw-w64-x86-64` build-package): with `-shared`, MinGW auto-exports the
non-static `usb_*` symbols, so no `.def` file is needed, `--kill-at` guards against
accidentally-decorated exports and `-static-libgcc` keeps libgcc_s_seh-1.dll out of
the game's search path. All three binaries are on `runtime-prune`'s must-exist list,
so removing one there fails the build rather than shipping a snap whose launcher
cannot start the game. The launcher copies `libusb0.dll` next to `ZwiftApp.exe` at
launch (the same `ensure_shim` path that copies `ZwiftLauncher.exe`), which is soon
enough because the game lazily loads the DLL on first ANT+ use.

## Waiting for the game window

`wait-for-window.exe PID` blocks until that process shows a real window, asking wine's
own window API so it behaves the same on X11 and Wayland. It reports exit `0` when the
window is visible, `2` when it times out, and `1` on any error the launcher should treat
as a failure. The timeout is 120 s (`TIMEOUT`, from `wait.rs`); the loop polls with a
100 ms `WaitForSingleObject`, not a sleep, so a game that dies without ever showing a
window is noticed at once.

The launcher's progress dialog stays open until this reports the first window (including
splash screens), rather than closing as soon as the process starts. Exit `2` is not fatal:
`launch.rs` logs `Timed out waiting for the Zwift window; leaving Zwift running` and lets
the game keep running; only exit `1` fails the launch.
