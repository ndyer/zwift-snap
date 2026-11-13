# Yarf smoke test

`workshop run smoke` starts a Mir compositor, runs the confined `zwift` Rust
launcher with `--skip-login` and drives this suite with yarf. It is
credential-free: the login gate is bypassed, so authenticated game startup is
tested manually (see the [pairing suite](../pairing/)).

The compositor renders on the host display through the workshop `desktop` interface
when one is available, otherwise on a virtual output. Nested rendering needs the
`desktop` and `gpu` interfaces connected to the host compositor: `SMOKE_DISPLAY=nested`
fails with the `workshop connect` commands to fix it when they are missing, while the
default `auto` falls back to the virtual output.

## Where to look

- [../.kb/smoke-test.md](../.kb/smoke-test.md) — how the suite works, the
  first-run cost, and how to read its diagnostics.
- [../.kb/dl-cache.md](../.kb/dl-cache.md) — the `~/zwift-cache` game download
  cache, the `dl-cache status`/`clear` actions, and `--fresh`.
