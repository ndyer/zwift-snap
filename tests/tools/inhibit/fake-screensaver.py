import os
import sys

import dbus
import dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

LOG = os.environ["INHIBIT_LOG"]
# Which of the two services the launcher may use is being faked. The launcher
# tries the freedesktop one first, so only one may be on the bus at a time.
SERVICE = os.environ.get("FAKE_INHIBIT_SERVICE", "freedesktop")
COOKIE = 41 if SERVICE == "freedesktop" else 77


def record(line):
    with open(LOG, "a") as handle:
        handle.write(line + "\n")


class Saver(dbus.service.Object):
    """`org.freedesktop.ScreenSaver`, as implemented by GNOME and KDE."""

    @dbus.service.method("org.freedesktop.ScreenSaver", in_signature="ss", out_signature="u")
    def Inhibit(self, app, reason):
        record(f"freedesktop-inhibit {app} | {reason}")
        return COOKIE

    @dbus.service.method("org.freedesktop.ScreenSaver", in_signature="u", out_signature="")
    def UnInhibit(self, cookie):
        record(f"freedesktop-uninhibit {cookie}")


class SessionManager(dbus.service.Object):
    """`org.gnome.SessionManager`, which the launcher falls back to."""

    @dbus.service.method("org.gnome.SessionManager", in_signature="susu", out_signature="u")
    def Inhibit(self, app_id, toplevel_xid, reason, flags):
        record(f"gnome-inhibit {app_id} | {toplevel_xid} | {reason} | {flags}")
        return COOKIE

    @dbus.service.method("org.gnome.SessionManager", in_signature="u", out_signature="")
    def Uninhibit(self, cookie):
        record(f"gnome-uninhibit {cookie}")


DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
# The BusName and the object have to stay referenced, or they are collected and
# the name is dropped from the bus.
if SERVICE == "gnome":
    name = dbus.service.BusName("org.gnome.SessionManager", bus)
    service = SessionManager(bus, "/org/gnome/SessionManager")
else:
    name = dbus.service.BusName("org.freedesktop.ScreenSaver", bus)
    service = Saver(bus, "/org/freedesktop/ScreenSaver")
print(f"fake {SERVICE} service ready", flush=True)
GLib.timeout_add_seconds(int(os.environ.get("INHIBIT_SECONDS", "60")), lambda: sys.exit(0))
GLib.MainLoop().run()