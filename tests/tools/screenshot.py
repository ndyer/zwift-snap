"""Grab one frame of a wayland compositor to a PNG, using yarf's screencopy client.

Runs inside the yarf snap (see tests/tools/yarf.sh), whose python has pywayland and
yarf.lib.wayland.screencopy. Usage: screenshot.py SOCKET OUTPUT.png
"""

import asyncio
import sys

sys.path.insert(0, "/snap/yarf/current/lib/python3.12/site-packages")
from yarf.lib.wayland.screencopy import Screencopy  # noqa: E402


async def main(socket: str, output: str) -> None:
    screencopy = Screencopy(socket)
    await screencopy.connect()
    try:
        image = await screencopy.grab_screenshot()
        image.save(output)
    finally:
        await screencopy.disconnect()


if len(sys.argv) != 3:
    sys.exit(__doc__)
asyncio.run(main(sys.argv[1], sys.argv[2]))
