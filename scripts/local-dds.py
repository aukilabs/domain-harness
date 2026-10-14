#!/usr/bin/env python3
"""LOCAL ONLY: a stand-in for the DDS P2P authority while Auki's DDS answers
"route not found" on its P2P routes.

It serves the routes the SDK needs to mint P2P credentials, signing them with
its own P-256 key, and forwards every other request to the real DDS:

  POST /api/v1/domains/{id}/p2p/challenge   (and p2p/zitadel/...)
  POST /api/v1/domains/{id}/p2p/verify
  GET  /service/p2p-verification-keys
  GET  /api/v1/accessible-domains
  PUT/DELETE /api/v1/domains/{id}/p2p/advertisements/self, GET .../p2p/advertisements

Peers that should talk to each other must all use this server as their DDS
URL, because they trust only the keys it publishes. The caller's DDS bearer is
checked against the real DDS (GET /api/v1/domains/{id}) before a challenge is
issued, so only accounts the real DDS lets into the Domain get a credential.

Usage: python3 scripts/local-dds.py [--listen 127.0.0.1:4350] [--state data/local-dds]
                                    [--upstream https://dds.dev.aukiverse.com]
                                    [--tls-cert C --tls-key K]
"""
import argparse
import base64
import hashlib
import json
import os
import secrets
import ssl
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import jwt
from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

TOKEN_TTL = 30 * 60
CHALLENGE_TTL = 60
KEY_OVERLAP = 31 * 60
ADVERTISEMENT_LEASE = 180
FILTERS = ("protocol", "peer_type", "peer_id", "subject_id")
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
PEER_TYPES = {"app-access": "app", "user-access": "user"}


def b64url(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def unb64url(text):
    return base64.urlsafe_b64decode(text + "=" * (-len(text) % 4))


def base58(data):
    number = int.from_bytes(data, "big")
    out = ""
    while number:
        number, rest = divmod(number, 58)
        out = B58[rest] + out
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + out


def ed25519_peer(public_key_protobuf):
    """libp2p protobuf Ed25519 key -> (verifier, Peer ID)."""
    if len(public_key_protobuf) != 36 or public_key_protobuf[:4] != b"\x08\x01\x12\x20":
        raise ValueError("public_key must be a libp2p Ed25519 key")
    key = Ed25519PublicKey.from_public_bytes(public_key_protobuf[4:])
    return key, base58(b"\x00\x24" + public_key_protobuf)


def now_text(seconds):
    return datetime.fromtimestamp(seconds, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


class Authority:
    def __init__(self, state, upstream):
        self.upstream = upstream.rstrip("/")
        state.mkdir(parents=True, exist_ok=True)
        key_file = state / "signing-key.pem"
        if not key_file.exists():
            key = ec.generate_private_key(ec.SECP256R1())
            key_file.write_bytes(key.private_bytes(
                serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
                serialization.NoEncryption()))
            key_file.chmod(0o600)
        self.key = serialization.load_pem_private_key(key_file.read_bytes(), None)
        public = self.key.public_key()
        self.public_pem = public.public_bytes(
            serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo).decode()
        der = public.public_bytes(
            serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo)
        self.key_id = hashlib.sha256(der).hexdigest()
        self.challenges = {}
        self.allowed = {}
        self.advertisements = {}
        self.lock = threading.Lock()

    def keys(self):
        return {"version": 1, "generation": 1, "previous_key_overlap_seconds": KEY_OVERLAP,
                "keys": [{"id": self.key_id, "status": "current", "signing_method": "ES256",
                          "public_key": self.public_pem}]}

    def upstream_request(self, method, path, headers, body=None):
        request = urllib.request.Request(self.upstream + path, data=body, method=method)
        for name, value in headers.items():
            request.add_header(name, value)
        try:
            with urllib.request.urlopen(request, timeout=20) as response:
                return response.status, response.headers, response.read()
        except urllib.error.HTTPError as error:
            return error.code, error.headers, error.read()

    def bearer_claims(self, bearer):
        token = bearer.removeprefix("Bearer ").strip()
        return jwt.decode(token, options={"verify_signature": False})

    def domain_allowed(self, domain_id, subject, headers):
        """Ask the real DDS (domain auth) whether this account may enter the Domain."""
        cache_key = (subject, domain_id)
        with self.lock:
            if self.allowed.get(cache_key, 0) > time.time():
                return True
        forwarded = {k: v for k, v in headers.items() if k != "content-type"}
        status, _, _ = self.upstream_request("POST", f"/api/v1/domains/{domain_id}/auth", forwarded)
        if status == 200:
            with self.lock:
                self.allowed[cache_key] = time.time() + 25 * 60
        return status == 200

    def challenge(self, domain_id, body, headers):
        peer_key, peer_id = ed25519_peer(unb64url(body["public_key"]))
        if peer_id != body["peer_id"]:
            return 400, {"error": "peer_id does not match public_key"}
        claims = self.bearer_claims(headers.get("authorization", ""))
        peer_type = PEER_TYPES.get(claims.get("type"))
        if peer_type is None:
            return 401, {"error": f"unsupported bearer type {claims.get('type')!r}"}
        if not self.domain_allowed(domain_id, claims.get("sub"), headers):
            return 403, {"error": "the real DDS does not let this account into the Domain"}
        challenge = secrets.token_bytes(32)
        challenge_id = str(uuid.uuid4())
        expires = int(time.time()) + CHALLENGE_TTL
        with self.lock:
            self.challenges = {k: v for k, v in self.challenges.items() if v["expires"] > time.time()}
            self.challenges[challenge_id] = {
                "challenge": challenge, "key": peer_key, "peer_id": peer_id,
                "domain_id": domain_id, "peer_type": peer_type, "expires": expires,
                "sub": claims.get("sub", peer_id), "org": claims.get("org")}
        return 200, {"challenge_id": challenge_id, "challenge": b64url(challenge),
                     "expires_at": now_text(expires)}

    def verify(self, domain_id, body):
        with self.lock:
            pending = self.challenges.pop(body["challenge_id"], None)
        if pending is None or pending["expires"] < time.time() or pending["domain_id"] != domain_id:
            return 401, {"error": "unknown or expired challenge"}
        try:
            pending["key"].verify(unb64url(body["signature"]), pending["challenge"])
        except InvalidSignature:
            return 401, {"error": "bad signature"}
        issued = int(time.time())
        claims = {"type": "p2p-access", "iss": "dds", "aud": ["auki-p2p"], "sub": pending["sub"],
                  "peer_type": pending["peer_type"], "peer_id": pending["peer_id"],
                  "domain_ids": [domain_id], "iat": issued, "exp": issued + TOKEN_TTL}
        if pending["org"]:
            claims["organization_id"] = pending["org"]
        token = jwt.encode(claims, self.key, algorithm="ES256", headers={"kid": self.key_id})
        print(f"[local-dds] issued {pending['peer_type']} credential for {pending['peer_id']} "
              f"in {domain_id}", flush=True)
        return 200, {"peer_id": pending["peer_id"], "domain_id": domain_id,
                     "peer_type": pending["peer_type"], "p2p_access_token": token,
                     "p2p_access_expires_at": now_text(issued + TOKEN_TTL)}

    def p2p_claims(self, headers, domain_id):
        """Claims of a P2P credential this server issued, for discovery calls."""
        token = headers.get("authorization", "").removeprefix("Bearer ").strip()
        claims = jwt.decode(token, self.key.public_key(), algorithms=["ES256"],
                            audience="auki-p2p", issuer="dds")
        if domain_id not in claims["domain_ids"]:
            raise jwt.InvalidTokenError("credential is not for this Domain")
        return claims

    def publish(self, domain_id, body, headers):
        claims = self.p2p_claims(headers, domain_id)
        record = {"peer_id": claims["peer_id"], "routes": list(body["routes"]),
                  "protocols": list(body["protocols"]), "subject_id": claims["sub"],
                  "peer_type": claims.get("peer_type") or "",
                  "expires_at": now_text(int(time.time()) + ADVERTISEMENT_LEASE)}
        with self.lock:
            fresh = (domain_id, claims["peer_id"]) not in self.advertisements
            self.advertisements[(domain_id, claims["peer_id"])] = record
        if fresh:
            print(f"[local-dds] {record['peer_type']} {record['peer_id']} advertises "
                  f"{', '.join(record['routes'])}", flush=True)
        return 200, record

    def withdraw(self, domain_id, headers):
        claims = self.p2p_claims(headers, domain_id)
        with self.lock:
            self.advertisements.pop((domain_id, claims["peer_id"]), None)
        return 204, b""

    def list_advertisements(self, domain_id, query, headers):
        self.p2p_claims(headers, domain_id)
        params = dict(urllib.parse.parse_qsl(query))
        filters = {name: params.get(name, "") for name in FILTERS}
        now = now_text(int(time.time()))
        with self.lock:
            records = [r for (d, _), r in self.advertisements.items()
                       if d == domain_id and r["expires_at"] > now]
        for name, value in filters.items():
            if value:
                records = [r for r in records
                           if (value in r["protocols"] if name == "protocol" else r[name] == value)]
        return 200, {"advertisements": records, "applied_filters": filters}

    def accessible_domains(self, query, headers):
        status, _, body = self.upstream_request("GET", "/api/v1/domains?with=domain_server", headers)
        if status != 200:
            return status, {"error": body.decode(errors="replace")[:200]}
        listed = json.loads(body)
        listed = listed.get("domains", listed) if isinstance(listed, dict) else listed
        domains = [{"id": d["id"], "name": d.get("name", ""), "description": d.get("description") or "",
                    "organization_id": d.get("organization_id")} for d in listed]
        return 200, {"domains": domains, "total": len(domains), "limit": 100, "offset": 0}


def handler_for(authority):
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt, *args):
            print(f"[local-dds] {self.command} {self.path.split('?')[0]} -> {args[1]}", flush=True)

        def reply(self, status, payload, content_type="application/json", headers=None):
            body = payload if isinstance(payload, bytes) else json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            for name, value in (headers or {}).items():
                self.send_header(name, value)
            self.end_headers()
            self.wfile.write(body)

        def forwarded_headers(self):
            keep = ("authorization", "posemesh-client-id", "accept", "content-type")
            return {k.lower(): v for k, v in self.headers.items() if k.lower() in keep}

        def handle_any(self):
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            path, _, query = self.path.partition("?")
            parts = path.strip("/").split("/")
            headers = self.forwarded_headers()
            try:
                if path == "/service/p2p-verification-keys":
                    return self.reply(200, authority.keys())
                if path == "/api/v1/accessible-domains":
                    return self.reply(*authority.accessible_domains(query, headers))
                if parts[:3] == ["api", "v1", "domains"] and parts[4:6] == ["p2p", "advertisements"]:
                    domain_id = parts[3]
                    if parts[6:] == ["self"] and self.command == "PUT":
                        return self.reply(*authority.publish(domain_id, json.loads(raw), headers))
                    if parts[6:] == ["self"] and self.command == "DELETE":
                        return self.reply(*authority.withdraw(domain_id, headers))
                    if parts[6:] == [] and self.command == "GET":
                        return self.reply(*authority.list_advertisements(domain_id, query, headers))
                if (len(parts) in (6, 7) and parts[:3] == ["api", "v1", "domains"]
                        and parts[4] == "p2p" and self.command == "POST"):
                    domain_id, action = parts[3], parts[-1]
                    body = json.loads(raw or b"{}")
                    if action == "challenge":
                        return self.reply(*authority.challenge(domain_id, body, headers))
                    if action == "verify":
                        return self.reply(*authority.verify(domain_id, body))
            except jwt.PyJWTError as error:
                print(f"[local-dds] refused credential on {path}: {error}", flush=True)
                return self.reply(401, {"error": str(error)})
            except (KeyError, ValueError) as error:
                print(f"[local-dds] rejected {path}: {type(error).__name__}: {error}", flush=True)
                return self.reply(400, {"error": str(error)})
            status, upstream_headers, body = authority.upstream_request(
                self.command, self.path, headers, raw or None)
            return self.reply(status, body, upstream_headers.get("Content-Type", "application/json"))

        do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = handle_any

    return Handler


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--listen", default="127.0.0.1:4350")
    parser.add_argument("--state", type=Path, default=Path("data/local-dds"))
    parser.add_argument("--upstream", default="https://dds.dev.aukiverse.com")
    parser.add_argument("--tls-cert")
    parser.add_argument("--tls-key")
    args = parser.parse_args()
    host, _, port = args.listen.rpartition(":")
    authority = Authority(args.state, args.upstream)
    server = ThreadingHTTPServer((host, int(port)), handler_for(authority))
    scheme = "http"
    if args.tls_cert:
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(args.tls_cert, args.tls_key)
        server.socket = context.wrap_socket(server.socket, server_side=True)
        scheme = "https"
    print(f"[local-dds] {scheme}://{args.listen} key {authority.key_id[:12]} "
          f"upstream {args.upstream}", flush=True)
    server.serve_forever()


if __name__ == "__main__":
    sys.exit(main())
