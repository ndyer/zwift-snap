# Preface

House conventions across languages and files: how the launcher is written, where
user-visible messages go, and the comment style. Read this before adding a dialog
message or a new source-language file.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Conventions

- Launcher: Rust with gtk-rs GTK4; `cargo fmt`, `cargo clippy` and `cargo test`
  are included in the lint/test actions. GUI is optional: no display or
  `ZWIFT_NO_GUI=1` selects terminal operation. The shared `wine-env.sh` bootstrap
  remains bash because library paths must be set before loading the GUI binary.
- User-visible launcher messages go through the Rust logger and are written
  to `~/snap/zwift/common/zwift.log`; keep that logging intact when editing — the
  smoke test and users debug from it.
- YAML files are linted with yamllint relaxed, line length 140.
- Comments explain *why*; several constraints documented in this knowledge base are
  encoded as comments next to the code they protect — keep them in sync when you
  change behavior.
