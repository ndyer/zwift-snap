# Agent skills

The Workshop agent skills used by this repository are **not vendored** here. They are
fetched from upstream at workshop launch into this directory, the standard VS Code Copilot
project skill location (`.github/skills/`; other harnesses also read `.claude/skills/` and
`.agents/skills/`).

Source: <https://github.com/canonical/use-workshop-skill> (GPL-3.0-only).

Fetched skills:

- `use-workshop` — operate the Workshop CLI: launch, refresh, run commands, wire interfaces,
  debug failed changes, orchestrate parallel environments.
- `onboard-workshop` — derive a workshop definition for a repository from its toolchain.
- `design-sdk` — design, build and publish Store SDKs with `sdkcraft`.

`onboard-workshop` and `design-sdk` borrow `use-workshop`'s references and templates by
sibling-relative path, so all three are fetched together.

## How they get here

`.workshop/zwift-snap-sdk/fetch-skills.sh`, run by the `zwift-snap-sdk` SDK's
`setup-project` hook on every workshop launch and applied refresh, clones the pinned
upstream ref and copies the three skill directories into this directory. It is
warn-and-continue: if the fetch fails, any previous copy stays and the launch still
succeeds. The upstream `tests/` eval suites and `_testlib` are omitted (upstream documents
that its suites run from its own repository only).

Only this `README.md` is tracked in git; the skill directories and the
`.fetched-revision` marker are gitignored and regenerated on every launch. **Do not edit
the fetched files** — the next launch overwrites them. To change a skill, propose the
change upstream.

Because the skills are fetched during launch, they are absent in a bare checkout that has
never launched the workshop, and in environments (e.g. GitHub-side agents) that only see
the repository.

## Version and overrides

The pinned upstream release tag is `v0.9.9`, recorded in the fetched
`.fetched-revision` marker together with the exact commit. To bump it, edit the defaults
in `.workshop/zwift-snap-sdk/fetch-skills.sh`, or override per-run with these environment
variables:

| Variable | Default | Purpose |
|---|---|---|
| `USE_WORKSHOP_SKILL_REPO` | `https://github.com/canonical/use-workshop-skill.git` | upstream clone URL |
| `USE_WORKSHOP_SKILL_REF` | `v0.9.9` | tag/branch to fetch |
| `USE_WORKSHOP_SKILL_CACHE` | `~/.cache/use-workshop-skill` | one shallow checkout per ref |
| `USE_WORKSHOP_SKILLS_DIR` | `/project/.github/skills` | destination |
