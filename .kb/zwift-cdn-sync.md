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

- `manifest.rs` — the XML: `Zwift_ver_cur.xml` (the current version, with `sversion` and
  the manifest name) and the manifest (`<path>`, `<length>`, `<modified>`, `<checksum>`).
  Checksums are signed 32-bit decimals, where `-1` means "absent".
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

Two environment variables, both parsed in `application.rs`, configure it:
`ZWIFT_CDN_BASE` overrides the CDN root (a `file://` directory for tests and offline
checks) and `ZWIFT_DOWNLOAD_JOBS` sets the number of parallel transfers.

## The launcher shim

The CDN manifest does not carry a `ZwiftLauncher.exe`, and `ZwiftApp.exe` only starts when
its parent's image path looks like Zwift's launcher, so the snap ships its own shim
(`zwsnap-helpers/src/shim.rs`) and copies it next to `ZwiftApp.exe` before a launch.
