#!/bin/bash
# Fetch the patched Wine source the snap's wine part builds from: github.com/ndyer/wine at
# branch zwift-radios-rebased (Wine 11.21 with the winebth Bluetooth support rebased on
# top). Called by the setup-project hook on every launch and after every applied refresh,
# so a fresh checkout, a fresh workshop or a CI runner gets the tree snapcraft.yaml expects
# at $ZWIFT_WINE_SRC without anyone having to clone it by hand.
#
# Unlike fetch-skills.sh this IS fatal: without the Wine source nothing can be built or
# tested, so a failure has to fail the change instead of warning. setup-project runs under
# errexit, so exiting non-zero here makes workshop report the change as Error.
#
# An existing $ZWIFT_WINE_SRC is left strictly alone: it may hold local Wine patches or a
# full (non-shallow) checkout, and re-cloning over it would silently discard both. Delete
# the directory by hand to force a fresh clone.
#
# Overridable for a fork, a pinned tag, a full checkout or a scratch tree:
#   ZWIFT_WINE_REPO    repository to clone    (default https://github.com/ndyer/wine.git)
#   ZWIFT_WINE_REF     branch or tag to clone (default zwift-radios-rebased)
#   ZWIFT_WINE_DEPTH   clone depth, 0 for the full single-branch history (default 1)
#   ZWIFT_WINE_SRC     where the wine part expects it (default /project/wine-src)
#
# HTTPS rather than SSH because this also runs on CI runners, which have no SSH key.
set -euo pipefail

REPO="${ZWIFT_WINE_REPO:-https://github.com/ndyer/wine.git}"
REF="${ZWIFT_WINE_REF:-zwift-radios-rebased}"
DEPTH="${ZWIFT_WINE_DEPTH:-1}"
TARGET="${ZWIFT_WINE_SRC:-/project/wine-src}"

fail() { printf 'fetch-wine: %s\n' "$*" >&2; exit 1; }

if [[ -e ${TARGET} ]]; then
  echo "fetch-wine: ${TARGET} already exists; leaving it untouched"
  exit 0
fi

command -v git > /dev/null || fail "git is not installed"

# --single-branch and --depth keep the clone small: the wine part builds from
# source-type local, so it needs neither the other branches nor any git history at all.
# Set ZWIFT_WINE_DEPTH=0 when the release procedure has to tag wine-src, which needs the
# tags a shallow clone does not fetch.
clone_args=(--quiet --single-branch --branch "${REF}")
if [[ ${DEPTH} != 0 ]]; then
  clone_args+=(--depth "${DEPTH}")
fi

# Clone into a staging directory beside the target and only then move it into place, so an
# interrupted or failed clone never leaves a half-written wine-src behind that the next
# launch would mistake for a finished checkout.
staging="${TARGET}.tmp.$$"
trap 'rm -rf "${staging}"' EXIT
rm -rf "${staging}"

if ! git clone "${clone_args[@]}" -- "${REPO}" "${staging}"; then
  fail "could not clone ${REPO} at ${REF}"
fi

# The wine part configures the tree through the autotools plugin; a checkout without
# ./configure is not Wine and would fail much later with a confusing error.
[[ -f ${staging}/configure ]] || fail "${REPO} at ${REF} does not look like a Wine checkout"

mkdir -p "$(dirname "${TARGET}")"
mv "${staging}" "${TARGET}"

revision="$(git -C "${TARGET}" rev-parse --short HEAD 2> /dev/null)" || revision=""
echo "fetch-wine: cloned ${REPO} at ${REF} (${revision:-unknown}) into ${TARGET}"
