#!/bin/bash
# Fetch the canonical Workshop agent skills (use-workshop, onboard-workshop, design-sdk)
# from github.com/canonical/use-workshop-skill into the project working tree instead of
# vendoring them. Called by the setup-project hook on every launch and after every applied
# refresh, so the tree always reflects the pinned upstream revision.
#
# The skills land in $USE_WORKSHOP_SKILLS_DIR (default /project/.github/skills), the
# standard VS Code Copilot project skill location, which the host agent reads. That path
# is gitignored; only .github/skills/README.md is tracked.
#
# Failure is never fatal: without network, git, or upstream the script warns and exits 0,
# leaving an existing fetch in place. A workshop without the skills is still usable, and
# setup-project runs under errexit.
set -uo pipefail

REPO="${USE_WORKSHOP_SKILL_REPO:-https://github.com/canonical/use-workshop-skill.git}"
REF="${USE_WORKSHOP_SKILL_REF:-v0.9.9}"
CACHE="${USE_WORKSHOP_SKILL_CACHE:-${HOME:-/root}/.cache/use-workshop-skill}"
TARGET="${USE_WORKSHOP_SKILLS_DIR:-/project/.github/skills}"
SKILLS=(use-workshop onboard-workshop design-sdk)
MARKER=.fetched-revision

warn() { printf 'fetch-skills: %s\n' "$*" >&2; }

# Already at the requested ref with all three skills present: nothing to do. This is the
# common path on a relaunch and needs neither network nor git.
if grep -qxF "ref=$REF" "${TARGET}/${MARKER}" 2>/dev/null; then
  current=1
  for skill in "${SKILLS[@]}"; do
    [[ -f ${TARGET}/${skill}/SKILL.md ]] || { current=0; break; }
  done
  if ((current)); then
    echo "fetch-skills: ${TARGET} is already at ${REF}"
    exit 0
  fi
fi

if ! command -v git >/dev/null; then
  warn "git is not installed; leaving ${TARGET} untouched"
  exit 0
fi

# One shallow checkout per ref under the cache, so a pinned ref is cloned once and a ref
# bump gets its own directory rather than fighting over a mutable one.
source_dir="${CACHE}/${REF}"
if [[ ! -d ${source_dir}/.git ]]; then
  rm -rf "${source_dir}"
  mkdir -p "$(dirname "${source_dir}")"
  # -c advice.detachedHead=false: a --branch tag checkout lands on a detached HEAD, and git
  # prints a paragraph of advice about it even under --quiet, which would clutter the launch.
  if ! git -c advice.detachedHead=false clone --quiet --depth 1 --branch "${REF}" -- "${REPO}" "${source_dir}"; then
    warn "could not clone ${REPO} at ${REF}; leaving ${TARGET} untouched"
    exit 0
  fi
fi

revision="$(git -C "${source_dir}" rev-parse HEAD 2>/dev/null)" || revision=""
if [[ -z ${revision} ]]; then
  warn "could not read the revision of ${source_dir}; leaving ${TARGET} untouched"
  exit 0
fi

for skill in "${SKILLS[@]}"; do
  if [[ ! -f ${source_dir}/.github/skills/${skill}/SKILL.md ]]; then
    warn "upstream ${REF} has no .github/skills/${skill}/SKILL.md"
    exit 0
  fi
done

# Stage on the same filesystem as the target, then move each skill into place, so a failed
# or interrupted copy never leaves a half-written skill directory behind.
staging="${TARGET}.tmp.$$"
trap 'rm -rf "${staging}"' EXIT
rm -rf "${staging}"
mkdir -p "${staging}"
for skill in "${SKILLS[@]}"; do
  cp -a "${source_dir}/.github/skills/${skill}" "${staging}/${skill}"
  # Omit the upstream promptfoo eval suites: upstream documents that they run from its own
  # repo only, and the shared .github/skills/_testlib they need is not copied either.
  rm -rf "${staging:?}/${skill}/tests"
done

mkdir -p "${TARGET}"
for skill in "${SKILLS[@]}"; do
  rm -rf "${TARGET:?}/${skill}"
  mv "${staging}/${skill}" "${TARGET}/${skill}"
done

{
  printf 'repo=%s\n' "${REPO}"
  printf 'ref=%s\n' "${REF}"
  printf 'revision=%s\n' "${revision}"
} >"${TARGET}/${MARKER}"

echo "fetch-skills: installed ${SKILLS[*]} at ${REF} (${revision:0:12}) into ${TARGET}"
