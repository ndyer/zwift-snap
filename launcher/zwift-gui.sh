# shellcheck shell=bash
# yad dialogs for the Zwift snap launcher. This file is meant to be sourced, not executed.
#
# One progress dialog stays open while Zwift is installed or updated and is closed when
# Zwift starts. Problems are reported with an error dialog. Without yad or a display all
# functions do nothing except print messages, so zwift still works from a terminal.

readonly YAD="${YAD:-yad}"
GUI_PROGRESS_PID=""
GUI_ARGS=()
# The snap has no SVG pixbuf loader, so icons from the icon theme do not show, use the png
GUI_ICON="${SNAP:-}/meta/gui/zwift.png"

gui_available() {
    [[ ${ZWIFT_NO_GUI:-0} -ne 1 ]] || return 1
    command -v "${YAD}" > /dev/null 2>&1 || return 1
    [[ -n ${DISPLAY:-} ]] || [[ -n ${WAYLAND_DISPLAY:-} ]]
}

# Arguments shared by all dialogs, stored in GUI_ARGS
gui_common_args() {
    GUI_ARGS=(--center --no-markup --class=zwift --name=zwift)
    if [[ -f ${GUI_ICON} ]]; then
        GUI_ARGS+=(--window-icon="${GUI_ICON}")
    fi
    return 0
}

# Open the progress dialog. It reads its commands from file descriptor 9:
#   #text    sets the text on the progress bar
#   number   sets the percentage, 100 closes the dialog
# The Cancel button kills this script (--auto-kill), the cleanup trap then stops wine.
gui_progress_start() {
    gui_available || return 0
    [[ -z ${GUI_PROGRESS_PID} ]] || return 0

    local dir
    dir="$(mktemp -d "${SNAP_USER_COMMON:-${TMPDIR:-/tmp}}/yad.XXXXXX")" || return 0
    if ! mkfifo "${dir}/progress"; then
        rm -rf "${dir}"
        return 0
    fi
    # Open the fifo for reading and writing: writing never fails with SIGPIPE, even when the
    # user closed the dialog, and yad never sees an end of file before we are done.
    exec 9<> "${dir}/progress"
    rm -rf "${dir}"

    gui_common_args
    # Started directly from the main shell so that this script is the parent for --auto-kill
    "${YAD}" "${GUI_ARGS[@]}" --progress --title="Zwift" --text="Getting Zwift ready" \
        --progress-text="Starting" --percentage=0 --auto-close --auto-kill --no-escape \
        --width=480 --button="Cancel:1" <&9 &
    GUI_PROGRESS_PID=$!
}

# gui_progress PERCENT TEXT: show a step of the work, also written to the terminal and the log.
# While the install progress sampler runs (zwift-setup.sh), it owns the percentage and this
# function only updates the text, so the fixed step percentages do not jump the bar backwards.
gui_progress() {
    local percent="${1:?}"
    local text="${2:-}"

    msgbox info "${text}"
    [[ -n ${GUI_PROGRESS_PID} ]] || return 0

    # 100 is reserved for gui_progress_stop, it closes the dialog
    if [[ ${percent} -gt 99 ]]; then percent=99; fi
    printf '#%s\n' "${text}" >&9
    if [[ -z ${PROGRESS_SAMPLER_PID:-} ]]; then
        printf '%s\n' "${percent}" >&9
    fi
    return 0
}

# Close the progress dialog
gui_progress_stop() {
    [[ -n ${GUI_PROGRESS_PID} ]] || return 0

    printf '100\n' >&9
    # give the dialog two seconds to close itself
    local tries=10
    while [[ ${tries} -gt 0 ]] && kill -0 "${GUI_PROGRESS_PID}" 2> /dev/null; do
        sleep 0.2
        ((tries--))
    done
    kill "${GUI_PROGRESS_PID}" 2> /dev/null || true
    wait "${GUI_PROGRESS_PID}" 2> /dev/null || true
    exec 9>&-
    GUI_PROGRESS_PID=""
}

# Run a yad dialog and return its exit code
gui_dialog() {
    gui_common_args
    "${YAD}" "${GUI_ARGS[@]}" "$@"
}

gui_show_log() {
    gui_dialog --text-info --title="Zwift log" --filename="${ZWIFT_LOG}" --tail \
        --width=800 --height=500 --button="Close:0"
}

# gui_failure TITLE TEXT PRIMARY_LABEL HEADLESS_RC
# Show an error dialog with Show log, the primary button (left out when PRIMARY_LABEL is empty)
# and Quit. Returns 0 when the primary button was chosen, 1 for Quit. Without a GUI nothing
# is shown and HEADLESS_RC is returned.
gui_failure() {
    local title="${1:?}"
    local text="${2:?}"
    local primary="${3:-}"
    local headless_rc="${4:-1}"

    if ! gui_available; then
        return "${headless_rc}"
    fi

    # the progress dialog is in the way, it is reopened when the user chooses to go on
    gui_progress_stop

    local -a buttons=(--button="Show log:20")
    if [[ -n ${primary} ]]; then buttons+=(--button="${primary}:10"); fi
    buttons+=(--button="Quit:1")

    local rc
    while true; do
        local -a image=()
        if [[ -f ${GUI_ICON} ]]; then image=(--image="${GUI_ICON}"); fi
        gui_dialog --title="${title}" "${image[@]}" --text="${text}" --width=480 "${buttons[@]}"
        rc=$?
        case ${rc} in
            10)
                gui_progress_start
                return 0
                ;;
            20) gui_show_log ;;
            *) return 1 ;;
        esac
    done
}
