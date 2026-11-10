# Preface

The release checklist for both tagged repositories. Follow it exactly — the wine
tag and the main-repository tag must stay in step, and the pack step in between
is what validates the pairing.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Release Procedures

- Tags in the main repository `/project` follow semantic versioning: `v0.10`, `v0.11`, `v0.12`
- Wine source tags in `/project/wine-src` use the pattern: `zwift-snap-v0.12`
- When releasing a new version:
  1. Ensure both repositories are clean, don't proceed otherwise.
  2. Figure out the next version in sequence.
  3. Tag wine-src.
  4. Tag main repository
  6. Move/re-tag main repository's v0.12 to point to the final commit

A tag is not the only way a revision reaches the Store: the daily edge build
workflow publishes `main` to `latest/edge` on its own (see
`.github/.kb/edge-build-workflow.md`). Its Wine source comes from the workshop's
`setup-project` hook, which clones shallowly and therefore without tags, so step 3
needs `ZWIFT_WINE_DEPTH=0` in a fresh workshop.

The intention in the future is that a github workflow will be added to run full tests including smoke and promote tagged builds to beta.