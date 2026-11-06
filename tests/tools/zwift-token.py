#!/usr/bin/env python3
"""Request a real Zwift OAuth token interactively, without Wine."""

import argparse
import base64
import getpass
import hashlib
import json
import os
from pathlib import Path
import secrets
import sys
import urllib.error
import urllib.parse
import urllib.request


AUTH_URL = "https://secure.zwift.com/auth/realms/zwift/protocol/openid-connect/auth"
TOKEN_URL = "https://secure.zwift.com/auth/realms/zwift/protocol/openid-connect/token"
REDIRECT_URI = "http://zwift/"
OAUTH_ERRORS = {
    "unauthorized_client": "The client is not allowed to use this authentication flow.",
    "invalid_client": "The server rejected the OAuth client.",
    "invalid_grant": "The credentials or authentication grant were rejected.",
    "invalid_request": "The server rejected the request parameters.",
    "unsupported_grant_type": "The server does not support this authentication grant.",
}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise urllib.error.HTTPError(req.full_url, code, "Redirect refused", headers, fp)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "output", type=Path,
        help="new file outside the repository for the secret token JSON (never overwritten)",
    )
    args = parser.parse_args()
    if args.output.exists() or args.output.is_symlink():
        parser.error("output already exists; choose a new file")
    if not sys.stdin.isatty():
        parser.error("run interactively in a terminal to keep the authorization code hidden")

    state = secrets.token_urlsafe(32)
    verifier = secrets.token_urlsafe(48)
    challenge = base64.urlsafe_b64encode(
        hashlib.sha256(verifier.encode("ascii")).digest()
    ).rstrip(b"=").decode("ascii")
    authorization_url = AUTH_URL + "?" + urllib.parse.urlencode({
        "client_id": "Game_Launcher",
        "response_type": "code",
        "redirect_uri": REDIRECT_URI,
        "scope": "openid",
        "state": state,
        "code_challenge": challenge,
        "code_challenge_method": "S256",
    })
    print("Open this URL in your browser and log in directly to Zwift:")
    print(authorization_url)
    print(
        "After login, the browser redirects to http://zwift/ (it may fail to load).\n"
        "Copy the complete redirected URL from the address bar; do not share it."
    )
    redirect = getpass.getpass("Paste the complete redirected URL (hidden): ").strip()
    try:
        parsed = urllib.parse.urlsplit(redirect)
        params = urllib.parse.parse_qs(parsed.query)
    except ValueError:
        parser.error("the redirected URL is invalid")
    if (parsed.scheme, parsed.netloc, parsed.path) != ("http", "zwift", "/") or parsed.fragment:
        parser.error("expected a redirect to http://zwift/")
    if params.get("state") != [state]:
        parser.error("authorization state did not match; use the URL from this run")
    if "error" in params:
        parser.error("Zwift did not authorize the login; no token request was sent")
    codes = params.get("code", [])
    if len(codes) != 1 or not codes[0]:
        parser.error("the redirected URL must contain one authorization code")

    request = urllib.request.Request(
        TOKEN_URL,
        data=urllib.parse.urlencode({
            "client_id": "Game_Launcher",
            "grant_type": "authorization_code",
            "code": codes[0],
            "redirect_uri": REDIRECT_URI,
            "code_verifier": verifier,
        }).encode("utf-8"),
        headers={
            "Accept": "application/json",
            "Content-Type": "application/x-www-form-urlencoded",
        },
    )
    # Do not send real credentials through the pairing-test proxy or follow redirects.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())
    try:
        with opener.open(request, timeout=30) as response:
            token = json.load(response)
    except urllib.error.HTTPError as error:
        detail = "No recognized OAuth error was returned."
        try:
            body = json.loads(error.read(65536))
        except (ValueError, UnicodeError):
            detail = "The error response was not valid JSON."
        except OSError:
            detail = "Unable to read the error response."
        else:
            code = body.get("error") if isinstance(body, dict) else None
            if isinstance(code, str) and code in OAUTH_ERRORS:
                # Only print known codes, not a server response that could echo secrets.
                detail = f"{code}: {OAUTH_ERRORS[code]}"
        print(
            f"Zwift rejected the token request (HTTP {error.code}). {detail}",
            file=sys.stderr,
        )
        return 1
    except (urllib.error.URLError, TimeoutError, OSError):
        print("Unable to reach Zwift securely; check your network connection.", file=sys.stderr)
        return 1
    except (ValueError, UnicodeError):
        print("Zwift returned an invalid JSON response.", file=sys.stderr)
        return 1

    if not isinstance(token, dict) or not all(
        isinstance(token.get(key), str) and token[key]
        for key in ("access_token", "refresh_token")
    ):
        print("Zwift's response did not contain access and refresh tokens.", file=sys.stderr)
        return 1

    try:
        fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            json.dump(token, output)
            output.write("\n")
    except OSError:
        print("Unable to save the token file; check the output path and permissions.", file=sys.stderr)
        return 1

    print("Token received and saved with owner-only permissions.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
