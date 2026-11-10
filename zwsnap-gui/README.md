# Rust Zwift launcher

`zwsnap-gui` is the snap's default `zwift` app: a GTK4 (gtk-rs) Rust program that
owns installing and updating Zwift, display preferences, process supervision,
cancellation and logging.

## Where to look

- [.kb/launcher-architecture.md](.kb/launcher-architecture.md) — what the binary
  owns: download and sync, the window and display mode, the WebKit login panel,
  logging and cancellation, the shared runtime bootstrap and the Windows helpers.
- [.kb/launcher-development.md](.kb/launcher-development.md) — development
  toolchain, build ordering, the `lint`/`test`/`gtk-preview`/`build` actions, what
  each test covers, and the command line.
- [.kb/login-and-remember-me.md](.kb/login-and-remember-me.md) — the OAuth login
  flow, the token file, the remember-me cookie, and the zwift-offline reference.
- [.kb/graphics-profile.md](.kb/graphics-profile.md) — the GPU profile override and
  the GPU benchmark, and what was deliberately not adopted from
  bpauli/zwift-bootstrap.

Sibling crates: [../zwsnap-helpers/](../zwsnap-helpers/) builds the Windows helpers
this crate runs, and [../zwsnap-runtime/](../zwsnap-runtime/) holds the shared
`wine-env.sh` command-chain bootstrap. The repository knowledge base is indexed in
[../AGENTS.md](../AGENTS.md).
