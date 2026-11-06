#!/usr/bin/env bash
# snapimport.sh: prepare *.snap files in the current directory for pkgproxy's
# snap cache, keyed by their sha3-384 digest.
#
# Usage: snapimport.sh [--dry-run]
#
# Renames each *.snap to <hex> beside a <hex>.meta.json sidecar, without
# copying files or writing to the system cache. Run in the cache's sha3-384
# directory, or move the resulting pairs there afterwards.
#
# Requires: python3 (sha3-384 via hashlib), unsquashfs (squashfs-tools),
# yq (YAML parsing).

set -euo pipefail

usage() {
    echo "Usage: snapimport.sh [--dry-run]" >&2
    echo >&2
    echo "Renames *.snap files in the current directory to their sha3-384 digest" >&2
    echo "and writes a <digest>.meta.json sidecar beside each." >&2
    echo "Existing destination files are never overwritten." >&2
    echo >&2
    echo "Options:" >&2
    echo "  --dry-run         print what would be renamed, don't write" >&2
    echo "  -h, --help        show this help" >&2
}

dry_run=0

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run) dry_run=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option: $1" >&2; usage; exit 2 ;;
    esac
done

command -v python3 >/dev/null 2>&1 || {
    echo "snapimport.sh: python3 not found in PATH" >&2
    exit 1
}
command -v unsquashfs >/dev/null 2>&1 || {
    echo "snapimport.sh: unsquashfs not found in PATH (install squashfs-tools)" >&2
    exit 1
}

command -v yq >/dev/null 2>&1 || {
    echo "snapimport.sh: yq not found in PATH" >&2
    exit 1
}

# Compute sha3-384 hex digest of a file (via python3 hashlib.sha3_384).
sha3_384_hex() {
    python3 -c "
import hashlib, sys
h = hashlib.sha3_384()
with open(sys.argv[1], 'rb') as snap:
    for chunk in iter(lambda: snap.read(1024 * 1024), b''):
        h.update(chunk)
print(h.hexdigest())
" "$1"
}

# Extract name and revision from a .snap file's meta/snap.yaml via
# unsquashfs -cat (no temp dir needed).
extract_meta() {
    local file="$1"
    local yaml name rev
    yaml="$(unsquashfs -cat "$file" meta/snap.yaml)" || {
        echo "snapimport.sh: cannot read meta/snap.yaml from $file" >&2
        return 1
    }

    name="$(printf '%s\n' "$yaml" | yq -r '.name // ""')"
    rev="$(printf '%s\n' "$yaml" | yq -r '.revision // 0')"

    if [[ ! "$name" =~ ^[a-z0-9][a-z0-9-]*$ ]] || [[ ! "$rev" =~ ^[0-9]+$ ]]; then
        echo "snapimport.sh: invalid name or revision in $file" >&2
        return 1
    fi
    printf '%s %s\n' "$name" "$rev"
}

tmp_meta=""
trap 'if [ -n "$tmp_meta" ]; then rm -f -- "$tmp_meta"; fi' EXIT

count=0
shopt -s nullglob
for file in ./*.snap; do
    [ -f "$file" ] || continue
    stem="$(basename "$file" .snap)"
    hex="$(sha3_384_hex "$file")"
    size="$(stat -c %s "$file")"
    obj_path="./$hex"
    meta_path="$obj_path.meta.json"

    if [ -e "$obj_path" ] || [ -L "$obj_path" ] || [ -e "$meta_path" ] || [ -L "$meta_path" ]; then
        echo "snapimport.sh: destination already exists for $file: $obj_path or $meta_path" >&2
        exit 1
    fi

    metadata="$(extract_meta "$file")"
    read -r name rev <<< "$metadata"
    now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

    if [ "$dry_run" -eq 1 ]; then
        echo "rename: $stem ($size bytes) -> $hex (name=$name, rev=$rev)"
        count=$((count + 1))
        continue
    fi

    # Metadata sidecar mirrors internal/store.ObjectMeta JSON shape.
    tmp_meta="$(mktemp ./.snapimport-meta.XXXXXX)"
    python3 - "$hex" "$size" "$now" "$name" "$rev" > "$tmp_meta" <<'PY'
import json
import sys

digest, size, now, name, revision = sys.argv[1:]
json.dump({
    "ContentHash": "sha3-384/" + digest,
    "ContentType": "", "ETag": "", "Immutable": True,
    "LastModified": "0001-01-01T00:00:00Z", "MaxAge": 0,
    "Size": int(size), "StoredAt": now, "Name": name,
    "Revision": int(revision),
}, sys.stdout)
sys.stdout.write("\n")
PY
    mv -- "$file" "$obj_path"
    mv -- "$tmp_meta" "$meta_path"
    tmp_meta=""

    echo "rename: $stem ($size bytes) -> $hex (name=$name, rev=$rev)"
    count=$((count + 1))
done

if [ "$dry_run" -eq 1 ]; then
    echo "snapimport.sh: would rename $count snap(s)" >&2
else
    echo "snapimport.sh: renamed $count snap(s)" >&2
fi