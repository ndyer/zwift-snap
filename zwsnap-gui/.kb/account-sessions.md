# Preface

How the launcher remembers riders: each account gets its own WebKit cookie jar
on disk (its Keycloak login), while OAuth tokens stay ephemeral. Read this
before touching `auth.rs`, `gui/session.rs`, the login
recording in `gui/login_panel.rs`, `accounts.rs`, or the forget-on-exit purge in
`runtime.rs`.

IMPORTANT: Read the `.kb/agents.md` file.


# Laws

## §1 - Tokens Are Ephemeral

The launcher never writes a token to a file and never stores one between runs.
A token is minted at need (an interactive login, or a silent re-authentication
from an account's stored session) and handed to the game as `--token=<json>`.
Between runs the only persisted login state is each account's cookie jar and the
account metadata.

## §2 - One Cookie Jar Per Account

Keycloak keeps one SSO session **per cookie jar**, so every account owns a
`NetworkSession` rooted at `accounts/<id>/webkit/` (owner-only dir 0700, cookie
db 0600). That directory *is* the remembered login. The directory is created
before WebKit opens it (see `gui::session::account_session`).

## §3 - Cookies Are Copied, Never Moved

The login panel always runs on an **ephemeral** (RAM-only) session. On a
successful, remembered login the panel's realm cookies are **copied** into the
account's persistent session with `gui::session::copy_realm_cookies`, preserving
their expiry (that is what makes "Remember me" survive a restart). The copy keeps
the target session alive until WebKit has written the cookies.

## §4 - NEVER-WRITE For Forget-On-Exit

A login whose "Remember me" box is unchecked copies **no** cookies: the account's
`webkit/` is never created (and is purged at exit), and the session is marked
`forget_on_exit`. When the cookie cannot be read at all the login is *kept*,
because a remembered card whose session was never written is worse than one extra
stored login.

## §5 - One Shared Token, Two Threads

`Arc<AuthToken>` is a trivial `Mutex<Option<Value>>` shared by the GUI thread
(which sets it after a login or a silent re-auth) and the launcher worker thread
(which reads it at launch time via `TokenSource` and clears it in the
forget-on-exit purge). It is `Send + Sync`.


# Concepts

## auth.rs

`crate::auth` holds two things: the `TokenSource` trait (the seam that replaced
`LaunchConfig.token_file`) and `AuthToken`, the in-memory cell. Nothing else lives
there; there is no persistence and no backend selection.

## gui/session.rs

The per-account session machinery:

- `account_session(dir)` creates `dir` (0700) and its `cookies.sqlite` (0600), then
  builds a persistent `NetworkSession` over it.
- `ephemeral_webview()` builds a WebView on a RAM-only session (the login panel and
  the embed page use this, so no login cookie lingers in the panel).
- `copy_realm_cookies(from, to)` copies the Keycloak realm cookies from one session
  into another, preserving expiry (see §3).
- `authorize(session, interactive, cancellation, on_result)` runs the OIDC
  authorization-code flow in a **hidden** WebView on `session`. With
  `interactive = false` the authorize URL carries `prompt=none`, so a session that
  still holds a valid SSO cookie redirects straight to `http://zwift/?code=...`
  with no form; a session that cannot be re-authenticated silently comes back as
  `LoginOutcome::NeedsLogin`. The WebView lives only for the flow, kept alive by a
  strong reference until the result fires.

## Flow: interactive login (gate or add)

1. The panel (always ephemeral) loads the Keycloak authorize URL and the user logs in.
2. On the `http://zwift/?code=...` redirect the code is exchanged for a token
   off-thread.
3. The token is handed to `AuthToken`; the account's identity is recorded
   (`Store::upsert` from the token's claims) and selected.
4. `remember` is decided: the gate reads the `KEYCLOAK_REMEMBER_ME` cookie; an
   "add another account" login is always remembered (its ephemeral session has no
   cookie to consult).
5. If remembered, the realm cookies are copied into `accounts/<id>/webkit/`; if
   forgotten, nothing is copied and `forget_on_exit` is set.

## Flow: silent launch (a card is clicked)

The picker records the selection, then calls `session::authorize` on that
account's session with `prompt=none`. A valid session yields a token with no
prompt and the game launches; `NeedsLogin`/failure falls back to the interactive
gate (`gui::relogin`). A card whose account has no session at all therefore simply
asks for one password.

## Flow: startup profile refresh

`gui::refresh_accounts` walks every account that `Store::has_session` reports,
silently re-authenticating each (one hidden WebView and one profile fetch at a
time, serialized) to mint a throwaway token for the profile API. The token is used
once and dropped; the fetched profile and avatar update the card. Best-effort
throughout: an account that cannot be re-authenticated is skipped.

## Store layout

`Store::open($SNAP_USER_COMMON)` points at:

- `accounts.json` - `{ "version": 1, "last_used": "<id>", "accounts": [...] }`, 0600.
- `accounts/<hex id>/avatar.<ext>` - cached avatar, 0600.
- `accounts/<hex id>/webkit/` - that rider's WebKit session: the cookie jar that
  remembers the login (§2).

`Store::has_session(id)` is true once `accounts/<id>/webkit/cookies.sqlite`
exists. `Store::remove` deletes the whole account directory, so the login goes
with it. `Store::cleanup_legacy` deletes any `pairing-token.json` (or per-account
`token.json`) left by an older launcher; a token cannot become a session, so the
rider logs in again.

## Migration

On startup the store loads and calls `cleanup_legacy`: legacy token files are
simply removed. Nothing is adopted, because a bearer token cannot seed a WebKit
cookie jar.
