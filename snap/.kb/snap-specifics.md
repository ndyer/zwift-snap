# Preface

The snap packaging details that are easy to break: pruning and its guards, build
ordering, stage-package auditing, the local Wine source, winetricks invocation, and
the confinement/layout decisions. Read the relevant subsection before editing
`snap/snapcraft.yaml` or `snap/prune-closure.sh`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Pruning

The wine part aggressively prunes (man pages, locales, wine's build
toolchain, ~776 static import libraries, plus everything unreachable via the
`snap/prune-closure.sh` closure walk). Guards in `runtime-prune`'s `override-prime` fail
the build on over-pruning (runtime file missing) or a stale prime directory. Bump the
`v<N>` comment there when editing `prune-closure.sh`, otherwise craft-parts will not
re-run the prime step.

## Stale prime after removing a part

Both guards above say to run the `clean` action, but that reinstalls the build environment and rebuilds Wine from scratch
(tens of minutes). A part deleted from `snapcraft.yaml` leaves
its output in `prime/` and `stage/` and its tree in `parts/` forever, because
snapcraft cannot clean a part it no longer knows about, and every later build then
fails the stale-prime guard with e.g.
`prune guard: bin/runfromprocess-rs.exe is a build-time-only file and must not be primed`.
Delete the orphaned files and the orphaned part directory by hand instead
(`sudo rm -f prime/bin/<file> stage/bin/<file>; sudo rm -rf parts/<part>`) and
rebuild: only the later parts restage, Wine is skipped. Check with
`unsquashfs -l <snap> | grep <name>` that nothing stale ships.

## Build ordering

`runtime-prune` depends on Wine, the Rust launcher, the runtime helpers and the Windows helpers; GPU cleanup follows it. Never make Wine depend on the
launcher just to order pruning: this invalidates Wine's build on every launcher change.

## Windows binaries

`zwsnap-helpers/` is cross-compiled by the `zwsnap-helpers`
part (rustup's `x86_64-pc-windows-gnu` target, not snapcraft's Rust plugin, which
builds for the host). The binaries land in `bin/` and are on `runtime-prune`'s
must-exist list, so removing one there fails the build instead of shipping a snap
whose launcher cannot start the game.

## Do not prune

Do not prune `usr/lib/*/samba` (ntlm_auth needs it) and the nss libraries
listed in the `lint.ignore` block of `snap/snapcraft.yaml` (dlopened by name).

## Stage only what ships

A stage-package whose files are not in the final prime
tree is dead weight, and the two mechanisms that delete files make this easy to get
wrong. The `gpu-2604` part runs `gpu-2604-cleanup mesa-2604` after `runtime-prune` and
deletes everything the Mesa content snap provides, which includes the X11 and Wayland
client libraries (`lists/mesa-2604.amd64.list`), so `libwayland-client0`,
`libwayland-egl1`, `libx11-6`, `libxext6`, `libxfixes3` and `libxxf86vm1` are
deliberately *not* staged: wine's `winex11.so`/`winewayland.so` resolve them from
`/snap/zwift/current/gpu-2604/mesa-2604` at runtime, which needs the `gpu-2604` plug
connected (an absent `libX11.so.6` in `prime/` is therefore expected, not a pruning
bug). `libxkbcommon0`/`libxkbregistry0` are not in that list and must stay staged.
`prune-closure.sh` deletes the rest of the unreachable closure (e.g. `libGLU`, and
`libsdl2-2.0-0`, whose only copy lands in a private `sdl2-classic/` directory that is
not on the library search path), so check a package's files in `prime/` before adding
it. Audit per package against `prime/`, not `dpkg -L`: craft-parts extracts
stage-packages into a part tree without registering them with the host's dpkg, so
`dpkg -L` cannot enumerate 14 of them here.

## Wine source

The `wine` part builds the local checkout `/project/wine-src`
(`source: /project/wine-src`, `source-type: local`), a clone of
[ndyer/wine](https://github.com/ndyer/wine) (fork of wine-mirror/wine) on its
`zwift-radios-rebased` branch. Commit and push there first: the snap builds whatever
that working tree holds, dirty or not. Local sources are copied into
`parts/wine/build` (`config.status` has `srcdir = .`) while `parts/wine/src`
hardlinks `/project/wine-src`, so editing `wine-src/` does *not* change the copies
the build tree compiles from. To compile-check a Wine patch in minutes, copy the
changed file into the matching `parts/wine/build` path and run
`sudo make -C parts/wine/build/dlls/<dir>` (`make -C dlls/<dir> unix` does not exist).
Any repull of the part — for example when a property such as `stage-packages`
changes — prunes files it does not find in the source, which deletes every object
file and `.so` in `parts/wine/build` and silently turns the next build into a full
Wine rebuild rather than an incremental one. Confirm with
`find parts/wine/build -name '*.o' | wc -l` before starting a build you expect to be
quick. A full rebuild costs tens of minutes, not hours: `make -j22` on this
22-core machine compiles roughly 1000 objects per minute, so a from-scratch build
is done well inside an hour. Do not put off a Wine fix over the build time.

## Wine runtime

New-style WoW64 has no `wine64` binary or i386 Unix packages;
32-bit Windows programs use `lib/wine/i386-windows`. Check the winetricks steps
(`d3dcompiler_47`, `renderer=vulkan`) on a desktop. The runtime is not a
separate content snap: existing core24 Wine runtime libraries do not match this
build, and splitting it adds a snap without reducing the total download.

## winetricks invocation

The launcher installs both prerequisites with one
`winetricks -q d3dcompiler_47 renderer=vulkan` call, not one
call per verb: winetricks runs every command-line verb in a single process, in
order, and exits non-zero on the first failure, so this is faster and still
stops the install before Zwift is downloaded. The per-verb dialog titles are
read from the `Executing w_do_call <verb>` lines winetricks prints to stderr
(`WINETRICKS_VERB_MARKER` in `zwsnap-gui/src/install.rs`), so the pinned
winetricks and the launcher must change together if that output changes.
`.NET Framework 4.8` (`dotnet48`) and `WebView2` (`webview2`) are not installed:
they existed for Zwift's own `ZwiftLauncher.exe` (a .NET launcher hosting a
WebView2 UI), which the launcher shim replaces and nothing runs any more. The
game is a native 64-bit binary and needs neither.

## Bluetooth

The `zwift-radios-rebased` wine branch implements the BLE
advertisement watcher (`adv_watcher_Start` in
`dlls/windows.devices.bluetooth/advertisement.c`, LE-only discovery) and GATT on top
of winebth. Upstream wine 11.21 stubs the watcher with `E_NOTIMPL`, so check this
when moving the part to another branch or tag.

## pkgproxy

pkgproxy caches apt and snap downloads in the workshop; the base snap,
https sources and the wine git clone are not cached. The `stats` action shows
the savings.

## Confinement and layouts

- **No `home` plug**: wine user folders live inside the wine prefix
  (`~/snap/zwift/common/.wine/drive_c/users/<user>/`); the launcher replaces links
  to the real home with private directories at every start.
- **No SVG pixbuf loader / no gecko/mono**: dialogs use a png icon, mshtml/mscoree
  are disabled at wineboot (`WINEDLLOVERRIDES`).
- **glycin layouts**: `layout:` in snapcraft.yaml bind-mounts the glycin loader
  directories because GTK image loading uses absolute paths from its config.
- **gtk data at runtime**: `wine-env.sh` generates the gdk-pixbuf loaders cache and
  the mime cache per snap revision, and compiles glib schemas at build time
  (`glib-compile-schemas` in override-prime). Keep these runtime data available for GTK.
