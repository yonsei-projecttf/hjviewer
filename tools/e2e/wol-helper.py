#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""What the WOL E2E (tools/e2e/wol-e2e.sh) signs itself: the app's account requests, and the wake
commands of a server that stands in hjv-api's place before a relay.

    python3 -I wol-helper.py selftest
    python3 -I wol-helper.py request <LocalConfig file> <CA file> <METHOD> <URL> [<body>]
    python3 -I wol-helper.py impostor <address:port> <folder> <command key file> <MAC> <network>
    python3 -I wol-helper.py forge <folder> <case> [<case>...]

selftest: the Ed25519 signatures and the two signed texts below are the documents' test vectors
(server/api/README.md "Test vectors", relay/PROTOCOL.md section 13).

request: one account request as the app sends it (server/api/README.md "Account requests"), with
the token and the account key a logged-in headless client keeps in its LocalConfig (on Linux the
account key is the option hjv-ctl-key): `Authorization: Bearer <token>` and

    X-HJV-Proof: v1.<t>.<n>.<signature over "hjv-req-v1" NUL method NUL path and query NUL t NUL n
                                              NUL base64(SHA-256(body)) NUL base64(SHA-256(token))>

It prints the status and the body on one line, "202 {...}", and fails only without an answer. The
app's wake requests are Dart's, which no headless client sends, so the E2E sends them this way.

impostor: an HTTP server that answers hjv-relay's hello and poll (relay/PROTOCOL.md section 5) as
hjv-api would, behind the test's Caddy, and hands the relay the commands `forge` asks for: what
someone who answers in the server's name can send a relay, with and without the command key
(section 14). It holds a poll as hjv-api does and answers it at once when `forge` named cases:

    no-signature    a command without its signature                              bad_command
    bad-signature   a signature of 64 random bytes                               bad_signature
    other-key       signed with another key, under the command key's number      bad_signature
    unknown-key     signed with the command key, under a number no relay pins    unknown_key
    tampered        a signed command whose MAC was changed afterwards            bad_signature
    expired         issued two minutes ago, expired a minute ago                 expired
    other-relay     addressed to another relay                                   wrong_relay
    five-macs       five MACs                                                    bad_macs
    broadcast-mac   the MAC ff:ff:ff:ff:ff:ff                                    bad_macs
    unknown-net     a network the relay has no interface on                      no_iface
    port-80         a port other than 9 and 7                                    bad_params
    valid           a command as hjv-api signs it, one round (repeat 1)          sent
    again:<case>    the very entry an earlier case got, once more                replayed

(the last column is the `error` of the relay's result, section 7.3). The cases of one `forge` go
out in one poll answer, in their order; a case named twice is numbered from its second use
(valid, valid-2, ...). Each result the relay reports becomes a line of <folder>/results.jsonl,
{"case","error","packets","iface","job"}, and each request a line of <folder>/requests.jsonl.

Standard library only. The Ed25519 here is for test keys: it takes no care that its time or
memory tell nothing of a key.
"""
import base64
import hashlib
import http.client
import http.server
import json
import os
import re
import ssl
import sys
import threading
import time
import urllib.parse

# --- Ed25519 (RFC 8032, section 5.1), to sign with test keys ---

P = 2**255 - 19
L = 2**252 + 27742317777372353535851937790883648493
D = -121665 * pow(121666, P - 2, P) % P
_BY = 4 * pow(5, P - 2, P) % P
_BX = 15112221349535400772501151409588531511454012693041857206046113283949847762202
BASE = (_BX, _BY, 1, _BX * _BY % P)


def _add(p, q):
    """Point addition in extended coordinates (RFC 8032 5.1.4); it also doubles."""
    x1, y1, z1, t1 = p
    x2, y2, z2, t2 = q
    a = (y1 - x1) * (y2 - x2) % P
    b = (y1 + x1) * (y2 + x2) % P
    c = 2 * D * t1 * t2 % P
    d = 2 * z1 * z2 % P
    e, f, g, h = b - a, d - c, d + c, b + a
    return (e * f % P, g * h % P, f * g % P, e * h % P)


def _mul(s, p):
    q = (0, 1, 1, 0)
    while s:
        if s & 1:
            q = _add(q, p)
        p = _add(p, p)
        s >>= 1
    return q


def _encode(p):
    x, y, z, _ = p
    inverse = pow(z, P - 2, P)
    x, y = x * inverse % P, y * inverse % P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _secret(seed):
    h = hashlib.sha512(seed).digest()
    a = int.from_bytes(h[:32], "little")
    return (a & ((1 << 254) - 8)) | (1 << 254), h[32:]


def public_key(seed):
    return _encode(_mul(_secret(seed)[0], BASE))


def sign(seed, message):
    a, prefix = _secret(seed)
    public = _encode(_mul(a, BASE))
    r = int.from_bytes(hashlib.sha512(prefix + message).digest(), "little") % L
    big_r = _encode(_mul(r, BASE))
    k = int.from_bytes(hashlib.sha512(big_r + public + message).digest(), "little") % L
    return big_r + ((r + k * a) % L).to_bytes(32, "little")


# --- the texts the two keys sign ---

def b64(data):
    return base64.b64encode(data).decode()


def b64url(data):
    return base64.urlsafe_b64encode(data).decode().rstrip("=")


def sha256(data):
    return hashlib.sha256(data).digest()


def request_proof(seed, method, target, body, token, t, n):
    """The X-HJV-Proof of an account request (server/api/README.md, "Account requests")."""
    message = b"\0".join([
        b"hjv-req-v1", method.encode(), target.encode(), str(t).encode(), n.encode(),
        b64(sha256(body)).encode(), b64(sha256(token.encode())).encode(),
    ])
    return "v1.%s.%s.%s" % (t, n, b64url(sign(seed, message)))


def command_signature(seed, text):
    """A wake command's signature (relay/PROTOCOL.md section 7.2)."""
    return b64(sign(seed, b"hjv-wake-v1\0" + text.encode()))


def selftest():
    # RFC 8032 section 7.1, test 2
    seed = bytes.fromhex("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb")
    assert public_key(seed).hex() == "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"
    assert sign(seed, b"\x72").hex() == (
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da"
        "085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00")
    # server/api/README.md "Test vectors": the account key of the seed [3; 32]
    account = bytes([3]) * 32
    assert b64(public_key(account)) == "7UkoxijRwsbq6QM4kFmVYSlZJzpcY/k2NsFGFKyHN9E="
    token = "hjv_AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8"
    n = b64url(bytes(range(16)))
    assert request_proof(
        account, "GET", "/api/peers?current=1&pageSize=100&accessible=&status=1", b"", token, 1760000000, n
    ) == ("v1.1760000000.AAECAwQFBgcICQoLDA0ODw.9tsrnUGYVeHhGWQC2oHD1-9Ahw62G854ZjhmxV4CclBAKgpKGR2ou"
          "iNwO4v88CKY0W1jqDqrm_jyzdGq_a2YCA")
    assert request_proof(
        account, "POST", "/api/logout", b'{"id":"987654321","uuid":"dXVpZA=="}', token, 1760000000, n
    ) == ("v1.1760000000.AAECAwQFBgcICQoLDA0ODw.HgexD8dQd4Gq9iVRcGKrG_nqtOsGX5FbjSlBwJRkA4bdnFtbaQ48x"
          "nizvYV301E2G4vHaI_vvmoMsq_X2R_DDg")
    # relay/PROTOCOL.md section 13: the command key of the seed 20 21 ... 3f and its command
    command = bytes(range(0x20, 0x40))
    assert b64(public_key(command)) == "Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc="
    text = ('{"v":1,"job":"0123456789abcdef0123456789abcdef","relay":"r-kzdvvj2umnduyauf",'
            '"iat":1760000000,"exp":1760000060,"net":"10.1.4.0/22","macs":["00:00:5e:00:53:01"],'
            '"ports":[9,7],"repeat":3,"gap_ms":200}')
    assert command_signature(command, text) == (
        "91MXS5rMOB9k3ZP7XxPue9M++nXy9w4VIpjcqGJspnFI46U37TGC+st2hv6D8DxcUJOaN/2PcY7XeQ6wgfmLBw==")
    assert hashlib.sha256(text.encode()).hexdigest() == (
        "eefb709bfc6dae46233f75d43c02d0dfb173218d3ed9010c40dc316b4c3d7abe")
    print("selftest: Ed25519 (RFC 8032), X-HJV-Proof (server/api/README.md) and the wake command "
          "(relay/PROTOCOL.md 13) are the documents' vectors")
    return 0


# --- request: the app's account request ---

def request(config, ca, method, url, body=None):
    with open(config, encoding="utf-8") as f:
        text = f.read()
    # the options the client wrote, each a line: name = 'value'
    options = {m.group(1): m.group(3) for m in re.finditer(r"""^([\w-]+) = ('|")(.*)\2$""", text, re.M)}
    token, key = options.get("access_token", ""), options.get("hjv-ctl-key", "")
    if not token or not key:
        sys.stderr.write("wol-helper: %s holds no login (access_token, hjv-ctl-key)\n" % config)
        return 1
    parts = urllib.parse.urlsplit(url)
    target = parts.path + ("?" + parts.query if parts.query else "")
    data = b"" if body is None else body.encode()
    # the app's clock, with the offset it learned from the server (0 unless its clock was off)
    t = int(time.time()) + int(options.get("hjv-clock-offset") or 0)
    headers = {
        "Authorization": "Bearer " + token,
        "X-HJV-Proof": request_proof(base64.b64decode(key), method, target, data, token, t, b64url(os.urandom(16))),
    }
    if body is not None:
        headers["Content-Type"] = "application/json"
    connection = http.client.HTTPSConnection(
        parts.hostname, parts.port or 443, timeout=20, context=ssl.create_default_context(cafile=ca))
    try:
        connection.request(method, target, body=data if body is not None else None, headers=headers)
        answer = connection.getresponse()
        text = answer.read().decode("utf-8", "replace")
    except (OSError, http.client.HTTPException) as e:
        sys.stderr.write("wol-helper: %s %s: %s\n" % (method, url, e))
        return 1
    finally:
        connection.close()
    print("%d %s" % (answer.status, " ".join(text.split())))
    return 0


# --- impostor: a server in hjv-api's place, and forge, which tells it what to hand out ---

OTHER_KEY = bytes([0xE9]) * 32   # no relay pins its public key
UNKNOWN_KEY_ID = 65000           # nor a key of this number (the E2E's relay pins key 1)
OTHER_RELAY = "r-" + "a" * 16
UNKNOWN_NET = "198.18.99.0/24"   # RFC 2544's test range: no interface of the test is on it
CASES = ("no-signature", "bad-signature", "other-key", "unknown-key", "tampered", "expired", "other-relay",
         "five-macs", "broadcast-mac", "unknown-net", "port-80", "valid")


class Impostor:
    def __init__(self, folder, key, mac, net):
        self.folder, self.key, self.mac, self.net = folder, key, mac, net
        self.lock = threading.Lock()
        self.entries = {}       # case name -> the entry it got
        self.waiting = []       # (case name, cmd) in the order sent, until the relay reports them
        self.used = {}          # case -> times named

    def log(self, name, record):
        with open(os.path.join(self.folder, name), "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False) + "\n")

    def entry(self, case, relay, now):
        """The `commands` entry of a case, for the relay that asks."""
        if case.startswith("again:"):
            return self.entries[case[len("again:"):]]
        command = {
            "v": 1, "job": os.urandom(16).hex(), "relay": relay, "iat": now, "exp": now + 60,
            "net": self.net, "macs": [self.mac], "ports": [9, 7], "repeat": 1, "gap_ms": 200,
        }
        if case == "expired":
            command.update(iat=now - 120, exp=now - 60)
        elif case == "other-relay":
            command["relay"] = OTHER_RELAY
        elif case == "five-macs":
            command["macs"] = ["00:00:5e:00:53:%02x" % i for i in range(0xA1, 0xA6)]
        elif case == "broadcast-mac":
            command["macs"] = ["ff:ff:ff:ff:ff:ff"]
        elif case == "unknown-net":
            command["net"] = UNKNOWN_NET
        elif case == "port-80":
            command["ports"] = [80]
        text = json.dumps(command, separators=(",", ":"))
        entry = {"c": text, "s": command_signature(self.key, text), "k": 1}
        if case == "no-signature":
            del entry["s"]
        elif case == "bad-signature":
            entry["s"] = b64(os.urandom(64))
        elif case == "other-key":
            entry["s"] = command_signature(OTHER_KEY, text)
        elif case == "unknown-key":
            entry["k"] = UNKNOWN_KEY_ID
        elif case == "tampered":
            entry["c"] = text.replace(self.mac, self.mac[:-2] + ("00" if self.mac[-2:] != "00" else "01"))
        return entry

    def take_stage(self, relay, now):
        """The entries of the cases `forge` asked for, if it did."""
        path = os.path.join(self.folder, "stage")
        try:
            with open(path, encoding="utf-8") as f:
                cases = json.load(f)
        except (OSError, ValueError):
            return None
        os.remove(path)
        entries = []
        for case in cases:
            self.used[case] = self.used.get(case, 0) + 1
            name = case if self.used[case] == 1 else "%s-%d" % (case, self.used[case])
            entry = self.entry(case, relay, now)
            self.entries[name] = entry
            self.waiting.append((name, hashlib.sha256(entry["c"].encode()).hexdigest()))
            entries.append(entry)
        return entries

    def results(self, results):
        for result in results:
            cmd = result.get("cmd")
            index = next((i for i, (_, c) in enumerate(self.waiting) if c == cmd), None)
            case = self.waiting.pop(index)[0] if index is not None else "?"
            self.log("results.jsonl", {
                "case": case, "error": result.get("error"), "packets": result.get("packets"),
                "iface": result.get("iface"), "job": result.get("job"),
            })

    def answer(self, op, envelope):
        now = int(time.time())
        relay = envelope.get("id", "")
        payload = json.loads(envelope.get("p", "{}"))
        self.log("requests.jsonl", {
            "at": now, "op": op, "relay": relay, "results": len(payload.get("results", [])),
            "ifaces": [i.get("name") for i in payload.get("ifaces") or []],
        })
        if op == "hello":
            return {"server_time": now, "state": "active"}
        with self.lock:
            self.results(payload.get("results", []))
        end = time.monotonic() + min(int(payload.get("wait", 0)), 20)
        while True:
            with self.lock:
                commands = self.take_stage(relay, int(time.time()))
            if commands is not None or time.monotonic() >= end:
                break
            time.sleep(0.05)
        return {"server_time": int(time.time()), "state": "active", "commands": commands or []}


def impostor(listen, folder, key_file, mac, net):
    with open(key_file, encoding="utf-8") as f:
        key = base64.b64decode(f.read().strip())
    host, port = listen.rsplit(":", 1)
    os.makedirs(folder, exist_ok=True)
    state = Impostor(folder, key, mac.lower(), net)

    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_POST(self):
            op = self.path.rsplit("/", 1)[-1]
            body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            try:
                if self.path not in ("/api/hjv/relay/hello", "/api/hjv/relay/poll"):
                    raise ValueError("not a relay path")
                status, answer = 200, state.answer(op, json.loads(body))
            except (ValueError, KeyError, TypeError) as e:
                status, answer = 400, {"error": "bad_request", "server_time": int(time.time())}
                state.log("requests.jsonl", {"at": int(time.time()), "path": self.path, "refused": str(e)})
            data = json.dumps(answer).encode()
            try:
                self.send_response(status)
                self.send_header("Content-Type", "application/json; charset=utf-8")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            except OSError:
                pass  # the relay went away while its poll was held

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer((host, int(port)), Handler)
    server.daemon_threads = True
    print("impostor listening on %s" % listen, flush=True)
    server.serve_forever()
    return 0


def forge(folder, cases):
    for case in cases:
        if case not in CASES and not case.startswith("again:"):
            sys.stderr.write("wol-helper: no such case: %s\n" % case)
            return 2
    path = os.path.join(folder, "stage")
    with open(path + ".new", "w", encoding="utf-8") as f:
        json.dump(cases, f)
    os.replace(path + ".new", path)
    return 0


def main(argv):
    command, args = (argv[1] if len(argv) > 1 else ""), argv[2:]
    if command == "selftest" and not args:
        return selftest()
    if command == "request" and len(args) in (4, 5):
        return request(*args)
    if command == "impostor" and len(args) == 5:
        return impostor(*args)
    if command == "forge" and len(args) >= 2:
        return forge(args[0], args[1:])
    sys.stderr.write(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
