# Preface

How login works and how the "Remember me" setting can (and cannot) be observed, with
the zwift-offline findings that shaped it. Read the secret-handling warnings here
before probing cookies — a logged cookie value ends up in `zwift.log`.

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Login and "Remember me" detection

The launcher logs in through a WebKitGTK WebView in the bottom panel
(`zwsnap-gui/src/gui/login_panel.rs`, shown at all times; it loads Zwift's launcher
embed page until the login gate opens, and the `?registration=true` variant on the
very first run). It does OAuth PKCE against Keycloak
(`zwsnap-gui/src/oauth.rs`), watches navigation for the `http://zwift/?code=...`
redirect, checks `state`, exchanges the code and saves `pairing-token.json`, which
`launch.rs` passes to the game as `--token`.

Link clicks and new-window requests in the panel are opened in the system browser
with `xdg-open`, never in the panel; only the launcher's own navigations (the embed
page and the Keycloak authorize URL) are followed there. Because a link click is no
longer followed in the panel, the panel cannot be navigated away from by the page.
In the snap this has to be the base's `/usr/bin/xdg-open`, snapd's proxy to the
host's handler, so `xdg-utils` is not staged (see `snap/.kb/snap-specifics.md`).
The handoff is logged through the launcher `Logger`: the request at debug, the opener
invocation at info, and the opener's exit status plus stderr on a helper thread (a
warning on non-zero exit). `user-open error: cannot open supplied URL` in `zwift.log`
means the host has no browser — it is not a snap fault, and it is the normal result in
the workshop, which has no URL handler.

- The launcher cannot read the "Remember me" checkbox. The token has no remember-me claim: the
  refresh-token lifetime was 691200 s whether the box was ticked or not.
- The setting is visible as a cookie in the WebView's network session. The cookies are scoped to
  `https://secure.zwift.com/auth/realms/zwift/`. Querying `https://secure.zwift.com/` alone returns
  only `_dd_s_v2` and `sticky`, so use the realm path.
- Ticked: `KEYCLOAK_REMEMBER_ME` is present (31 bytes), and `KEYCLOAK_IDENTITY` and
  `KEYCLOAK_SESSION` expire in about a year. Unticked: `KEYCLOAK_REMEMBER_ME` is absent, and
  `KEYCLOAK_IDENTITY` is a session cookie with no expiry.
- The login WebView keeps its cookies in `~/snap/zwift/common/webkit/cookies.sqlite`
  (`persistent_webview` in `gui.rs`), so a remember-me cookie survives a launcher restart. The
  directory is 0700 and the database 0600. Those cookies are live session secrets: never copy, log
  or commit them.
- webkit6 0.4 `CookieManager::all_cookies` needs the `v2_42` feature, which the crate does not enable.
  Use `CookieManager::cookies(uri, cancellable, callback)` instead.
- When probing cookies, log names, flags, expiry and length, or a hash prefix. Never log values:
  they are session secrets and would end up in `zwift.log`.

## zoffline reference (fatsbrown/zwift-offline)

A clone of [fatsbrown/zwift-offline](https://github.com/fatsbrown/zwift-offline), the maintained fork of
the deprecated zoffline, lives at `~/zwift-offline` (outside the repo). It is not part of the snap. It is a
local stand-in for Zwift's servers, so it shows the shape of the real login flow but not the real
account chooser. Findings:

- Its launcher login form, `cdn/static/web/launcher/login_form.html`, has the same "Remember me"
  checkbox (`name="remember"`). It is a single username and password form. There is no account list.
- Remember-me is server-side. `login()` in `zwift_offline.py` (~line 651) stores `user.remember` and
  sets Flask-Login's `remember_token` cookie. This is an analogy for `KEYCLOAK_REMEMBER_ME`, not
  the same cookie.
- The launcher handoff is `zwift_offline.py` (~line 4930): the `.../openid-connect/auth` route
  redirects to `http://zwift/?code=zwift_refresh_token<JWT>`. The JWT carries the session cookie.
  `oauth.rs` intercepts the same redirect for the real login.
- The token endpoint (`zwift_offline.py` ~line 4968) returns a fake access and refresh token pair.
  It does no real identity check.
- The real token endpoints used by the fork's scripts are `secure.zwift.com/auth/realms/zwift/protocol/openid-connect/token`
  and `.../tokens/access/codes` (`scripts/get_profile.py` ~line 57).
- No account-list endpoint was found, either in zoffline or in the launcher's strings. The
  `ZwiftLauncher.exe` account list is still unexplained.
