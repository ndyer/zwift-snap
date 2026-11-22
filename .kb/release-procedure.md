# Preface

Where the release procedure lives, and the conventions it depends on. Read this to find the
right document; the procedure itself is not repeated here.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Where the procedure lives

- `.github/skills/release/SKILL.md` — the step-by-step procedure: preflight, local
  pre-validation, release notes, local tagging, and the hand-off to the user.
- `.github/.kb/release-workflow.md` — how `release.yml` and `publish.yml` work: the
  build/publish split, the prerequisites gate, the Store environment and artefact hand-off.
- `.github/release-notes/` — one tracked notes file per version, read from `main` by both
  workflows and used verbatim as the GitHub Release body.

The tags are created locally and pushed by the user; the agent holds no GitHub credentials.

## Conventions

- Main repository tags: `v0.8`, `v0.9`, ... `v0.17`. Wine tags:
  `wine-<wine-version>-zwiftsnap-v<version>` (for example `wine-11.21-zwiftsnap-v0.18`).
- **Wine tags must start with the Wine version.** `wine --version` prints the `git describe`
  output verbatim, so a tag that does not begin with `wine-<version>-` (for example the old
  `zwift-snap-v0.12`) makes Wine's own reported version wrong. Read the version from
  `wine-src/VERSION`; `release.yml` derives it too and fails if `git describe` does not start
  with `wine-<version>-`.
- The snap version comes from `git describe --tags` (the `version-generator` part), so the
  main tag must exist before the snap is built.
- One version, one tag: the Store rejects an already-published version, so a re-tag means a
  new version number.
- `wine-src` is a shallow clone from the workshop hook; tag its synced branch tip. Delete it
  and relaunch with `ZWIFT_WINE_DEPTH=0` if a full clone is ever needed.
- The wine fork's `zwift-snap-*` tags were not pushed before this pipeline existed; from now
  on the wine tag is part of a release and `release.yml` refuses to build without it.