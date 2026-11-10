# Preface

What this repository is, and what it deliberately is not. Read this first, so the
constraints it records — Zwift's own installer never running, a fork of wine built
from source, strict confinement — are not accidentally undone.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## What this project is

An unofficial snap launcher for [Zwift](https://zwift.com) for Linux. Zwift itself is
proprietary and NOT part of the snap — the Rust launcher downloads and updates it
into a wine prefix at runtime, straight from Zwift's CDN manifest
(`zwsnap-gui/src/{cdn,manifest,sync,download}.rs`). Zwift's own installer
(`ZwiftSetup.exe`) and patcher are never downloaded or run. Wine is built from source
by the snap's `wine` part from the local checkout `/project/wine-src`, a clone of
[ndyer/wine](https://github.com/ndyer/wine) (a fork of the official wine-mirror/wine
GitHub mirror) on its `zwift-radios-rebased` branch: wine-11.21 plus BLE support —
advertisement watcher, GATT, Windows.Devices.Radios; new-style WoW64, no i386 host
libs. Strict confinement, amd64 only, base `core26`.
