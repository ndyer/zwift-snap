# Preface

Where each component lives and which build trees must never be edited by hand.
Consult this before adding a new top-level directory or moving code between one.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Layout

```
zwsnap-gui/        Rust/gtk-rs launcher: lifecycle, logging, the CDN sync, and the Rust
                   unit/integration tests
zwsnap-helpers/    Rust crate cross-compiled to the Windows binaries the snap ships:
                   bin/ZwiftLauncher.exe (the parent process ZwiftApp.exe demands) and
                   bin/wait-for-window.exe
zwsnap-runtime/    shared wine-env.sh bootstrap and pulse-client.conf
snap/snapcraft.yaml     the snap definition (parts, prune filters, plugs, layouts)
snap/prune-closure.sh   DT_NEEDED closure walk that prunes unreachable libraries
tests/             unit tests (bats in tests/unit, stubs in tests/unit/stubs), the visual smoke
                   test (tests/smoke.sh + tests/smoke/), the bluetooth plumbing test
                   (tests/bluetooth.sh), the pairing-screen test (tests/pairing.sh +
                   tests/pairing/ + tests/zwift-stub.py)
  tools/bluetooth/   bluezoo mock tooling shared by both bluetooth tests: the helper bluezoo.sh,
                   the RSSI wrapper bluezoo-run.py, the scan client bluetooth-scan.py and the
                   BLE trainer mock bluetooth-trainer.py
  tools/yarf/        yarf tooling: the screenshot/input/OCR debugging tool yarf.sh (+ screenshot.py,
                   input.py, ocr_frame.py), the OCR thread cap yarf_ocr.py and the frame dump
                   yarf_frames.py used by the robot suites
  tools/dl-cache/    the smoke test's Zwift download cache helper dl-cache.sh, with the
                   tests/dl-cache.sh entrypoint for it one level up
  .workshop/         workshop definition (build environment) + in-project SDK zwift-snap-sdk
overlay/, prime/, stage/, parts/   build trees, gitignored, never edit by hand
```

## Apps

The snap has three apps: `zwift` (the launcher), `wine`, `winecfg`; the last two let
users run arbitrary wine commands. All share the command-chain
`gpu-2604-wrapper -> wine-env.sh`.

## Knowledge base layout

This repository organizes agent documentation as a Canonical knowledge base
(`.kb/*.md` files, indexed by `AGENTS.md`). See `.kb/agents.md` for the laws. Topic
files live next to the code they describe: `.workshop/.kb/`, `tests/.kb/`,
`snap/.kb/`, `zwsnap-gui/.kb/`, `zwsnap-helpers/.kb/` and `zwsnap-runtime/.kb/`.
