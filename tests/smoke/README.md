# Yarf smoke test

`workshop run smoke` starts a Mir compositor, runs the confined `zwift` Rust
launcher and drives this suite with yarf. It checks the GTK log button while
installation is in progress, then requires the launcher's `Login to Zwift`
button and mandatory-login message. The game cannot start until OAuth login
succeeds, so this credential-free suite stops at that gate. Authenticated game
startup is tested manually; the pairing suite exercises the game with fake servers.

The first run installs Wine prerequisites and downloads Zwift, taking tens of
minutes. Later runs reuse the prefix. After a rebuild, purge and reinstall when
testing a fresh prefix; the harness never installs or removes the snap itself.

Diagnostics are under `~/zwift-smoke-out/`; the launcher writes
`~/snap/zwift/common/zwift.log`. Output goes through `tee` because snap commands
cannot reliably start with stdout redirected to regular files in this environment.

OCR runs sequentially and does not queue frames. If the test keeps waiting while
the expected screen is visible, compare a fresh `workshop run yarf screenshot`
capture with yarf's diagnostics; a separate capture succeeding does not establish
that the running yarf client's Wayland connection is healthy.
