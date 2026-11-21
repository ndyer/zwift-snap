# version-generator marker

This directory holds no content the `version-generator` part uses. The part runs
`git describe` against the project directory and nothing else.

It exists as the part's *source* for one reason: craft-parts only re-runs a step
when the part is dirty, and a local source is the only thing that can be made
dirty on demand (a git source is never re-pulled). This file is that source, and
the workshop `build` action touches it before every `snapcraft pack`, so the part
re-runs each build and the snap version stays current.

It used to be `source: .`, the whole project. That made craft-parts walk every
entry under `/project` on each build and re-hardlink the changed ones into
`parts/version-generator/src` — including `wine-src/`, `.ccache/` and the cargo
`target/` trees, several GiB that have nothing to do with the version. Touching
this one file dirties the part for the cost of a single `stat`.

Do not delete this file; the part and the `build` action both reference it.
