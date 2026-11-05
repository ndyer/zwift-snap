"""Send pointer and keyboard input to a wayland compositor, using yarf's virtual input clients.

Runs inside the yarf snap (see tests/tools/yarf.sh), the same way as screenshot.py. It is a
debugging aid for driving the game by hand during development; the real pairing test uses yarf's
Robot keywords (tests/pairing/). Reads one command per line of its arguments, joined by the
literal word "then":

  click X Y       left-click at proportional position X Y (each 0.0-1.0)
  move X Y        move the pointer to proportional X Y
  type TEXT...    type the rest of the line
  key NAME...     press a key combo (xkb key names, e.g. Return, Tab)
  sleep SECONDS   wait

Usage: input.py SOCKET "click 0.5 0.6" then "type hello" then "key Return"
"""

import asyncio
import sys
import time

sys.path.insert(0, "/snap/yarf/current/lib/python3.12/site-packages")
from yarf.lib.wayland.virtual_keyboard import VirtualKeyboard  # noqa: E402
from yarf.lib.wayland.virtual_pointer import Button, VirtualPointer  # noqa: E402


async def run(socket: str, commands: list[str]) -> None:
    pointer = VirtualPointer(socket)
    keyboard = VirtualKeyboard(socket)
    await pointer.connect()
    await keyboard.connect()
    try:
        for command in commands:
            verb, _, rest = command.strip().partition(" ")
            if verb == "click":
                x, y = (float(n) for n in rest.split())
                pointer.move_to_proportional(x, y)
                pointer.button(Button.LEFT, True)
                pointer.button(Button.LEFT, False)
            elif verb == "move":
                x, y = (float(n) for n in rest.split())
                pointer.move_to_proportional(x, y)
            elif verb == "type":
                keyboard.type(rest)
            elif verb == "key":
                keyboard.key_combo(rest.split())
            elif verb == "sleep":
                time.sleep(float(rest))
            else:
                sys.exit(f"unknown command: {command!r}")
    finally:
        await keyboard.disconnect()
        await pointer.disconnect()


if len(sys.argv) < 3:
    sys.exit(__doc__)
asyncio.run(run(sys.argv[1], " ".join(sys.argv[2:]).split(" then ")))
