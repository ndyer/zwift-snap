# shellcheck shell=bash
# Shared helpers for the Zwift snap launcher scripts.
# Adapted from https://github.com/netbrain/zwift (Unlicense).
# This file is meant to be sourced, not executed.

readonly DEBUG="${DEBUG:-0}"
if [[ ${DEBUG} -eq 1 ]]; then set -x; fi

readonly COLORED_OUTPUT="${COLORED_OUTPUT:-0}"
if [[ -t 1 ]] || [[ ${COLORED_OUTPUT} -eq 1 ]]; then
    readonly COLOR_WHITE="\033[0;37m"
    readonly COLOR_RED="\033[0;31m"
    readonly COLOR_GREEN="\033[0;32m"
    readonly COLOR_BLUE="\033[0;34m"
    readonly COLOR_YELLOW="\033[0;33m"
    readonly RESET_STYLE="\033[0m"
else
    readonly COLOR_WHITE=""
    readonly COLOR_RED=""
    readonly COLOR_GREEN=""
    readonly COLOR_BLUE=""
    readonly COLOR_YELLOW=""
    readonly RESET_STYLE=""
fi

readonly VERBOSITY="${VERBOSITY:-1}"
readonly WINE_DISABLE_EGL="${WINE_DISABLE_EGL:-0}"

msgbox() {
    local type="${1:?}" # Type: info, ok, warning, error, debug
    local msg="${2:?}"  # Message: the message to display

    local timestamp=""
    [[ ${VERBOSITY} -ge 2 ]] && printf -v timestamp '%(%T)T|' -1

    case ${type} in
        info) [[ ${VERBOSITY} -ge 1 ]] && echo -e "${COLOR_BLUE}[snap|${timestamp}*] ${msg}${RESET_STYLE}" ;;
        ok) echo -e "${COLOR_GREEN}[snap|${timestamp}✓] ${msg}${RESET_STYLE}" ;;
        warning) echo -e "${COLOR_YELLOW}[snap|${timestamp}!] ${msg}${RESET_STYLE}" ;;
        error) echo -e "${COLOR_RED}[snap|${timestamp}✗] ${msg}${RESET_STYLE}" >&2 ;;
        debug) [[ ${VERBOSITY} -ge 3 ]] && echo -e "${COLOR_WHITE}[snap|${timestamp}◉] ${msg}${RESET_STYLE}" ;;
        *) echo "msgbox - unknown type ${type}" >&2 && exit 1 ;;
    esac
    return 0
}

###########################################
##### Wine prefix and Zwift directories #####

# The wine prefix lives in SNAP_USER_COMMON so it survives snap refreshes.
export WINEPREFIX="${WINEPREFIX:-${SNAP_USER_COMMON:?SNAP_USER_COMMON is not set, run this inside the snap}/.wine}"
export WINEDEBUG="${WINEDEBUG:-fixme-all}"
export XDG_CACHE_HOME="${XDG_CACHE_HOME:-${SNAP_USER_COMMON}/.cache}"
export ZWIFT_INSTALL_DIR="${WINEPREFIX}/drive_c/Program Files (x86)/Zwift"

# The wine user name follows the host user name, so look the home directory up
# instead of assuming it. Sets WINE_USER_HOME and ZWIFT_DATA_DIR.
resolve_wine_user_dirs() {
    local users_dir="${WINEPREFIX}/drive_c/users"
    local dir

    WINE_USER_HOME=""
    if [[ -d ${users_dir} ]]; then
        for dir in "${users_dir}"/*/; do
            dir="${dir%/}"
            [[ -d ${dir} ]] || continue
            [[ ${dir##*/} == "Public" ]] && continue
            WINE_USER_HOME="${dir}"
            break
        done
    fi

    if [[ -n ${WINE_USER_HOME} ]]; then
        ZWIFT_DATA_DIR="${WINE_USER_HOME}/AppData/Local/Zwift"
    else
        ZWIFT_DATA_DIR=""
    fi
    export WINE_USER_HOME ZWIFT_DATA_DIR
}

resolve_wine_user_dirs

#####################################
##### Wine task helper functions #####

wine_task_info() {
    local task_name="${1:?}"
    wine tasklist /fo list /fi "IMAGENAME eq ${task_name}"
}

wine_task_pid() {
    local task_name="${1:?}"
    wine_task_info "${task_name}" | grep -m1 -Po '^PID:[\t ]*\K[0-9]+'
}

is_wine_task_running() {
    local task_name="${1:?}"
    [[ -n $(wine_task_info "${task_name}" || true) ]]
}

kill_wine_tasks() {
    for task in "${@}"; do
        msgbox debug "Killing wine task '${task}'"
        wine taskkill /f /im "${task}" > /dev/null 2>&1 || true
    done
}

wait_until() {
    local condition="${1:?}"
    local timeout="${2:-20}"
    local delay="${3:-0.1}"
    local counter=1

    while ! eval "${condition}" && [[ ${counter} -le ${timeout} ]]; do
        msgbox debug "Waiting... (${counter}/${timeout})"
        sleep "${delay}"
        ((counter++))
    done

    eval "${condition}"
}

wait_until_wine_task_started() {
    local task_name="${1:?}"
    msgbox info "Waiting for ${task_name} to start..."
    wait_until "is_wine_task_running ${task_name}"
}

#####################################
##### Display and graphics setup #####

nvidia_proprietary_driver() {
    [[ -e /dev/nvidiactl ]]
}

# Snaps get their own XDG_RUNTIME_DIR, but the host wayland socket lives in the
# real one. Make it reachable so native wayland works.
link_wayland_socket() {
    local host_runtime_dir
    host_runtime_dir="/run/user/$(id -u)"
    local socket="${WAYLAND_DISPLAY:-}"

    [[ -n ${socket} ]] || return 0
    [[ ${socket} == /* ]] && return 0
    [[ -n ${XDG_RUNTIME_DIR:-} ]] || return 0
    [[ -e "${XDG_RUNTIME_DIR}/${socket}" ]] && return 0

    if [[ -S "${host_runtime_dir}/${socket}" ]]; then
        ln -sf "${host_runtime_dir}/${socket}" "${XDG_RUNTIME_DIR}/${socket}" || true
    fi
}

configure_graphics() {
    local session_type="${XDG_SESSION_TYPE:-x11}"

    if [[ ${session_type} == "wayland" ]]; then
        msgbox info "Enabling native Wayland"
        link_wayland_socket
        unset DISPLAY # DISPLAY variable needs to be empty for wine to use native wayland
    elif [[ ${WINE_DISABLE_EGL} -eq 1 ]]; then
        msgbox info "Disabling EGL (using GLX instead)"
        wine reg.exe add 'HKCU\Software\Wine\X11 Driver' /f /v UseEGL /d N > /dev/null 2>&1 || return 1
    elif nvidia_proprietary_driver; then
        msgbox info "Detected nvidia graphics, performing EGL vendor library workaround"
        export __EGL_VENDOR_LIBRARY_FILENAMES=""
    else
        msgbox debug "Using X11 with EGL on non-nvidia system"
    fi
}
