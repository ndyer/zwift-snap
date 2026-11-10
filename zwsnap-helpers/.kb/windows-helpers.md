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

The CDN manifest does not carry a `ZwiftLauncher.exe`, and `ZwiftApp.exe` only starts
when its parent's image path looks like Zwift's launcher, so the snap ships its own
shim (`src/shim.rs`) and copies it next to `ZwiftApp.exe` before a launch. Both
helpers use Win32 rather than GTK: GDK can enumerate its own windows, not another
process's Wayland windows.

## Cross-compilation and shipping

The crate is built by the `zwsnap-helpers` part, not snapcraft's Rust plugin: the
plugin builds for the host, while these must be Windows targets. The part uses
rustup's `x86_64-pc-windows-gnu` target. The binaries land in `bin/` and are on
`runtime-prune`'s must-exist list, so removing one there fails the build rather
than shipping a snap whose launcher cannot start the game.
