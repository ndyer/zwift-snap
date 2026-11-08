#!/usr/bin/env bash
# Manage the Zwift download cache the smoke test uses
# (tests/tools/dl-cache/dl-cache.sh).
#
# Usage: tests/dl-cache.sh {status|capture [--force]|stage [--force]|clear}
#
#   status           describe the cache and the Zwift version it holds
#   capture          record the installed prefix's downloads in the cache
#   stage            copy the cached downloads into the installed prefix
#   clear            delete the cache
#
# The smoke test stages and captures automatically, so this is mainly for
# inspecting the cache, or for restoring the game into a prefix after
# `sudo snap remove --purge zwift` without re-downloading everything.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly ROOT

die() {
    echo "dl-cache: ${*}" >&2
    exit 1
}

# shellcheck source=tests/tools/dl-cache/dl-cache.sh
source "${ROOT}/tests/tools/dl-cache/dl-cache.sh"

case "${1:-}" in
status)
    dl_cache_status
    ;;
capture)
    dl_cache_capture "${2:-}"
    ;;
stage)
    dl_cache_stage "${2:-}"
    ;;
clear)
    dl_cache_clear
    ;;
*)
    echo "usage: $0 {status|capture [--force]|stage [--force]|clear}" >&2
    exit 2
    ;;
esac
