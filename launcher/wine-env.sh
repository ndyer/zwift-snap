#!/usr/bin/env bash
# Command wrapper for the snap: sets up the environment for wine and runs the given command.
# Used as the command-chain of every app so wine, winetricks and the libraries are found.
set -uo pipefail

export WINE_ROOT="${SNAP}/opt/wine-devel"
export PATH="${SNAP}/bin:${WINE_ROOT}/bin:${SNAP}/usr/bin:${SNAP}/usr/games:${PATH}"

# wine is a new style WoW64 build: only 64-bit unix libraries are needed, 32-bit Windows
# programs run through the PE files in lib/wine/i386-windows
#
# Some Ubuntu libraries find their private helper libraries through an absolute RUNPATH such as
# /usr/lib/x86_64-linux-gnu/pulseaudio, which does not exist inside the snap:
# - pulseaudio: libpulse needs libpulsecommon, without it wine has no audio
# - samba: ntlm_auth (needed by zwift/wine) and its libraries
libs=(
    "${WINE_ROOT}/lib"
    "${SNAP}/usr/lib/x86_64-linux-gnu"
    "${SNAP}/lib/x86_64-linux-gnu"
    "${SNAP}/usr/lib/x86_64-linux-gnu/pulseaudio"
    "${SNAP}/usr/lib/x86_64-linux-gnu/samba"
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

# GTK needs the data of the staged packages: GSettings schemas, icon themes and
# the glycin loader configuration. Snaps without the desktop helpers do not have $SNAP/usr/share
# in XDG_DATA_DIRS.
export GSETTINGS_SCHEMA_DIR="${SNAP}/usr/share/glib-2.0/schemas"
export XDG_DATA_DIRS="${XDG_DATA_DIRS:+${XDG_DATA_DIRS}:}${SNAP}/usr/share"

# gdk-pixbuf loaders, same fix as ubuntu/snapcraft-desktop-helpers common/desktop-exports.
# The loaders.cache that Debian creates in a postinst script is not part of the staged
# packages, and it holds absolute paths, so it is generated here. It is regenerated when it is
# missing or points to another snap revision.
pixbuf_dir="${SNAP}/usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0"
export GDK_PIXBUF_MODULE_FILE="${XDG_CACHE_HOME:-${SNAP_USER_COMMON}/.cache}/gdk-pixbuf-loaders.cache"
export GDK_PIXBUF_MODULEDIR="${pixbuf_dir}/2.10.0/loaders"
if [[ -x ${pixbuf_dir}/gdk-pixbuf-query-loaders ]] \
    && ! grep -qxF "# LoaderDir = ${GDK_PIXBUF_MODULEDIR}" "${GDK_PIXBUF_MODULE_FILE}" 2> /dev/null; then
    mkdir -p "$(dirname "${GDK_PIXBUF_MODULE_FILE}")" || exit 1
    # write to a temporary file first so that parallel starts never see a half written cache
    if "${pixbuf_dir}/gdk-pixbuf-query-loaders" > "${GDK_PIXBUF_MODULE_FILE}.$$"; then
        mv -f "${GDK_PIXBUF_MODULE_FILE}.$$" "${GDK_PIXBUF_MODULE_FILE}"
    else
        rm -f "${GDK_PIXBUF_MODULE_FILE}.$$"
    fi
fi

# MIME database, same recipe as ubuntu/snapcraft-desktop-helpers common/desktop-exports.
# gdk-pixbuf and GTK use it to recognise image files (the png loader, the symbolic icons of the
# window buttons). The staged shared-mime-info package only has the source files, the
# mime.cache that update-mime-database builds from them is missing. The database is built in a
# per revision directory (SNAP_USER_DATA) that is added to XDG_DATA_DIRS. XDG_DATA_HOME is not
# changed because wine uses it too.
mime_data_dir="${SNAP_USER_DATA:-${SNAP_USER_COMMON}}/.local/share"
if [[ ! -f ${SNAP}/usr/share/mime/mime.cache ]] && command -v update-mime-database > /dev/null; then
    if [[ ! -f ${mime_data_dir}/mime/mime.cache ]]; then
        mkdir -p "${mime_data_dir}" || exit 1
        # build in a temporary directory first so that parallel starts never see a half built cache
        mime_tmp="${mime_data_dir}/mime.$$"
        rm -rf "${mime_tmp}"
        if cp -dR --preserve=timestamps "${SNAP}/usr/share/mime" "${mime_tmp}" \
            && update-mime-database "${mime_tmp}" > /dev/null 2>&1; then
            rm -rf "${mime_data_dir}/mime"
            mv -f "${mime_tmp}" "${mime_data_dir}/mime"
        else
            rm -rf "${mime_tmp}"
        fi
    fi
    if [[ -f ${mime_data_dir}/mime/mime.cache ]]; then
        export XDG_DATA_DIRS="${mime_data_dir}${XDG_DATA_DIRS:+:${XDG_DATA_DIRS}}"
    fi
fi

exec "$@"
