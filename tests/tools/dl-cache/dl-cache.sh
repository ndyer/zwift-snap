#!/usr/bin/env bash
# Shared helper for the Zwift download cache used by the smoke test.
#
# The first smoke run downloads Zwift from the CDN (several GB) and lets
# winetricks download the Wine prerequisites. Both are slow and unchanged between
# runs, so the harness keeps them in a cache under $HOME/zwift-cache:
#
#   game/        the whole drive_c/Program Files (x86)/Zwift directory of the wine
#                prefix: what the launcher's own CDN sync writes there. Restoring it
#                satisfies the launcher's installed() check, so a staged prefix
#                launches without installing anything.
#   winetricks/  winetricks' own download cache (its W_CACHE directory)
#   meta.json    the Zwift version and capture time, so staleness is visible
#
# The cache is test tooling only: it never ships in the snap and the launcher
# neither reads nor writes it. It lives next to the other harness directories
# (zwift-smoke-out, zwift-yarf, ...) and deliberately NOT under ~/snap/zwift,
# because `sudo snap remove --purge zwift` deletes all of that - the exact case
# the cache exists to recover from.
#
# Sourced by tests/dl-cache.sh and tests/smoke.sh. The caller must provide a
# `die MESSAGE` function before sourcing.
#
# The confined snap cannot see $HOME/zwift-cache, so the winetricks half is
# staged into the snap-visible $SNAP_USER_COMMON, whose absolute path is the same
# inside and outside the snap; smoke.sh exports it as winetricks' W_CACHE.
# Setting W_CACHE is required, not optional: the launcher's cleanup deletes the
# default winetricks cache ($XDG_CACHE_HOME/winetricks) at the end of every run,
# so anything left there is gone before it can be captured.

# Cache layout
readonly DL_CACHE_DIR="${HOME}/zwift-cache"
readonly DL_CACHE_GAME="${DL_CACHE_DIR}/game"
readonly DL_CACHE_WINETRICKS="${DL_CACHE_DIR}/winetricks"
readonly DL_CACHE_META="${DL_CACHE_DIR}/meta.json"
# The live locations in the wine prefix. $SNAP_USER_COMMON is ~/snap/zwift/common
# on the host and at the same absolute path inside the snap.
readonly DL_CACHE_COMMON="${HOME}/snap/zwift/common"
readonly DL_CACHE_PREFIX="${DL_CACHE_COMMON}/.wine"
readonly DL_CACHE_GAME_SUBDIR="drive_c/Program Files (x86)/Zwift"
# winetricks' W_CACHE inside the snap. Deliberately outside .cache, which the
# launcher's cleanup_artifacts removes ($XDG_CACHE_HOME/winetricks) every run.
readonly DL_CACHE_WINETRICKS_LIVE="${DL_CACHE_COMMON}/winetricks-cache"

# _dl_cache_nonempty DIR: true when DIR exists and holds at least one entry
_dl_cache_nonempty() {
    [[ -d ${1:?} && -n $(ls -A "${1}" 2>/dev/null) ]]
}

# dl_cache_present: true when the cache holds a usable game directory and metadata
dl_cache_present() {
    [[ -f ${DL_CACHE_META} ]] && _dl_cache_nonempty "${DL_CACHE_GAME}"
}

# dl_cache_version DIR: the Zwift sversion installed in DIR, or empty. Mirrors
# zwsnap-gui/src/version.rs: Zwift_ver_cur_filename.txt (NUL padded) names the
# real version file, else Zwift_ver_cur.xml is read.
dl_cache_version() {
    local dir="${1:?}" name="Zwift_ver_cur.xml"
    if [[ -f "${dir}/Zwift_ver_cur_filename.txt" ]]; then
        name="$(tr -d '\0' < "${dir}/Zwift_ver_cur_filename.txt" | head -n1)"
    fi
    [[ -f "${dir}/${name}" ]] || return 0
    sed -n 's/.*sversion="\([^" ]*\).*/\1/p' "${dir}/${name}" | head -n1
}

# _dl_cache_meta FIELD: the named string field of meta.json, or empty
_dl_cache_meta() {
    [[ -f ${DL_CACHE_META} ]] || return 0
    sed -n "s/.*\"${1:?}\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "${DL_CACHE_META}" | head -n1
}

# dl_cache_stage [--force]: copy the cached game directory and winetricks downloads
# into the wine prefix. `cp -a` keeps modification times, so a restored tree is
# indistinguishable from the captured one. A prefix that already holds the cached
# version is left alone unless --force is given (the copy moves gigabytes).
#
# A changed version is not handled here: the launcher's sync diffs the installed
# files against the CDN manifest and fetches the difference itself.
dl_cache_stage() {
    local force=""
    [[ ${1:-} == "--force" ]] && force=1
    if ! dl_cache_present; then
        die "the Zwift download cache is empty (${DL_CACHE_DIR}); run a smoke test to populate it"
    fi
    local target="${DL_CACHE_PREFIX}/${DL_CACHE_GAME_SUBDIR}"
    local cached prefix
    cached="$(dl_cache_version "${DL_CACHE_GAME}")"
    prefix="$(dl_cache_version "${target}")"
    if [[ -z ${force} && -n ${prefix} && ${prefix} == "${cached}" ]]; then
        echo "dl-cache: the wine prefix already holds Zwift ${prefix}, not restoring"
    else
        echo "dl-cache: restoring Zwift ${cached:-unknown} into the wine prefix"
        mkdir -p "${target}"
        cp -a "${DL_CACHE_GAME}/." "${target}/"
    fi
    if _dl_cache_nonempty "${DL_CACHE_WINETRICKS}"; then
        echo "dl-cache: restoring the winetricks downloads into ${DL_CACHE_WINETRICKS_LIVE}"
        mkdir -p "${DL_CACHE_WINETRICKS_LIVE}"
        cp -a "${DL_CACHE_WINETRICKS}/." "${DL_CACHE_WINETRICKS_LIVE}/"
    fi
}

# dl_cache_capture [--force]: record the prefix's game directory and the live
# winetricks cache in the download cache. Skipped when the cache already holds the
# prefix's version, unless --force is given.
dl_cache_capture() {
    local force="" src version files
    [[ ${1:-} == "--force" ]] && force=1
    src="${DL_CACHE_PREFIX}/${DL_CACHE_GAME_SUBDIR}"
    if [[ ! -d ${src} ]]; then
        die "no installed Zwift to capture in ${src}"
    fi
    version="$(dl_cache_version "${src}")"
    if [[ -z ${force} ]] && dl_cache_present && [[ -n ${version} ]] \
        && [[ ${version} == "$(_dl_cache_meta version)" ]]; then
        echo "dl-cache: the cache already holds Zwift ${version}, not capturing (use --force to recapture)"
        return 0
    fi
    echo "dl-cache: capturing Zwift ${version:-unknown} into ${DL_CACHE_DIR}"
    mkdir -p "${DL_CACHE_GAME}"
    cp -a "${src}/." "${DL_CACHE_GAME}/"
    if [[ -d ${DL_CACHE_WINETRICKS_LIVE} ]]; then
        mkdir -p "${DL_CACHE_WINETRICKS}"
        cp -a "${DL_CACHE_WINETRICKS_LIVE}/." "${DL_CACHE_WINETRICKS}/"
    fi
    files="$(find "${DL_CACHE_GAME}" -type f | wc -l)"
    cat > "${DL_CACHE_META}" <<JSON
{
  "version": "${version}",
  "captured_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "files": ${files}
}
JSON
    echo "dl-cache: captured ${files} files"
}

# dl_cache_status: describe the cache on stdout
dl_cache_status() {
    echo "dl-cache: cache directory ${DL_CACHE_DIR}"
    if ! dl_cache_present; then
        echo "dl-cache: empty; run a smoke test to populate it"
        return 0
    fi
    echo "dl-cache: Zwift version $( _dl_cache_meta version )"
    echo "dl-cache: captured at $( _dl_cache_meta captured_at )"
    echo "dl-cache: game $( du -sh "${DL_CACHE_GAME}" | cut -f1 ) in ${DL_CACHE_GAME}"
    if _dl_cache_nonempty "${DL_CACHE_WINETRICKS}"; then
        echo "dl-cache: winetricks $( du -sh "${DL_CACHE_WINETRICKS}" | cut -f1 ) in ${DL_CACHE_WINETRICKS}"
    else
        echo "dl-cache: no winetricks downloads cached"
    fi
}

# dl_cache_clear: delete the cache
dl_cache_clear() {
    rm -rf "${DL_CACHE_DIR}"
    echo "dl-cache: removed ${DL_CACHE_DIR}"
}
