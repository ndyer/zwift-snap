# Yarf smoke test

`workshop run smoke` starts a Mir compositor, runs the confined `zwift` Rust
launcher and drives this suite with yarf. It checks the GTK log button while
installation is in progress, then requires the launcher's login-gate message
(the always-visible web panel switches to the sign-in page there). The game
cannot start until OAuth login
succeeds, so this credential-free suite stops at that gate. Authenticated game
startup is tested manually; the pairing suite exercises the game with fake servers.

The first run installs Wine prerequisites and downloads Zwift, taking tens of
minutes. Later runs reuse the prefix. After a rebuild, purge and reinstall when
testing a fresh prefix; the harness never installs or removes the snap itself.

The downloads are also kept in a cache under `~/zwift-cache`: the harness restores
it into the wine prefix before starting and updates it after a passing run, so a
purged prefix (or `sudo snap remove --purge zwift`, which deletes it) does not cost
another multi-GB download. A restored game tree satisfies the launcher's
installation check, so a warm run starts the game instead of re-downloading it.
The cache holds the game tree and winetricks' downloads for one specific Zwift
version; `workshop run dl-cache status` shows which, and
`workshop run dl-cache clear` throws it away. Pass `--fresh` (or `SMOKE_CACHE=0`)
to ignore the cache and wipe what it staged, for a real cold install. `--fresh` is
handled by `tests/smoke.sh` and never reaches yarf.

Diagnostics are under `~/zwift-smoke-out/`; the launcher writes
`~/snap/zwift/common/zwift.log`. Output goes through `tee` because snap commands
cannot reliably start with stdout redirected to regular files in this environment.

OCR runs sequentially and does not queue frames. If the test keeps waiting while
the expected screen is visible, compare a fresh `workshop run yarf screenshot`
capture with yarf's diagnostics; a separate capture succeeding does not establish
that the running yarf client's Wayland connection is healthy.
