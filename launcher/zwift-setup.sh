# shellcheck shell=bash
# Install and update Zwift inside the wine prefix.
# Adapted from https://github.com/netbrain/zwift (Unlicense).
# This file is meant to be sourced by zwift-run.sh, after zwift-common.sh and zwift-gui.sh.

SETUP_EXE="${SNAP_USER_COMMON:-/tmp}/ZwiftSetup.exe"
readonly SETUP_EXE
# After the installation the launcher downloads the game, at most this many polls (240 * 5s = 20 minutes)
readonly ZWIFT_DOWNLOAD_ITERATIONS="${ZWIFT_DOWNLOAD_ITERATIONS:-240}"

# Run a command with its output in the log and on the terminal. The command runs in the
# background and is waited for, so the cancel trap of zwift-run.sh runs without delay.
run_logged() {
    {
        "$@" 2>&1 | tee -a "${ZWIFT_LOG}"
        exit "${PIPESTATUS[0]}"
    } &
    wait $!
}

# version_ge A B: success when version A is the same as or newer than version B.
# A missing version (empty or "none", meaning Zwift is not installed) is older than any version.
version_ge() {
    local a="${1:-}"
    local b="${2:-}"

    if [[ -z ${a} ]] || [[ ${a} == "none" ]]; then return 1; fi
    if [[ -z ${b} ]] || [[ ${b} == "none" ]]; then return 0; fi
    [[ "$(printf '%s\n%s\n' "${a}" "${b}" | sort -V | head -n 1)" == "${b}" ]]
}

get_current_version() {
    # if Zwift_ver_cur_filename.txt exists, it holds the true current version filename
    # if it does not exist, use Zwift_ver_cur.xml as fallback
    # if neither exist or contain a valid version, use none (Zwift not installed)
    # The installer only writes <Zwift version="0.0.0" .../>, a placeholder without sversion,
    # which also gives none: the game is not downloaded yet.

    local version_filename="Zwift_ver_cur.xml"
    if [[ -f "${ZWIFT_INSTALL_DIR}/Zwift_ver_cur_filename.txt" ]]; then
        version_filename="$(tr '\0' '\n' < "${ZWIFT_INSTALL_DIR}/Zwift_ver_cur_filename.txt")"
    fi

    local version
    version="$(grep -oP 'sversion="\K.*?(?=\s)' "${ZWIFT_INSTALL_DIR}/${version_filename}" 2> /dev/null | cut -f 1 -d ' ')"
    echo "${version:-none}"
}

get_latest_version() {
    local version
    version="$(wget --no-cache --quiet -O - https://cdn.zwift.com/gameassets/Zwift_Updates_Root/Zwift_ver_cur.xml \
        | grep -oP 'sversion="\K.*?(?=")' | cut -f 1 -d ' ')" || return 1
    [[ -n ${version} ]] || return 1
    echo "${version}"
}

zwift_installed() {
    [[ -f "${ZWIFT_INSTALL_DIR}/ZwiftApp.exe" ]] && [[ -f "${ZWIFT_INSTALL_DIR}/ZwiftLauncher.exe" ]]
}

# Install wine prerequisites and Zwift. This runs unattended, the installer shows no windows.
install_zwift() {
    # prevent wine from installing mono and gecko
    gui_progress 2 "Setting up wine"
    WINEDLLOVERRIDES="mscoree,mshtml=;winemenubuilder.exe=d" run_logged wineboot -u || return 1

    # the wine user is only known after the prefix exists
    resolve_wine_user_dirs
    private_user_dirs || return 1

    gui_progress 8 "Installing .NET Framework 4.8, this takes several minutes"
    run_logged winetricks -q dotnet48 || return 1
    gui_progress 55 "Installing the Direct3D shader compiler"
    run_logged winetricks -q d3dcompiler_47 || return 1
    gui_progress 60 "Installing WebView2"
    run_logged winetricks -q webview2 || return 1

    # video routes need a d3d11 device with video support, wine only provides that with the vulkan renderer
    gui_progress 75 "Enabling the Vulkan renderer for video routes"
    run_logged winetricks -q renderer=vulkan || return 1

    gui_progress 80 "Downloading Zwift"
    run_logged wget -O "${SETUP_EXE}" https://cdn.zwift.com/app/ZwiftSetup.exe || return 1

    gui_progress 88 "Installing Zwift"
    # The installer is silent, so its own log (Inno Setup /LOG, wine maps Z: to /) is the only way
    # to see what it did. It is printed below, also when the installer fails.
    local setup_log="${SNAP_USER_COMMON:-/tmp}/ZwiftSetup.log"
    local setup_rc=0
    rm -f -- "${setup_log}"
    run_logged wine "${SETUP_EXE}" /SP- /VERYSILENT /SUPPRESSMSGBOXES /NORESTART /NOCANCEL \
        /LOG="Z:${setup_log//\//\\}" || setup_rc=$?
    show_installer_log "${setup_log}"
    [[ ${setup_rc} -eq 0 ]] || return 1

    wineserver -k || true # workaround for issue 420, ZwiftLauncher v1.2.0 crashes immediately

    # The installer only installs the launcher, with version 0.0.0 as a placeholder. It does not
    # contain the game, the launcher downloads that, so it is updated like any other update.
    if [[ ! -f "${ZWIFT_INSTALL_DIR}/ZwiftLauncher.exe" ]]; then
        msgbox error "The installer finished but the Zwift launcher is not installed"
        list_zwift_files
        return 1
    fi

    local zwift_latest_version
    if ! zwift_latest_version="$(get_latest_version)"; then
        msgbox error "Unable to retrieve the latest Zwift version number, Zwift cannot be downloaded"
        return 1
    fi

    msgbox info "Zwift launcher installed, downloading Zwift ${zwift_latest_version}"
    update_zwift_using_launcher "${zwift_latest_version}" "$(get_current_version)" "${ZWIFT_DOWNLOAD_ITERATIONS}" 90 9 \
        || return 1

    if ! zwift_installed; then
        msgbox error "The launcher finished but Zwift is not installed"
        list_zwift_files
        return 1
    fi
}

# Print the Zwift files in the wine prefix, to see what an installation left behind
list_zwift_files() {
    msgbox info "Zwift files in the wine prefix:"
    run_logged find "${WINEPREFIX}/drive_c" -maxdepth 4 -iname '*zwift*' || true
}

# Print the log of the Zwift installer, so it is on the terminal and in the zwift log
show_installer_log() {
    local file="${1:?}"

    if [[ -s ${file} ]]; then
        msgbox info "Zwift installer log:"
        run_logged cat "${file}" || true
    else
        msgbox warning "The Zwift installer wrote no log (${file})"
    fi
}

# update_zwift_using_launcher LATEST ORIGINAL [ITERATIONS [PERCENT_START [PERCENT_SPAN]]]
# Update Zwift by letting the Zwift launcher do it. Only called when Zwift is older than the latest
# version, or when Zwift was just installed and the launcher has to download it (ORIGINAL is none).
# The launcher is checked every ZWIFT_POLL_INTERVAL seconds for at most ITERATIONS times, and the
# progress bar moves from PERCENT_START by PERCENT_SPAN.
update_zwift_using_launcher() {
    local zwift_latest_version="${1:?}"
    local zwift_original_version="${2:?}"
    local max_iterations="${3:-60}" # 60 * 5s = 5 minutes max
    local percent_start="${4:-10}"
    local percent_span="${5:-80}"
    local zwift_current_version="${zwift_original_version}"

    if [[ ${zwift_original_version} == "none" ]]; then
        gui_progress "${percent_start}" "Downloading Zwift ${zwift_latest_version}, this takes several minutes"
    else
        gui_progress "${percent_start}" "Updating Zwift from version ${zwift_original_version} to ${zwift_latest_version}"
    fi

    local zwift_wine_dir
    if ! zwift_wine_dir="$(winepath -w "${ZWIFT_INSTALL_DIR}")" || [[ -z ${zwift_wine_dir} ]]; then
        msgbox error "Failed to convert Zwift installation path to win32 path for wine"
        return 1
    fi
    if ! wine start /d "${zwift_wine_dir}" ZwiftLauncher.exe SilentLaunch; then
        msgbox error "Failed to start Zwift launcher using wine!"
        return 1
    fi
    msgbox ok "Zwift launcher started using wine"

    local counter=1

    # also stop if launcher exits before update finishes, so we don't hang forever
    while [[ ${zwift_current_version} == "${zwift_original_version}" ]] && [[ ${counter} -le ${max_iterations} ]] && is_wine_task_running ZwiftLauncher.exe; do
        # The counter is a timeout budget, not a download size: spell out the
        # elapsed and maximum time so the message cannot be read as "21 of 240 files".
        gui_progress $((percent_start + counter * percent_span / max_iterations)) \
            "Updating Zwift to version ${zwift_latest_version}, waited $((counter * ZWIFT_POLL_INTERVAL / 60)) of $((max_iterations * ZWIFT_POLL_INTERVAL / 60)) minutes"
        sleep "${ZWIFT_POLL_INTERVAL}"
        zwift_current_version="$(get_current_version)"
        ((counter++))
    done

    # The launcher does not stop until the wine server is stopped. Zwift is started with a fresh
    # launcher and wine server afterwards, as when the update was a separate command.
    kill_wine_tasks ZwiftLauncher.exe ZwiftWindowsCrashHandler.exe
    wineserver -k || true

    # if launcher exited unexpectedly or update timeout, Zwift is still at the old version
    if [[ ${zwift_current_version} == "${zwift_original_version}" ]]; then
        if [[ ${counter} -gt ${max_iterations} ]]; then
            msgbox error "Update timed out after $((counter * ZWIFT_POLL_INTERVAL)) seconds"
        else
            msgbox error "Launcher exited unexpectedly, update did not complete"
        fi
        return 1
    fi

    # Zwift may update to a version newer than the one advertised, that is fine
    if ! version_ge "${zwift_current_version}" "${zwift_latest_version}"; then
        msgbox error "Zwift updated to an older version than expected (Expected: ${zwift_latest_version}, Actual: ${zwift_current_version})"
        return 1
    fi

    msgbox ok "Zwift updated to version ${zwift_current_version}"
}

# Update Zwift when a newer version is available.
# Returns 0 when Zwift is up to date, was updated, or the version check was not possible,
# and 1 when the update failed.
update_zwift_if_needed() {
    gui_progress 5 "Checking for Zwift updates"

    local zwift_latest_version
    if ! zwift_latest_version="$(get_latest_version)"; then
        msgbox warning "Unable to check for Zwift updates, starting the installed version"
        return 0
    fi

    local zwift_current_version
    zwift_current_version="$(get_current_version)"
    if version_ge "${zwift_current_version}" "${zwift_latest_version}"; then
        msgbox ok "Zwift is up to date (installed ${zwift_current_version}, latest ${zwift_latest_version})"
        return 0
    fi

    update_zwift_using_launcher "${zwift_latest_version}" "${zwift_current_version}"
}

# Remove files that are only needed during the installation
cleanup_install_artifacts() {
    rm -f -- "${SETUP_EXE}" || true
    rm -rf -- "${XDG_CACHE_HOME:?}/winetricks" || true
    if [[ -n ${WINE_USER_HOME:-} ]]; then
        rm -rf -- "${WINE_USER_HOME}/AppData/Local/Temp/"* || true
    fi
}
