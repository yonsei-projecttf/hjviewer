# hjv-relay ⇄ hjv-api wire protocol, version 1

This is the contract between **hjv-relay** (the Wake-on-LAN relay that runs on an always-on Linux
machine inside a PC's LAN, such as the user's Synology NAS) and **hjv-api** (the HJViewer account
API behind nginx on the HJViewer server). The relay in this directory implements the relay side;
hjv-api implements the server side from this document. Where this document says MUST, SHOULD and
MAY, it means them in the sense of RFC 2119.

What the protocol does, in one paragraph: the relay has its own Ed25519 key. It introduces itself
with a signed `hello`; the server keeps it *pending* until an administrator approves its
enrollment code. An *active* relay keeps one signed long-poll request open: the server holds it up
to 20 seconds and answers at once when there is a wake command for it. A wake command is a small
JSON object that hjv-api signs with its **command key**; the relay checks the signature over the
exact bytes it received, then the command's addressee, time, job id, MACs and parameters, picks
its own network interface on the PC's network, sends magic packets, and reports the result in its
next poll. The relay only makes outgoing HTTPS requests; nothing listens on the NAS.

Conformance material in this directory: `tests/common/mod.rs` (`check_request`) implements the
server checks of section 4.3 and `tests/loop.rs` runs the relay against a test server that applies
them; `tests/vectors.rs` checks that this document shows the test vectors of section 13 and
that the relay's code produces them.

Contents: 1 Transport · 2 Encodings · 3 Keys and identity · 4 Request envelope · 5 Endpoints ·
6 Errors and back-off · 7 Wake commands · 8 Results · 9 Interfaces · 10 Time and timing ·
11 Registration states (server side) · 12 Versions · 13 Test vectors · 14 Security notes

---

## 1. Transport

- **HTTPS only**, HTTP/1.1, method `POST`, one request per TCP connection
  (`Connection: close`). TLS 1.2 or 1.3. The relay verifies the certificate against the Mozilla
  root list compiled into it (webpki-roots); there is no fallback without verification. A test may
  replace the roots with one CA (`hjv-relay run --ca FILE`).
- **Server name**: compiled into the release build (`HJV_SERVER`, a host name such as
  `rd.example.com`, optionally `:port`; default port 443; scripts/package-relay.sh refuses
  placeholder and test names like that one outside test builds). Requests go to
  `https://<HJV_SERVER>/api/hjv/relay/hello` and `https://<HJV_SERVER>/api/hjv/relay/poll`.
  The `Host` header is the name, with `:port` when the port is not 443. SNI is the name.
- **Request headers** the relay sends: `Host`, `User-Agent: hjv-relay/<version>`,
  `Content-Type: application/json`, `Accept: application/json`, `Content-Length`,
  `Connection: close`. No cookies, no `Authorization`, no proxy support.
- **Request body**: one JSON object, the envelope of section 4, at most **65536 bytes**.
- **Response**: status `200` for success. The body MUST be one JSON object, UTF-8, sent with
  `Content-Type: application/json; charset=utf-8` and framed by `Content-Length` or chunked
  transfer coding (the relay also accepts a body ended by closing the connection). The relay
  refuses response bodies over **262144 bytes** and response heads over 16 KiB. It ignores fields
  it does not know. `1xx` responses before the final one are skipped.
- The relay gives up on a request **35 seconds** after it started connecting (DNS, connect, TLS,
  write and the whole long-poll wait included).
- nginx in front of hjv-api passes `/api/hjv/` through (the long-poll of at most 20 s is inside
  nginx's default 60 s `proxy_read_timeout`), sets `X-Real-IP`, which hjv-api records as the
  relay's egress address, and limits hello per source (section 4.3).

## 2. Encodings

| Name in this document | Exact meaning |
|---|---|
| base64 | RFC 4648 section 4 alphabet `A–Z a–z 0–9 + /` **with** `=` padding |
| base64url | RFC 4648 section 5 alphabet `A–Z a–z 0–9 - _` **without** padding |
| base32 | RFC 4648 section 6 alphabet `A–Z 2–7`, no padding (lowercase where stated) |
| hex | lowercase hexadecimal, two digits per byte |
| time | integer Unix seconds (UTC); JSON numbers without fraction or exponent |
| SHA-256 | FIPS 180-4 |
| Ed25519 | RFC 8032 (pure Ed25519, no prehash, no context) |
| NUL | the single byte 0x00 |
| MAC (in JSON) | six two-digit hex bytes separated by `:` (`00:00:5e:00:53:01`) |
| IPv4 network | `a.b.c.d/p` in dotted decimal, `p` in plain decimal digits |

JSON integers in all objects of this protocol MUST be written as integers (`60`, not `60.0`).

## 3. Keys and identity

### 3.1 The relay key, relay ID and enrollment code

- On its first run the relay makes an Ed25519 key pair and keeps the private key on the NAS
  (`/volume1/@hjv-relay/relay.key`, root only). The private key never leaves the NAS. `pk` is the
  32-byte public key.
- **fingerprint** = the first 10 bytes (80 bits) of SHA-256(`pk`).
- **relay ID** = `r-` followed by base32 of the fingerprint in **lowercase**: 16 characters, so
  the ID matches `^r-[a-z2-7]{16}$`.
- **enrollment code** = the same 16 base32 characters in **uppercase**, in four groups of four
  joined by `-`: `^[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}-[A-Z2-7]{4}$`. The relay writes it to its
  log at every start and whenever it becomes pending. An administrator approves a relay by this
  code; the server SHOULD accept the code case-insensitively and ignore `-` and spaces in it.
- A relay keeps its key, and so its ID, across restarts, reboots and upgrades. A new key is a new
  relay that must be approved again.

### 3.2 Command keys

- hjv-api signs wake commands with an Ed25519 **command key**. Each command key has a **key id**,
  an integer 1–65535. Its private key stays on the server (for example as a systemd encrypted
  credential); its public key is compiled into hjv-relay.
- The relay build variable is `HJV_COMMAND_PUBKEY` =
  `<key id>:<base64 public key>[,<key id>:<base64 public key>]`: one or two keys with distinct
  ids, such as `1:<base64 public key>`. A relay built without this variable, or with a value
  that does not parse (a weak, small-order key, more than two keys, duplicate ids), refuses to
  `run`; `hjv-relay version` prints the keys in this form, or the problem.
  scripts/package-relay.sh refuses to package such a build, and also the public test keys of
  this repository (section 13 and the tests) unless it is told that the build is a test build.
  The key generator of hjv-api SHOULD print exactly this form.
- Rotation: release relays built with both the current key and the next one (ids 1 and 2), then
  switch the server to sign with the next key, then drop the old key from later relay builds.
- The command key MUST NOT sign anything other than wake commands (section 7.2 also separates
  domains by prefix).

## 4. Request envelope

Every request body is a JSON object, the **envelope**:

| Field | Type | Value |
|---|---|---|
| `id` | string | the relay ID (section 3.1) |
| `t` | integer | the relay's estimate of the **server's** current time (section 10.1), in seconds |
| `n` | string | a nonce: 16 random bytes in base64url (22 characters), new for every request |
| `p` | string | the payload: a JSON object serialized to text, carried as a JSON **string** |
| `s` | string | base64 of the 64-byte Ed25519 signature (88 characters) by the relay key |
| `pk` | string | **hello only**: base64 of the relay's 32-byte public key |

Unknown envelope fields MUST be ignored by the server.

### 4.1 Signed bytes

`s` signs these bytes (the parts joined by single NUL bytes, no trailing NUL):

```
"hjv-relay-v1" NUL op NUL id NUL t NUL n NUL base64(SHA-256(p))
```

- `op` is `hello` or `poll`, the last part of the request path. A signature for one endpoint is
  therefore never valid for the other.
- `id`, `n` are the envelope's strings as received. `t` is the decimal representation of the
  integer `t` (ASCII digits, no sign, no leading zeros).
- `p` here means the UTF-8 bytes of the envelope's `p` string **after** JSON unescaping, that is,
  the payload text exactly as the relay serialized it. The SHA-256 digest is put in the message
  as base64 (44 characters, with padding).
- The server MUST verify the signature over the bytes it received (rebuilding the message from
  the received strings) **before** it parses `p`. It MUST NOT re-serialize the payload to verify.

### 4.2 Payload

`p` parses to a JSON object with `"v": 1` and the fields of the endpoint (section 5). Unknown
payload fields MUST be ignored by the server.

### 4.3 Server checks, in this order

The server MUST apply these checks to both endpoints in this order, and answer with the first
failure (section 6 lists the bodies):

1. **Size**: body at most 65536 bytes; otherwise `413 too_large`.
2. **Shape**: the body is a JSON object; `id` matches `^r-[a-z2-7]{16}$`; `t` is an integer
   greater than 0; `n` is a string of 22 base64url characters that decodes to 16 bytes; `p` is a
   string; `s` is base64 that decodes to 64 bytes; for hello, `pk` is base64 that decodes to 32
   bytes and `id` equals the relay ID derived from `pk` (section 3.1). Otherwise
   `400 bad_request`.
3. **Key**: for hello, the key is `pk` from the envelope; if the server already has a relay
   with this `id` but a different stored key, `401 bad_signature` (the stored key is never
   replaced). For poll, it is the key the server stored for `id` at hello; if the server has no
   relay with this `id` (never seen, pending entry expired, or deleted), `404 unknown_relay`.
4. **Signature**: Ed25519 verification of `s` over the bytes of section 4.1 with that key, in
   the strict form that rejects non-canonical signatures and small-order keys (for example
   ed25519-dalek's `verify_strict`); otherwise `401 bad_signature`.
5. **Time**: `|t - server time| <= 300`; otherwise `400 clock` with `server_time` in the body.
6. **Nonce**: the pair (`id`, `n`) was not accepted in the last 600 seconds; otherwise
   `409 replay`. The server remembers accepted pairs for at least 600 seconds.
7. **Payload**: `p` is a JSON object with `"v": 1` and the endpoint's required fields with the
   right JSON types; otherwise `400 bad_request`.

Only after these come hjv-api's rate limits (`429`) and the endpoint's work. A request refused in
steps 1–6 changes nothing on the server.

Requests cost the server a signature check and a nonce entry before any of hjv-api's limits
apply, and hello is open to anyone with a key, which costs nothing to make. So:

- nginx MUST limit `POST /api/hjv/relay/hello` per source before the request reaches hjv-api,
  keyed by the IPv4 address or the IPv6 /64 (for example `limit_req` at 10 requests per minute
  with a burst of 5, and `limit_req_status 429`). It SHOULD answer with the JSON body of
  section 6 (`{"error":"rate_limited","retry_after":60}`, for example through `error_page 429`);
  a relay that gets any other body backs off. Poll needs no such limit: it is refused at step 3
  unless the relay is known.
- hjv-api SHOULD bound the nonce store (for example at 100 000 entries) and answer
  `503 unavailable` with `retry_after` while it is full, rather than forget nonces early.

## 5. Endpoints

### 5.1 `POST /api/hjv/relay/hello`

The relay sends hello when it starts and whenever it has no known registration (after a
`404 unknown_relay`). It is idempotent.

**Payload**

| Field | Type | Value |
|---|---|---|
| `v` | integer | `1` |
| `version` | string | the relay's version, `MAJOR.MINOR.PATCH` (for example `1.6.0`) |
| `arch` | string | CPU architecture: `x86_64`, `aarch64`, `arm`, ... |
| `os` | string | for DSM `DSM <productversion>-<buildnumber>[ Update <n>]`, for example `DSM 7.2.1-69057 Update 5`; else the os-release name or `Linux`; at most 100 characters |
| `kernel` | string | the kernel release (`uname -r`), at most 100 characters |
| `ifaces_hash` | string | hex SHA-256 of the `ifaces` JSON text (section 9) |
| `ifaces` | array | the relay's usable interfaces (section 9) |

**Server behaviour**

- Unknown `pk` with a **pre-approval** for its enrollment code (section 11): create the relay
  directly as **active** for the account of the pre-approval, which is then used up. The limits
  below do not apply to it.
- Other unknown `pk`: create a **pending** entry with the ID, `pk`, the time, the egress
  address (`X-Real-IP`) and the payload's `version`, `arch`, `os`, `kernel` and `ifaces`. Limits:
  at most 5 new pending entries per source address per hour (per IPv6 /64), at most 20 pending
  entries in all; beyond them `429 rate_limited` with `retry_after`. A pending entry expires 24
  hours after it was made; then the relay is unknown again (the relay's next poll gets
  `404 unknown_relay`, and its next hello makes a new entry, with the same code). Since keys
  cost nothing, a few addresses can keep these limits full; pre-approval is the way past that.
- Known relay (pending, active or revoked): update the stored `version`, `arch`, `os`, `kernel`,
  `ifaces`, `ifaces_hash`, egress address and last-seen time (a revoked relay keeps only what
  section 11 allows); answer with its state. These requests MUST NOT count against the limits
  for new entries (a generous per-relay limit, such as 60 an hour, is fine).
- The server answers hello at once (no long-poll).

**Response 200**

| Field | Type | Value |
|---|---|---|
| `server_time` | integer | the server's time now (required) |
| `state` | string | `pending`, `active` or `revoked` (required) |
| `code` | string | the enrollment code of `pk` (SHOULD be present; the relay logs a warning if it differs from its own) |
| `poll_after` | integer | optional: seconds before the first poll (the relay uses at most 3600) |

After a 200 the relay polls: at once when active (subject to the 2-second minimum of section
5.2), after `poll_after` or 30 s when pending, after `poll_after` or 3600 s when revoked. A
`state` value other than the three above is treated as a failed request (section 6).

### 5.2 `POST /api/hjv/relay/poll` (long-poll)

**Payload**

| Field | Type | Value |
|---|---|---|
| `v` | integer | `1` |
| `version`, `arch`, `os`, `kernel` | string | as in hello |
| `wait` | integer | 0–20: the longest the server may hold this request, in seconds (the relay sends 20, and 0 in the at most two polls that deliver its last results before it exits to restart with a new executable) |
| `ifaces_hash` | string | hex SHA-256 of the relay's current `ifaces` JSON text |
| `ifaces` | array | **optional**: present when the interfaces changed since the server last got them, or when the last response asked for them (`send_ifaces`) |
| `results` | array | results of commands not yet acknowledged (section 8), possibly empty; at most 50 |

**Server behaviour**, after the checks of section 4.3:

1. Apply `results` (section 8.2) and store `version`, `arch`, `os`, `kernel`, the egress address,
   the poll time and, when present, `ifaces` with `ifaces_hash`.
2. If `ifaces` is absent and `ifaces_hash` differs from the hash stored with the last `ifaces`
   (or the server has none), set `send_ifaces: true` in the response.
3. By state:
   - **revoked**: answer at once with `state: "revoked"`; never commands.
   - **pending**: answer at once or hold up to `min(wait, 20)` seconds (the server's choice);
     never commands.
   - **active**: if commands are queued for this relay, answer at once with them (section 7.4).
     Otherwise hold the request until a command is queued for this relay or `min(wait, 20)`
     seconds have passed, then answer, with the commands if any.
   The server SHOULD answer within `min(wait, 20) + 5` seconds; the relay abandons the request
   35 seconds after it began.
4. The time of the last accepted poll is the relay's liveness for relay selection (the design
   notes count a relay online when it polled in the last 60 seconds).

**Response 200**

| Field | Type | Value |
|---|---|---|
| `server_time` | integer | required |
| `state` | string | `pending`, `active` or `revoked` (required) |
| `commands` | array | optional, only when `state` is `active`: signed commands, each `{"c": string, "s": string, "k": integer}` (section 7) |
| `send_ifaces` | boolean | optional, default `false`: include `ifaces` in the next poll |
| `latest_version` | string | optional: the newest relay version (the relay logs a notice when it is older) |
| `min_version` | string | optional: the oldest relay version the server still supports (the relay logs that it needs replacing; it does not update itself) |
| `poll_after` | integer | optional: seconds to wait before the next poll (the relay uses at most 3600) |

**Relay behaviour after a 200**: it drops the results it delivered, handles the commands
(section 7.3), and polls again **at once** if it sent packets for a command, so that the result
reaches the server immediately; otherwise it waits `poll_after`, or by default 0 s when active,
30 s when pending and 3600 s when revoked. Apart from that immediate poll it never starts two
polls less than 2 seconds apart, so refused commands go back with the next regular poll and
cannot make the relay poll faster. When the server holds polls as described, an active relay
keeps one request open almost all the time and a command reaches it within a second.

## 6. Errors and back-off

Every non-200 response SHOULD carry a JSON body `{"error": "<code>", "server_time": <time>}`
plus `retry_after` (integer seconds) where stated. The relay reads `server_time` from any
response that has it (section 10.1).

| Status | `error` | Meaning | Relay |
|---|---|---|---|
| 400 | `bad_request` | malformed envelope or payload (4.3 steps 2 and 7) | back off |
| 400 | `clock` | `t` more than 300 s from the server's time; body has `server_time` | adopts `server_time`, re-signs and sends the same payload **once** at once; a second failure backs off |
| 401 | `bad_signature` | the signature does not verify | back off |
| 404 | `unknown_relay` | poll from a relay the server does not know | becomes unknown and sends hello after one back-off step |
| 409 | `replay` | the nonce was used | back off (every request has a new nonce) |
| 413 | `too_large` | body over 65536 bytes | back off |
| 429 | `rate_limited` | a limit was hit; body has `retry_after` | waits `retry_after` (clamped to 1–3600 s); backs off if it is missing |
| 503 | `unavailable` | the server cannot serve now; `retry_after` optional | waits `retry_after` if given, else backs off |
| other | any | anything else, including a body that is not JSON | back off |

Network errors (DNS, connect, TLS, timeouts) and 200 responses that are not the expected JSON
also back off.

Every `error` code and `state` value in this document is 1–32 lowercase ASCII letters, digits
and `_`, and hjv-api MUST keep to that form for codes it adds. The relay writes a response's
`error` or `state` to its log and status file only when it has that form, and no other text
from a response (section 14).

**Back-off**: 5, 10, 20, 40, 80, 160, 300, 300, ... seconds, each multiplied by a random factor
between 0.8 and 1.2, reset after any 200. The relay logs the first failure, then at most one line
per 10 minutes while failures continue, and one line when it recovers.

## 7. Wake commands

### 7.1 The command object

The command `c` is a JSON object:

| Field | Type | Value |
|---|---|---|
| `v` | integer | `1` |
| `job` | string | the job id: 32 hex digits (128 random bits), lowercase |
| `relay` | string | the relay ID this command is for |
| `iat` | integer | issued at, server time |
| `exp` | integer | expires at, server time; `0 < exp - iat <= 120`; the server uses `iat + 60` |
| `net` | string | the PC's IPv4 network from its wake report, canonical (host bits zero), prefix 8–32, for example `10.1.4.0/22` |
| `macs` | array of strings | 1–4 distinct MACs of the PC's wired adapters; individual addresses (least significant bit of the first byte 0), not `00:00:00:00:00:00`; lowercase (the relay also accepts uppercase) |
| `ports` | array of integers | a non-empty subset of `9` and `7` without repeats, in sending order; the server sends `[9, 7]` |
| `repeat` | integer | rounds, 1–5; the server sends `3` |
| `gap_ms` | integer | milliseconds between rounds, 100–1000; the server sends `200` |

A version 1 relay ignores fields it does not know. A change that an old relay must not misread
needs a new `v`.

### 7.2 Signing

- The server serializes the command object to compact JSON text once (UTF-8, any key order) and
  signs the bytes `"hjv-wake-v1" NUL <command text>` with the command key.
- In a poll response each command is `{"c": "<command text as a JSON string>", "s":
  "<base64 signature, 88 characters>", "k": <key id>}`.
- The relay verifies the signature over the UTF-8 bytes of the received `c` string (after JSON
  unescaping), and only then parses it. The server MUST keep the exact text, signature and key id
  of every command and send that same triple whenever it sends the command again; serializing
  the object again may change the bytes.
- `cmd`, the command's name in results, is the hex SHA-256 of the command text (64 characters).

### 7.3 Relay checks, in this order

For each entry of `commands` (at most 16 per response; the server sends any others again) the
relay runs these checks. The first that fails names the result's `error`, and nothing is sent:

1. The entry is `{"c": string, "s": string, "k": integer}` (otherwise `bad_command`, with `cmd`
   null when `c` is not a string); `k` is a key id of this build (`unknown_key`); `s` is base64
   of 64 bytes and verifies strictly over `"hjv-wake-v1" NUL c` with that key (`bad_signature`).
2. `c` is a JSON object whose `v` is a number (otherwise `bad_command`); `v` is the integer 1
   (any other number, `1.5` included: `bad_version`); every other field of 7.1 is present with its
   JSON type, `job` is 32 lowercase hex digits, `net` is a canonical IPv4 network with prefix
   8–32 (otherwise `bad_command`).
3. `relay` is this relay's ID (`wrong_relay`).
4. With `now` = the relay's estimate of the server's time (section 10.1): `exp > now`
   (`expired`); `iat <= now + 30` (`not_yet_valid`); `1 <= exp - iat <= 120` (`bad_lifetime`);
   and `iat >= newest - 120`, where `newest` is the largest `iat` of the commands that passed
   checks 1–4 earlier since the relay started, with no limit before the first (`expired`).
   This last part rests on signed values only:
   such a command had expired before the server signed a later one, whatever `server_time` says,
   so setting `server_time` back cannot bring back a recorded command. A command that passes
   check 4 raises `newest`, whatever happens next.
5. The relay has not seen `job` since it started (`replayed`). A job is remembered from the
   moment its command passes checks 1–4, whatever happens next; a job id is good for one
   attempt. The relay may forget a job once check 4 refuses its command anyway
   (`iat < newest - 120`). It remembers at most 4096 jobs; when it forgets one only because of
   that limit, it refuses every later command whose `iat` is not greater than that job's
   (`replayed`).
6. `macs`: 1–4 entries, each a MAC as in 7.1, individual, not all zeros, no repeats (`bad_macs`).
7. `ports`, `repeat`, `gap_ms` as in 7.1 (`bad_params`).
8. The relay has an interface on `net` (section 9.2) (`no_iface`).
9. Limits: at most 10 commands and 120 packets in any 60 seconds (`rate_limited`); a command is
   counted when it passes this check, and refused whole if its packets would exceed the limit.

A command that passes is sent at once (section 9.3). Its result's `error` is null when every
packet went out, `partial` when some did, `send_failed` when none did. Commands that arrive while
the relay is pending or revoked are not checked or sent; their results say `not_active`.

### 7.4 The server's command queue

- A wake request in hjv-api queues one command for one relay. The server includes every queued,
  unexpired command of the relay in each poll response to it (oldest first, at most 16 per
  response) until it has received a result for the command (section 8.2) or the command has
  expired; then it removes it.
- A command that expires without a result is a failed job (for example `no_result`).
- Delivering a command again is safe: the relay runs a job at most once (check 5).

## 8. Results

### 8.1 The result object

| Field | Type | Value |
|---|---|---|
| `cmd` | string or null | hex SHA-256 of the command text `c` as received; null only when the entry had no string `c` |
| `job` | string or null | the command's `job` when the command got past check 2, else null |
| `sent_at` | integer or null | the relay's estimate of the server time when the last packet went out; null when nothing was sent |
| `iface` | string or null | the interface check 8 chose (also when check 9 then refused), else null |
| `packets` | integer | the number of magic packets sent (0 when refused) |
| `error` | string or null | null, `partial`, `send_failed`, or a refusal of section 7.3: `unknown_key`, `bad_signature`, `bad_command`, `bad_version`, `wrong_relay`, `expired`, `not_yet_valid`, `bad_lifetime`, `replayed`, `bad_macs`, `bad_params`, `no_iface`, `rate_limited`, `not_active` |

### 8.2 Delivery and handling

- The relay sends results in the `results` of its next poll: at once after it sent packets,
  otherwise at the next regular poll (section 5.2). It sends them again in every poll until one
  gets a `200`, so the server can receive a result more than once. It keeps at most 100
  undelivered results (the oldest are dropped) and sends at most 50 per poll. Results do not
  survive a restart of the relay; before it exits for a new executable, the relay delivers the
  results still waiting in at most two polls with `wait: 0`, and gives up after a failed one.
- The server matches a result to its command by `cmd`, and MUST treat results idempotently.
  Suggested job states: `error` null or `partial` with `packets > 0` → **sent** at `sent_at`;
  `replayed` → no new information (ignore); any other `error` → **failed** with that code.
  Results whose `cmd` matches no command of this relay are logged and ignored.
- Whether the PC woke up is not part of this protocol: hjv-api confirms it from the PC's
  registration with hbbs (the design notes, section 6-5).

## 9. Interfaces

### 9.1 Interfaces the relay uses and reports

An interface is **usable** when it is up, can broadcast, is neither loopback nor point-to-point,
its name does not start with `docker`, `br-`, `veth`, `virbr`, `lxcbr`, `lxdbr`, `cni`,
`flannel`, `cali`, `kube`, `tun`, `tap`, `ppp`, `wg`, `tailscale`, `zt`, `nebula`, `dummy`,
`sit`, `gre`, `erspan`, `ip6` or `vxlan` (and is not `lo`), and it has at least one usable IPv4
address: prefix 8–30, not loopback, link-local, multicast, the network or the broadcast address.
DSM's `eth0`, `eth1`, `bond0` and the Open vSwitch bridge `ovs_eth0` are usable.

`ifaces` lists the usable interfaces, sorted by name, at most 16:

| Field | Type | Value |
|---|---|---|
| `name` | string | the device name |
| `mac` | string or null | its MAC |
| `ipv4` | array of strings | its usable addresses as `a.b.c.d/p` (sorted, at most 8) |
| `up` | boolean | always true for a usable interface |
| `running` | boolean | the link has carrier |
| `gateway` | string or null | the gateway of the interface's lowest-metric default route |
| `gateway_mac` | string or null | that gateway's MAC from the ARP cache, when known |

`ifaces_hash` is the hex SHA-256 of the JSON text of `ifaces` exactly as the relay serialized it.
The server only compares it for equality with the hash that came with the last full `ifaces`.

The server picks a relay for a PC from these reports (design notes, section 6-3): same network
and gateway first, then overlapping networks with the same gateway IP or gateway MAC.

### 9.2 Which interface a command uses

Among the usable addresses whose network overlaps `net`: those on an interface whose link has
carrier (`running`) first, since packets sent on a port without a cable go nowhere; then an
**exact** match (same network and prefix); then an address whose network **contains** `net`
(shorter prefix); then one whose network lies **within** `net` (longer prefix, for example a NAS
with /24 on the PC's /22 wire); among equals the smallest prefix difference, then the interface
name. No overlap: `no_iface`. An interface without carrier is still used when it is the only
overlap; the result then counts the packets the kernel accepted.

### 9.3 What is sent (informative)

- One UDP socket per command, bound to the chosen address (port chosen by the kernel), with
  `SO_BROADCAST`, and `SO_BINDTODEVICE` to the chosen interface where the kernel allows it
  without privileges (Linux 5.7 and later; otherwise the address bind alone picks the interface).
- Payload: the 102-byte magic packet, 6 bytes `0xFF` and the MAC 16 times.
- Destinations: `255.255.255.255` and the chosen address's own subnet broadcast (when that is
  different); never the PC's broadcast address (when the NAS's prefix differs from the PC's, that
  address is off-link for the NAS and goes to the gateway), never a unicast address, never a port
  other than 9 or 7.
- Order: `repeat` rounds, `gap_ms` apart; in each round, for each MAC, for each port in the given
  order, for each destination one packet. Packets = repeat × MACs × ports × destinations: the
  default command (3 rounds, one MAC, ports 9 and 7, two destinations) sends 12.

## 10. Time and timing

### 10.1 Server time

Every response (200 and errors) carries `server_time`, the server's clock in Unix seconds. The
relay's estimate of the server's time is the last `server_time` it received plus the time since
then on its monotonic clock; before its first response it uses its own clock. It uses the
estimate for `t` (section 4) and for the command checks (section 7.3), so neither depends on the
NAS's clock. `server_time` values outside 2020–3000 are ignored.

TLS is different: the certificate check (section 1) uses the NAS's own clock, as any TLS client
does. A NAS clock that is off by more than the certificate's validity allows (a dead clock
battery after a power cut, NTP blocked) makes every request fail as a TLS error; the relay then
logs a hint to check the NAS's time, and the install guide says how.

### 10.2 Numbers

| What | Value |
|---|---|
| poll hold by the server | at most `min(wait, 20)` s; the relay sends `wait: 20` (`wait: 0` in its last polls before a restart) |
| relay request timeout | 35 s from connect to the last byte |
| least time between poll starts | 2 s, except the poll right after packets were sent |
| poll interval when pending / revoked | `poll_after`, else 30 s / 3600 s (`poll_after` at most 3600) |
| back-off | 5 → 300 s, doubling, ±20 %; `retry_after` 1–3600 s |
| request time window | `|t - server time| <= 300` s |
| nonce memory (server) | at least 600 s |
| command lifetime | `exp - iat` 1–120 s; the server uses 60 s |
| allowance for an early `iat` | `iat <= now + 30` s |
| floor for `iat` (relay) | `iat >= newest - 120` s, `newest` the largest accepted `iat` since the relay started |
| job memory (relay) | since the relay started, until `iat < newest - 120`; at most 4096 jobs |
| relay rate limit | 10 commands and 120 packets in any 60 s |
| hello per source (nginx) | for example 10 per minute, burst 5, per IPv4 address or IPv6 /64 |
| pending entries (server) | 5 new per source address (IPv6 /64) per hour, 20 in all, 24 h lifetime; not for pre-approved codes |
| pre-approval (server) | for example 7 days |
| results | at most 50 per poll, 100 kept |
| commands handled per response | 16 |

## 11. Registration states (server side)

```
            hello (new key)              admin approve <code> --owner-of <PC ID>
 unknown ───────────────────▶ pending ─────────────────────────────────────────▶ active
    │ ▲                          │ 24 h without approval                         ▲  │
    │ └──────────────────────────┘                                               │  │
    │        hello (new key whose code was pre-approved)                         │  │ admin
    └────────────────────────────────────────────────────────────────────────────┘  │ revoke
                                                                                    ▼
                                                                                 revoked
```

- **pending**: made by hello (section 5.1). Approval by an administrator binds the relay to the
  account that owns the given PC (`--owner-of <PC ID>`), so no portal account name has to be
  typed.
- **Approving** trusts whoever reported the code and the PC ID, and PC IDs are not secret.
  Before approving, the administrator MUST confirm the request with the owner of that PC
  through a channel the administrator already trusts (in person, by phone, or from the owner's
  known e-mail address), and MUST compare the pending entry's egress address and the networks
  of its interfaces with that PC's wake report (`wol-info`, design notes section 4): a relay
  that cannot reach the PC's network is the wrong entry. The approval tool MUST show the
  pending entry's egress address, its interfaces' networks (MACs masked) and its age, next to
  the PC's reported networks and egress address.
- **Pre-approval**: `admin relay approve <code> --owner-of <PC ID>` for a code with no pending
  entry (the relay has not said hello yet, or the pending limits turned it away) stores the
  code and the account, after the same confirmation with the PC's owner. When a hello arrives
  whose `pk` gives that code, the relay becomes active at once, whatever the pending limits, and
  the pre-approval is used up. Pre-approvals expire (for example after 7 days). The code is 80
  bits of SHA-256 of a key, so nobody can make another key that matches a pre-approved code.
  The networks cannot be compared before the hello: the administrator SHOULD compare them once
  the relay is active (the approval tool SHOULD list relays with the same details) and revoke
  it on a mismatch.
- **active**: gets commands for PCs of its owner account (or, when the server allows sharing,
  other accounts).
- **revoked**: never gets commands. The server SHOULD keep a minimal record (relay ID, `pk`,
  `revoked`, the time) and delete the rest (owner, interfaces, addresses), so that the same key
  cannot enroll again unnoticed; the relay then asks only hourly. Deleting the record entirely
  makes the relay unknown: it says hello again and becomes pending again.
- Self-service enrollment (an app entering the code) can be added later without changing this
  protocol.

## 12. Versions

- `v` of the payloads and of the command object is `1`. A relay ignores unknown response fields;
  the server ignores unknown envelope and payload fields. Incompatible changes get a new `v` (and
  new domain strings `hjv-relay-v2`, `hjv-wake-v2`).
- `version` is the relay's `MAJOR.MINOR.PATCH`. With `latest_version` and `min_version` the server
  can tell users that a relay needs replacing; the relay itself only logs it and never updates
  itself. The server MAY stop queuing commands for relays older than `min_version`.

## 13. Test vectors

Relay key: the Ed25519 private key (seed) is the 32 bytes `00 01 02 ... 1f`.

| | |
|---|---|
| `pk` (base64) | `A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg=` |
| fingerprint (hex) | `56475aa75463474c0285` |
| relay ID | `r-kzdvvj2umnduyauf` |
| enrollment code | `KZDV-VJ2U-MNDU-YAUF` |

A hello at `t` = `1760000000` with `n` = base64url of the bytes `00 01 ... 0f` =
`AAECAwQFBgcICQoLDA0ODw` and this payload text `p` (`ifaces_hash` is the SHA-256 of `[]`):

```
{"v":1,"version":"1.6.0","arch":"x86_64","os":"DSM 7.2.1-69057 Update 5","kernel":"5.10.55+","ifaces_hash":"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945","ifaces":[]}
```

- base64(SHA-256(`p`)) = `PZ3ddx3dke+FIcRE2SqeSlomxD+MMZtr3gsC2ztzq0Q=`
- signed bytes (116 bytes; `\0` is NUL):
  `hjv-relay-v1\0hello\0r-kzdvvj2umnduyauf\01760000000\0AAECAwQFBgcICQoLDA0ODw\0PZ3ddx3dke+FIcRE2SqeSlomxD+MMZtr3gsC2ztzq0Q=`
- `s` = `xfOpDyAd0LYdAww0n+eYJg2DvHTymuHWYOZuGIy89rWYRqHcqEb0PVwTD5MUQIlzSNdUAmsGenMyLh8iqngcAQ==`
- the request body:

```
{"id":"r-kzdvvj2umnduyauf","t":1760000000,"n":"AAECAwQFBgcICQoLDA0ODw","p":"{\"v\":1,\"version\":\"1.6.0\",\"arch\":\"x86_64\",\"os\":\"DSM 7.2.1-69057 Update 5\",\"kernel\":\"5.10.55+\",\"ifaces_hash\":\"4f53cda18c2baa0c0354bb5f9a3ecbe5ed12ab4d8e11ba873c2f11161202b945\",\"ifaces\":[]}","s":"xfOpDyAd0LYdAww0n+eYJg2DvHTymuHWYOZuGIy89rWYRqHcqEb0PVwTD5MUQIlzSNdUAmsGenMyLh8iqngcAQ==","pk":"A6EHv/POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg="}
```

Command key: the seed is the 32 bytes `20 21 22 ... 3f`, key id 1, so
`HJV_COMMAND_PUBKEY=1:Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=`. The command text `c`:

```
{"v":1,"job":"0123456789abcdef0123456789abcdef","relay":"r-kzdvvj2umnduyauf","iat":1760000000,"exp":1760000060,"net":"10.1.4.0/22","macs":["00:00:5e:00:53:01"],"ports":[9,7],"repeat":3,"gap_ms":200}
```

- signed bytes: `hjv-wake-v1` NUL then the command text
- `s` = `91MXS5rMOB9k3ZP7XxPue9M++nXy9w4VIpjcqGJspnFI46U37TGC+st2hv6D8DxcUJOaN/2PcY7XeQ6wgfmLBw==`
- `cmd` = `eefb709bfc6dae46233f75d43c02d0dfb173218d3ed9010c40dc316b4c3d7abe`
- the entry in `commands`:

```
{"c":"{\"v\":1,\"job\":\"0123456789abcdef0123456789abcdef\",\"relay\":\"r-kzdvvj2umnduyauf\",\"iat\":1760000000,\"exp\":1760000060,\"net\":\"10.1.4.0/22\",\"macs\":[\"00:00:5e:00:53:01\"],\"ports\":[9,7],\"repeat\":3,\"gap_ms\":200}","k":1,"s":"91MXS5rMOB9k3ZP7XxPue9M++nXy9w4VIpjcqGJspnFI46U37TGC+st2hv6D8DxcUJOaN/2PcY7XeQ6wgfmLBw=="}
```

These private keys are public test values: never use them for anything else.

## 14. Security notes

- Requests are authenticated by the relay key and commands by the command key, independently of
  TLS. A party that can answer in the server's name, whether it intercepts or terminates TLS
  with a certificate the relay trusts or takes over nginx or the server's socket, can read
  requests and withhold answers, but cannot forge commands. Holding responses back only makes
  waking fail.
- Responses other than the commands are not signed, so such a party can also change the state a
  relay believes it has, or set `server_time`. Setting it back does not bring back a recorded
  command while the relay runs: the relay refuses a job it has seen, and a command issued more
  than 120 s before the newest command it accepted (section 7.3, checks 4 and 5). Both are kept
  in memory only, so after a relay restart such a party can replay commands it recorded
  earlier, each at most once and only in the order they were issued, within the rate limits:
  the worst outcome is an unwanted wake-up of a PC that was woken before. The relay never
  writes text from a response to its log or status file beyond short codes and numbers, so such
  a party cannot put lines (a false enrollment code) into the log the install guide tells
  people to read.
- A relay acts only on its own network: commands name MACs, never addresses; packets go only to
  broadcast addresses of its own interfaces, only to ports 9 and 7, with fixed content, within
  rate limits. A stolen command key therefore allows no more than unwanted wake-ups on the
  relay's LAN; a stolen relay key allows false results and learning which PCs are woken.
- The relay starts as root on DSM only to read its key and open its files, then runs as
  uid/gid 65534 with no supplementary groups and `PR_SET_NO_NEW_PRIVS`, and it is non-dumpable
  from the start. Started from a terminal, it gives the terminal up first, so the
  unprivileged process can neither read from it nor push input into it. It keeps no PC
  passwords, tokens or account data; its log masks MACs to their last two bytes.
- Enrollment is open to anyone with a key (section 5.1), so approval is where trust is decided:
  section 11 requires the administrator to confirm each request with the PC's owner and to
  compare the relay's networks with the PC's. An approved wrong relay would receive the MACs and
  networks of the owner's PCs and could report false results.
- A residual risk outside this protocol: whoever can write to the folder of the hjv-relay
  executable on the NAS runs code as root within about a minute, because the relay restarts
  itself when its executable changes and the boot-up task's root loop then starts the new
  file. The install guide keeps it in a folder only administrators can write, with no sync
  tasks, sharing links or file requests on it (docs/relay-dsm.md, section 2).
