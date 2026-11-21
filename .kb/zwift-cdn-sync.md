# Preface

How the launcher fetches and updates Zwift straight from Zwift's CDN, and why no
file Zwift ships is ever executed. Read this before touching the sync modules in
`zwsnap-gui/src/`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Zwift download and update

The launcher fetches the game itself, straight from Zwift's CDN. Nothing Zwift ships is
executed as an installer: `ZwiftSetup.exe` is never downloaded and the patcher never runs.
Four modules split the work:

- `manifest.rs` — the XML: `Zwift_ver_cur.xml` (the current version, with the sync-key
  `version`, the human-facing `sversion` and the manifest name) and the manifest
  (`<path>`, `<length>`, `<modified>`, `<checksum>`). Checksums are signed 32-bit
  decimals, where `-1` means "absent".
- `cdn.rs` — the transfer layer: URLs built from the base
  (`DEFAULT_BASE`, `https://cdn.zwift.com/gameassets/Zwift_Updates_Root/`), retries with
  backoff, and the `Fetcher` trait, whose `file://` implementation lets the tests and
  offline checks run without the network.
- `download.rs` — a worker pool over a shared queue. A file is fetched into the staging
  directory and renamed into place only after its length and checksum verified, so an
  interrupted run leaves no truncated file that a later diff would consider present, and
  at most `jobs` files are staged at once.
- `sync.rs` — `assess`/`diff`/`sync`: compare the installed version against the served
  one, then download only the files whose length or checksum differ. An update is
  therefore a delta, and a current install transfers nothing.

`install.rs` (fresh prefix) and `update.rs` (existing prefix) drive the sync; the version
the launcher records is the server's own `Zwift_ver_cur.xml`, written after every sync
(`version.rs`).

Two version numbers travel together and are easy to confuse. The sync key is the
`version` attribute (`major.minor.build`, for example `1.0.168455`) — `Version`, the
counter the CDN uses to decide whether an update exists. The human-facing number is the
leading token of `sversion` (for example `1.126.0`), which the game shows as *Game
Version*, with `version.build` as its *Build Number*; `manifest.rs` keeps it on `VerCur`
and `VerCur::display_version` renders the pair as `1.126.0 (build 168455)` (the bare build
key when a ver_cur carries no `sversion`, such as the installer's `0.0.0` seed). The sync
log lines print that form — the same two fields the game shows on its About screen —
while the version files on disk and the smoke test's cache keep the raw `sversion`.

Two environment variables, both parsed in `application.rs`, configure it:
`ZWIFT_CDN_BASE` overrides the CDN root (a `file://` directory for tests and offline
checks) and `ZWIFT_DOWNLOAD_JOBS` caps the number of parallel transfers (default 32).

The transfer count is also adaptive by default: unless `download_adapt` is turned off in
`settings.json`, `download.rs` starts below the `ZWIFT_DOWNLOAD_JOBS` ceiling and grows the
pool by one while throughput keeps improving, so the count settles where the link
saturates rather than being guessed up front. It measures throughput as a *rate* over the
real elapsed time of the sampling window (not raw bytes, which vary with a stalled window),
smooths it with an exponential moving average, and only trims the pool back after a
sustained run of declines (four windows in a row each down at least a tenth, and then by a
quarter, not a half) so one noisy patch cannot ratchet the count down toward one. A window
is skipped after any change so the next reading reflects it, and near the end of the
transfer a tail guard stops adapting once the remaining bytes are less than about two
windows' payload. Each change is logged with the current rate in MB/s, and the workers'
per-file ticks repeat the last measured rate so the GUI's progress bar keeps showing it.

Because a window's throughput depends on the sizes in flight, and `diff` returns the plan
in manifest (path) order, which clusters similar-sized files together, `sync.rs` reorders
the plan before transferring it (`order_for_download`): it sorts by size and then
interleaves the largest and smallest remaining files. Every window of the transfer then
spans the whole size range, so the rate the controller reads tracks the link rather than
drift from the plan's shape — without the interleave, the run reaches a few large files
first and a long tail of tiny ones last, the rate falls as the small files take over, and
the controller trims the pool exactly when the many small, latency-bound files want more
transfers, not fewer.

## The launcher shim

The CDN manifest does not carry a `ZwiftLauncher.exe`, and `ZwiftApp.exe` only starts when
its parent's image path looks like Zwift's launcher, so the snap ships its own shim
(`zwsnap-helpers/src/shim.rs`) and copies it next to `ZwiftApp.exe` before a launch.
