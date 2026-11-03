#!/usr/bin/env bash
# Start Zwift using wine.
# Adapted from https://github.com/netbrain/zwift (Unlicense).
# Unlike upstream, there is no automatic login: the user logs in through the Zwift window.
set -uo pipefail

# shellcheck source=launcher/zwift-common.sh
source "${SNAP}/bin/zwift-common.sh"

readonly ZWIFT_OVERRIDE_RESOLUTION="${ZWIFT_OVERRIDE_RESOLUTION:-}"
readonly ZWIFT_NO_GAMEMODE="${ZWIFT_NO_GAMEMODE:-1}"

####################################
#### Install Zwift on first run #####

if ! [[ -f "${ZWIFT_INSTALL_DIR}/ZwiftApp.exe" ]]; then
    msgbox info "Zwift is not installed yet, installing it first (this takes a while)"
    if ! "${SNAP}/bin/zwift-update.sh" --install; then
        msgbox error "Zwift installation failed, run zwift again to retry"
        exit 1
    fi
    # the wine prefix and user directories did not exist before the installation
    resolve_wine_user_dirs
fi

if ! [[ -f "${ZWIFT_INSTALL_DIR}/ZwiftApp.exe" ]]; then
    msgbox error "ZwiftApp.exe not found after installation. Was the installer cancelled?"
    exit 1
fi

readonly ZWIFT_PREFS_FILE="${ZWIFT_DATA_DIR}/prefs.xml"

if ! zwift_wine_dir="$(winepath -w "${ZWIFT_INSTALL_DIR}")" || [[ -z ${zwift_wine_dir} ]]; then
    msgbox error "Failed to convert Zwift installation path to win32 path for wine"
    exit 1
else
    msgbox debug "Zwift installation wine path is: ${zwift_wine_dir}"
fi

if ! configure_graphics; then
    msgbox error "Failed to configure graphics"
    exit 1
fi

######################################
##### Configure Zwift #####

declare -a zwift_args
zwift_args=()

if [[ -n ${ZWIFT_OVERRIDE_RESOLUTION} ]]; then
    if [[ -f ${ZWIFT_PREFS_FILE} ]]; then
        msgbox info "Setting zwift resolution to ${ZWIFT_OVERRIDE_RESOLUTION}."
        updated_prefs="$(awk -v resolution="${ZWIFT_OVERRIDE_RESOLUTION}" '{
            gsub(/<USER_RESOLUTION_PREF>.*<\/USER_RESOLUTION_PREF>/,
                 "<USER_RESOLUTION_PREF>" resolution "</USER_RESOLUTION_PREF>")
        } 1' "${ZWIFT_PREFS_FILE}")"
        echo "${updated_prefs}" > "${ZWIFT_PREFS_FILE}"
    else
        msgbox warning "Preferences file does not exist yet. Resolution ${ZWIFT_OVERRIDE_RESOLUTION} cannot be set."
    fi
fi

##########################################
##### Automatically stop wine server #####

cleanup() {
    msgbox info "Stopping wine server"
    wineserver -k || true
}

trap cleanup EXIT

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

msgbox info "Starting Zwift launcher using wine"

if ! wine start /d "${zwift_wine_dir}" ZwiftLauncher.exe SilentLaunch || ! wait_until_wine_task_started ZwiftLauncher.exe; then
    msgbox error "Failed to start Zwift launcher using wine!"
    exit 1
fi

if ! launcher_pid="$(wine_task_pid ZwiftLauncher.exe)"; then
    msgbox error "Unable to get launcher process id. Did it crash?"
    exit 1
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
    msgbox error "Failed to start Zwift using wine!"
    exit 1
fi

msgbox info "Killing Zwift launcher and background tasks"
kill_wine_tasks ZwiftLauncher.exe ZwiftWindowsCrashHandler.exe

msgbox ok "Zwift started using wine"

##################################
##### Wait for Zwift to exit #####

counter=1
while is_wine_task_running ZwiftApp.exe; do
    msgbox debug "Waiting for Zwift to exit... ($((counter++)))"
    sleep 5
done

msgbox info "Zwift closed, exiting"
