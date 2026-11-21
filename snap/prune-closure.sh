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
# Canonicalise once. Every path this script builds is derived from it and compared
# against the keep set, so resolving it here keeps the per-file paths stable.
prime="$(readlink -f -- "${prime}")"

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

# Data trees of the removed binaries and other dead weight. The Adwaita icon
# theme (icons and cursors) arrives transitively with libgtk-4-1; the launcher
# carries its own bundled icons in the binary (see zwsnap-gui/build.rs) and names
# no theme icons, so the whole theme is dropped here.
# The locale, man and iso-codes trees arrive transitively with the WebKit
# stage-packages of the launcher (iso-codes is pulled in by libwebkitgtk-6.0-4):
# usr/share/locale holds ~170 languages x 14 domains (WebKitGTK-6.0.mo plus 13
# iso_* catalogs), usr/share/xml/iso-codes and usr/share/iso-codes the ISO
# country/language/currency databases in XML and JSON form, usr/share/man the
# man pages of the stage-packages. Nothing in the snap reads them: the launcher
# is English-only, calls no gettext/textdomain and sets no locale environment,
# so gettext falls back to the English msgid strings, and no code touches the
# iso-codes data. Wine's own translations never ship (its po/*.mo are
# build-time inputs for wrc, compiled into PE resources); wine's runtime NLS
# data is a different tree (opt/wine-devel/share/wine/nls) and is NOT touched.
removed_trees=(
    usr/share/perl
    usr/lib/x86_64-linux-gnu/perl
    usr/share/perl5
    usr/share/sounds
    usr/share/icons/Adwaita
    usr/share/locale
    usr/share/man
    usr/share/xml/iso-codes
    usr/share/iso-codes
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
declare -A needed_cache=()
# Resolution cache for the closure walk. The same (directory, library name) pair is
# probed once per importer, so without this the walk repeats the identical stat
# thousands of times; caching turns that into an associative-array lookup.
declare -A exists_cache=()
# Symlink target cache. add_root is offered one library once per importer, and most
# staged libraries are symlinks (libfoo.so.1 -> libfoo.so.1.2.3), so without this
# every repeat forked readlink to resolve a target it had already resolved.
declare -A resolve_cache=()
queue=()
qhead=0
needs=""
needed_lines=""
removed=0
removed_bytes=0

# ELF magic, read with a shell builtin. This runs once per candidate file and the
# root scan below offers thousands, so the old head|od|tr pipeline (three forked
# processes per call) was a large part of the script's runtime.
is_elf() {
    local magic
    IFS= read -rN4 magic < "$1" 2>/dev/null || return 1
    [[ ${magic} == $'\x7fELF' ]]
}

# Whether a path exists, memoised. $1 is always an absolute path this script built
# itself, so it is a stable cache key.
have_file() {
    local cached=${exists_cache[${1}]+x}
    if [[ -n ${cached} ]]; then
        return "${exists_cache[${1}]}"
    fi
    if [[ -e ${1} ]]; then
        exists_cache["${1}"]=0
        return 0
    fi
    exists_cache["${1}"]=1
    return 1
}

# Map a directory from an ELF runpath into the prime directory.
map_dir() {
    local dir="$1" base="${2%/*}"
    dir="${dir/\$ORIGIN/${base}}"
    if [[ ${dir} == /* ]]; then
        dir="${prime}${dir}"
    else
        dir="${base}/${dir}"
    fi
    if [[ -d ${dir} ]]; then
        printf '%s' "${dir}"
    fi
}

# Parse DT_NEEDED and DT_RUNPATH/RPATH of an ELF file into $needed_lines, caching
# the result: the gstreamer denylist loop and the closure walk both read the same
# plugins, and readelf is the most expensive program this script runs. The output
# is parsed with the shell rather than piped through sed, which saves a process
# per library.
load_needed() {
    if [[ -n ${needed_cache[${1}]+x} ]]; then
        needed_lines="${needed_cache[${1}]}"
        return 0
    fi
    local raw line kind value out=""
    raw="$(readelf -d -- "$1" 2>/dev/null)" || raw=""
    while IFS= read -r line; do
        case ${line} in
            *"(NEEDED)"*) kind=NEEDED ;;
            *"(RPATH)"* | *"(RUNPATH)"*) kind=RUNPATH ;;
            *) continue ;;
        esac
        value="${line##*[}"
        value="${value%%]*}"
        out+="${kind} ${value}"$'\n'
    done <<< "${raw}"
    needed_lines="${out}"
    needed_cache["${1}"]="${out}"
}

add_root() {
    local f="$1" resolved
    # Resolve symlinks only. Every other path the script builds is already
    # canonical - prime is resolved above and no runpath in the tree uses ".." -
    # so forking readlink for each regular file was wasted work.
    if [[ -L ${f} ]]; then
        if [[ -n ${resolve_cache[${f}]+x} ]]; then
            f="${resolve_cache[${f}]}"
        else
            resolved="$(readlink -f -- "${f}" 2>/dev/null)" || return 0
            resolve_cache["${f}"]="${resolved}"
            f="${resolved}"
        fi
    fi
    [[ -f ${f} && -z ${keep[${f}]+x} ]] || return 0
    is_elf "${f}" || return 0
    keep["${f}"]=1
    queue+=("${f}")
}

remove_file() {
    local bytes="${2:-}"
    if [[ -z ${bytes} ]]; then
        # stat reports the same size as `du -b` (both are lstat-based) without
        # the extra cut process. Callers that already know the size pass it as
        # the second argument, which avoids the process entirely.
        bytes="$(stat -c %s -- "$1" 2>/dev/null)" || bytes=0
    fi
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
    load_needed "${plugin}"
    if [[ ${needed_lines} =~ ${gst_deny_re} ]]; then
        remove_file "${plugin}"
    fi
done

# --- collect roots ----------------------------------------------------------

# wine itself and every helper it installs. Its ELF files are the Unix libraries
# and the programs in bin/; the rest of the tree is PE (.dll/.exe), headers
# (.h/.idl), static archives (.a) and data, which is_elf would reject anyway.
# Filtering by name here keeps is_elf off the ~2100 non-ELF files.
while IFS= read -r -d '' f; do
    add_root "${f}"
done < <(find "${prime}/opt/wine-devel" -type f \
    \( -name '*.so' -o -path '*/bin/*' -o -name 'wine' -o -name 'wine-preloader' \) \
    -print0 2>/dev/null)

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
    load_needed "${f}"
    # candidate directories: the file's runpath first, then the default set,
    # deduplicated (a runpath often repeats a default search directory)
    candidates=()
    declare -A seen_cand=()
    needs=""
    while IFS= read -r line; do
        [[ -n ${line} ]] || continue
        kind="${line%% *}"
        value="${line#* }"
        if [[ ${kind} == NEEDED ]]; then
            needs="${needs:+${needs} }${value}"
        else
            mapped="$(map_dir "${value}" "${f}")"
            if [[ -n ${mapped} && -z ${seen_cand[${mapped}]+x} ]]; then
                seen_cand["${mapped}"]=1
                candidates+=("${mapped}")
            fi
        fi
    done <<< "${needed_lines}"
    for dir in "${search_dirs[@]}"; do
        dir="${prime}/${dir}"
        if [[ -z ${seen_cand[${dir}]+x} ]]; then
            seen_cand["${dir}"]=1
            candidates+=("${dir}")
        fi
    done

    for needed in ${needs}; do
        for dir in "${candidates[@]}"; do
            if have_file "${dir}/${needed}"; then
                add_root "${dir}/${needed}"
                break
            fi
        done
    done
    unset seen_cand
done

# --- delete unreachable files from the pool ---------------------------------

while IFS= read -r -d '' size && IFS= read -r -d '' f; do
    # A symlink is compared through its target (add_root keyed it that way); a
    # regular file's path is already canonical, so readlink is skipped for it.
    if [[ -L ${f} ]]; then
        resolved="$(readlink -f -- "${f}")"
    else
        resolved="${f}"
    fi
    if [[ -z ${keep[${resolved}]+x} ]]; then
        remove_file "${f}" "${size}"
    fi
done < <(find "${prime}/usr/lib/x86_64-linux-gnu" "${prime}/usr/lib" -maxdepth 1 \
    \( -name '*.so*' -o -name '*.a' \) \( -type f -o -type l \) -printf '%s\0%p\0' 2>/dev/null)

# dangling symlinks left behind by the deletions above
find "${prime}/usr/lib/x86_64-linux-gnu" "${prime}/usr/lib" -maxdepth 1 -xtype l -delete 2>/dev/null || true

printf 'prune-closure: removed %d files, %d bytes\n' "${removed}" "${removed_bytes}"
