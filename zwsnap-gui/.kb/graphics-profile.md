# Preface

The GPU-profile override: what it selects, how it measures the GPU, how it stores and
restores Zwift's config files, and what was deliberately not adopted. Read this before
changing `graphics.rs` or `gpu_test.rs`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Graphics profile

The launcher can override Zwift's GPU table, which rates many cards below what they can
render. The `graphics_profile` setting in `settings.json` (loaded in `application.rs`,
applied in `graphics.rs`) selects `auto` (default), `basic`, `medium`, `high`, `ultra` or
`default`:

- `auto` measures the GPU at launch (`gpu_test.rs`, a port of the benchmark from
  [bpauli/zwift-bootstrap](https://github.com/bpauli/zwift-bootstrap), see the module doc for
  attribution) and maps the frame time to a profile with `MAX_FRAME_MS_FOR_60_FPS`.
- A fixed profile skips the measurement.
- `default` leaves Zwift's files untouched, so its own GPU table decides again.

The profiles are the game's `data/configs/*.txt`. The launcher writes the chosen profile into
*all four* files (so it does not matter which the game reads) and keeps the originals in
`~/snap/zwift/common/graphics/` plus an `applied` marker recording what it wrote. On the next
run a file whose content differs from the marker was replaced by an update and its saved
original is refreshed before the profile is applied again. `graphics.rs` has the unit tests;
`tests/entrypoint.rs` covers the end-to-end write/restore/validation paths.

`gpu_test.rs` uses bpauli's `glow` + `glutin_egl_sys` + `libloading` stack (not the
`khronos-egl` + `glow` combination that was first considered) so the published frame-time
thresholds stay calibrated against the same measurement. The GPU test needs EGL; without it
the launcher logs a warning and leaves the files to the game.

## Not adopted from bpauli/zwift-bootstrap

- ANT+ support: strict confinement has no `CAP_NET_ADMIN`, so the snap cannot open the raw
  sockets an ANT+ USB stick needs. Not implementable here.
- The 30–50 ms BLE connection interval: same `CAP_NET_ADMIN` constraint.
