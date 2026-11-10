# Yarf smoke test

`workshop run smoke` starts a Mir compositor, runs the confined `zwift` Rust
launcher and drives this suite with yarf. It is credential-free: it stops at the
mandatory login gate, so authenticated game startup is tested manually (see the
[pairing suite](../pairing/)).

## Where to look

- [../.kb/smoke-test.md](../.kb/smoke-test.md) — how the suite works, the
  first-run cost, and how to read its diagnostics.
- [../.kb/dl-cache.md](../.kb/dl-cache.md) — the `~/zwift-cache` game and
  winetricks download cache, the `dl-cache status`/`clear` actions, and `--fresh`.
