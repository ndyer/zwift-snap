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
