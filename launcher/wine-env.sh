#!/usr/bin/env bash
# Command wrapper for the snap: sets up the environment for wine and runs the given command.
# Used as the command-chain of every app so wine, winetricks and the i386 libraries are found.
set -uo pipefail

export WINE_ROOT="${SNAP}/opt/wine-devel"
export PATH="${SNAP}/bin:${WINE_ROOT}/bin:${SNAP}/usr/bin:${SNAP}/usr/games:${PATH}"

# wine is a multilib install: 64-bit and 32-bit libraries must both be found
libs=(
    "${WINE_ROOT}/lib"
    "${WINE_ROOT}/lib64"
    "${SNAP}/usr/lib/x86_64-linux-gnu"
    "${SNAP}/usr/lib/i386-linux-gnu"
    "${SNAP}/lib/x86_64-linux-gnu"
    "${SNAP}/lib/i386-linux-gnu"
)
ld_path="$(IFS=:; echo "${libs[*]}")"
export LD_LIBRARY_PATH="${LD_LIBRARY_PATH:+${LD_LIBRARY_PATH}:}${ld_path}"

# Do not create menu entries or desktop icons on the host
export WINEDLLOVERRIDES="${WINEDLLOVERRIDES:-winemenubuilder.exe=d}"
export WINEARCH="${WINEARCH:-win64}"

# Enable video decoding in mesa (see upstream Dockerfile)
export ANV_DEBUG="${ANV_DEBUG:-video-decode}"
export RADV_EXPERIMENTAL="${RADV_EXPERIMENTAL:-video_decode}"

# gstreamer needs to find the plugins shipped in the snap (h264 decoding for video routes)
export GST_PLUGIN_SYSTEM_PATH="${SNAP}/usr/lib/x86_64-linux-gnu/gstreamer-1.0"
export GST_PLUGIN_SCANNER="${SNAP}/usr/lib/x86_64-linux-gnu/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner"

# Audio goes through the audio-playback interface, which allows the host pulseaudio socket.
# Inside a snap XDG_RUNTIME_DIR is /run/user/<uid>/snap.<name>, where libpulse would look
# for the socket, so point it at the real one.
export PULSE_SERVER="${PULSE_SERVER:-unix:/run/user/$(id -u)/pulse/native}"
export PULSE_CLIENTCONFIG="${PULSE_CLIENTCONFIG:-${SNAP}/etc/zwift/pulse-client.conf}"

mkdir -p "${SNAP_USER_COMMON}" || exit 1

exec "$@"
