#!/bin/sh
# Drive the launcher's screensaver inhibitor against fake session services.
#
# The launcher talks to org.freedesktop.ScreenSaver and falls back to
# org.gnome.SessionManager, neither of which exists outside a running desktop.
# This starts each one in turn on a private session bus and runs the opt-in
# integration test in zwsnap-gui/tests/inhibit.rs against it, once per backend.
#
# Nothing here needs a display, wine or the snap: it exercises the D-Bus
# plumbing, which is where the argument marshalling lives.
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)

# The fakes have to own their names on the same bus the launcher connects to. A
# private session bus keeps them out of the developer's own desktop, where the
# real services would answer instead.
if [ -z "${INHIBIT_PRIVATE_BUS:-}" ] && command -v dbus-run-session >/dev/null; then
    INHIBIT_PRIVATE_BUS=1 exec dbus-run-session -- "$0" "$@"
fi

# Wait for the fake to own its name instead of sleeping for a fixed period.
wait_for() {
    i=0
    while [ "$i" -lt 100 ]; do
        if dbus-send --session --dest=org.freedesktop.DBus --type=method_call --print-reply \
            /org/freedesktop/DBus org.freedesktop.DBus.NameHasOwner \
            "string:$1" 2>/dev/null | grep -q 'boolean true'; then
            return 0
        fi
        i=$((i + 1))
        sleep 0.1
    done
    echo "timed out waiting for $1" >&2
    return 1
}

# run <backend> <service name>
run() {
    backend=$1
    service=$2
    INHIBIT_LOG=$(mktemp -t "inhibit-$backend.XXXXXX")
    export INHIBIT_LOG
    export ZWIFT_INHIBIT_SERVICE_LOG="$INHIBIT_LOG"
    export ZWIFT_INHIBIT_BACKEND="$backend"

    echo "=== $backend ($service) ==="
    FAKE_INHIBIT_SERVICE="$backend" INHIBIT_SECONDS=60 \
        python3 "$repo/tests/tools/inhibit/fake-screensaver.py" &
    fake=$!
    trap 'kill $fake 2>/dev/null || true' EXIT INT TERM

    wait_for "$service"
    cargo test --manifest-path "$repo/zwsnap-gui/Cargo.toml" --features gtk-ui \
        --test inhibit -- --nocapture

    echo "--- what the fake saw ---"
    cat "$INHIBIT_LOG"
    kill "$fake" 2>/dev/null || true
    wait "$fake" 2>/dev/null || true
    trap - EXIT INT TERM
}

run freedesktop org.freedesktop.ScreenSaver
run gnome org.gnome.SessionManager