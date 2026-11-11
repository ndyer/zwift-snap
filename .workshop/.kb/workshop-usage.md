# Preface

How this repository uses the Workshop environment: which action runs what, where the
SDK lives, and the `project-` prefix subtlety that trips up references. Read this
before adding an action or a connection.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Running actions

All building and testing happens in a [Workshop](https://ubuntu.com/workshop/docs/)
environment. Use `workshop run -- <action> [arguments...]`; the `--` disambiguates
the action from the workshop name when they overlap. `workshop --help` lists actions.
The local runner's header at `.workshop/zwift-snap-sdk/workshop` documents its
behavior and limitations.

There is no `setup` action: snapcraft, shellcheck, yamllint, bats, bluez, bluezoo and
the pkgproxy cache are installed by the `setup-base` hook of the `zwift-snap-sdk` SDK
at launch and refresh (see `.workshop/zwift-snap-sdk/hooks/setup-base`).

The `setup-project` hook (same directory) does the project-side setup at every launch
and applied refresh: it installs the runner, fetches the agent skills, and runs
`.workshop/zwift-snap-sdk/fetch-wine.sh` to clone the Wine source the snap builds from
into `/project/wine-src`. That clone is deliberately fatal on failure, unlike the
skills fetch which only warns: without the Wine source nothing can be built or tested,
so failing the change beats leaving a workshop that looks ready and is not. It is a
no-op whenever `wine-src` already exists, so a local tree is never overwritten; delete
the directory to force a fresh clone. `ZWIFT_WINE_REPO`, `ZWIFT_WINE_REF`,
`ZWIFT_WINE_DEPTH` and `ZWIFT_WINE_SRC` override the repository, the ref, the clone
depth (0 for the full single-branch history, which the release procedure's wine
tagging needs) and the target.

## The Use Workshop skill

The canonical agent documentation for operating Workshop is the `use-workshop` skill
(`onboard-workshop` and `design-sdk` are its siblings) under `.github/skills/`. It is not
vendored: the `setup-project` hook of the `zwift-snap-sdk` SDK runs
`.workshop/zwift-snap-sdk/fetch-skills.sh` at every launch and applied refresh to fetch the
pinned upstream release of github.com/canonical/use-workshop-skill there. The
`.github/skills/` skill directories are gitignored and regenerated each launch, so do not
edit them (only `.github/skills/README.md` is tracked, with the provenance, the pinned ref
and the override variables). They are absent until the workshop has been launched once, and
in environments that see only the repository. Route Workshop CLI questions there — in
particular `.github/skills/use-workshop/references/in-project-sdk.md` for the SDK layout and
hook rules used by this project.

## Definition, actions and SDK naming

The workshop definition is `.workshop/zwift-snap.yaml`; actions are plain bash
with `errexit`/`pipefail` set. In-project SDKs are listed with a `project-` prefix in
`sdks:` (`project-zwift-snap-sdk` for `.workshop/zwift-snap-sdk/sdk.yaml`), but plug and
slot references elsewhere (e.g. `connections:`) use the bare name (`zwift-snap-sdk`),
because the `project-` prefix only selects the source and is not part of the name.
The workshop desktop and gpu interfaces are NOT auto-connected, the `connections:`
entries are required.

## ccache

`setup-base` installs ccache and, in `/usr/local/lib/ccache`, symlinks named after
every compiler the wine part uses: the host `gcc`/`g++`/`cc`/`c++` and the mingw
cross compilers. The wine part's `build-environment` prepends that directory to
`PATH` and sets `CCACHE_DIR=/project/.ccache`, so ccache wraps every compiler and the
cache lands in the mounted workspace, restored and saved by the edge workflow (see
`.github/.kb/edge-build-workflow.md`).
