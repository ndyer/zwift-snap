# Preface

The smoke test's download cache: what it holds, why it lives in `$HOME`, and the
`dl-cache` action for restoring a prefix by hand. Read this before changing
`tests/smoke.sh`'s cache handling or wondering why winetricks downloads are never
captured.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Zwift download cache (smoke test)

The smoke test's slow parts are downloads, and they are unchanged between runs, so
`tests/smoke.sh` keeps them in `~/zwift-cache` (test tooling only — the launcher never
reads or writes it, and nothing of it ships):

- `game/` — the whole `drive_c/Program Files (x86)/Zwift` directory of the wine
  prefix: what the launcher's CDN sync writes there. Restoring it satisfies the
  launcher's `installed()` check, so a staged prefix launches without installing
  anything.
- `winetricks/` — winetricks' own download cache, its `W_CACHE` directory.
- `meta.json` — the Zwift `sversion` and capture time, so staleness is visible.

`smoke.sh` restores the cache into the prefix before starting and captures it again
after a passing run; `--fresh` or `SMOKE_CACHE=0` (defined in `smoke.sh`, never
forwarded to yarf) skips both and wipes what was staged. The `dl-cache` action
(`tests/dl-cache.sh`) exposes `status`, `capture [--force]`, `stage [--force]` and
`clear` for inspecting the cache or restoring a prefix by hand after
`sudo snap remove --purge zwift`. `tests/tools/dl-cache/dl-cache.sh` holds the
implementation and `tests/unit/dl-cache.bats` its tests.

Two details it depends on, both verified live:

- The cache lives in `$HOME`, not under `~/snap/zwift`, because
  `snap remove --purge` deletes the latter — the case the cache exists for.
- `smoke.sh` exports `W_CACHE` (`~/snap/zwift/common/winetricks-cache`) before `snap
  run` (the snap sees that absolute path unchanged). Without it the launcher's
  cleanup deletes the default `$XDG_CACHE_HOME/winetricks` every run, so winetricks'
  downloads can never be captured. `cp -a` everywhere, so a restored tree is
  indistinguishable from the captured one.

## What is and is not cached

Only the game directory is cached, not the whole prefix: winetricks prerequisites
live elsewhere in `drive_c` and are detected separately (`installed()` checks their
proof paths), so a restored game tree in a prefix without them still reinstalls
them. The cache is valid for the Zwift version in `meta.json`; a version change is
left to the launcher's sync, which diffs the installed files against the CDN
manifest and fetches only the difference.

Because `installed()` also requires the winetricks prerequisite proofs, a restored
game tree alone does not satisfy it: the run takes the `install()` path and
reinstalls the prerequisites. That path re-checks the version before downloading,
so a staged tree already at the latest version is recognised and the game download
is skipped. `--fresh` (or a prefix purged without the cache) is what still covers
the full download and installation.
