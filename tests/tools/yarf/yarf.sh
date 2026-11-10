#!/usr/bin/env bash
# yarf-based helpers for driving and inspecting a running Mir compositor (such as the one
# tests/smoke.sh or tests/pairing.sh starts), for debugging what Zwift shows on screen.
#
# grim does not work against Mir (it fails with "wl_shm.create_pool: invalid arguments"), so
# these run yarf's own wayland clients (screencopy, virtual pointer/keyboard) inside the yarf
# snap, the same capture/input path the tests use.
#
# Usage:
#   tests/tools/yarf/yarf.sh screenshot [-s SOCKET] [OUTPUT.png]
#       Save one frame of the compositor. OUTPUT defaults to ~/zwift-yarf/shot-<time>.png and
#       must be under $HOME outside hidden directories (the yarf snap cannot see anything else).
#   tests/tools/yarf/yarf.sh input [-s SOCKET] COMMAND [then COMMAND ...]
#       Send pointer/keyboard input. COMMAND is one of (see tests/tools/yarf/input.py):
#         click X Y | move X Y | type TEXT | key NAME | sleep SECONDS   (X, Y are 0.0-1.0)
#       Example: tests/tools/yarf/yarf.sh input type offline@example.com then key Return
#   tests/tools/yarf/yarf.sh ocr [-g SUBSTRING] PATH [PATH ...]
#       Print what yarf's OCR reads in saved frames (a file, or a directory of them), with the
#       same engine and thread cap the suites use. -g keeps only the frames whose text contains
#       SUBSTRING. See tests/tools/yarf/ocr_frame.py and YARF_FRAME_DIR in yarf_frames.py.
#       Example: tests/tools/yarf/yarf.sh ocr ~/zwift-pairing-out/frames -g KICKR
#
# SOCKET is the compositor's wayland socket, default $WAYLAND_DISPLAY (smoke.sh uses wayland-99).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
readonly ROOT
# yarf is a strict snap and cannot see /project or /tmp, so the helpers and the default output
# live under $HOME, in a non-hidden directory it can read and write
readonly STAGE_DIR="${HOME}/zwift-yarf"

die() {
    echo "yarf-tool: ${*}" >&2
    exit 1
}

usage() {
    sed -n '2,/^set -uo/p' "${BASH_SOURCE[0]}" | sed '$d; s/^# \{0,1\}//'
    exit "${1:-1}"
}

# resolve_socket: read an optional leading "-s SOCKET", else $WAYLAND_DISPLAY; validate it and
# link it into yarf's own XDG_RUNTIME_DIR (snap.yarf). Sets the globals `socket` and `runtime_dir`.
resolve_socket() {
    socket="${WAYLAND_DISPLAY:-}"
    if [[ ${1:-} == "-s" ]]; then
        socket="${2:?-s needs a socket name}"
        shift 2
    fi
    REST=("$@")
    [[ -n ${socket} ]] || die "no wayland socket given, use -s SOCKET or set WAYLAND_DISPLAY"
    [[ ${socket} != /* ]] || die "give the socket name, not a path"
    runtime_dir="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
    [[ -S "${runtime_dir}/${socket}" ]] || die "no wayland socket ${runtime_dir}/${socket}, is the compositor running?"
    snap list yarf > /dev/null 2>&1 || die "the yarf snap is not installed (the smoke test installs it)"
    mkdir -p "${STAGE_DIR}" || die "unable to create ${STAGE_DIR}"
    # yarf has its own XDG_RUNTIME_DIR, the compositor socket must be linked into it
    mkdir -p "${runtime_dir}/snap.yarf"
    ln -sf "${runtime_dir}/${socket}" "${runtime_dir}/snap.yarf/${socket}"
}

# run_in_yarf HELPER.py ARGS...: stage the helper and run it inside the yarf snap. The single
# quotes are deliberate: the values are passed as arguments so the real home (not the remapped
# $HOME inside the yarf snap) is used. Output goes through a pipe because snap run fails silently
# when its stdout is a regular file.
run_in_yarf() {
    local helper="${1:?}"
    shift
    cp "${ROOT}/tests/tools/yarf/${helper}" "${STAGE_DIR}/${helper}" || die "unable to stage ${helper}"
    # shellcheck disable=SC2016
    snap run --shell yarf -c \
        'home="$1"; helper="$2"; socket="$3"; shift 3; python3 "$home/zwift-yarf/$helper" "$socket" "$@"' \
        _ "${HOME}" "${helper}" "${socket}" "$@" 2>&1 | cat
    return "${PIPESTATUS[0]}"
}

do_screenshot() {
    resolve_socket "$@"
    local output="${REST[0]:-${STAGE_DIR}/shot-$(date +%H%M%S).png}"
    case "$(realpath -m "${output}")" in
    "${HOME}"/.*) die "the output must not be in a hidden directory, yarf cannot write there" ;;
    "${HOME}"/*) ;;
    *) die "the output must be under ${HOME}, yarf cannot write anywhere else" ;;
    esac
    mkdir -p "$(dirname "${output}")" || die "unable to create the output directory"
    run_in_yarf screenshot.py "${output}" || die "no screenshot taken"
    [[ -s ${output} ]] || die "no screenshot was written to ${output}"
    echo "yarf-tool: screenshot ${output}"
}

do_input() {
    resolve_socket "$@"
    [[ ${#REST[@]} -gt 0 ]] || die "no input commands given"
    run_in_yarf input.py "${REST[@]}" || die "input failed"
}

# do_ocr needs no compositor: it reads frames saved by an earlier run
do_ocr() {
    [[ $# -gt 0 ]] || die "no frames given"
    local helper="ocr_frame.py"
    local path
    local expect_value=""
    for path in "$@"; do
        if [[ -n ${expect_value} ]]; then
            expect_value=""
            continue
        fi
        case "${path}" in
        -g) expect_value=1 ;;
        -*) ;;
        *) [[ -e ${path} ]] || die "no such frame: ${path}" ;;
        esac
    done
    mkdir -p "${STAGE_DIR}" || die "unable to create ${STAGE_DIR}"
    cp "${ROOT}/tests/tools/yarf/${helper}" "${STAGE_DIR}/${helper}" \
        || die "unable to stage ${helper}"
    # shellcheck disable=SC2016
    snap run --shell yarf -c \
        'home="$1"; helper="$2"; shift 2; python3 "$home/zwift-yarf/$helper" "$@"' \
        _ "${HOME}" "${helper}" "$@" 2>&1 | cat
    return "${PIPESTATUS[0]}"
}

case "${1:-}" in
screenshot)
    shift
    do_screenshot "$@"
    ;;
input)
    shift
    do_input "$@"
    ;;
ocr)
    shift
    do_ocr "$@"
    ;;
-h | --help | "")
    usage 0
    ;;
*)
    die "unknown subcommand '${1}', expected 'screenshot', 'input' or 'ocr'"
    ;;
esac
