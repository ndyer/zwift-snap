# Preface

The helper that obtains a real Zwift `Game_Launcher` token by hand, for driving the
game with real credentials. Read this before touching the script — the token it
produces is a live secret and the flow has traps (proxy handling, unrefreshable
mobile tokens).

IMPORTANT: Read the `.kb/agents.md` file.


# Concepts

## Trying real authentication

To request real `Game_Launcher` tokens independently of Wine:

```shell
python3 tests/tools/zwift-token.py ~/zwift-token.json
```

Run this in a terminal. Open the printed authorization URL in a browser (on the
host if running inside the workshop) and enter your credentials on Zwift's site.
After login, the browser redirects to `http://zwift/?...`, which may fail to load.
Copy that complete URL from the address bar and paste it at the script's hidden
prompt. Do not share the redirect URL: it contains an authorization code.

The script uses the `Game_Launcher` authorization-code flow with S256 PKCE and a
random state checked on return. The token exchange contacts `secure.zwift.com`
directly over verified HTTPS, ignoring proxy environment variables, and refuses
redirects. Recognized OAuth error codes are reported without printing raw server
responses. `Game_Launcher` rejects password grants with `unauthorized_client`;
mobile-client tokens cannot be refreshed by the game because their client differs.

The full response is saved to a new owner-only file, never printed or overwritten.
Treat this file like a password, keep it outside the repository, do not share it,
and delete it when finished. This helper does not launch the game or configure
persistent login.
