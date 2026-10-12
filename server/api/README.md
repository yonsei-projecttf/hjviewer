# hjv-api

The HJViewer account API: login with a Yonsei portal ID through the university's SSO, and the
per-account list of Windows PCs that the apps show as "accessible devices" (design v2 §9). Since
design v3 every login also registers an account key of its device, which signs every account
request after it and, on the PCs' trust lists, the connections between devices of the same
account that need no PC password. It runs next to hbbs/hbbr on the server, behind nginx, as a
systemd service that listens only on a Unix socket systemd holds. GNU AGPL-3.0-only, like the
rest of the repository.

## Build

```sh
cd server/api
HJV_API_COMMIT=$(git rev-parse HEAD) cargo build --release --locked   # target/release/hjv-api
cargo build --features fake-sso                                        # also target/debug/hjv-fake-sso
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

`HJV_API_COMMIT` is what `GET /api/hjv/source` reports as the running commit. TLS is rustls with
ring and the system's CA certificates; SQLite is built in (rusqlite `bundled`).

## Run

- `deploy/systemd/hjviewer-api.socket` holds `/run/hjviewer-api.sock` (root, group of nginx's
  workers, 0660); `deploy/systemd/hjviewer-api.service` runs `hjv-api serve` as the system user
  `hjviewer-api`, sandboxed like hbbs, with the egress filter in a server-side drop-in. Enable and
  start both: `sudo systemctl enable --now hjviewer-api.socket hjviewer-api.service`. With the
  socket alone, hjv-api would start only at the first request, and until then the admin commands
  and the hourly and daily upkeep (retention included) would not run.
- Start the service before `hjv-api admin allow import` (the admin commands talk to the running
  service), then switch nginx over: the allowlist is empty until then, so nobody can log in early.
- Its log lines go to the journal namespace `hjviewer`: `journalctl --namespace=hjviewer -u
  hjviewer-api`. They hold counts, results and tags (log.rs), never IDs, addresses or secrets.
- `deploy/nginx-hjviewer-api.conf` replaces the `/api/` block of
  `deploy/nginx-hjviewer-locations.conf`: the account paths go to the socket, `/api/login-options`
  and `/api/device-group/accessible` get static answers, all other `/api/` paths stay 404, and
  `/api/login` is limited per client address (10 a minute after a burst of 5, 4 at a time). Its
  two zones go to `/etc/nginx/conf.d/hjviewer-api-zones.conf` (the header of the file has the
  lines). Before installing, check that the server and http blocks have no `real_ip_header` or
  `set_real_ip_from` that would let a client choose `$remote_addr`: hjv-api's address limits trust
  the `X-Real-IP` nginx sets from it.
- For tests and the E2E only: `hjv-api serve --listen 127.0.0.1:<port> --env-file <file>
  --credentials <dir> --db <file> --admin-socket <path> --operator-log <file>`.

## Settings

Not secret, from the environment (`EnvironmentFile=/etc/hjviewer/hjviewer-api.env`, or
`--env-file`). `hjviewer-api.env.example` lists every name with its default; only the three SSO
settings are required. Unknown `HJV_API_` names stop the start.

Set `HJV_API_HBBS_PUBKEY` to hbbs's public key (the text of its `id_ed25519.pub`, the key the apps
are built with): hbbs signs its `hjv-pk` answers with it, and without it no PC is added to an
account (logins still work; the journal and `selfcheck` say so).

Wrong passwords reach the SSO at most `HJV_API_LIMIT_ID` plus `HJV_API_LIMIT_BOUND` times per
portal ID (the second is one limit for all the PCs of an account). Keep that sum, together with the
WordPress plugin's limit for the same IDs, below the portal's own lockout. `HJV_API_LIMIT_OK_ID`
and `HJV_API_LIMIT_OK_IP` limit successful logins, so that one known password cannot keep calling
the SSO; of the SSO calls of a minute (`HJV_API_LIMIT_SSO`) one account may take a quarter.

`HJV_API_CAMPUS_NETS` (the university's networks, such as `192.0.2.0/24,2001:db8::/32`) lets the
account's other devices see whether a new login came from on or off campus; the address itself is
never stored. `HJV_API_PASSWORDLESS_KINDS` is the connection kinds a device of the same account may
open without the PC's password (U3: `remote,file,camera`; `terminal` can be added; port forwarding
and RDP always ask for the password). The password-less switch is not a setting but state in the
database, off when installed (`hjv-api admin passwordless`).

## Credentials

Secrets come only from systemd credentials (`$CREDENTIALS_DIRECTORY`; `--credentials <dir>` for
tests), never from the environment or the repository:

| credential | content |
|---|---|
| `hjviewer-sso-client-secret` | what follows `Basic ` in the SSO token call (D2: the WordPress plugin's registration) |
| `hjviewer-sso-api-uuid` | the `apiUuid` of the SSO user call |
| `hjviewer-api-login-key` | X25519 secret key(s) that open sealed passwords, base64, one per line, current first |
| `hjviewer-api-log-key` | base64 of 32+ random bytes: the HMAC key of the tags in logs and records |

The user makes them on the server, without an agent (v2 §10-3), for example:

```sh
systemd-ask-password -n "SSO client secret:" | sudo systemd-creds encrypt --name=hjviewer-sso-client-secret - /etc/credstore.encrypted/hjviewer-sso-client-secret
hjv-api keygen seal | tee /media/usb/hjviewer-seal.key | sudo systemd-creds encrypt --name=hjviewer-api-login-key - /etc/credstore.encrypted/hjviewer-api-login-key
sudo hjv-api keygen log-key --encrypt-to /etc/credstore.encrypted/hjviewer-api-log-key
```

`keygen` never prints a secret to a terminal; `keygen seal` shows the public key for the app
build variable `HJV_LOGIN_SEAL_PUBKEY` and its key ID on stderr. Keep the offline copy of the seal
key: a host-bound credential is lost with the host. To change keys, put the new key line first and
keep the old one until every app build uses the new public key.

## Admin

`sudo hjv-api admin [--reveal] <command>` talks to the running service over
`/run/hjviewer-api-admin/admin.sock`, which root and the service's own user may use. Output is
masked unless `--reveal` is given (human operators only): IDs show their first four characters,
names do not show, accounts and devices carry tags, the same tags as in the journal. Commands take
an ID or `tag:<tag>`.

```text
allow import <file>       add the portal IDs in <file> to the allowlist (counts only)
allow remove <file>       remove them; their tokens and account keys stop working at once (their
                          PCs stay in their accounts and get a trust list without keys)
allow list                the allowlist
users | devices           accounts and the devices in them
key list [<id|tag:T>]     account keys: number, account, platform, the server's device name,
                          version, on or off campus, created, state (the device's own name and ID
                          with --reveal)
key revoke <ckid>         revoke an account key with its token
unlock <id|tag:T|ip:A>    clear the login counters of a portal ID (with its PCs' and successes'),
                          of a tag from a journal warning (an account's or an address's), or of a
                          client address (an IPv6 address by its /64)
user revoke-all <id|tag:T> | --all
                          revoke every token and account key of an account, or of every account
                          (devices log in again; PCs stay in their accounts)
revoke <id|tag:T>         the same as user revoke-all <id|tag:T>
device remove <id|tag:T>  take a device out of its account
user delete <id|tag:T>    delete an account with its tokens, account keys and devices
passwordless status | on | off [--account <id|tag:T> | --owner-of <PC ID>]
                          connections without the PC's password between an account's devices:
                          for every account (off when installed; off also ends the accounts turned
                          on one by one, so it is the incident switch), or for one account, named
                          by its portal ID or by one of its PCs
strict on | off           PCs ask the server at each password-less connection (incidents)
report                    counts, the login results of the last day and week, the limit counters,
                          the account keys, the switches and the change serial
selfcheck                 DNS, TCP and TLS to the SSO and hbbs's console, from inside the sandbox
keygen seal|log-key       as hjv-api keygen (runs locally, not in the service)
```

The rollout (design v3 §11): install with the switch off, `passwordless on --owner-of <the
operator's PC ID>`, a week of use, then `passwordless on`. To stop password-less connections, run
`passwordless off` and keep hjv-api running for 24 hours or until `report` shows that the PCs have
synced since: a PC that cannot reach hjv-api keeps its trust list for up to a day.

An allowlist file has one portal ID per line; `#` starts a comment, and a byte-order mark,
zero-width characters and Windows line ends do no harm. A line that is anything else refuses the
whole file (nothing changes) and the answer names only its line number.

`sudo hjv-api selfcheck --env-file /etc/hjviewer/hjviewer-api.env` runs the same check outside the
service, by hand; the unit's commented `ExecStartPost=` runs it with `--log-only` after each start.

### The operator record

The service writes two lines per command to `/var/log/hjviewer-api/operator.log`: one before the
command runs (a command whose line cannot be written does not run), with its targets as tags
(an allowlist file as the tags of its IDs), and one after, with what it changed in counts or that it
failed. Each line carries the hash of the one before; `hjv-api oplog-verify` checks the chain.

To make it append-only: start the service once, so that systemd creates `/var/log/hjviewer-api`
owned by `hjviewer-api` (0700) and hjv-api creates `operator.log` in it (0600); then
`sudo chattr +a /var/log/hjviewer-api/operator.log`. Do not create that directory as root or
`chown`/`chmod` it afterwards: systemd then walks it to fix the owner, fails on the append-only
file, and the unit does not start. (To create it in advance: `sudo install -d -o hjviewer-api -g
hjviewer-api -m 0700 /var/log/hjviewer-api`.) If the record cannot be opened, hjv-api still serves
logins, without the admin socket, and says so in the journal.

## API

`src/api.rs` lists the paths and the rules every request follows; `src/login.rs` the login checks
in the order of v2 §9-2. Every failed login that depends on the ID gets the same 401 and body,
whatever stopped it (a wrong password, an ID off the list, a limit, a single SSO error), so that
the answer does not tell who is on the allowlist; v2 §9-1 had 503 for the SSO-call limit and 502
for SSO errors. 429 (address limit), 400 (sealing, account key), 409 (a seal's time) and 503 (SSO
calls paused, too many logins at once) do not depend on the ID. While the SSO refuses this server
(its address or its client credentials) or fails five times in a row, SSO calls pause for a few
minutes, every login gets the same 503 before the allowlist is looked at, and the journal hears of
it once.

Every path that takes a token takes it only with the signature of the token's account key
(`X-HJV-Proof`, below): a token alone opens nothing. The one exception is `POST /api/logout`
without the signature, which ends that token and nothing else.

## Formats

The formats the app (client patch 0030), hbbs (server patch 0004) and hjv-api must agree on, as
design v3 §4-1, §4-2 and §5-2 define them. Each is defined in one place in the code; the test
vectors at the end are checked against the code by `tests/keys.rs`, which also checks that this
section shows them. Encodings: base64 is RFC 4648 §4 with padding, base64url is §5 without
padding, NUL is the byte 0x00, times are Unix seconds.

### Login: `POST /api/login` (`src/login.rs`, `src/seal.rs`, `src/keys.rs`, `src/devsig.rs`)

```text
{"username":"<portal ID>","password":"hjv-sealed-v1.<kid>.<sealed>","id":"<device ID>",
 "uuid":"<the app's uuid>","autoLogin":true,"type":"account",
 "deviceInfo":{"os":"<windows|ios|android|…>","type":"client","name":"<host name or model code>"},
 "hjv":{"v":1,"sig":"<base64>","installed":true,"ver":"1.6.0"}}
```

- `<sealed>`: base64url of libsodium's sealed box of the payload to the server's X25519 key
  (`HJV_LOGIN_SEAL_PUBKEY` in the app), `<kid>` the first 8 bytes of SHA-256 of that key in hex.
  The payload: `{"v":1,"pw":"<password>","t":<time>,"n":"<base64 of 16-64 random bytes>",
  "id":"<device ID>","kid":"<kid>","ck":"<base64 of the account key's 32-byte public key>",
  "cp":"<base64 of a 64-byte Ed25519 signature>"}`.
- `cp`: the new account key's signature over `"hjv-ckey-v1" NUL n NUL id NUL ck`, with `n`, `id`
  and `ck` as the payload has them. The app makes a new account key for every password login and
  keeps it only when the answer has a token.
- `hjv.sig`: the device key's signature over `"hjv-login-v1" NUL id NUL t NUL n NUL h NUL installed
  NUL os` (`h` base64 of SHA-256 of the sealed bytes, `installed` "1" or "0", `os` as
  `deviceInfo.os`). `hjv.ver` is the app's version, not signed.
- After the SSO accepted the password: 400 with the failure text without `ck` and `cp`, for a `cp`
  that is not the strict signature, or a key another row has; 403
  `{"error":"로그인한 기기가 너무 많습니다. 다른 기기에서 정리해 주세요.","hjv":"too-many-keys"}`
  past 20 active keys of the account (the keys a login replaces do not count). The key is stored
  with what the server saw: time, `hjv.ver`, platform, the device name of `src/devmodel.rs` (an
  iPhone's model code becomes its name), on or off campus; the device's own name apart. The
  account's older keys of the same device (the tag of `uuid`) are revoked with their tokens.
- 200:

```text
{"type":"access_token","access_token":"hjv_<base64url of 32 bytes>",
 "user":{"name":"<portal ID>","display_name":"<name>","status":1},
 "hjv":{"ks":<the key's serial>,"binding":"<32 hex>","login_at":<time>}}
```

  `ks` is the change serial the key's registration made. `binding` (the PC's binding ID, random
  128 bits, kept while the same account logs in on the PC again) and `login_at` (the server's time
  of this login, also the device row's `login_at`) come only when this login added the device or
  found it already in the account: an installed Windows PC whose `hjv.sig` matches the key hbbs
  holds for its ID.
- A seal whose `t` is more than `HJV_API_SEAL_MAX_SKEW` (600 s) off the server's clock: 409
  `{"error":"기기의 날짜와 시간이 맞지 않습니다. 날짜와 시간을 자동으로 맞추게 한 뒤 다시 해 주세요.","hjv":"clock","server_time":<time>}`.
  Other sealing errors: 400 with the failure text.
- While SSO calls are paused: 503 `{"error":"연세 인증 서버에 지금 연결할 수 없습니다. 잠시 뒤 다시 해 주세요."}`
  for every ID.

### Account requests: `X-HJV-Proof` (`src/proof.rs`, checked in `api::session`)

```text
X-HJV-Proof: v1.<t>.<n>.<sig>
```

- `t`: the device's time (decimal, no sign or leading zero); `n`: base64url of 16 random bytes,
  new for every request; `sig`: base64url of the account key's Ed25519 signature over

```text
"hjv-req-v1" NUL <method> NUL <path and query> NUL <t> NUL <n> NUL base64(SHA-256(body)) NUL base64(SHA-256(token))
```

  with the request target exactly as sent (`/api/peers?current=1&pageSize=100&accessible=&status=1`),
  `t` and `n` as the header has them, the body's hash (of no bytes without a body) and the bearer
  token's hash (`hjv_…`).
- hjv-api: the token, then the signature with the token's key (`verify_strict`), then
  |t − server time| ≤ 600 s, then, unless the method is GET or HEAD, the key's nonce `n` not seen
  in the last 20 minutes. No header, a wrong signature (also with a wrong time) or a nonce again:
  401, the answer to a wrong token. A right signature whose time is out of the window: 409
  `{"error":"기기의 날짜와 시간이 서버와 10분 넘게 다릅니다. 날짜와 시간을 자동으로 맞추게 해 주세요.","hjv":"clock","server_time":<time>}`.
- On every token path: currentUser, logout, users, peers, `/api/hjv/this-device` and its undo,
  `/api/hjv/devices/{id}`, `/api/hjv/keys…`.
- `POST /api/logout`: signed by the token's key (the time may be off: the token ends with this
  request), the token and its key are revoked and, when the token's login added or found the PC in
  the account, the PC leaves the list. Without the signature only the token ends; the key, left
  without a login, is revoked by the next hourly upkeep.

### Account keys: `/api/hjv/keys` (`src/keys.rs`)

```text
GET    /api/hjv/keys                   {"total":n,"data":[{"ckid":12,"name":"iPhone 14 Pro","app_name":"iPhone15,2","platform":"ios","app_ver":"1.6.0","net":"off-campus","created":<time>,"last_login":<time>,"last_used":null,"last_used_pc":null,"use":[],"current":true,"new":false}]}
POST   /api/hjv/keys/{ckid}/ack        {"ack":true}       ("내 기기 맞음" on the new-device notice)
DELETE /api/hjv/keys/{ckid}            {"revoked":true}   (the key and its token; "로그아웃시키기")
POST   /api/hjv/keys/revoke-others     {"revoked":n}      (every key of the account but the asking one)
```

The active keys of the account, newest first, in pages as `/api/peers`. `name` is the server's
device name, `app_name` the device's own; `net` is `on-campus`, `off-campus` or `""`; `new` marks a
key made after the asking one that no device confirmed, for 7 days. A key of another account or
none: 404 `{"error":"not found"}`. At most 30 calls a minute per account (429). `last_used`,
`last_used_pc` and `use` (the PCs' reports, U6) come with the PC sync.

### Device list: `GET /api/peers`

```text
{"total":n,"data":[{"id":"<PC ID>","user_name":"<portal ID>","info":{"device_name":"<host name>","os":"windows"},"status":1,
  "note":"<the card's second line>","hjv":{"pk":"<base64 of the PC's key>","pwless":"<state>","kinds":["remote","file","camera"],"wol":"unknown"}}]}
```

`pk` is the key hbbs held when the PC was bound (the app checks the PC it reaches against it),
`kinds` is `HJV_API_PASSWORDLESS_KINDS`. `pwless` and `note`:

| `pwless` | `note` |
|---|---|
| `active` | 비밀번호 없이 연결 |
| `listed` | 이 PC에서 로그인 필요 |
| `off` | PC에서 꺼 둠 |
| `disabled` | 비밀번호 없는 연결 일시 중지(관리자) |
| `expired` | 마지막 확인 <n>일 전 |
| `needs-relogin` | 이 PC에서 다시 로그인 필요 |

Until the PCs report through the sync (design v3 §3-6), a PC is `listed`, or `disabled` while its
account's switch is off.

### hbbs and the SSO

- hbbs's `hjv-pk <id>` answer, a device key signed with hbbs's key pair: `src/hbbs.rs`.
- The SSO calls and how their outcomes are sorted: `src/sso.rs`.

### Test vectors

Made with libsodium 1.0.18 (`crypto_sign_seed_keypair`, `crypto_sign_detached`), as the app signs.
The account key is the Ed25519 key of the seed `[3; 32]` (32 bytes 0x03); "signed" is the signed
bytes in hex.

```text
account key      7UkoxijRwsbq6QM4kFmVYSlZJzpcY/k2NsFGFKyHN9E=

ck and cp of a login sealed with n = AAECAwQFBgcICQoLDA0ODw== (bytes 00 01 … 0f), id = 987654321
signed (91)      686a762d636b65792d76310041414543417751464267634943516f4c4441304f44773d3d003938373635343332310037556b6f78696a527773627136514d346b466d5659536c5a4a7a7063592f6b324e734647464b79484e39453d
cp               tw4dTyxkVccRMV1aqo4jlinOOftOSRfHy4dAv9ItCeS/3P1eyIEUyYOuyjjyVfzhFUE2TplccLESFxCLtB3ICQ==

X-HJV-Proof with t = 1760000000, n = AAECAwQFBgcICQoLDA0ODw (bytes 00 01 … 0f) and the token
token            hjv_AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8
SHA-256(token)   h6q74v9/1g0Rq8wvBTM5yQIFPZwyCOmhw1Pff+EcZ6g=

GET /api/peers?current=1&pageSize=100&accessible=&status=1, no body
SHA-256(body)    47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=
signed (193)     686a762d7265712d763100474554002f6170692f70656572733f63757272656e743d31267061676553697a653d3130302661636365737369626c653d267374617475733d3100313736303030303030300041414543417751464267634943516f4c4441304f4477003437444551706a38484253612b2f54496d572b354a4365755165526b6d354e4d704a575a473368537546553d00683671373476392f316730527138777642544d3579514946505a7779434f6d6877315066662b45635a36673d
X-HJV-Proof      v1.1760000000.AAECAwQFBgcICQoLDA0ODw.9tsrnUGYVeHhGWQC2oHD1-9Ahw62G854ZjhmxV4CclBAKgpKGR2ouiNwO4v88CKY0W1jqDqrm_jyzdGq_a2YCA

POST /api/logout, body {"id":"987654321","uuid":"dXVpZA=="}
SHA-256(body)    cJ6RcjurhsiVfQGk9qmNH0XPkZ1ylZbMSytqCP6X6do=
signed (151)     686a762d7265712d763100504f5354002f6170692f6c6f676f757400313736303030303030300041414543417751464267634943516f4c4441304f447700634a3652636a7572687369566651476b39716d4e483058506b5a31796c5a624d537974714350365836646f3d00683671373476392f316730527138777642544d3579514946505a7779434f6d6877315066662b45635a36673d
X-HJV-Proof      v1.1760000000.AAECAwQFBgcICQoLDA0ODw.HgexD8dQd4Gq9iVRcGKrG_nqtOsGX5FbjSlBwJRkA4bdnFtbaQ48xnizvYV301E2G4vHaI_vvmoMsq_X2R_DDg
```

These keys are public test values: never use them for anything else.

## Tests

`cargo test` runs unit tests (among them libsodium-made vectors for sealing and signatures) and
integration tests that start hjv-api in-process with the fake SSO (`src/fake_sso.rs`), a fake hbbs
console and the admin socket, and the binary under `systemd-socket-activate` when it is installed.
The test client logs in as the app does, with a new account key each time, and signs its account
requests (`tests/common/mod.rs`); `tests/keys.rs` has the account keys, the switches and the test
vectors above, `tests/http.rs` the request proof.
Their files and sockets go to the system's temporary directory (`/tmp` when `$TMPDIR` is long), so
that socket paths stay short whatever the target directory. Nothing calls the real SSO.
`hjv-fake-sso --listen 127.0.0.1:18600` serves the fake SSO for the E2E with fixed test accounts
(`2026000001`, `2026000002`, `Y9001`; passwords in `src/fake_sso.rs`) and usernames `sso-*` that
play the SSO's failures.

## Data

SQLite with `secure_delete`, the WAL truncated after deletions: the allowlist; accounts (portal ID
as the SSO sends it, name, times, the account's password-less switch); token hashes, each with the
account key of its login; account keys (public key, the tag of the app's uuid, the device ID, the
platform, the server's device name, the device's own name, the app version, on or off campus, the
times of creation, login, confirmation and revocation); the nonces of signed requests for 20
minutes; devices (ID, OS, host name, public key, binding ID, times); the change serial and the
switches; login-limit counters under tags; login, device and key records (tags, client address,
result, time).

Kept: tokens 90 days unused / a year at most (a new login replaces the token of the same device,
and an account keeps at most 20); account keys while their token lasts (an hourly upkeep revokes
the keys no valid token is left for), then 30 days revoked; devices until removed or 180 days
unseen; accounts until a year
unused, unless still on the allowlist with a PC in use (seen within the device time), so that a lab
PC logged in once stays listed while it is on; records six months; limit counters for their longest
window, a day at least. Of failed logins, at most 20 a minute are recorded (the rest are counted in
the daily summary of the journal), so that a flood cannot fill the disk; successes are always
recorded. No password, SSO token, e-mail, uuid or user name of a device is stored, and the journal
gets tags and counts only.

## Later phases

The PC sync and trust lists of design v3 and Wake-on-LAN plug in as modules of their own: routes
in `api::route` (which also lists the planned paths), sessions from `api::session`, device
signatures from `devsig`, hbbs keys from `hbbs`, tables through `db::MIGRATIONS` (the PC sync adds
part B of schema 2, Wake-on-LAN schema 3), records through `AppState::record`, and every change to
what a trust list says through `db::bump_serial`.
