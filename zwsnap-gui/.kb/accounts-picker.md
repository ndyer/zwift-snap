# Preface

The launcher can remember several Zwift riders and show a picker before the game starts: a grid of rider cards over Zwift artwork, each with an avatar, a name, three stat chips, a level/FTP/weight line and a small remove icon in its top-right corner. The whole card is the launch target - clicking anywhere on it (except the remove icon) selects that rider.

This file covers the account store (`accounts.rs`), the picker widget (`gui/accounts_panel.rs`), the launcher-side wiring in `gui.rs`, and the GTK4 traps that made two real bugs.

IMPORTANT: Read the `.kb/agents.md` file.


# Laws

## §1 - Selection Records The Active Account

No token is ever copied or written to disk. `launch.rs` takes a `TokenSource` (see `.kb/account-sessions.md`): the worker reads the ephemeral token from `crate::auth::AuthToken` at launch time. `Store::select` only records `last_used`; the token is produced by a silent re-authentication from the account's session when its card is clicked. Never write a token to a file, and never make the worker read one.

## §2 - Zero Accounts Falls Back To The WebView Login

The picker is only shown when the store holds at least one account. With none, `Event::LoginRequired` starts the WebView login and leaves the picker hidden, so a fresh install (and a store whose accounts were all forgotten) logs in through the WebView. The runtime/preview/close GTK suites cover that path.

## §3 - Nothing Here May Block A Launch

Every account feature is best-effort. Profile refresh, avatar download and legacy migration all swallow their errors (debug log only) because a rider must always be able to start the game offline. A failure keeps the cached card data rather than clearing it.


# Concepts

## Store layout

`Store::open($SNAP_USER_COMMON)` points at:

- `accounts.json` - `{ "version": 1, "last_used": "<id>", "accounts": [...] }`. Written atomically (temp file + rename) with 0600.
- `accounts/<hex id>/avatar.<ext>` - cached avatar, 0600.
- `accounts/<hex id>/webkit/` - that rider's WebKit session (the remembered login).

A rider's token is **not** on disk: it is ephemeral. `cleanup_legacy` deletes any
legacy `accounts/<hex id>/token.json` or `pairing-token.json` left by an older
launcher; nothing is adopted.

The directory key is the account id hex-encoded, so a hostile `sub` claim can never contain `/` or `..`. `Account` holds only the fields a card draws: `name`, `first_name`, `last_name`, `email`, `avatar_url`, `avatar`, `use_metric`, `distance_meters`, `elevation_meters`, `time_minutes`, `weight_grams`, `ftp`, `level`, `level_progress`, `height_millimeters`. Loading is tolerant: a missing or broken `accounts.json` is an empty store, a single bad entry is skipped, and a `last_used` naming a vanished account is dropped.

## Identity and naming

`identity_of` prefers the token's `sub` claim, then the profile's `public_id`, then its numeric `id`, then the email. Claims win because they survive a profile that could not be fetched. `display_name` prefers `"First Last"` from the profile and falls back to `preferred_username` and then to `Zwift rider`.

## Levels

The profile's `achievement_level` (protobuf field 49) packs the level as `level * 100 + progress`; zwift-offline's `api_users_login` derives the level with `achievement_level // 100`. `merge_profile` therefore stores `level = raw / 100` and `level_progress = raw % 100` (0-99). The remainder is *assumed* to be percent toward the next level (which is what the card's bar draws), but that has not been verified against a live profile. A raw stored `level` (from an older build) self-heals on the next profile refresh.

## The picker

`AccountsPanel` owns the `Store` (behind `Rc<RefCell<..>>`), a `gtk::FlowBox` grid (`insert(&card, -1)` - `append` needs GTK 4.6, past this crate's API level) and an "Add another account" button, which the launcher places in the window header next to the log toggle and Settings. The panel exposes that button via `add_button()` and shows/hides it with the grid via `set_visible()`.

The panel is placed at the top of the window, in the space the install tiles occupied, with the WebView kept below it: the picker appears *above* the WebView rather than replacing it. `refresh()` clears the grid and rebuilds one card per account; the store is the only state.

A card is a `gtk::Overlay`: the base layer is a plain `gtk::Box` (CSS class `account-card`, tooltip naming the rider) that *is* the whole card, so there is no separate Play button. The card's content - a vertical box with a horizontal top row (avatar, or an initials badge, beside a vertical info column with the name row, an optional "Last used" pill, three chips and a `dim-label` meta line) - is laid out inside that box. Activation (a click, or Space/Enter during keyboard navigation) is reported natively by the `FlowBox`'s `child-activated` signal; `connect_child_activated` maps the child's `index()` back to the store order and runs the `select(id)` callback. The remove control is a `gtk::Button` (`zwsnap-remove-symbolic`, CSS class `account-remove`) in the overlay's top-right corner (`halign` End, `valign` Start); it is icon-only by default and shows the text "Remove" in text mode; its click is claimed at the target phase, so removing never also fires `child-activated` and launches the rider. The info column reserves room for the control with `set_margin_end(28)` so text never runs under it.

Keyboard navigation is the `GtkFlowBox` built-in rather than a hand-rolled key handler. The flow box keeps its default `SelectionMode::Single`, so `move-cursor` (Left/Right step child-by-child, Up/Down jump rows, Home/End/PgUp/PgDn) both moves the cursor and selects the child; the highlight is the `flowboxchild:selected .account-card` rule (a 2px border over the card's always-present transparent border, so selection does not shift the layout). The built-in theme would otherwise paint the selected `flowboxchild` itself with the accent-blue `background-color` (and white text) and add a 2px focus `outline` when the seeded child is focus-visible; because the card's background is translucent that fill bled through the whole card (it looked like the card turned blue). The picker's CSS therefore neutralises the child node - `flowbox > flowboxchild` and `flowbox > flowboxchild:selected` set `background-color: transparent`, `color: inherit`, `outline: none` (and `box-shadow: none`) - so the card border is the only highlight. `refresh()` seeds the cursor by calling `child_focus(TabForward)` on the child at the last-used account's index (falling back to the first card), which runs the flow box's own focus + selection update. Enter/Space then activate, and Escape is a window-level `GtkShortcutController` (`ShortcutTrigger::parse_string("Escape")` to a `CallbackAction`) that calls `window.close()` to quit the whole launcher.

Each chip (`chip(icon, label, value)`) shows a bundled icon (`zwsnap-distance/-elevation/-time-symbolic`) beside the stat's *value*, or the label and value together in text mode: the value comes from `stats::distance`, `stats::elevation` and `stats::duration` (`233 km`, `8,018 ft`, `13h`), with metric or imperial chosen by the account's `use_metric` (default metric), and the label words (`Distance`, `Elevation`, `Time`) live in `stats::DISTANCE_LABEL` and friends. Unset values render as `-`. When the account carries a `level` and `level_progress`, a slim `gtk::ProgressBar` (CSS `account-level-bar`, no visible text, tooltip naming the next level) is appended under the meta line, its fraction from `stats::level_fraction(progress)`.

Avatars are decoded at up to 72 by 72 pixels and displayed in a 72-pixel square; invalid images fall back to initials. Cards and the grid align to the start instead of filling the available width, and long names and metadata wrap. The compositor-backed `card_bounds_large_avatar_and_wraps_long_name` test checks the image allocation, card width, name wrapping and grid spacing.

The grid's height is pinned by `AccountsPanel::update_height`: a `FlowBox` reports its natural height for its *natural* width, which packs every card onto one line, so the `ScrolledWindow`'s `min_content_height` is set from the grid measured at the width it actually has (capped at `MAX_GRID_HEIGHT`). The width is only known after allocation, so `request_height_update` retries on a short timer until it is.

Remove shows a `gtk::MessageDialog` whose confirm button is labelled **Remove account**, then `Store::remove` deletes the whole account directory (its avatar and its WebKit session, so the login goes too), clears the active selection when that account was selected or when it was the last account, saves and rebuilds the grid. The remove control is icon-only (its tooltip names the rider), so it never competes with the card's name text as an OCR click target; because every card's remove icon is identical, automated suites must identify the *removed* card by position (first match) and the *surviving* card by name.

## Selecting and adding

The panel is given two callbacks. `select(id)` runs `Store::select` + `save`, then silently re-authenticates from the account's session (`gui::session::authorize`, `prompt=none`): a token answers the worker's pending decision with `true`, while a `NeedsLogin`/failure falls back to the interactive gate (`gui::relogin`). `add()` resets the panel to a fresh ephemeral view (so the current rider's SSO cookie cannot sign the new login back into itself), sets `set_remember_session(false)` (an added account is always remembered) and starts a login whose result lands in an `mpsc` channel polled by `glib::timeout_add_local(50ms)`. When the login finishes, `LoginPanel::show_embed` rebuilds the ephemeral view and reloads `EMBED_URL`.

The login panel records the new account at login time (upsert + select) and copies the login's realm cookies into the account's session (see `.kb/account-sessions.md`). `finish_add_account` then reads the ephemeral token from `AuthToken`, fetches the profile best-effort, merges it into the card, downloads the avatar and answers the decision. A profile failure still remembers the account, named from its claims.

## Profile refresh

The picker opens with cached data, then `refresh_accounts` (in `gui.rs`) refreshes each account's profile. It cannot read a stored token (there is none), so for every account with a session it performs a **silent re-authentication** (`gui::session::authorize`, `prompt=none`) to mint a throwaway token for the profile API, then fetches the profile and avatar on a background thread. The work is serialized (one hidden WebView and one fetch at a time) and best-effort: an account that cannot be re-authenticated is skipped, keeping its cached card. It downloads an avatar when the cached file is missing or the profile URL changes, and retries a missing image using the cached URL. Results are posted back on a channel polled every 100 ms; stale results for removed or changed accounts are discarded. `Store::refresh` merges profile fields while keeping omitted values. A failed image replacement keeps the old file and URL so the next refresh retries it. Profile and image failures are logged at debug level without blocking launch.

## Legacy migration

An install from before the picker has `pairing-token.json` (and possibly per-account `token.json` files). A bearer token cannot seed a WebKit session, so at startup - before the login panel is built - the launcher loads the store and calls `Store::cleanup_legacy`, which simply **deletes** those files; nothing is adopted and the rider logs in again. `Store::remove` clears the selection once the last account is forgotten, so an emptied store is never re-adopted.

## GTK4 traps

- `GtkDialog`/`GtkMessageDialog` no longer hide themselves when a response fires: the handler must call `confirm.close()`. Otherwise the modal stays on screen, swallows the next click and the launch never happens (the removal itself still succeeds, which makes the bug confusing).
- `CssProvider::load_from_data` (not `load_from_string`), installed with `gtk::style_context_add_provider_for_display` at `STYLE_PROVIDER_PRIORITY_APPLICATION`.
- `Shortcut::new(trigger, action)` takes `Option<impl IsA<..>>` **by value**: pass `Some(trigger)`/`Some(action)` (owned), not `Some(&trigger)`. `IsA` is implemented for the object type, not for references. `ShortcutTrigger::parse_string` returns `Option<ShortcutTrigger>`.
- The crate's API level has no `v4_6`/`v4_8` gates, so use `FlowBox::insert` and `MessageDialog` rather than the newer shorthands.
- A selected `flowboxchild` is styled by the theme itself (accent-blue `background-color`, white text, plus a 2px `outline` when focus-visible), *not* just by your descendant rules. A translucent card inside it lets that fill bleed through and the child's padding shows as a ring. Reset the child node (`background-color: transparent`, `color: inherit`, `outline: none`) so only your own highlight shows. App CSS at `STYLE_PROVIDER_PRIORITY_APPLICATION` outranks the theme.

## Testing

`tests/gtk-preview/accounts.robot` (driven by `tests/gtk-preview.sh`, stage `accounts-suite`) seeds a fixture with two accounts and `settings.json`, launches the binary with `ZWIFT_NO_GUI=0 --text-ui`, and asserts the picker text ("Choose a rider", both names, a chip), removes one account by clicking the `Remove` label (the launcher runs in text mode, so the controls carry text labels rather than icons; the two remove controls are identical, so the first match is what goes), confirms the dialog, then launches the survivor by clicking its rider name (the card is the launch target now) - trying "Offline Tester" first and falling back to "Second Rider" with `Run Keyword And Return Status`. The bash side additionally asserts that `window-ready`/`game-running` appear (polled for up to 20 s) and that the surviving account is the one launched. Because the first run has no stored session, clicking a card shows the login gate; the suite drives the **visible** login against the stub IdP once, which writes the account's `accounts/<id>/webkit/` session, and asserts a second launcher run then re-authenticates silently (the login page is never shown). See `tests/.kb/`.`

Useful while debugging: every captured frame lands in `$YARF_FRAME_DIR` with `index.log` mapping frame numbers to wait labels, and `Move Pointer To` costs a full OCR pass (~2.5 s), so a click lands later than the timestamp of the frame it was measured on.