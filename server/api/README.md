# hjv-api

The HJViewer account API: login with a Yonsei portal ID through the university's SSO, and the
per-account list of Windows PCs that the apps show as "accessible devices" (design v2 §9). It runs
next to hbbs/hbbr on the server, behind nginx, as a systemd service that listens only on a Unix
socket systemd holds. GNU AGPL-3.0-only, like the rest of the repository.

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
allow remove <file>       remove them; their tokens stop working at once
allow list                the allowlist
users | devices           accounts and the devices in them
unlock <id|tag:T|ip:A>    clear the login counters of a portal ID (with its PCs' and successes'),
                          of a tag from a journal warning (an account's or an address's), or of a
                          client address (an IPv6 address by its /64)
revoke <id|tag:T>         revoke every token of an account
device remove <id|tag:T>  take a device out of its account
user delete <id|tag:T>    delete an account with its tokens and devices
report                    counts, the login results of the last day and week, the limit counters
selfcheck                 DNS, TCP and TLS to the SSO and hbbs's console, from inside the sandbox
keygen seal|log-key       as hjv-api keygen (runs locally, not in the service)
```

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
whatever stopped it (a wrong password, an ID off the list, a limit, the SSO's trouble), so that
the answer does not tell who is on the allowlist; v2 §9-1 had 503 for the SSO-call limit and 502
for SSO errors. 429 (address limit), 400 (sealing) and 503 (too many logins at once) do not depend
on the ID. While the SSO refuses this server or fails five times in a row, SSO calls pause for a few
minutes and the journal hears of it once.

The formats client patch 0008 and server patch 0004 must follow are defined in one place each:

- the sealed password and its key ID: `src/seal.rs`;
- the device signature on a login: `src/devsig.rs`;
- hbbs's `hjv-pk <id>` answer, a device key signed with hbbs's key pair: `src/hbbs.rs`;
- the SSO calls and how their outcomes are sorted: `src/sso.rs`.

## Tests

`cargo test` runs unit tests (among them libsodium-made vectors for sealing and signatures) and
integration tests that start hjv-api in-process with the fake SSO (`src/fake_sso.rs`), a fake hbbs
console and the admin socket, and the binary under `systemd-socket-activate` when it is installed.
Their files and sockets go to the system's temporary directory (`/tmp` when `$TMPDIR` is long), so
that socket paths stay short whatever the target directory. Nothing calls the real SSO.
`hjv-fake-sso --listen 127.0.0.1:18600` serves the fake SSO for the E2E with fixed test accounts
(`2026000001`, `2026000002`, `Y9001`; passwords in `src/fake_sso.rs`) and usernames `sso-*` that
play the SSO's failures.

## Data

SQLite with `secure_delete`, the WAL truncated after deletions: the allowlist; accounts (portal ID
as the SSO sends it, name, times); token hashes; devices (ID, OS, host name, public key, times);
login-limit counters under tags; login and device records (tags, client address, result, time).

Kept: tokens 90 days unused / a year at most (a new login replaces the token of the same device,
and an account keeps at most 20); devices until removed or 180 days unseen; accounts until a year
unused, unless still on the allowlist with a PC in use (seen within the device time), so that a lab
PC logged in once stays listed while it is on; records six months; limit counters for their longest
window, a day at least. Of failed logins, at most 20 a minute are recorded (the rest are counted in
the daily summary of the journal), so that a flood cannot fill the disk; successes are always
recorded. No password, SSO token, e-mail, uuid or user name of a device is stored, and the journal
gets tags and counts only.

## Later phases

The password-less connection proof (design v3) and Wake-on-LAN (v2 §7) plug in as modules of
their own: routes in `api::route` (which also lists the planned paths), sessions from
`api::session`, device signatures from `devsig`, hbbs keys from `hbbs`, tables through a new entry
in `db::MIGRATIONS`, and records through `AppState::record`.
