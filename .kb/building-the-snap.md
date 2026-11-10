# Preface

How to build, install and iterate on the snap without paying for a full rebuild each
time. Read this before running a build from an agent — in particular the `tee`
requirement, which exists because a piped `tail` makes a live build look hung.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Build actions

Use the `build` action to build and the `install` action to install the result.

When running the `build` action (or any long-running action) from an agent,
pipe the output through `tee` instead of swallowing it with `tail`: a
piped `tail` hides the build's live progress (and makes it look hung), while
`tee` shows the output as it streams and keeps it in a file for later reading,
for example by appending `| tee build.log` to the appropriate command. Keep
`pipefail` in mind when chaining commands afterwards.

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
