# Preface

The screensaver inhibitor test: fake session services on a private D-Bus, and the
two marshalling mistakes it exists to catch. Read this before changing
`zwsnap-gui/src/inhibit.rs` or the arguments it sends.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## What it covers

`tests/inhibit.sh` starts a fake `org.freedesktop.ScreenSaver` and then a fake
`org.gnome.SessionManager` on a private session bus, and runs the opt-in cargo
test in `zwsnap-gui/tests/inhibit.rs` against each in turn. The test acquires an
inhibitor in fullscreen mode, releases it, and asserts on what the fake recorded:
the application name, the reason, the GNOME toplevel xid and flags, and that the
cookie returned by `Inhibit` is the one passed to `UnInhibit`/`Uninhibit`.

It needs no display, no wine and no installed snap, because the whole subject is
the D-Bus plumbing. The workshop action is `workshop run -- inhibit` and CI runs
it next to `gtk-preview`.

## Why it is opt-in

`cargo test` alone does nothing here: without `ZWIFT_INHIBIT_SERVICE_LOG` the
test prints why and returns, so `cargo test` and the CI unit-test step keep no
D-Bus service dependency. The script sets the variable, the backend name
(`ZWIFT_INHIBIT_BACKEND`) and the recording path (`INHIBIT_LOG`) the fake appends
to.

## The mistakes it catches

Both make the desktop ignore the request without any error the launcher could
report, which is why they are not left to review:

- **Parameters must be a tuple.** `UnInhibit` takes one `u`, and passing a bare
  `u32` variant makes `g_dbus_proxy_call_sync` assert
  (`parameters == NULL || g_variant_is_of_type (parameters, G_VARIANT_TYPE_TUPLE)`)
  and the call never leaves the process. The correct value is `(cookie,)`.
- **The reply is a tuple too.** Even a single out argument arrives as `(u)`, so
  the cookie has to be read with `reply.get::<(u32,)>()`; `reply.get::<u32>()`
  returns `None`, and the launcher treats that as "no cookie" and falls through to
  the next backend.

## The fake

`tests/tools/inhibit/fake-screensaver.py` picks the service from
`FAKE_INHIBIT_SERVICE` (`freedesktop` or `gnome`), replies with a fixed cookie
(41 and 77, so the assertion also proves the right backend was used) and appends
one line per call to `INHIBIT_LOG`. Two things about it are load-bearing: the
`BusName` and the service object must stay referenced, or Python collects them
and the name leaves the bus, and the script waits for the name to have an owner
(`NameHasOwner`) instead of sleeping.