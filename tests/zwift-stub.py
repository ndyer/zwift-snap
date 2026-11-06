#!/usr/bin/env python3
"""Local stand-in for the Zwift servers, for tests only (never shipped in the snap).

Zwift is started with https_proxy pointing at this process, so every HTTPS connection comes here
as a CONNECT. Only the two hosts Zwift's login and game API live on are intercepted
(secure.zwift.com, us-or-rly101.zwift.com): for those the proxy terminates TLS itself with a
throwaway CA (created in --dir, the CA certificate is ca.pem) and answers the requests locally.
Every other host is tunnelled straight through with a plain TCP relay, so the launcher's update
check, winetricks downloads and the Zwift patcher keep working; sentry (crash reporting) is
refused outright so the tests send nothing out.

Every intercepted request is logged to --log. Bodies are never logged, only their size and, for
form posts, the field names, because the login form carries the password the user typed.

Usage: zwift-stub.py --dir DIR --log FILE [--port 8443] [--token] [--token-file FILE] [--ready-file FILE]
  --token       answer the OAuth token endpoint with a fake token (otherwise everything is a 404)
  --token-file  write fake OAuth token JSON for ZwiftApp.exe --token (tests only)
  --ready-file  write the listening port to this file once the socket is up (and a .pid next to it)
"""

import argparse
import base64
import json
import os
import select
import socket
import socketserver
import ssl
import struct
import subprocess
import sys
import threading
import time
import urllib.parse
import uuid
import zlib
from pathlib import Path

# hosts whose requests are answered locally; everything else is tunnelled unchanged
INTERCEPT_HOSTS = {"secure.zwift.com", "us-or-rly101.zwift.com"}
# hosts refused outright (no crash reports leave the test)
BLOCK_HOSTS = ("ingest.sentry.io", ".sentry.io")
# the throwaway server certificate covers these names
CERT_HOSTS = ["secure.zwift.com", "us-or-rly101.zwift.com", "launcher.zwift.com", "cdn.zwift.com"]
TOKEN_PATH = "/auth/realms/zwift/protocol/openid-connect/token"
GAME_URL = "https://us-or-rly101.zwift.com"

# Fixed JSON answers of the game server API, by (method, path). They hold just what the game reads.
JSON_ROUTES = {
    # where the game finds its OAuth server: without it the token refresh is never attempted
    ("GET", "/api/auth"): {"realm": "zwift", "launcher": "https://launcher.zwift.com/launcher",
                           "url": "https://secure.zwift.com/auth/"},
    # telemetry is switched off, the game then has nothing to post
    ("GET", "/api/telemetry/config"): {
        "analyticsEvents": False, "telemetryEvents": False, "isEnabled": False, "gzip": False,
        "batchInterval": 120, "sampleInterval": 60, "remoteLogLevel": 3,
        "url": f"{GAME_URL}/v1/track", "urlBatch": f"{GAME_URL}/hvc-ingestion-service/batch",
        "structuredEventsUrl": f"{GAME_URL}/api/actions-service/structured-events/batch"},
    ("POST", "/v1/track"): {"success": True},
    ("POST", "/hvc-ingestion-service/batch"): {"success": True},
}

log_lock = threading.Lock()
settings = argparse.Namespace()


def log(message: str) -> None:
    with log_lock, open(settings.log, "a", encoding="utf-8") as out:
        out.write(f"{time.strftime('%T')} {message}\n")


def b64(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def fake_jwt(claims: dict) -> str:
    header = b64(json.dumps({"alg": "RS256", "typ": "JWT"}).encode())
    return f"{header}.{b64(json.dumps(claims).encode())}.{b64(b'fake')}"


def fake_token() -> dict:
    now = int(time.time())
    claims = {
        "exp": now + 21600,
        "iat": now,
        "jti": str(uuid.uuid4()),
        "iss": "https://secure.zwift.com/auth/realms/zwift",
        "aud": "Game_Launcher",
        "sub": "00000000-0000-4000-8000-000000000001",
        "typ": "Bearer",
        "azp": "Game_Launcher",
        "scope": "openid",
        "preferred_username": "offline@example.com",
        "email": "offline@example.com",
    }
    return {
        "access_token": fake_jwt(claims),
        "expires_in": 21600,
        "refresh_expires_in": 1209600,
        "refresh_token": fake_jwt({**claims, "typ": "Refresh", "exp": now + 1209600}),
        "token_type": "bearer",
        "id_token": fake_jwt({**claims, "typ": "ID"}),
        "not-before-policy": 0,
        "session_state": str(uuid.uuid4()),
        "scope": "openid",
    }


def varint(value: int) -> bytes:
    out = bytearray()
    while value > 0x7F:
        out.append(value & 0x7F | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)


def pb_varint(field: int, value: int) -> bytes:
    return varint(field << 3) + varint(value)


def pb_bytes(field: int, value: bytes | str) -> bytes:
    data = value.encode() if isinstance(value, str) else value
    return varint(field << 3 | 2) + varint(len(data)) + data


def login_response() -> bytes:
    """LoginResponse with only the required parts (no telemetry nodes, no economy config)."""
    info = pb_bytes(1, f"{GAME_URL}/relay") + pb_varint(3, int(time.time()))
    return pb_bytes(1, "abc") + pb_bytes(2, info) + pb_varint(3, 1) + pb_varint(4, 70)


def profile() -> bytes:
    """A test PlayerProfile with license, sport and goal onboarding already completed."""
    # saved_game chunk 0x10000010: length, count, then (key, type, value) records.
    # Accepting the license adds this integer record; captured from the test client's PUT.
    # The end chunk is mandatory: without it the client hangs while loading the profile.
    accepted_license = (struct.pack("<IIIIIQ", 0x10000010, 28, 1, 0xE412BB7B, 2, 1)
                        + struct.pack("<II", 0x10000005, 8))
    attributes = {
        "SPORT_SELECT_TYPE": "CYCLIST",
        "TODAYS_SPORT_SELECT_TYPE": "CYCLIST",
        "FINAL_SPORT_SELECT_TYPE": "CYCLIST",
        "ONBOARD_CUSTOMIZATION_USER_TYPE_ZWIFT_GOAL": "RACING",
    }
    return b"".join([
        pb_varint(1, 1),  # id
        pb_varint(2, 1),  # server_realm
        pb_bytes(3, "offline@example.com"),
        pb_bytes(4, "Offline"),
        pb_bytes(5, "Tester"),
        pb_varint(6, 1),  # is_male
        pb_varint(9, 70000),  # weight_in_grams
        pb_varint(10, 200),  # ftp
        pb_bytes(33, accepted_license),  # saved_game
        pb_varint(42, 1750),  # height_in_millimeters
        pb_varint(48, 1),  # player_type NORMAL
        pb_varint(50, 1),  # use_metric
        pb_varint(55, 30),  # age
        *(pb_bytes(113, pb_varint(1, zlib.crc32(name.encode())) + pb_bytes(5, value))
          for name, value in attributes.items()),
    ])


def world_list() -> bytes:
    """DropInWorldList with one empty public world (Watopia, course 6)."""
    now = int(time.time())
    world = (pb_varint(1, 1) + pb_bytes(2, "Public Watopia") + pb_varint(3, 6) + pb_varint(5, 0)
             + pb_varint(6, (now - 1414016075) * 1000) + pb_varint(7, now))
    return pb_bytes(1, world)


# Protobuf answers of the game server API, by (method, path)
PROTO_ROUTES = {
    ("POST", "/api/users/login"): login_response,
    ("GET", "/api/profiles/me"): profile,
    ("GET", "/relay/dropin"): world_list,
    ("GET", "/relay/worlds"): world_list,
}

# Paths the game only needs to see succeed, with an empty body
EMPTY_ROUTES = {
    ("POST", "/api/users/logout"),
    ("GET", "/api/scoring/current"),
    ("GET", "/api/game-asset-patching-service/manifest"),
    ("POST", "/api/workout/progress"),
    ("GET", "/api/power-curve/power-profile/proto"),
    ("GET", "/api/coach/public/plan-settings"),
    ("GET", "/api/membership/profiles/me/feature-gate-access"),
}


def make_certs(directory: Path) -> None:
    """Create the CA and a server certificate for all the Zwift hosts, unless they exist."""
    if (directory / "server.pem").exists():
        return
    directory.mkdir(parents=True, exist_ok=True)

    def openssl(*args: str) -> None:
        subprocess.run(["openssl", *args], cwd=directory, check=True, capture_output=True)

    openssl("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", "ca.key", "-out", "ca.pem",
            "-days", "30", "-subj", "/CN=zwift-stub test CA")
    openssl("req", "-newkey", "rsa:2048", "-nodes", "-keyout", "server.key", "-out", "server.csr",
            "-subj", f"/CN={CERT_HOSTS[0]}")
    (directory / "ext.cnf").write_text(
        "subjectAltName=" + ",".join(f"DNS:{h}" for h in CERT_HOSTS) + "\nbasicConstraints=CA:FALSE\n")
    openssl("x509", "-req", "-in", "server.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial",
            "-out", "server.pem", "-days", "30", "-extfile", "ext.cnf")


def read_headers(rfile) -> tuple[str, dict] | None:
    line = rfile.readline(65537)
    if not line.strip():
        return None
    request_line = line.decode("latin-1").strip()
    headers = {}
    while True:
        line = rfile.readline(65537).decode("latin-1").strip()
        if not line:
            return request_line, headers
        name, _, value = line.partition(":")
        headers[name.strip().lower()] = value.strip()


def respond(wfile, status: int, body: bytes = b"", content_type: str = "application/json") -> None:
    reason = {200: "OK", 404: "Not Found"}.get(status, "Status")
    wfile.write(f"HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\n"
                f"Content-Length: {len(body)}\r\n\r\n".encode() + body)
    wfile.flush()


def serve_requests(rfile, wfile, default_host: str) -> None:
    """Answer the HTTP requests of one (decrypted) connection until it closes."""
    while True:
        parsed = read_headers(rfile)
        if parsed is None:
            return
        request_line, headers = parsed
        method, _, rest = request_line.partition(" ")
        target = rest.rpartition(" ")[0] or rest
        size = int(headers.get("content-length") or 0)
        body = rfile.read(size) if size else b""
        host = headers.get("host", default_host)
        fields = ""
        if "x-www-form-urlencoded" in headers.get("content-type", ""):
            fields = " fields=" + ",".join(sorted(urllib.parse.parse_qs(body.decode("latin-1"))))
        log(f"{method} {host}{target} body={size}B{fields} auth={'yes' if 'authorization' in headers else 'no'}"
            f" accept={headers.get('accept', '')!r}")
        path = target.split("?")[0]
        # the login form posts a password grant, the launcher a refresh/code grant: answer them all
        if settings.token and method == "POST" and path == TOKEN_PATH:
            respond(wfile, 200, json.dumps(fake_token()).encode())
        elif (method, path) in JSON_ROUTES:
            respond(wfile, 200, json.dumps(JSON_ROUTES[(method, path)]).encode())
        elif (method, path) in PROTO_ROUTES:
            respond(wfile, 200, PROTO_ROUTES[(method, path)](), "application/x-protobuf-lite")
        elif (method, path) in EMPTY_ROUTES or (method == "PUT" and path.startswith("/api/profiles/")):
            respond(wfile, 200)
        else:
            respond(wfile, 404)
        if headers.get("connection", "").lower() == "close":
            return


def tunnel(client: socket.socket, host: str, port: int) -> None:
    """Relay bytes both ways between the game and the real host, unchanged."""
    with socket.create_connection((host, port), timeout=30) as upstream:
        client.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
        sockets = [client, upstream]
        while True:
            readable, _, errored = select.select(sockets, [], sockets, 60)
            if errored or not readable:
                return
            for source in readable:
                data = source.recv(65536)
                if not data:
                    return
                (upstream if source is client else client).sendall(data)


class Handler(socketserver.StreamRequestHandler):
    def handle(self) -> None:
        try:
            parsed = read_headers(self.rfile)
            if parsed is None:
                return
            request_line, headers = parsed
            method, _, rest = request_line.partition(" ")
            if method != "CONNECT":
                log(f"plain proxy request {request_line!r} (not handled)")
                respond(self.wfile, 404)
                return
            authority = rest.split(" ")[0]
            host, _, port_text = authority.partition(":")
            port = int(port_text or 443)
            if host in INTERCEPT_HOSTS:
                log(f"CONNECT {authority} (intercepted)")
                self.wfile.write(b"HTTP/1.1 200 Connection established\r\n\r\n")
                self.wfile.flush()
                tls = self.server.context.wrap_socket(self.connection, server_side=True)
                serve_requests(tls.makefile("rb"), tls.makefile("wb"), authority)
            elif any(host == b.lstrip(".") or host.endswith(b) for b in BLOCK_HOSTS):
                log(f"CONNECT {authority} (blocked)")
                self.wfile.write(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
                self.wfile.flush()
            else:
                log(f"CONNECT {authority} (tunnelled)")
                tunnel(self.connection, host, port)
        except ssl.SSLError as error:
            log(f"TLS error: {error}")
        except (ConnectionError, TimeoutError, OSError) as error:
            log(f"connection error: {error}")


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--dir", required=True, type=Path)
    parser.add_argument("--log", required=True)
    parser.add_argument("--port", type=int, default=8443)
    parser.add_argument("--token", action="store_true")
    parser.add_argument("--token-file", type=Path)
    parser.add_argument("--ready-file")
    parser.parse_args(namespace=settings)

    make_certs(settings.dir)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(settings.dir / "server.pem", settings.dir / "server.key")
    # 127.0.0.1 only: the stub must never be reachable from the network
    with Server(("127.0.0.1", settings.port), Handler) as server:
        server.context = context
        port = server.server_address[1]
        if settings.token_file:
            # runfromprocess forwards a Windows command line; spaces split the token argument.
            settings.token_file.write_text(json.dumps(fake_token(), separators=(",", ":")))
        if settings.ready_file:
            Path(settings.ready_file + ".pid").write_text(f"{os.getpid()}\n")
            Path(settings.ready_file).write_text(f"{port}\n")
        print(f"zwift-stub: listening on 127.0.0.1:{port}, CA in {settings.dir}/ca.pem", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
