#!/usr/bin/env bash
# Closure-based pruning for the zwift snap.
#
# Called from runtime-prune's override-prime with the prime directory as the
# only argument. It walks the DT_NEEDED closure of every ELF file that can be
# reached at runtime (wine itself, the launcher helpers and the trees that are
# dlopened by name) and deletes every library in the prime pool that nothing
# keeps alive. These are exactly the libraries snapcraft's linter reports as
# "unused library", for example the onnxruntime/tensorflow/dnnl stack that
# only the gstreamer neural network plugins link against.
#
# Kept regardless of the closure, because they are dlopened by name at
# runtime and would look unreachable to the walk:
#   - gstreamer plugins (except the denylist below)
#   - gdk-pixbuf loaders, gio modules, gtk modules
#   - gconv modules and glibc nss modules (dlopened by libc)
#   - the samba tree (ntlm_auth loads samba modules)
#
# The script is idempotent: running it again on an already pruned prime
# directory removes nothing.
set -euo pipefail

prime="${1:?usage: prune-closure.sh <prime-dir>}"

# gstreamer plugins whose only users are codecs Zwift never plays. Removing
# the plugin frees its exclusive dependencies in the closure walk below:
# neural networks (onnx, tflite, dnnl), speech synthesis (flite), openaptx,
# teletext (zvbi), timecode (ltc) and sound file reading (sndfile).
gst_deny_re='lib(flite|onnxruntime|tensorflow-lite|freeaptx|zvbi|ltc|sndfile)\.so'

# Binaries of packages that nothing in the snap runs at runtime. The launcher
# only needs wine, wineboot, winepath, wineserver, cabextract,
# wget and ntlm_auth. samba ships a full tool
# chest of which ntlm_auth is the only needed program, and gnupg/debconf/ucf/
# perl/fontconfig-utils/gtk-update-icon-cache are build or packaging helpers.
removed_binaries=(
    chacl cpan5.40-x86_64-linux-gnu dbwrap_tool
    debconf debconf-apt-progress debconf-communicate debconf-copydb
    debconf-escape debconf-set-selections debconf-show
    dirmngr dirmngr-client
    fc-cache fc-cat fc-conflist fc-list fc-match fc-pattern fc-query fc-scan fc-validate
    getfacl gpg gpg-agent gpg-connect-agent gpgconf gpgsm gtk-update-icon-cache
    gtk4-update-icon-cache net nmblookup perl perl5.40-x86_64-linux-gnu perl5.40.1
    pinentry-curses samba-log-parser samba-regedit setfacl smbcontrol smbpasswd
    testparm ucf ucfq ucfr unzipsfx user-session-migration wbinfo winexe
)

# Data trees of the removed binaries and other dead weight.
removed_trees=(
    usr/share/perl
    usr/lib/x86_64-linux-gnu/perl
    usr/share/perl5
    usr/share/sounds
)

# Library search path used to resolve DT_NEEDED entries, in ld.so order.
search_dirs=(
    usr/lib/x86_64-linux-gnu
    lib/x86_64-linux-gnu
    usr/lib
    lib
    usr/lib/x86_64-linux-gnu/pulseaudio
    usr/lib/x86_64-linux-gnu/samba
)

# Trees whose files are dlopened by name: every ELF file in them is a root.
dlopen_dirs=(
    usr/lib/x86_64-linux-gnu/gstreamer-1.0
    usr/lib/x86_64-linux-gnu/gdk-pixbuf-2.0
    usr/lib/x86_64-linux-gnu/gio
    usr/lib/x86_64-linux-gnu/gtk-3.0
    usr/lib/x86_64-linux-gnu/gconv
    usr/lib/x86_64-linux-gnu/samba
    usr/lib/x86_64-linux-gnu/libproxy
    usr/lib/x86_64-linux-gnu/alsa-lib
    usr/libexec/glycin-loaders
)

declare -A keep=()
queue=()
qhead=0
needs=""
removed=0
removed_bytes=0

is_elf() {
    [[ $(head -c 4 -- "$1" | od -An -tx1 | tr -d ' \n') == "7f454c46" ]]
}

# Map a directory from an ELF runpath into the prime directory.
map_dir() {
    local dir="$1"
    dir="${dir/\$ORIGIN/$(dirname -- "$2")}"
    if [[ ${dir} == /* ]]; then
        dir="${prime}${dir}"
    else
        dir="$(dirname -- "$2")/${dir}"
    fi
    if [[ -d ${dir} ]]; then
        printf '%s' "${dir}"
    fi
}

# Parse DT_NEEDED and DT_RUNPATH/RPATH of an ELF file.
needed_of() {
    readelf -d -- "$1" 2>/dev/null \
        | sed -n 's/.*(\(NEEDED\|RPATH\|RUNPATH\)).*\[\(.*\)\].*/\1 \2/p'
}

add_root() {
    local f resolved
    f="$(readlink -f -- "$1" 2>/dev/null)" || return 0
    [[ -f ${f} && -z ${keep[${f}]+x} ]] || return 0
    is_elf "${f}" || return 0
    keep["${f}"]=1
    queue+=("${f}")
}

remove_file() {
    local bytes
    bytes="$(du -b -- "$1" 2>/dev/null | cut -f1)" || bytes=0
    printf 'prune-closure: removing %s\n' "${1#"${prime}"/}"
    rm -f -- "$1"
    removed=$((removed + 1))
    removed_bytes=$((removed_bytes + bytes))
}

# --- explicit removals ------------------------------------------------------

for bin in "${removed_binaries[@]}"; do
    if [[ -e "${prime}/usr/bin/${bin}" ]]; then
        remove_file "${prime}/usr/bin/${bin}"
    fi
done
for tree in "${removed_trees[@]}"; do
    if [[ -e "${prime}/${tree}" ]]; then
        printf 'prune-closure: removing %s\n' "${tree}"
        rm -rf -- "${prime:?}/${tree}"
    fi
done

# The libcanberra gtk module plays event sounds; without a desktop environment
# they are only noise and pull libcanberra plus the freedesktop sound theme.
find "${prime}/usr/lib/x86_64-linux-gnu/gtk-3.0" -name 'libcanberra*' -print -delete 2>/dev/null || true
find "${prime}/usr/lib/x86_64-linux-gnu/gtk-3.0" -name 'immodules.cache' -print -delete 2>/dev/null || true

# Dead trees and modules: helper programs of removed binaries and plugins that
# would fail to load anyway because their dependencies are gone. Nothing in
# the snap ever loads them:
# - usr/lib/gnupg: helper programs spawned by the removed gpg-agent
# - usr/libexec/samba: samba server rpc daemons, ntlm_auth is client only
# - libv4l: v4l1 compat preloads and tracing, wine uses libv4l2 directly
# - spa-0.2/filter-graph: pipewire plugin tree, pipewire is not in the snap
# - caca/libgl_plugin: needs libglut/libGLU, which the snap never staged
# - the four gphoto2 camlibs that need libgd, which nothing else needs
rm -rf -- "${prime}/usr/lib/gnupg" \
    "${prime}/usr/libexec/samba" \
    "${prime}/usr/lib/x86_64-linux-gnu/libv4l" \
    "${prime}/usr/lib/x86_64-linux-gnu/spa-0.2" \
    "${prime}"/usr/lib/x86_64-linux-gnu/caca/libgl_plugin.so* \
    "${prime}"/usr/lib/x86_64-linux-gnu/libgphoto2/*/ax203.so \
    "${prime}"/usr/lib/x86_64-linux-gnu/libgphoto2/*/docupen.so \
    "${prime}"/usr/lib/x86_64-linux-gnu/libgphoto2/*/st2205.so \
    "${prime}"/usr/lib/x86_64-linux-gnu/libgphoto2/*/tp6801.so

# Remove the gstreamer plugins on the denylist before the walk, so the
# libraries only they need become unreachable.
for plugin in "${prime}"/usr/lib/x86_64-linux-gnu/gstreamer-1.0/*.so; do
    [[ -e ${plugin} ]] || continue
    if needed_of "${plugin}" | grep -qE "${gst_deny_re}"; then
        remove_file "${plugin}"
    fi
done

# --- collect roots ----------------------------------------------------------

# wine itself and every helper it installs
while IFS= read -r -d '' f; do
    add_root "${f}"
done < <(find "${prime}/opt/wine-devel" -type f -print0 2>/dev/null)

# helper binaries the launcher and wine call through PATH
add_root "${prime}/bin/zwsnap-gui"
for bin in ntlm_auth cabextract wget bwrap update-mime-database \
    glycin-thumbnailer; do
    add_root "${prime}/usr/bin/${bin}"
done

# services and helpers that dbus or glib autolaunch by absolute path
while IFS= read -r -d '' f; do
    add_root "${f}"
done < <(find "${prime}/usr/libexec" -maxdepth 1 -type f -print0 2>/dev/null)

# WebKit launches its private processes and loads the injected bundle by name;
# the GUI's DT_NEEDED entries do not describe these extra dependency roots.
while IFS= read -r -d '' f; do
    add_root "${f}"
done < <(find "${prime}/usr/lib/x86_64-linux-gnu/webkitgtk-6.0" -type f -print0 2>/dev/null)

# gstreamer plugin scanner, run by winegstreamer to build its plugin registry
add_root "${prime}/usr/lib/x86_64-linux-gnu/gstreamer1.0/gstreamer-1.0/gst-plugin-scanner"

# everything dlopened by name (find instead of globs: globs stay literal when
# they do not match and are relative to the working directory, not to $prime)
for dir in "${dlopen_dirs[@]}"; do
    if [[ -d "${prime}/${dir}" ]]; then
        while IFS= read -r -d '' f; do
            add_root "${f}"
        done < <(find "${prime}/${dir}" -type f -name '*.so*' -print0 2>/dev/null)
    fi
done
# the glycin image loaders are rust binaries, not .so files
while IFS= read -r -d '' f; do
    add_root "${f}"
done < <(find "${prime}/usr/libexec/glycin-loaders" -type f -perm /111 -print0 2>/dev/null)

# glibc resolves names through nss modules and charset conversions by name
while IFS= read -r -d '' f; do
    add_root "${f}"
done < <(find "${prime}/usr/lib/x86_64-linux-gnu" -maxdepth 1 -name 'libnss_*.so*' -print0 2>/dev/null)
# nss crypto backends and root certificates, dlopened by libnss3
while IFS= read -r -d '' f; do
    add_root "${f}"
done < <(find "${prime}/usr/lib/x86_64-linux-gnu" -maxdepth 1 \
    \( -name 'libsoftokn3.so*' -o -name 'libfreebl*' -o -name 'libnssdbm3.so*' -o -name 'libnssckbi.so*' \) \
    -print0 2>/dev/null)

# --- closure walk -----------------------------------------------------------

# FIFO walk with an index instead of array slicing, which copied the whole
# queue on every pop and is O(n^2).
while [[ ${qhead} -lt ${#queue[@]} ]]; do
    f="${queue[${qhead}]}"
    qhead=$((qhead + 1))
    # candidate directories: the file's runpath first, then the default set
    candidates=()
    while IFS= read -r line; do
        kind="${line%% *}"
        value="${line#* }"
        if [[ ${kind} == NEEDED ]]; then
            needs="${needs:+${needs} }${value}"
        else
            mapped="$(map_dir "${value}" "${f}")"
            [[ -n ${mapped} ]] && candidates+=("${mapped}")
        fi
    done < <(needed_of "${f}")
    for dir in "${search_dirs[@]}"; do
        candidates+=("${prime}/${dir}")
    done

    for needed in ${needs}; do
        for dir in "${candidates[@]}"; do
            if [[ -e "${dir}/${needed}" ]]; then
                add_root "${dir}/${needed}"
                break
            fi
        done
    done
    needs=""
done

# --- delete unreachable files from the pool ---------------------------------

while IFS= read -r -d '' f; do
    resolved="$(readlink -f -- "${f}")"
    if [[ -z ${keep[${resolved}]+x} ]]; then
        remove_file "${f}"
    fi
done < <(find "${prime}/usr/lib/x86_64-linux-gnu" "${prime}/usr/lib" -maxdepth 1 \
    \( -name '*.so*' -o -name '*.a' \) \( -type f -o -type l \) -print0 2>/dev/null)

# dangling symlinks left behind by the deletions above
find "${prime}/usr/lib/x86_64-linux-gnu" "${prime}/usr/lib" -maxdepth 1 -xtype l -delete 2>/dev/null || true

printf 'prune-closure: removed %d files, %d bytes\n' "${removed}" "${removed_bytes}"
