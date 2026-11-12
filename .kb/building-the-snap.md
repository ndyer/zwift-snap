# Preface

How to build, install and iterate on the snap without paying for a full rebuild each
time. Read this before running a build from an agent — in particular the `tee`
requirement, which exists because a piped `tail` makes a live build look hung.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Build actions

Use the `build` action to build and the `install` action to install the result. The
Wine source the build needs is cloned for you by the workshop's `setup-project` hook
at launch, so `build` works on a fresh checkout; deleting `wine-src` makes the next
launch re-clone it and the next build a full Wine rebuild (see
`.workshop/.kb/workshop-usage.md`).

When running the `build` action (or any long-running action) from an agent,
pipe the output through `tee` instead of swallowing it with `tail`: a
piped `tail` hides the build's live progress (and makes it look hung), while
`tee` shows the output as it streams and keeps it in a file for later reading,
for example by appending `| tee build.log` to the appropriate command. Keep
`pipefail` in mind when chaining commands afterwards.

## Root's Rust toolchain

`build` makes sure root has a default Rust toolchain before snapcraft runs. The rust
plugin installs the toolchain its build step needs in the part's *pull* step, into
`$HOME/.rustup` — under `sudo` that is `/root/.rustup`, which the workshop recreates at
every launch, while the part's pull state under `/project/parts` does persist. Since
craft-parts then skips the pull, root can be left without a toolchain and the
`zwsnap-gui` build fails with `rustup could not choose a version of cargo to run,
because one wasn't specified explicitly, and no default is configured`.

The guard in the action uses the rust SDK's rustup (which exists from launch) rather
than the rustup snap (which snapcraft only installs while the build is running), and
installs a toolchain only on the first build after a launch. Any other snapcraft
invocation — the `snapcraft prime` of the fast iteration loop below, or a manual
`snapcraft pack` — needs that toolchain too and can hit the same error; run
`sudo /var/lib/workshop/sdk/rust/bin/rustup default stable` first in that case.

## Fast iteration with snap try

For launcher changes, avoid a full `build` on each edit. `snap try` mounts an
unpacked snap directory in place of the installed revision, so the launcher can
run from the prime tree:

1. Refresh the prime tree (rebuilds only changed parts, e.g. the launcher):
   `sudo snapcraft prime --destructive-mode`
2. Mount it: `sudo snap try /project/prime`. `snap list zwift` then shows the
   revision marked `try` (mounted from `/project/prime`).
3. Run it: `snap run zwift`. Check `~/snap/zwift/common/zwift.log` for the launcher's log.

Notes:

- Run `snap try` only after a successful `snapcraft prime`. An empty or missing
  prime tree fails with a missing `snap.yaml`.
- Running the dev build directly (`cargo run`) outside the snap fails with
  `No such file or directory (os error 2)` at `wine start`, because the wine
  binaries exist only inside the snap. Use `snap try` to test runtime behaviour.
- Dialogs and the game need a display: start the launcher from a session that
  has one. The game process keeps running after the launcher is closed.
- To undo `snap try`, install a `.snap` file over it with
  `sudo snap install --dangerous <file>.snap` (or `--amend`). This has not been
  verified against a `try` mount here. Avoid `sudo snap remove zwift` between
  iterations: it is slow, because the snap's multi-GB data is snapshotted. Do not
  use `--purge`, because it deletes the wine prefix and login state in
  `~/snap/zwift/common`.
- Do not `snap try` a directory that is not a prime tree (for example, the project root).

## ccache

The wine part compiles through ccache (`build-environment` in `snap/snapcraft.yaml`):
`PATH` is prepended with `/usr/local/lib/ccache`, where the workshop `setup-base` hook
symlinks ccache under every compiler name — host `gcc`/`g++` and the mingw cross
compilers — and `CCACHE_DIR` points at `/project/.ccache` so the cache is the mounted
workspace's (restored and saved by the edge workflow). Adding these properties re-pulls
the part and prunes its build objects once, so the first cache-enabled build is a full
Wine rebuild; later builds are incremental with cache hits. The cache is compressed and
capped at 4 GiB.
