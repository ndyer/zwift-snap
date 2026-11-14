# Preface

The launcher can remember several Zwift riders and show a picker before the game starts: a grid of rider cards over Zwift artwork, each with an avatar, a name, three stat chips, a level/FTP/weight line, an orange Play button and a Remove button.

This file covers the account store (`accounts.rs`), the picker widget (`gui/accounts_panel.rs`), the launcher-side wiring in `gui.rs`, and the GTK4 traps that made two real bugs.

IMPORTANT: Read the `.kb/agents.md` file.


# Laws

## §1 - Selection Copies The Launch Token

Nothing else in the launcher knows about accounts. `LaunchConfig.token_file` is still `$SNAP_USER_COMMON/pairing-token.json` and `launch.rs` reads that file only, so `Store::select` copies `accounts/<id>/token.json` over it (atomically, 0600) and remembers `last_used`. Never launch from an account directory directly and never make the worker account-aware.

## §2 - Zero Accounts Falls Back To The WebView Login

The picker is only shown when the store holds at least one account. With none, `Event::LoginRequired` starts the WebView login and leaves the picker hidden, so first runs and fully-forgotten stores behave as before. The runtime/preview/close GTK suites cover that path.

## §3 - Nothing Here May Block A Launch

Every account feature is best-effort. Profile refresh, avatar download and legacy migration all swallow their errors (debug log only) because a rider must always be able to start the game offline. A failure keeps the cached card data rather than clearing it.


# Concepts

## Store layout

`Store::open($SNAP_USER_COMMON)` points at:

- `accounts.json` - `{ "version": 1, "last_used": "<id>", "accounts": [...] }`. Written atomically (temp file + rename) with 0600.
- `accounts/<hex id>/token.json` - that rider's token, 0600.
- `accounts/<hex id>/avatar.<ext>` - cached avatar, 0600.
- `pairing-token.json` - the launch token, shared with the rest of the launcher.

The directory key is the account id hex-encoded, so a hostile `sub` claim can never contain `/` or `..`. `Account` holds only the fields a card draws: `name`, `first_name`, `last_name`, `email`, `avatar_url`, `avatar`, `use_metric`, `distance_meters`, `elevation_meters`, `time_minutes`, `weight_grams`, `ftp`, `level`, `height_millimeters`. Loading is tolerant: a missing or broken `accounts.json` is an empty store, a single bad entry is skipped, and a `last_used` naming a vanished account is dropped.

## Identity and naming

`identity_of` prefers the token's `sub` claim, then the profile's `public_id`, then its numeric `id`, then the email. Claims win because they survive a profile that could not be fetched. `display_name` prefers `"First Last"` from the profile and falls back to `preferred_username` and then to `Zwift rider`.

## The picker

`AccountsPanel` owns the `Store` (behind `Rc<RefCell<..>>`), a `gtk::FlowBox` grid (`insert(&card, -1)` - `append` needs GTK 4.6, past this crate's API level) and an "Add another account" footer pinned to the right-hand side.

The panel is placed at the top of the window, in the space the install tiles occupied, with the WebView kept below it: the picker appears *above* the WebView rather than replacing it. `refresh()` clears the grid and rebuilds one card per account; the store is the only state.

A card is a horizontal box: avatar (or an initials badge), a vertical info column (name row with an optional "Last used" pill, three chips, a `dim-label` meta line), then the Play and Remove buttons. Chips format through `stats::distance`, `stats::elevation` and `stats::duration`, with metric or imperial chosen by the account's `use_metric` (default metric). Unset values render as `-`.

Remove shows a `gtk::MessageDialog` whose confirm button is labelled **Remove account** (the card's own button stays **Remove**, so an OCR click target is never ambiguous), then `Store::remove` deletes the directory, clears `pairing-token.json` when that account was selected, saves and rebuilds the grid.

## Selecting and adding

The panel is given two callbacks. `select(id)` runs `Store::select` + `save` and answers the worker's pending login decision with `true`. `add()` switches the login panel to `SessionMode::Ephemeral` - a cookie-less WebView, so the current rider's SSO cookie cannot silently sign the new login back into itself - and starts a login whose result lands in an `mpsc` channel polled by `glib::timeout_add_local(50ms)`. This is the only case besides the gate's own login where the WebView leaves its embed page; when the login finishes, `LoginPanel::show_embed` rebuilds it on the persistent session and reloads `EMBED_URL`.

`finish_add_account` then reads the launch token back, derives claims, fetches the profile best-effort, upserts the account, downloads the avatar, selects the new account and answers the decision. A profile failure still remembers the account, named from its claims.

## Profile refresh

The picker opens with the stats cached when each account was added, then `refresh_accounts` (in `gui.rs`) spawns a thread that calls `profile::fetch` per stored token and posts the results back on a channel polled every 100 ms. `Store::refresh` merges a fetched profile into the existing account by id, keeping any field the profile omits, and returns `false` for an account removed while the fetch was in flight. Nothing is shown when a fetch fails.

Avatars are only fetched when an account is added; a refreshed `avatar_url` therefore does not update the picture.

## Legacy migration

An install from before the picker has `pairing-token.json` but no `accounts.json`. On `LoginRequired`, when the store loads empty, the launcher derives claims from that token and calls `Store::import_legacy`, which adopts it as the first account (`upsert` with an empty profile) and marks it last used. Without this an upgrading rider would be shown the WebKit login again despite a valid token.

## GTK4 traps

- `GtkDialog`/`GtkMessageDialog` no longer hide themselves when a response fires: the handler must call `confirm.close()`. Otherwise the modal stays on screen, swallows the next click and the launch never happens (the removal itself still succeeds, which makes the bug confusing).
- `CssProvider::load_from_data` (not `load_from_string`), installed with `gtk::style_context_add_provider_for_display` at `STYLE_PROVIDER_PRIORITY_APPLICATION`.
- The crate's API level has no `v4_6`/`v4_8` gates, so use `FlowBox::insert` and `MessageDialog` rather than the newer shorthands.

## Testing

`tests/gtk-preview/accounts.robot` (driven by `tests/gtk-preview.sh`, stage `accounts-suite`) seeds a fixture with two accounts and `settings.json`, launches the binary with `ZWIFT_NO_GUI=0`, and asserts the picker text ("Choose a rider", both names, a stat chip), removes the second account through the confirmation dialog, then plays the first. The bash side additionally asserts that `window-ready`/`game-running` appear (polled for up to 20 s) and that `pairing-token.json` holds exactly the played account's token.

Useful while debugging: every captured frame lands in `$YARF_FRAME_DIR` with `index.log` mapping frame numbers to wait labels, and `Move Pointer To` costs a full OCR pass (~2.5 s), so a click lands later than the timestamp of the frame it was measured on.