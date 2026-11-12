# Preface

The shared runtime bootstrap all three snap apps pass through, why it is bash, and
the GTK runtime data it must keep available. Read this before editing
`zwsnap-runtime/wine-env.sh`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## The shared command chain

All three snap apps (`zwift`, `wine`, `winecfg`) share the command-chain
`gpu-2604-wrapper -> wine-env.sh`. `wine-env.sh` prepares Wine, GTK caches,
GStreamer and audio for every app.

## Why bash

The bootstrap remains bash because library paths must be set before loading the GTK
binary: the dynamic loader resolves them before `zwsnap-gui` starts, so a Rust
bootstrap could not set them in time. `pulse-client.conf` supplies the audio client
configuration.

## GTK data at runtime

`wine-env.sh` generates the gdk-pixbuf loaders cache and the mime cache per snap
revision, and glib schemas are compiled at build time (`glib-compile-schemas` in
`runtime-prune`'s override-prime). Keep these runtime data available for GTK.

## Library search path

`libs` is exported as `LD_LIBRARY_PATH` because some Ubuntu libraries find their
private helper libraries through an *absolute* RUNPATH, for example
`/usr/lib/x86_64-linux-gnu/pulseaudio`. Inside the confinement that absolute path
resolves to the base snap, not to `$SNAP`, so unless the directory is on the search
path the loader never finds the helper sitting next to the library:

- `libpulse` needs `libpulsecommon`; without it wine has no audio
- `ntlm_auth` (needed by zwift/wine) and its samba modules
- gio's `libgiolibproxy.so` needs `libproxy.so.1`, which needs
  `libpxbackend-1.0.so` from the same private directory

A staged library that grows a new absolute RUNPATH belongs in `libs` here, not in
`snapcraft.yaml`: `libpxbackend-1.0.so` was staged all along — `libproxy1v5` ships
it, and `prune-closure.sh` keeps the directory as a dlopen root — the loader just
never looked in `usr/lib/x86_64-linux-gnu/libproxy`. The symptom of getting this
wrong is a per-start `libpxbackend-1.0.so: cannot open shared object file` followed
by `Failed to load module: .../gio/modules/libgiolibproxy.so`, not a build failure.
To audit the staged tree for further cases, collect the RUNPATHs of every primed
library (`find prime -type f -name '*.so*' -print0 | xargs -0 readelf -d`): the
pulseaudio and libproxy directories are the only absolute ones.
