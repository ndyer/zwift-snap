# Preface

The daily edge build workflow (`.github/workflows/edge-build.yml`): what it runs,
what it skips and why, and the one secret it needs. Read this before changing the
workflow, or before wondering why a Store revision appeared on its own.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## What it does

`.github/workflows/edge-build.yml` runs daily at 03:17 UTC and on demand
(`workflow_dispatch`, with a `force` input). On a GitHub-hosted `ubuntu-24.04`
runner it launches the Workshop, builds the snap, runs the fast test suites inside
it and publishes the result to the Store's `latest/edge` channel. `grade: devel`
in `snap/snapcraft.yaml` is what makes `latest/edge` the only sensible target.

Publishing uses the `SNAPCRAFT_STORE_CREDENTIALS` secret from the `latest/edge`
GitHub environment: the contents of a `snapcraft export-login` file. Without it
the build and the tests still run, and Store publishing is skipped with a notice.

## The change gate

The `changed` job decides whether the expensive `edge` job runs at all. It asks
`gh run list --workflow edge-build.yml --branch main --status success --limit 1`
for the commit the last successful run built, and compares it with `github.sha`:
when they match, `main` has not moved and nothing is built. Both "no successful
run yet" and "every previous run failed" mean build, so a failed run is retried
the next day. `force: true` skips the gate.

This is what makes a daily schedule affordable: an idle day costs one cheap job
that only runs `gh`. It has no checkout, so `GH_REPO` is set to
`github.repository` explicitly; otherwise `gh` tries to infer the repository
from Git and fails.

## Which suites run, and which do not

Run: `lint`, `test` (bats unit tests and the launcher's cargo tests),
`gtk-preview`, `build`, `install`, then `bluetooth`.

Skipped: `smoke` and `pairing`. Both want Zwift downloaded (several GB) and a real
Mir compositor on the host display, and a headless runner has no display to give
them. The `desktop` interface cannot be auto-connected, and Workshop skips a
`connections:` entry that fails its interface policy *silently* — so the connection
declared in `.workshop/zwift-snap.yaml` simply does not happen on a runner, and the
launch still succeeds. `gtk-preview` is different and does run: it renders into its
own virtual Mir output and drives the GUI from a local CDN fixture, so it needs
neither a host display nor the network.

`bluetooth` needs the snap installed and `zwift:bluez` connected, which the
workflow does explicitly first, because snapd does not auto-connect that plug.

## What the workflow has to do by hand

`version-generator` derives the snap version from `git describe --tags`, so the
checkout uses `fetch-depth: 0`, and the workflow runs
`sudo git config --global --add safe.directory /project` because `git describe`
runs as root against a repository owned by another user: git refuses that with
"dubious ownership", and the version would silently become `unknown`. The "Check
the snap got a real version" step fails the run when the built snap is named
`zwift_unknown_*`, so that silent fallback stops the build instead of publishing
a revision nobody can identify.

The Wine clone and the agent skills are NOT fetched by the workflow itself. They
come from the `setup-project` hook of the workshop SDK, which runs during the
workshop launch (see `.workshop/.kb/workshop-usage.md`).

## The ccache cache

The `edge` job restores `.ccache` before building and saves it afterwards, via
`actions/cache/restore` and `actions/cache/save` (the producer side is the wine
part's `build-environment`; see `.kb/building-the-snap.md`). The key embeds
`github.run_id` so every run writes a fresh entry, while the `restore-keys` prefix
`ccache-ubuntu-26.04-` pulls back the newest; GitHub evicts the oldest entry once the
10 GiB repo quota is hit. Both the "make it readable" and the save step run under
`if: always()` so a failed build still banks the objects it compiled, and the save
needs `actions: write` (granted on the `edge` job); the chmod step first makes the
root-owned cache readable by the runner user.

## Validating a change to the workflow

`yamllint -d '{extends: relaxed, rules: {line-length: {max: 140}}}'
.github/workflows/edge-build.yml` is part of the `lint` action. For the
expressions, the `uses:` references and the embedded shell, run
[actionlint](https://github.com/rhysd/actionlint) on it; actionlint is not
installed in the Workshop because it is a single Go binary fetched from GitHub.
