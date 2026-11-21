# Preface

The launcher can remember several Zwift riders and show a picker before the game starts: a grid of rider cards over Zwift artwork, each with an avatar, a name, three stat chips, a level/FTP/weight line and a small remove icon in its top-right corner. The whole card is the launch target - clicking anywhere on it (except the remove icon) selects that rider.

This file covers the account store (`accounts.rs`), the picker widget (`gui/accounts_panel.rs`), the launcher-side wiring in `gui.rs`, and the GTK4 traps that made two real bugs.

IMPORTANT: Read the `.kb/agents.md` file.


# Laws

## §1 - Selection Copies The Launch Token

Nothing else in the launcher knows about accounts. `LaunchConfig.token_file` is still `$SNAP_USER_COMMON/pairing-token.json` and `launch.rs` reads that file only, so `Store::select` copies `accounts/<id>/token.json` over it (atomically, 0600) and remembers `last_used`. Never launch from an account directory directly and never make the worker account-aware.

## §2 - Zero Accounts Falls Back To The WebView Login

The picker is only shown when the store holds at least one account. With none, `Event::LoginRequired` starts the WebView login and leaves the picker hidden, so a fresh install (and a store whose accounts were all forgotten) logs in through the WebView. The runtime/preview/close GTK suites cover that path.

## §3 - Nothing Here May Block A Launch

Every account feature is best-effort. Profile refresh, avatar download and legacy migration all swallow their errors (debug log only) because a rider must always be able to start the game offline. A failure keeps the cached card data rather than clearing it.


# Concepts

## Store layout

`Store::open($SNAP_USER_COMMON)` points at:

- `accounts.json` - `{ "version": 1, "last_used": "<id>", "accounts": [...] }`. Written atomically (temp file + rename) with 0600.
- `accounts/<hex id>/token.json` - that rider's token, 0600.
- `accounts/<hex id>/avatar.<ext>` - cached avatar, 0600.
- `pairing-token.json` - the launch token, shared with the rest of the launcher.

The directory key is the account id hex-encoded, so a hostile `sub` claim can never contain `/` or `..`. `Account` holds only the fields a card draws: `name`, `first_name`, `last_name`, `email`, `avatar_url`, `avatar`, `use_metric`, `distance_meters`, `elevation_meters`, `time_minutes`, `weight_grams`, `ftp`, `level`, `level_progress`, `height_millimeters`. Loading is tolerant: a missing or broken `accounts.json` is an empty store, a single bad entry is skipped, and a `last_used` naming a vanished account is dropped.

## Identity and naming

`identity_of` prefers the token's `sub` claim, then the profile's `public_id`, then its numeric `id`, then the email. Claims win because they survive a profile that could not be fetched. `display_name` prefers `"First Last"` from the profile and falls back to `preferred_username` and then to `Zwift rider`.

## Levels

The profile's `achievement_level` (protobuf field 49) packs the level as `level * 100 + progress`; zwift-offline's `api_users_login` derives the level with `achievement_level // 100`. `merge_profile` therefore stores `level = raw / 100` and `level_progress = raw % 100` (0-99). The remainder is *assumed* to be percent toward the next level (which is what the card's bar draws), but that has not been verified against a live profile. A raw stored `level` (from an older build) self-heals on the next profile refresh.

## The picker

`AccountsPanel` owns the `Store` (behind `Rc<RefCell<..>>`), a `gtk::FlowBox` grid (`insert(&card, -1)` - `append` needs GTK 4.6, past this crate's API level) and an "Add another account" button, which the launcher places in the window header next to the log toggle and Settings. The panel exposes that button via `add_button()` and shows/hides it with the grid via `set_visible()`.

The panel is placed at the top of the window, in the space the install tiles occupied, with the WebView kept below it: the picker appears *above* the WebView rather than replacing it. `refresh()` clears the grid and rebuilds one card per account; the store is the only state.

A card is a `gtk::Overlay`: the base layer is a `gtk::Button` (CSS class `account-card`) that *is* the whole card and launches the rider when clicked, so there is no separate Play button. The card's content - a vertical box with a horizontal top row (avatar, or an initials badge, beside a vertical info column with the name row, an optional "Last used" pill, three chips and a `dim-label` meta line) - is set as that button's child, so the box inherits the button's click. The remove control is a `gtk::Button` (`zwsnap-remove-symbolic`, CSS class `account-remove`) in the overlay's top-right corner (`halign` End, `valign` Start); it is icon-only by default and shows the text "Remove" in text mode. The info column reserves room for it with `set_margin_end(28)` so text never runs under the control. Each chip (`chip(icon, label, value)`) shows a bundled icon (`zwsnap-distance/-elevation/-time-symbolic`) beside the stat's *value*, or the label and value together in text mode: the value comes from `stats::distance`, `stats::elevation` and `stats::duration` (`233 km`, `8,018 ft`, `13h`), with metric or imperial chosen by the account's `use_metric` (default metric), and the label words (`Distance`, `Elevation`, `Time`) live in `stats::DISTANCE_LABEL` and friends. Unset values render as `-`. When the account carries a `level` and `level_progress`, a slim `gtk::ProgressBar` (CSS `account-level-bar`, no visible text, tooltip naming the next level) is appended under the meta line, its fraction from `stats::level_fraction(progress)`.

Avatars are decoded at up to 72 by 72 pixels and displayed in a 72-pixel square; invalid images fall back to initials. Cards and the grid align to the start instead of filling the available width, and long names and metadata wrap. The compositor-backed `card_bounds_large_avatar_and_wraps_long_name` test checks the image allocation, card width, name wrapping and grid spacing.

The grid's height is pinned by `AccountsPanel::update_height`: a `FlowBox` reports its natural height for its *natural* width, which packs every card onto one line, so the `ScrolledWindow`'s `min_content_height` is set from the grid measured at the width it actually has (capped at `MAX_GRID_HEIGHT`). The width is only known after allocation, so `request_height_update` retries on a short timer until it is.

Remove shows a `gtk::MessageDialog` whose confirm button is labelled **Remove account**, then `Store::remove` deletes the directory, clears `pairing-token.json` when that account was selected or when it was the last account, saves and rebuilds the grid. The remove control is icon-only (its tooltip names the rider), so it never competes with the card's name text as an OCR click target; because every card's remove icon is identical, automated suites must identify the *removed* card by position (first match) and the *surviving* card by name.

## Selecting and adding

The panel is given two callbacks. `select(id)` runs `Store::select` + `save` and answers the worker's pending login decision with `true`. `add()` switches the login panel to `SessionMode::Ephemeral` - a cookie-less WebView, so the current rider's SSO cookie cannot silently sign the new login back into itself - and starts a login whose result lands in an `mpsc` channel polled by `glib::timeout_add_local(50ms)`. This is the only case besides the gate's own login where the WebView leaves its embed page; when the login finishes, `LoginPanel::show_embed` rebuilds it on the persistent session and reloads `EMBED_URL`.

`finish_add_account` then reads the launch token back, derives claims, fetches the profile best-effort, upserts the account, downloads the avatar, selects the new account and answers the decision. A profile failure still remembers the account, named from its claims.

## Profile refresh

The picker opens with cached data, then `refresh_accounts` (in `gui.rs`) fetches each profile on a background thread. It downloads an avatar when the cached file is missing or the profile URL changes, and retries a missing image using the cached URL when the profile cannot be fetched. Results are posted back on a channel polled every 100 ms; stale results for removed or changed accounts are discarded. `Store::refresh` merges profile fields while keeping omitted values. A failed image replacement keeps the old file and URL so the next refresh retries it. Profile and image failures are logged at debug level without blocking launch.

## Legacy migration

An install from before the picker has `pairing-token.json` but no `accounts.json`. At startup, before the login panel is built, the launcher loads the store and, when it is empty, derives claims from that token and calls `Store::import_legacy`, which adopts it as the first account (`upsert` with an empty profile) and marks it last used. Without this an upgrading rider would be shown the WebKit login again despite a valid token. Because the store is loaded before the embed page is chosen, an upgraded install is never sent to the registration page. `Store::remove` clears the launch token once the last account is forgotten, so an emptied store is not re-adopted here.

## GTK4 traps

- `GtkDialog`/`GtkMessageDialog` no longer hide themselves when a response fires: the handler must call `confirm.close()`. Otherwise the modal stays on screen, swallows the next click and the launch never happens (the removal itself still succeeds, which makes the bug confusing).
- `CssProvider::load_from_data` (not `load_from_string`), installed with `gtk::style_context_add_provider_for_display` at `STYLE_PROVIDER_PRIORITY_APPLICATION`.
- The crate's API level has no `v4_6`/`v4_8` gates, so use `FlowBox::insert` and `MessageDialog` rather than the newer shorthands.

## Testing

`tests/gtk-preview/accounts.robot` (driven by `tests/gtk-preview.sh`, stage `accounts-suite`) seeds a fixture with two accounts and `settings.json`, launches the binary with `ZWIFT_NO_GUI=0 --text-ui`, and asserts the picker text ("Choose a rider", both names, a chip), removes one account by clicking the `Remove` label (the launcher runs in text mode, so the controls carry text labels rather than icons; the two remove controls are identical, so the first match is what goes), confirms the dialog, then launches the survivor by clicking its rider name (the card is the launch target now) - trying "Offline Tester" first and falling back to "Second Rider" with `Run Keyword And Return Status`. The bash side additionally asserts that `window-ready`/`game-running` appear (polled for up to 20 s) and that `pairing-token.json` holds exactly the played account's token.

Useful while debugging: every captured frame lands in `$YARF_FRAME_DIR` with `index.log` mapping frame numbers to wait labels, and `Move Pointer To` costs a full OCR pass (~2.5 s), so a click lands later than the timestamp of the frame it was measured on.