# Preface

The two release workflows: `release.yml` builds and verifies a tagged commit, `publish.yml`
uploads the result to the Store and creates the GitHub Release. Read this before changing
either, or before wondering why a tag did not reach the Store.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Why there are two workflows

They have different failure costs. Building and testing a tag takes hours (a cold Wine
build plus a Zwift download) and is safe to repeat; uploading a revision to the Store and
creating a Release is fast and is the irreversible part. Splitting them means a failed
upload is retried by dispatching `publish.yml` again against the build's artifact, without
rebuilding anything.

`release.yml` therefore produces an artifact and stops. `publish.yml` consumes that artifact
and does the publishing. The hand-off is the `zwift-snap` artifact, retained for 30 days.

This is the beta-promotion workflow `.kb/release-procedure.md` used to describe as future
work: a pushed `v<version>` tag is what promotes that commit to `latest/beta`.

## `release.yml`

Triggered by pushing a `v*` tag, or by `workflow_dispatch` with a `tag` input (for a
backfill or a re-validation). On a `ubuntu-24.04` runner it launches the Workshop as the
daily edge build does, then runs, in order: `lint`, `test`, `gtk-preview`, `inhibit`,
`build`, the version check, `install`, connecting `zwift:bluez`, `bluetooth`, `smoke` and
`pairing`. It shares the daily build's ccache key family (`ccache-ubuntu-26.04-`), so
release builds reuse and extend the same cache.

Unlike the edge build, it runs `smoke` and `pairing`. Both use `SMOKE_DISPLAY=auto` and a
runner has no host display (Workshop silently skips the non-auto-connectable `desktop`
plug), so both render on a virtual Mir output. `smoke` downloads Zwift on the cold prefix
(tens of minutes) and `pairing` then reuses that prefix, which is why `pairing` runs after
`smoke`. The smoke test's download cache lives in `$HOME` and is deliberately not shipped to
CI, so every release run pays for the download once.

## Prerequisites gate

Two checks run before the workshop launch, so a bad release fails in seconds rather than
hours:

- `.github/release-notes/<tag>.md` must exist **on `main`**. The notes are read from `main`
  in both workflows, never from the tag: a backfilled or re-validated old tag predates its
  notes file, and for a fresh release the notes commit is on `main` before the tag is made.
- The wine fork must have the tag `wine-<version>-zwiftsnap-<tag>` (for example
  `wine-11.21-zwiftsnap-v0.18`), checked with `git ls-remote` against a
  `wine-*-zwiftsnap-<tag>` pattern. The `wine-<version>-` prefix is mandatory: `wine
  --version` prints the `git describe` output verbatim, so any other prefix breaks its own
  reported version. Historically these tags were never pushed; from this pipeline onward
  they are part of the release.

## The Wine source

`setup-project` clones the wine fork's branch tip, which is not reproducible. After the
workshop launches, `release.yml` reads the Wine version from `/project/wine-src/VERSION`
and runs `workshop run -- wine-checkout wine-<version>-zwiftsnap-<tag>`, which fetches that
tag over HTTPS into `/project/wine-src` and checks it out detached (the clone's own origin
is SSH, which the workshop cannot use). It then asserts `git describe --tags` starts with
`wine-<version>-`, so a misnamed tag fails the release instead of shipping a bad Wine build
version. The wine part then builds from exactly the pushed source. The rest of the build is
the same as the edge build's.

## `publish.yml`

`workflow_dispatch` only, with inputs `tag` (required), `run-id` (optional, defaults to the
latest successful `release.yml` run for the tag) and `notes-only` (boolean). It runs in the
`latest/beta` GitHub environment and downloads the `zwift-snap` artifact from the resolved
run.

- **Store upload**: `snapcraft upload --release latest/beta`. Publishing needs
  `SNAPCRAFT_STORE_CREDENTIALS` in the `latest/beta` environment: the contents of a
  `snapcraft export-login` file. Create the environment and add the secret once; it can be
  the same credential file the edge build uses in `latest/edge`. When the secret is absent
  the upload is skipped with a notice and the Release is still created. `grade: devel` in
  `snap/snapcraft.yaml` is what makes `latest/beta` the right channel.
- **GitHub Release**: `gh release create` from the notes file, or `gh release edit` when the
  Release already exists, so a re-dispatch refreshes the body instead of failing. The snap
  is attached with `gh release upload --clobber`, skipped with a notice when it is over
  GitHub's 2 GiB asset limit.

The workflow is idempotent by construction, except the Store: a second upload of an
already-published version fails. Re-dispatch after a failure, not after a success.

## Backfill

`notes-only: true` skips the artifact download and the Store upload and only creates the
Release from the notes file. This is how the historical tags (`v0.8`-`v0.17`) get a Release
without a snap asset: their revisions predate CI and are already in the Store.

## Validating a change to these workflows

`yamllint` (as part of the `lint` action) covers both files. For the expressions, `uses:`
references and the embedded shell, run [actionlint](https://github.com/rhysd/actionlint) on
them; it is not installed in the workshop because it is a single Go binary fetched from
GitHub.