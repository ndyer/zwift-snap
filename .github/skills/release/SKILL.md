---
name: release
description: Cut a new Zwift snap release — pre-validate the full test suite locally, draft and commit the release notes, tag both repositories locally, then hand the tag push and the publish dispatch to the user. Use when asked to "cut a release", "release v0.X", "draft release notes", "tag a release", or "pre-validate a release". Never pushes tags or handles GitHub credentials.
argument-hint: "[preflight|validate|notes|tag|help]"
---

# Release skill

Cutting a release is mostly local validation and bookkeeping; the two steps that touch
GitHub (pushing tags, publishing) are done **by the user**, deliberately:

- **This agent never has GitHub credentials.** Do not install `gh`, do not ask for a token,
  and do not run a command that authenticates. The workshop has neither an SSH key nor a
  `gh` login, by design.
- **Tags are created locally only.** The user pushes them. Pushing is what starts the
  release pipeline, so it is the user's decision.
- **Publishing is user-dispatched.** The user runs `publish.yml` from the Actions UI.

Your job ends with local tags created and the exact commands for the user printed.

Read `.github/.kb/release-workflow.md` for how the two workflows work, and
`.kb/release-procedure.md` for the tag and version conventions.

## The pipeline in one picture

```
local: pre-validate suites -> draft notes -> user edits -> commit notes on main
local: tag wine-src, tag main
USER:  push main, push both tags
CI:    release.yml  (tag push)  -> suites + build -> artifact "zwift-snap"
USER:  dispatch publish.yml     -> Store latest/beta + GitHub Release
```

`release.yml` does not publish; `publish.yml` does not build. This split is what makes a
failed Store upload retryable without rebuilding.

## 1. Preflight

Run these and stop with a clear report if anything is off:

```bash
git -C /project status --porcelain          # must be empty (build artefacts are gitignored)
git -C /project fetch origin
git -C /project rev-parse HEAD origin/main  # must match, or say so and stop
git -C /project/wine-src fetch origin
git -C /project/wine-src rev-parse HEAD origin/zwift-radios-rebased  # must match
```

`wine-src` is a **shallow** clone (depth 1). `git fetch origin` works, but if it is
diverged or in a detached state from a previous `wine-checkout`, do not force anything:
delete `wine-src` and relaunch the workshop with `ZWIFT_WINE_DEPTH=0` (see
`.workshop/.kb/workshop-usage.md`). With `ZWIFT_WINE_DEPTH=0`, `wine-src` is a full clone
and can be tagged and used for a release.

Historical note: the wine fork's `zwift-snap-*` tags were **never pushed** before this
pipeline existed, and that naming was wrong anyway (see below). Only the main repository's
`v0.8`–`v0.17` tags exist on the remote. From now on the wine tag is pushed as part of the
release and `release.yml` refuses to build without it.

## 2. Pre-validate every suite locally

This is the point of running the release from here: prove green before anything is public.
Run each in order; stop at the first failure and report it.

The build must be **clean**: a release is built from scratch, and an incremental build can
hide a stale part or a stale `prime` tree. Run `clean` before `build`, and decide separately
whether to drop the Wine tree (`wine-src`) — see the note below.

```bash
workshop run -- lint
workshop run -- test
workshop run -- gtk-preview
workshop run -- inhibit
workshop run -- clean
workshop run -- build
```

`clean` runs `snapcraft clean --destructive-mode`, which discards `parts/` and `stage/`, so
`build` recompiles every part from scratch. That is the build CI does (a fresh runner has no
part state), and it is what proves the snap really does build clean. It is **cheap**: ccache
lives in `/project/.ccache`, outside the cleaned tree, so the Wine recompile is almost
entirely cache hits (the workshop's warm cache runs at roughly a 75% hit rate). Expect
minutes, not a cold rebuild; only a change to Wine itself or to the build environment costs
more.

If the Wine tree itself may be stale — `wine-src` differs from `origin`, or you used
`wine-checkout` — say so and offer to delete `wine-src` and relaunch first, so the build
starts from a fresh clone. Do not delete it silently: the re-clone is large.

Then a clean install and the runtime tests. **Purge first** or the tests exercise the stale
prefix (see `.kb/verification.md`):

```bash
sudo snap remove --purge zwift
workshop run -- install
workshop exec -- sudo snap connect zwift:bluez
workshop run -- bluetooth
workshop run -- smoke      # warm via ~/zwift-cache; auto-selects headless without a display
workshop run -- pairing    # reuses the prefix smoke downloaded Zwift into
```

`smoke` and `pairing` are exactly what CI runs in `release.yml`, so a green local run is a
strong predictor of a green release run. They need no special display setup: with no host
display both use a virtual Mir output.

Do **not** proceed if any suite is red.

## 3. Version and release notes

Next version is one past the highest tag:

```bash
git -C /project tag -l 'v*' | sort -V | tail -1
```

Draft `.github/release-notes/v<X.Y>.md`. Base it on the commits since the previous tag and
keep the house shape. **Every commit bullet carries its full 40-character SHA in
parentheses, with no markdown link**: GitHub auto-links a bare commit SHA in the rendered
Release body. Use the full SHA, not the abbreviated one — the short form does not autolink
reliably.

```bash
git -C /project log --no-merges --format='- %s (%H)' "v<prev>..HEAD"
```

That produces lines like `- Add a settings dialog to the launcher (1a2b3c4d5e6f...)`.
Write a short "Highlights" section for the user-facing changes (prose, no SHAs needed), then
the commit list.
**Stop and let the user edit the file.** Do not commit it until they have had a chance to
rewrite it — the notes become the GitHub Release body verbatim.

The notes must end up **on `main`** before the tag is pushed: `release.yml` checks them on
`main`, and for a backfilled or re-validated old tag the tag tree does not contain them.

```bash
git -C /project add .github/release-notes/v<X.Y>.md
git -C /project commit -m "Release notes for v<X.Y>"
```

## 4. Tag locally

**Wine tags must start with the Wine version.** Wine derives the build string it reports
(`wine --version`, and anything that parses it) from `git describe`, so a tag that does not
begin with `wine-<version>-` makes Wine's own version wrong. Read the version from the
fork's `VERSION` file and use the form `wine-<version>-zwiftsnap-v<X.Y>`:

```bash
wine_version="$(sed -n 's/^Wine version //p' /project/wine-src/VERSION)"
# e.g. 11.21 -> wine-11.21-zwiftsnap-v0.18
git -C /project/wine-src tag -a "wine-${wine_version}-zwiftsnap-v<X.Y>" -m "Zwift snap v<X.Y>"
```

Tag the notes commit in the main repository and the synced wine tip in `wine-src`. Annotated
tags, matching the existing ones:

```bash
git -C /project tag -a v<X.Y> -m "Zwift snap v<X.Y>"
git -C /project/wine-src describe --tags      # must print wine-<version>-zwiftsnap-v<X.Y>
git -C /project tag -n1 -l 'v<X.Y>'            # show the tag and its message
```

If the tag already exists locally, stop: a release version is never reused. See
"Moved tags" below.

## 5. Hand off to the user

Print this block with the real version substituted, and stop — do not run any of it:

```text
Push the notes commit and both tags, in this order (from wherever you normally push;
each repository's remote is `origin`):

  git -C /project push origin main
  git -C /project push origin refs/tags/v<X.Y>
  git -C /project/wine-src push origin refs/tags/wine-<version>-zwiftsnap-v<X.Y>

The tag push starts the release workflow:
  https://github.com/ndyer/zwift-snap/actions/workflows/release.yml

When it is green, dispatch the publish workflow with tag v<X.Y>:
  https://github.com/ndyer/zwift-snap/actions/workflows/publish.yml
```

If the user pushes from inside the workshop, the SSH remotes are unreachable (no key), so
they need the HTTPS URLs and their own credentials:
`git push https://github.com/ndyer/zwift-snap.git ...` and
`https://github.com/ndyer/wine.git`. Do not run either yourself.

Explain what each does: `release.yml` runs the suites, builds and uploads the artifact;
`publish.yml` uploads that artifact to the Store's `latest/beta` and creates the GitHub
Release from the notes. If publish fails, fix the cause and dispatch it again — nothing is
rebuilt.

## Rules and edge cases

- **Never push, never authenticate, never install `gh`.** Print commands instead.
- **One version, one tag.** The Store rejects an already-published version, so a re-tag
  means a new version number.
- **Wine tags start with `wine-<version>-`.** Never `zwift-snap-*`: Wine reads its own
  version from `git describe`, and a non-wine prefix breaks it. `release.yml` derives the
  tag from `wine-src/VERSION` and fails the build if `git describe` does not start with
  `wine-<version>-`.
- **Moved tags**: if a tag was pushed and the build is wrong, deleting and re-pushing it
  re-runs `release.yml`, but the Store will refuse that version. Cut a new version instead.
- **Notes are the source of truth for the Release body.** `publish.yml` reads
  `.github/release-notes/<tag>.md` from `main`.
- **Do not tag `wine-src` when it is a shallow clone from a previous `wine-checkout`
  detached HEAD.** Sync to the branch tip first (or re-clone with `ZWIFT_WINE_DEPTH=0`).
- **Backfilling an old release** (notes-only, no Store upload) needs a notes file on `main`
  and a `publish.yml` dispatch with `notes-only: true`. That is a one-off activity, not part
  of the normal release flow.