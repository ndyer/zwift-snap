# Preface

How this snap relates to the other popular ways of running Zwift on Linux, and how
to keep the `Alternatives` table in `README.md` from drifting out of date. Factual
comparison lives here so the README stays a compact snapshot.

IMPORTANT: Read the `.kb/agents.md` file.


# Laws

## §1 - Keep the README comparison current

- The `Alternatives` section of `README.md` is a **snapshot**, not a source of truth.
  Keep the rich facts and the update procedure here, in `alternatives-comparison.md`.
- Re-verify the comparison against the live repositories each time a release is cut,
  or when a project listed below changes packaging, Wine strategy or sensor support.
- When the facts change, edit the README table and the `## The four projects`
  subsections here together, in the same change.
- If a repository disappears, is renamed or libels this project, remove or correct it
  rather than keeping a stale row. Do not editorialise against competitors; describe
  behaviour neutrally.

## §2 - How to re-check a project

- The README of each project is the first source. Re-read it directly:

  - https://github.com/netbrain/zwift
  - https://github.com/bpauli/zwift-bootstrap
  - https://github.com/linuxundich/tretlager

- Confirm, for each, the current: packaging/distribution channel, implementation
  language, Wine strategy (bundled, system, built from source, patched), how game
  files are obtained (bundled, Windows installer, CDN manifest), whether it needs the
  Windows launcher + .NET/WebView2, and how sensors (Bluetooth, ANT+) reach the game.
- Note the commit count / star count / latest commit date only as a rough maturity
  signal, and only if it materially changes the comparison.


# Concepts

## The four projects

- **netbrain/zwift** — A Docker/Podman image with Zwift pre-installed. Sensors go
  through the Zwift Companion app; ANT+ is not supported that way. Motivation was
  running several Zwift instances on one machine. Most mature of the group.
- **bpauli/zwift-bootstrap** — A native Rust CLI (no sandbox). Its defining trait,
  shared with this snap, is installing/updating the game directly from the Zwift CDN
  and skipping the Windows launcher. Uses system wine-staging, bridges BLE through
  BlueZ from the game's pairing screen, and supports ANT+ (via the USB stick's serial
  device).
- **linuxundich/tretlager** — A Flatpak with a guided-setup assistant UI (PySide6 +
  Kirigami). Runs the official installer and handles .NET/WebView2 itself, signs in
  outside the launcher via Zwift's web page + desktop keyring, and ships a local BLE
  sensor bridge speaking Wahoo's Direct Connect (DIRCON) protocol.
- **This snap** — Strict-confinement snap on `core26`, amd64 only. Rust GTK4 launcher.
  Fetches the game from the Zwift CDN manifest (never the installer), builds Wine from
  the `ndyer/wine` fork with BLE support so the game talks to sensors directly, and
  supports ANT+ via a `serial-port` plug against snapd's hotplug slot for the USB stick.

## Dimensions that matter

- **Game files**: bundled image vs Windows installer vs CDN manifest. The CDN route
  (this snap, zwift-bootstrap) avoids the fragile .NET/WebView2 install chain entirely.
- **Wine**: stock (netbrain, tretlager-built, zwift-bootstrap uses wine-staging) vs
  patched (this snap adds BLE GATT / Windows.Devices.Radios).
- **Sensors**: Companion-app bridge, BlueZ-direct, an external BLE bridge (DIRCON), or
  patched Wine. ANT+ exists in zwift-bootstrap and in this snap (via a `serial-port`
  plug against snapd's hotplug slot for the USB stick); netbrain and tretlager do not
  support it.
- **Sandbox**: none (zwift-bootstrap), container (netbrain), Flatpak (tretlager), strict
  snap (this project — the tightest, and the reason for the `--devmode`/`snappy-debug`
  troubleshooting path).