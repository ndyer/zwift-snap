# Wayland touch input — implementation plan (Option A)

## Goal

Handle **basic single-touch input** in `dlls/winewayland.drv` (the driver the snap's
Wine build uses) so touch-only devices can drive Zwift. Gestures, swipes and pinch
are explicitly out of scope.

## Key findings (verified against wine-11.21 fork `zwift-radios-rebased`)

Wine has **no `WM_TOUCH` machinery**:

- No `INPUT_TOUCH` type; `union hw_input` in `server/protocol.def` has only
  `kbd` / `mouse` / `hw` members.
- `GetTouchInputInfo`, `CloseTouchInputHandle`, `RegisterTouchWindow`,
  `IsTouchWindow`, `UnregisterTouchWindow` are all `FIXME` +
  `ERROR_CALL_NOT_IMPLEMENTED` stubs in `dlls/user32/input.c`.
- `InjectTouchInput`, `NtUserInjectTouchInput`, `NtMITSynthesizeTouchInput` and
  every gesture API are `@ stub`.
- `WM_TOUCH` exists only as a constant (`winuser.rh`) and a spy name
  (`win32u/spy.c`).

There **is**, however, a working pointer-message path, used by `winex11.drv`:

- `winex11.drv/mouse.c` `X11DRV_TouchEvent()` maps XInput2 touch
  begin/update/end to `WM_POINTERDOWN/UPDATE/UP` sent as `INPUT_HARDWARE`.
- `win32u` + `server/queue.c` implement `WM_POINTER*` queueing, per-pointer
  tracking (`NtUserGetPointerInfoList`, `NtUserGetPointerType`, …), and they
  auto-synthesise `WM_MOUSEMOVE` / `WM_LBUTTONDOWN` / `WM_LBUTTONUP` for the
  primary pointer.

Other drivers: `wineandroid.drv` translates touch → mouse at the Java layer;
`winemac.drv` has none; `winebus.sys` only *classifies* udev digitiser devices
(`ID_INPUT_TOUCHSCREEN`/`TOUCHPAD`) into HID usages.

> **Correction.** An earlier draft of this plan asserted that "Wine has no touch
> machinery at all". That is **wrong**. The accurate statement is the one above:
> no `WM_TOUCH` machinery, but a functional `WM_POINTER*` path that
> `winex11.drv` already uses.

## Chosen approach — Option A: touch → mouse emulation

Driver-only change. Map the **primary `wl_touch` contact** onto the existing,
well-tested `INPUT_MOUSE` path (absolute move plus a synthetic left
button press/release). No changes to `server/`, `win32u/` or `user32/`.

Rationale: Zwift is a DirectX title that reads cursor / raw input, and moving the
real cursor is the most reliable behaviour for it. (See "Alternatives" for the
`WM_POINTER*` route.)

## Design

- `struct wayland_touch { struct wl_touch *wl_touch; HWND focused_hwnd;
  int32_t primary_id; pthread_mutex_t mutex; }` in `waylanddrv.h`, held in
  `process_wayland.touch`.
- Bind `wl_touch` when `WL_SEAT_CAPABILITY_TOUCH` is present (the seat is bound to
  `min(version, 8)`); release it on capability loss and on registry global removal.
- New `wayland_touch.c`; add it to `Makefile.in` `SOURCES`.
- Track only the first active contact; ignore additional simultaneous contacts.
- Motion: absolute move with the same `map_point_from_surface()` + `window.rect`
  clamp used by `pointer_handle_motion_internal()`.
- Frame: flush accumulated motion with `MOUSEEVENTF_MOVE_NOCOALESCE`, mirroring
  `pointer_handle_frame()`.
- `shape`/`orientation` (wl_touch v6+) are no-ops but must have non-NULL handlers,
  because the seat can be bound up to v8.

## Files touched

| File | Change |
|---|---|
| `dlls/winewayland.drv/waylanddrv.h` | `struct wayland_touch`, `process_wayland.touch`, decls |
| `dlls/winewayland.drv/wayland.c` | seat capability branch, global-remove, mutex init |
| `dlls/winewayland.drv/Makefile.in` | add `wayland_touch.c` to `SOURCES` |
| `dlls/winewayland.drv/wayland_touch.c` | new file — the translation logic |

## Status

Implemented and committed on the wine fork branch **`zwift-wayland-touch`**
(commit `0d90c2d`), based on `a898c65` (the tip of `zwift-radios-rebased`, which
already includes the axis-scroll → wheel change).

- Not yet built or verified.
- The `WS_TOUCH` field reset in `wayland_touch.c` is deliberately field-by-field
  (not `memset`): the mutex must be preserved and `primary_id` must be `-1`.

## Testing

- Build with `wine-src` checked out on `zwift-wayland-touch`; run
  `workshop run clean` first if the `SOURCES` change did not re-run `configure`.
- Manual: touch a touchscreen — the cursor should move and left-clicks should fire.

## Alternatives / follow-ups

- **Option B — `WM_POINTER*` messages** (what `winex11.drv` does): emit
  `INPUT_HARDWARE` + `WM_POINTERDOWN/UPDATE/UP`. More idiomatic and gives real
  Win32 pointer-API support, but `queue_pointer_message()` sets the message
  coordinates from the desktop cursor and never moves it, so a game relying on
  cursor position sees button events with no pointer movement. A hybrid
  (emit `WM_POINTER*` *and* move the cursor) is possible.
- **Android-style**: translate to mouse entirely in a front-end layer.
- Could gate emulation behind a setting if it should ever be optional.
