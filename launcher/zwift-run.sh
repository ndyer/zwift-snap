#!/usr/bin/env bash
# Install or update Zwift when needed, then start it using wine.
# Adapted from https://github.com/netbrain/zwift (Unlicense).
# Unlike upstream, there is no automatic login: the user logs in through the Zwift window.
set -uo pipefail

# shellcheck source=launcher/zwift-common.sh
source "${SNAP}/bin/zwift-common.sh"
# shellcheck source=launcher/zwift-gui.sh
source "${SNAP}/bin/zwift-gui.sh"
# shellcheck source=launcher/zwift-setup.sh
source "${SNAP}/bin/zwift-setup.sh"

readonly ZWIFT_DISPLAY_MODE="${ZWIFT_DISPLAY_MODE:-fullscreen}"
readonly ZWIFT_NO_GAMEMODE="${ZWIFT_NO_GAMEMODE:-1}"

init_log || echo "Unable to create the log file ${ZWIFT_LOG}" >&2

# Start capture before any Wine commands so diagnostics from the Wine server are
# retained even if it starts during prefix checks or setup.
if [[ ${ZWIFT_BLUETOOTH_DEBUG:-0} == 1 ]]; then
    exec 2> >(tee -a "${ZWIFT_LOG}" >&2)
fi

##########################################
##### Automatically stop wine server #####

cleanup() {
    stop_progress_sampler
    gui_progress_stop
    msgbox info "Stopping wine server"
    wineserver -k || true
    cleanup_install_artifacts
}

trap cleanup EXIT
# closing the progress dialog with Cancel, or Ctrl+C in a terminal, ends zwift
trap 'exit 1' TERM INT HUP

# a failure ends zwift, the error dialog was already shown by the failing step
fail() {
    msgbox error "${1:?}"
    exit 1
}

#################################################
##### Display and graphics, before any dialog #####

# the wayland socket must be reachable before the first dialog opens
if [[ ${XDG_SESSION_TYPE:-x11} == "wayland" ]]; then
    link_wayland_socket
fi
gui_progress_start

if ! configure_graphics; then
    gui_failure "Zwift" "Failed to configure graphics." "" 1
    fail "Failed to configure graphics"
fi

###################################
##### Install or update Zwift #####

# a prefix from an earlier run can still link its folders to the home directory
private_user_dirs || fail "Unable to make the wine user folders private"

if ! zwift_installed; then
    # The installation includes the download of Zwift by the launcher, so it is up to date afterwards
    while ! install_zwift; do
        if ! gui_failure "Zwift installation failed" \
            "Zwift could not be installed. Check the log and your network connection." "Retry" 1; then
            fail "Zwift installation failed"
        fi
    done
    resolve_wine_user_dirs
elif ! update_zwift_if_needed; then
    # Zwift is installed and can still be started, so an update problem is not fatal
    if ! gui_failure "Zwift update failed" \
        "Zwift could not be updated. The installed version can still be started." "Launch anyway" 0; then
        fail "Zwift update failed"
    fi
fi

if ! zwift_installed; then
    fail "ZwiftApp.exe not found, is Zwift installed?"
fi

readonly ZWIFT_PREFS_FILE="${ZWIFT_DATA_DIR}/prefs.xml"

if ! zwift_wine_dir="$(winepath -w "${ZWIFT_INSTALL_DIR}")" || [[ -z ${zwift_wine_dir} ]]; then
    gui_failure "Zwift" "Failed to find the Zwift installation." "" 1
    fail "Failed to convert Zwift installation path to win32 path for wine"
else
    msgbox debug "Zwift installation wine path is: ${zwift_wine_dir}"
fi

######################################
##### Configure Zwift #####

declare -a zwift_args
zwift_args=()

# Configure display mode (fullscreen or custom resolution)
configure_display_mode

##################################
##### Start Zwift using wine #####

# The Zwift launcher is not fully functional in wine:
# - It cannot launch Zwift
# - If Zwift itself is started independently, it will automatically start the launcher
# Workaround:
# 1. Start the launcher ZwiftLauncher.exe in the background using SilentLaunch
# 2. Obtain the launcher wine process id
# 3. Use runfromprocess to launch ZwiftApp.exe with the launcher process as parent
# 4. Kill ZwiftLauncher.exe

gui_progress 95 "Starting Zwift"

if ! wine start /d "${zwift_wine_dir}" ZwiftLauncher.exe SilentLaunch || ! wait_until_wine_task_started ZwiftLauncher.exe; then
    gui_failure "Zwift failed to start" "The Zwift launcher did not start." "" 1
    fail "Failed to start Zwift launcher using wine!"
fi

if ! launcher_pid="$(wine_task_pid ZwiftLauncher.exe)"; then
    gui_failure "Zwift failed to start" "The Zwift launcher stopped right after it started." "" 1
    fail "Unable to get launcher process id. Did it crash?"
fi

msgbox ok "Zwift launcher started using wine"
msgbox info "Starting Zwift using wine"

declare -a zwift_cmd
zwift_cmd=(wine start /d "${zwift_wine_dir}" /unix "${SNAP}/bin/runfromprocess-rs.exe" "${launcher_pid}" ZwiftApp.exe "${zwift_args[@]}")

if [[ ${ZWIFT_NO_GAMEMODE} -eq 1 ]] || ! command -v gamemoderun > /dev/null 2>&1; then
    msgbox info "Not using gamemode"
else
    msgbox info "Using gamemode"
    zwift_cmd=(gamemoderun "${zwift_cmd[@]}")
fi

if ! "${zwift_cmd[@]}" || ! wait_until_wine_task_started ZwiftApp.exe; then
    gui_failure "Zwift failed to start" "Zwift did not start." "" 1
    fail "Failed to start Zwift using wine!"
fi

msgbox info "Killing Zwift launcher and background tasks"
kill_wine_tasks ZwiftLauncher.exe ZwiftWindowsCrashHandler.exe

msgbox ok "Zwift started using wine"

if [[ -n ${GUI_PROGRESS_PID} ]]; then
    gui_progress 99 "Waiting for the Zwift window"
    if ! wait_until_zwift_window; then
        gui_failure "Zwift failed to start" "Unable to show the Zwift window. Check the log for details." "" 1
        fail "Failed to show the Zwift window"
    fi
fi

gui_progress_stop

##################################
##### Wait for Zwift to exit #####

counter=1
while is_wine_task_running ZwiftApp.exe; do
    msgbox debug "Waiting for Zwift to exit... ($((counter++)))"
    sleep "${ZWIFT_POLL_INTERVAL}"
done

msgbox info "Zwift closed, exiting"
