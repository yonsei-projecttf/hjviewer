# hjv-api

The HJViewer account API: login with a Yonsei portal ID through the university's SSO, and the
per-account list of Windows PCs that the apps show as "My PCs" (내 PC; design v2 §9). Since
design v3 every login also registers an account key of its device, which signs every account
request after it, and each bound PC's service syncs with hjv-api for its trust list: the account
keys that may open it without its password, signed with hjv-api's list key. Its phase 2 wakes a
sleeping PC of the account (Wake-on-LAN): the PC tells in its sync how to wake it, a relay on the
PC's LAN (hjv-relay on a NAS, relay/) long-polls hjv-api, and the app's wake request becomes a
command signed with hjv-api's command key, which the relay checks before it sends magic packets on
its own network. It also hands out the packages of new app versions (the Windows exe and the
Android APK that the signed version policy names): it checks who asks and sends the file itself,
so that nginx and the web site on the same server never read one (Updates below). It runs next to
hbbs/hbbr on the server, behind nginx, as a systemd service that listens only on a Unix socket
systemd holds. GNU AGPL-3.0-only, like the rest of the repository.

Since 1.8.0 HJViewer is for Yonsei staff (docs/access-tickets.md): hbbs takes a controller's
request or a PC's registration only with an access ticket, which hjv-api signs with its ticket key
for the apps logged in to a staff account and for the PCs bound to one (Access tickets, below), for
the App Review account's demo PCs, for whoever types the access password, one PC at a time (The
access password, below), and since 1.8.2 for a PC whose user types a support code that a logged-in
staff member made (Support codes, below).

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
- Start the service before any `hjv-api admin` command (they talk to the running service), then
  switch nginx over. With `HJV_API_LOGIN=open` (the default) the faculty and staff whom the Yonsei
  SSO accepts can log in from then on (HJViewer's ID rule, below), and no import is needed; with
  `list`, import the allowlist first (`allow import`), or nobody can log in.
- Its log lines go to the journal namespace `hjviewer`: `journalctl --namespace=hjviewer -u
  hjviewer-api`. They hold counts, results and tags (log.rs), never IDs, addresses or secrets.
  `deploy/journald@hjviewer.conf` (installed as `/etc/systemd/journald@hjviewer.conf`) keeps that
  namespace 30 days.
- Watched and backed up by units of their own (Monitoring, Backup and restore below):
  `hjviewer-api-check.timer` every five minutes, `hjviewer-api-backup.timer` every day, and
  `hjviewer-api-alert@.service` when one of them or the service fails. Enable the two timers once
  the service runs: `sudo systemctl enable --now hjviewer-api-check.timer
  hjviewer-api-backup.timer`.
- `deploy/nginx-hjviewer-api.conf` replaces the `/api/` block of
  `deploy/nginx-hjviewer-locations.conf`: the account paths go to the socket, `/api/login-options`
  and `/api/device-group/accessible` get static answers, all other `/api/` paths stay 404, and
  `/api/login` is limited per client address (10 a minute after a burst of 5, 4 at a time). That
  limit is nginx's and comes before hjv-api's: whoever shares the owner's address (a campus NAT,
  public Wi-Fi) can fill it, and then the login of a PC already in the account, which hjv-api
  lets through its own address limit, is refused by nginx as well; the runbook's daily report
  counts those refusals (nginx's error log). Its two zones go to `/etc/nginx/conf.d/hjviewer-api-zones.conf` (the header of the file has the
  lines). Before installing, check that the server and http blocks have no `real_ip_header` or
  `set_real_ip_from` that would let a client choose `$remote_addr`: hjv-api's address limits trust
  the `X-Real-IP` nginx sets from it.
- The packages of new app versions (Updates below) need three things of the operator's, none of
  them in the unit: the signed policy `/etc/hjviewer/version-policy.json` (root:root 0644, the
  file nginx hands to the apps), the package directory `/srv/hjviewer-updates/pkg`
  (root:hjviewer-api 0750, its files 0640: hjv-api reads them through its group, nginx's and the
  web site's user cannot), and, while door 2 is open, `/etc/hjviewer/door2.conf`
  (root:hjviewer-api 0640). hjv-api only reads them (`ProtectSystem=strict`) and reads each again
  when it changes: installing a package, a policy or door 2's file restarts nothing. Until they
  are there, the update paths answer 404 and everything else works. In
  `deploy/nginx-hjviewer-api.conf` three things are for them: `proxy_max_temp_file_size 0` (no
  answer of hjv-api goes to a temporary file of nginx), the locations of `/api/hjv/u/` and
  `/api/hjv/get/` with `error_log /dev/null crit` (nginx's error log names the request of every
  error, and these two have their code in the path), and `access_log off` in the error locations
  (where a request of `/api/` ends while hjv-api is down).
- For tests and the E2E only: `hjv-api serve --listen 127.0.0.1:<port> --env-file <file>
  --credentials <dir> --db <file> --admin-socket <path> --operator-log <file>`.

## Settings

Not secret, from the environment (`EnvironmentFile=/etc/hjviewer/hjviewer-api.env`, or
`--env-file`). `hjviewer-api.env.example` lists every name with its default; only the three SSO
settings are required. Unknown `HJV_API_` names stop the start.

`HJV_API_LOGIN` (the user's decision of 2026-10-14) says who may log in: `open` (the default),
the faculty and staff whom the Yonsei SSO accepts (HJViewer's ID rule takes their portal IDs only:
hbbs and the SSO, below), a portal ID that is not on the allowlist being put on it at its first
login; or `list`, only the IDs on the allowlist (imported, or registered by a login before), as at
first (D3): an ID off it never reaches the SSO and no login adds one, for an incident or a later
decision, without a new build.
In either mode an entry the operator blocked keeps its account out (`admin block`, below).
`selfcheck`, `report` and `allow list` say which mode runs, and the journal's start line too.

Set `HJV_API_HBBS_PUBKEY` to hbbs's public key (the text of its `id_ed25519.pub`, the key the apps
are built with): hbbs signs its `hjv-pk` answers with it, and without it no PC is added to an
account (logins still work; the journal and `selfcheck` say so).

Wrong passwords reach the SSO at most `HJV_API_LIMIT_ID` plus `HJV_API_LIMIT_BOUND` times per
portal ID (the second is one limit for all the PCs of an account). Keep that sum, together with the
WordPress plugin's limit for the same IDs, below the portal's own lockout. `HJV_API_LIMIT_OK_ID`
and `HJV_API_LIMIT_OK_IP` limit successful logins, so that one known password cannot keep calling
the SSO; of the SSO calls of a minute (`HJV_API_LIMIT_SSO`) one account may take a quarter.

With `HJV_API_LOGIN=open` any portal ID of HJViewer's ID rule reaches the SSO, which would make the
login a way to try passwords on every faculty and staff account, or to lock them where the SSO locks
an account after failures; the SSO would see this server's address (the WordPress plugin's too) as
the source. So a portal ID that is not on the allowlist has lower limits of its own: its failed
logins (`HJV_API_LIMIT_NEW_ID`, 2 a day, instead of `HJV_API_LIMIT_ID`, in the same counters), and
the SSO calls of all such IDs together (`HJV_API_LIMIT_SSO_NEW`, 10 an hour, one window, besides
`HJV_API_LIMIT_SSO`): at most 240 such calls a day that log no one in, and the IDs on the list keep
their own. One ID and one client address (an IPv6 address by its /64) may each hold a quarter of the
window, and a first login that goes through gives its call back, so that people logging in for the
first time one after another behind one address are not held. When that window is full, a first
login fails like a wrong password (`limited-sso-new` in the record, its failures not counted), the
journal says `login: SSO call limit of the portal IDs not on the allowlist reached` once a minute,
and the five-minute check alarms (`sso_new`); when an ID or an address holds its share, its first
logins fail the same way and the journal names its tag (`... holds its share ...`), which `admin
unlock` takes. One address cannot fill the window, but five can, with two wrong passwords an hour
each: someone with a few addresses can keep first logins failing, which list mode ends. An ID
imported ahead is on the list and not held by these limits. The user decides the numbers; list mode
does not use them (an ID off the list never reaches the SSO there). This lowers the risk and does
not remove it (docs/architecture.md 5-7, docs/server-runbook.md 7-13).

`HJV_API_CAMPUS_NETS` (the university's networks, such as `192.0.2.0/24,2001:db8::/32`) lets the
account's other devices see whether a new login came from on or off campus; the address itself is
never stored. `HJV_API_PASSWORDLESS_KINDS` is the connection kinds a device of the same account may
open without the PC's password (U3: `remote,file,camera`; `terminal` can be added; port forwarding
and RDP always ask for the password). The password-less switch is not a setting but state in the
database, off when installed (`hjv-api admin passwordless`).

`HJV_API_OPERATORS` is the super operators (최고운영자): portal IDs, comma-separated, compared as
the allowlist compares them; unset or empty, nobody is one, and every other account is a normal
user. They see in the app, read-only, who uses the service and the PCs in accounts with their names
and addresses (Admin overview below). Only the server's env file holds a real ID, never the
repository (the example has the setting empty); a change takes effect at the next restart, and
`hjv-api admin operators` shows which of them have an account. `HJV_API_ACTIVE_DAYS` (30, at most
365) is how recent a use makes an account active in that view; docs/privacy.md section 3 states
the 30 days, so a change of the setting changes that sentence too.

The trust lists (design v3 §3-6, §5-3): `HJV_API_TRUST_PUBKEYS` is the list keys the apps pin, the
value of their build variable `HJV_TRUST_PUBKEYS` (`1:<base64>,2:<base64>`, the key in use and the
next one); `HJV_API_LIST_KID` is the number of the key in the credential `hjviewer-api-list-key`
(1; 2 after a change of keys). hjv-api says in the journal at start, and `selfcheck` fails, when
the key in use is not the pinned key of its number: the PCs would refuse every list it signs.
`HJV_API_TRUST_REFRESH` is how often a bound PC asks (120 seconds; 5 to 300, the bounds the PCs
hold it to; production 60 to 300) and `HJV_API_TRUST_TTL` how long a list lives on a PC that
cannot reach hjv-api (U4: 86400, a day; at most 259200, the apps' bound of 72 hours).

Wake-on-LAN (design v3 §8, relay/PROTOCOL.md §3.2): `HJV_API_COMMAND_PUBKEYS` is the command keys
the relays pin, the value of their build variable `HJV_COMMAND_PUBKEY` (`1:<base64>,2:<base64>`),
and `HJV_API_COMMAND_KID` the number of the key in the credential `hjviewer-api-command-key` (1; 2
after a change of keys). As with the list key, the journal at start and `selfcheck` say so when
the key in use is not the pinned key of its number: the relays would refuse every command. Without
the credential no PC is woken (a wake request gets 503); everything else works.

The packages of new app versions (Updates below): `HJV_API_VERSION_POLICY`, `HJV_API_PACKAGE_DIR`
and `HJV_API_DOOR2_FILE` are where the signed policy, the packages and door 2's file are
(`/etc/hjviewer/version-policy.json`, `/srv/hjviewer-updates/pkg`, `/etc/hjviewer/door2.conf`;
absolute paths). `HJV_API_LIMIT_UPDATE` (10 a day) is the downloads that begin, per account for
its logins and per PC for its device key; `HJV_API_LIMIT_UPDATE_FAIL_IP` (30 a day) the requests
that fail for who asks, per client address, after which such a request gets 429, while one with
the right token, signature or code is never refused for its address. `HJV_API_DOWNLOADS_MAX` (4,
at most 16) is the files sent at the same time, packages and their notice files alike; one more
gets 503 with `Retry-After: 60`.

`HJV_API_REVIEW_FILE` (unset) is the file of the App Review account (below), an absolute path such
as `/etc/hjviewer/review.conf`; unset, there is no such account.

The access tickets (docs/access-tickets.md; Access tickets, below): `HJV_API_TICKET_PUBKEYS` is the
ticket keys hbbs takes, the values of the `KEY=` lines of its `/etc/hjviewer/access.conf`
(`1:<base64>,2:<base64>`), and `HJV_API_TICKET_KID` the number of the key in the credential
`hjviewer-api-ticket-key` (1; 2 after a change of keys). As with the other keys, the journal at
start and `selfcheck` say so when the key in use is not the one of its number there: hbbs would
refuse every ticket it signs, and with hbbs's access check on nobody could connect. The same two
say so for a public test key among `HJV_API_TICKET_PUBKEYS`, one made from a seed of one byte
repeated or of 32 consecutive byte values as the vectors' keys are (`selfcheck` fails): with it in
access.conf anyone could sign the tickets hbbs takes.
`HJV_API_TICKET_HOURS` (24) is how long the ticket of a logged-in app lives, and
`HJV_API_PC_TICKET_HOURS` (72) that of a bound PC, which each of its syncs renews: 1 to 168 hours,
since hbbs refuses a ticket of more than seven days. Without the credential no ticket is signed
(`POST /api/hjv/ticket` answers 503, the answers carry none); everything else works. A version
of hjv-api from before these settings does not start with them: take them out before going back
to one.

The access password (The access password, below): `HJV_API_GUEST_FILE` (unset) is its file, an
absolute path such as `/etc/hjviewer/guest.conf`; unset, nobody gets a ticket with an access
password. `HJV_API_GUEST_TICKET_HOURS` (12, 1 to 168) is how long its ticket lives.
`HJV_API_LIMIT_GUEST_IP` (`5/600,20/86400`) is its failed tries per client address,
`HJV_API_LIMIT_GUEST_ALL` (`60/3600`, one window) the tries of all addresses together, of which
one address holds a quarter, and `HJV_API_LIMIT_GUEST_TICKETS` (`50/86400`) its tickets per client
address; they all come before the password's hash. As with the ticket settings, take them out
before going back to a version of hjv-api from before them. Support codes (below) count against
the same three limits and need neither the file nor its hours; they have no settings of their own.

## Credentials

Secrets come only from systemd credentials (`$CREDENTIALS_DIRECTORY`; `--credentials <dir>` for
tests), never from the environment or the repository:

| credential | content |
|---|---|
| `hjviewer-sso-client-secret` | what follows `Basic ` in the SSO token call (D2: the WordPress plugin's registration) |
| `hjviewer-sso-api-uuid` | the `apiUuid` of the SSO user call |
| `hjviewer-api-login-key` | X25519 secret key(s) that open sealed passwords, base64, one per line, current first |
| `hjviewer-api-log-key` | base64 of 32+ random bytes: the HMAC key of the tags in logs and records |
| `hjviewer-api-list-key` | base64 of the 32-byte Ed25519 seed of the list key in use, number `HJV_API_LIST_KID`, which signs the trust lists |
| `hjviewer-api-command-key` | base64 of the 32-byte Ed25519 seed of the command key in use, number `HJV_API_COMMAND_KID`, which signs the relays' wake commands |
| `hjviewer-api-ticket-key` | base64 of the 32-byte Ed25519 seed of the ticket key in use, number `HJV_API_TICKET_KID`, which signs the access tickets hbbs checks |

The user makes them on the server, without an agent (v2 §10-3; the steps are
docs/server-runbook.md 7-4), for example:

```sh
systemd-ask-password -n "SSO client secret:" | sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-sso-client-secret - /etc/credstore.encrypted/hjviewer-sso-client-secret
hjv-api keygen seal | tee /media/usb/hjviewer-seal.key | sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-login-key - /etc/credstore.encrypted/hjviewer-api-login-key
sudo bash -c 'set -o pipefail; hjv-api keygen log-key | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-log-key - /etc/credstore.encrypted/hjviewer-api-log-key' || echo 'STOP: no log key'
sudo bash -c 'set -o pipefail; hjv-api keygen list --kid 1 | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-list-key - /etc/credstore.encrypted/hjviewer-api-list-key' || echo 'STOP: no list key 1'
hjv-api keygen list --kid 2 --out /media/usb/hjviewer-list-2.key     # offline, away from agents
sudo bash -c 'set -o pipefail; hjv-api keygen command --kid 1 | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-command-key - /etc/credstore.encrypted/hjviewer-api-command-key' || echo 'STOP: no command key 1'
hjv-api keygen command --kid 2 --out /media/usb/hjviewer-command-2.key   # offline, as list key 2
sudo bash -c 'set -o pipefail; hjv-api keygen ticket --kid 1 | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-ticket-key - /etc/credstore.encrypted/hjviewer-api-ticket-key' || echo 'STOP: no ticket key 1'
```

`--tpm2-pcrs=` with an empty value binds a credential to the TPM, where there is one, but to no
PCR. By default systemd-creds binds it to PCR 7 too (the Secure Boot state and its key lists), and
a reboot after a Secure Boot key update or a firmware setting change would leave every credential
sealed: hjv-api would not start, and the keys that live on the server only would be lost. A
cleared or replaced TPM still leaves them sealed; `--with-key=host` in its place keeps the TPM
out (the runbook's 7-4 leaves that choice to the user). `keygen --encrypt-to <file>` runs
`systemd-creds encrypt` with its defaults, hence the pipe. keygen writes the secret to the pipe
first and then shows the public values, so they show even when systemd-creds fails; `pipefail`
makes either failure print STOP: drop what that line showed and run it again (a new key, the file
replaced).

`keygen` never prints a secret to a terminal; `keygen seal` shows the public key for the app
build variable `HJV_LOGIN_SEAL_PUBKEY` and its key ID on stderr. Keep the offline copy of the seal
key: a host-bound credential is lost with the host. To change keys, put the new key line first and
keep the old one until every app build uses the new public key.

`keygen list --kid N` shows the list key's public key as the entry `N:<base64>` of
`HJV_TRUST_PUBKEYS` (the apps) and `HJV_API_TRUST_PUBKEYS` (here), and its fingerprint
`N:<hex SHA-256>`, which the app build's summary and `selfcheck` show too. `--out <file>` writes a
new file (0600, never over an existing one) for a key kept offline: key 1 lives on the server
only, key 2 waits offline until it replaces key 1 (design v3 §5-3: put it in with `systemd-creds
encrypt` from standard input, set `HJV_API_LIST_KID=2`, restart; a PC that got a list of key 2
never takes one of key 1 again).

`keygen command --kid N` makes a command key the same way and shows two lines on stderr: its
public key as exactly the line `N:<base64>`, the one text form wherever a command public key is
written, the entry of the relay build's `HJV_COMMAND_PUBKEY` and of `HJV_API_COMMAND_PUBKEYS`
(relay/PROTOCOL.md §3.2), and `fingerprint: N:<hex SHA-256>`, which `selfcheck`, the relay build's
summary and `hjv-relay version` show too, for the one who made the key to compare. Key 1 lives
on the server only, key 2 offline; the relays are built with both. To change keys: put key 2 in
from standard input (`systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-command-key - …`), set
`HJV_API_COMMAND_KID=2`, restart; commands queued before go out as they were signed (the relays
take both keys), and later relay builds drop key 1.

`keygen ticket --kid N` makes a ticket key the same way and shows the same two lines on stderr:
`N:<base64>`, exactly the value of a `KEY=` line of hbbs's access.conf and an entry of
`HJV_API_TICKET_PUBKEYS`, and `fingerprint: N:<hex SHA-256>`, which `selfcheck` and `admin ticket
status` show too. The unit loads the credential, so the service does not start without it (a test
or E2E directory may go without: no ticket then): on a running server, make it before the new
unit goes in (docs/server-runbook.md 6, the update of path B, step 4, stops without it). No app
pins a ticket key, so a key is changed on the server alone (docs/access-tickets.md 6-4): make key
2 and add its `KEY=2:` line to access.conf and to `HJV_API_TICKET_PUBKEYS`, put it into the
credential with `systemd-creds encrypt` from standard input, set `HJV_API_TICKET_KID=2`, restart,
and take key 1's line out once its last tickets ended (72 hours with the default lifetimes). A key
that leaked goes out of access.conf at once.

## Admin

`sudo hjv-api admin [--reveal] <command>` talks to the running service over
`/run/hjviewer-api-admin/admin.sock`, which root and the service's own user may use. Output is
masked unless `--reveal` is given (human operators only): IDs show their first four characters,
names do not show, accounts and devices carry tags, the same tags as in the journal. Commands take
an ID or `tag:<tag>`. Point with `tag:<tag>` (from `users --reveal`) rather than a portal ID or an
address: sudo logs the whole command line, and the shell keeps it in its history, while the
operator record keeps tags only.

```text
allow import <file>       add the portal IDs in <file> to the allowlist ahead of their first login
                          (counts only); in list mode only the IDs on it may log in. An entry there
                          already keeps its block
allow remove <file>       block the IDs in <file>, as block does (removing is blocking since the
                          open login; an ID without an account has no tag to name it by)
allow list                the allowlist: per entry its tag, the ID, registered or blocked, how it
                          came (login or import), when, and since when blocked; the mode in use
block <id|tag:T>          keep an account out: its tokens and account keys stop working at once,
                          its PCs stay in the account and get a trust list without keys, and its
                          next login, once the SSO accepted the password, gets 403 with the
                          blocked text; the entry stays blocked until unblock. An ID not on the
                          list is put on it blocked. The other IDs the SSO named for the same
                          person are blocked with it (below)
unblock <id|tag:T>        lift a block (with the person's other IDs): the account may log in
                          again (its devices log in again); its entry counts as put on the list now
users | devices           accounts and the devices in them
operators                 the super operators of HJV_API_OPERATORS: the tag of each, its ID,
                          whether it has an account, its allowlist entry (yes, blocked or no), its
                          last login (its name with --reveal)
key list [<id|tag:T>]     account keys: number, account, platform, the server's device name,
                          version, on or off campus, created, state (the device's own name and ID
                          with --reveal), and a line per PC that reported the key's password-less
                          use: the last time and the count of 30 days (U6; host name with --reveal)
key revoke <ckid>         revoke an account key with its token
unlock <id|tag:T|ip:A>    clear the login counters of a portal ID (with its PCs' and successes'),
                          of a tag from a journal warning (an account's or an address's), or of a
                          client address (an IPv6 address by its /64; its access password's
                          and support codes' tries too), and the SSO calls of IDs not on the
                          allowlist and the access password's tries of all that the ID or
                          address holds
user revoke-all <id|tag:T> | --all
                          revoke every token and account key of an account, or of every account
                          (devices log in again; PCs stay in their accounts)
revoke <id|tag:T>         the same as user revoke-all <id|tag:T>
binding list              the PCs in accounts: password-less state, the PC's mark of its binding,
                          last sync, version; the released bindings kept, by how they went
binding revoke <PC ID|tag:T>
                          release a PC from its account: its next sync gets a signed revoked
device remove <PC ID|tag:T>
                          the same as binding revoke
user delete <id|tag:T>    delete an account with its tokens and account keys, its PCs released
                          first (each gets a signed revoked)
passwordless status | on | off [--account <id|tag:T> | --owner-of <PC ID>]
                          connections without the PC's password between an account's devices:
                          for every account (off when installed; off also ends the accounts turned
                          on one by one, so it is the incident switch), or for one account, named
                          by its portal ID or by one of its PCs
strict on | off           PCs ask the server at each password-less connection (incidents)
report                    counts, the login mode, the allowlist (who may log in, how many a login
                          registered in the last day and week, how many are blocked), the login
                          results of the last day and week (a denied person by the reason:
                          denied-user-refused, denied-pcode-rule, denied-bad-value), the limit
                          counters, the account keys, the switches and the change serial; the PCs
                          holding their binding and those synced in the last 10 minutes, those not
                          synced since the last switch change, needs-relogin, PCs by app version,
                          refusals, released bindings by how they went, password-less connections
                          of 30 days; the relays by state and each active relay's last poll, the
                          PCs that reported how to wake them, the wake jobs of the last day and
                          week by result
selfcheck                 DNS, TCP and TLS to the SSO and hbbs's console, from inside the sandbox;
                          the fingerprints of the pinned list keys and command keys and whether the
                          keys in use are the pinned ones, the same for the ticket key and the keys
                          hbbs takes (none of them a public test key); the login mode
ticket status             the access tickets: the ticket key in use with its fingerprint and the
                          KEY= line hbbs's access.conf takes it by, whether it is that key of
                          HJV_API_TICKET_PUBKEYS, the lifetimes, and the tickets handed out and
                          refused since the start by kind (login ctl|both|review, request
                          ctl|both|review, sync both, guest ctl|pc, refused
                          off|account|request|too-often, guest refused …); the access password's
                          file (on until, closed since, off) by the tag of its hash, its tickets'
                          lifetime and the last day's tries by result; the support codes open
                          now and held in memory, and the last day's code records by result;
                          no person, address, device or code
backup [--keep-days N] <file>.sqlite3 | <directory>
                          a checked copy of the database into a directory only you may enter
                          (Backup and restore)
health                    the counts the five-minute check reads, as JSON (not recorded)
relay list                the NAS relays: state, account, name, last poll, version and
                          architecture (only in their plain forms: anyone can make a pending
                          relay say anything; its OS text with --reveal); for an active relay
                          whether it is on the network of each PC of its account (the verdicts;
                          the addresses and networks with --reveal, MACs always masked); pending
                          relays with their codes; the codes approved ahead
relay approve <code> --owner-of <PC ID> [--name <name>] [--force]
                          approve a relay for the account that has the PC (no portal ID typed),
                          after the PC's owner confirmed the code: shows the verdicts of the
                          relay against that PC's wake report and approves when the relay is on
                          the PC's network (by the relay's own report), or with --force; a code
                          no relay has said hello with is approved ahead for 7 days. --name is
                          what the apps call the relay (else "NAS" and the code's first group);
                          given for a relay already active for that account, it renames it
relay revoke <relay ID|code>
                          the relay takes no commands any more; it keeps only its ID, key and
                          times, and its key is never approved again; withdraws an approval ahead
relay share on|off <relay ID>
                          kept for later: in this version a relay wakes its account's PCs only (W2)
keygen seal|log-key|list|command|ticket
                          as hjv-api keygen (runs locally, not in the service)
```

The packages' counters are in the same table as the logins' (Updates below), so `unlock` clears
them too: an account's downloads and links with its login counters (`unlock <id>`), a PC's
downloads by its tag from the journal (`unlock tag:<tag>`), an address's failed requests (`unlock
ip:<address>`). `report` shows them among the limit counters (`update-fail-ip`,
`update-ok-account`, `update-ok-device`, `update-link`) and the packages sent among the records
(`update token-windows`, `update device-windows`, `update link-windows`, `update door2-windows`,
…). A PC that is given away or scrapped leaves its account with `device remove`: until then its
device key gets the PC's package.

Approving a relay (design v3 §8-3, relay/PROTOCOL.md §11): the NAS's owner reports the enrollment
code from the relay's log together with the ID of one of their PCs; before approving, confirm the
request with that person through a channel already trusted (in person, by phone, a known e-mail
address), since PC IDs are not secret. The verdicts say whether one of the relay's interfaces is on
the PC's network (the same network and gateway, or overlapping networks with the same gateway IP or
MAC; on a private network also behind the same egress address), whether the egress addresses are
the same, and how long the entry has waited; the default output holds no address, so that it may
go into a chat or a ticket. A relay that cannot reach the PC's network is the wrong entry. The
verdict on the network rests on what the relay says of itself, a pending one too: its interfaces,
their networks and gateways are its own report (kept only with the prefixes a relay uses, /8 to
/30), compared with the PC's signed report, and only the egress addresses are what hjv-api saw
itself. So a "yes" catches a mistake, a wrong code or another NAS, and it is no proof against a
relay that reports the PC's network on purpose: what decides is the owner's confirmation of the
code, before the approval, with or without `--force`.

The relay's name is what the apps show for it as it is, in the progress of a wake ("… 에 켜기
요청을 전달하는 중…"), so one that reads on its own does best, such as "연구실 NAS"; without
`--name` it is `NAS` and the first group of the code.

The rollout (design v3 §11): install with the switch off, `passwordless on --owner-of <the
operator's PC ID>`, a week of use, then `passwordless on`. To stop password-less connections, run
`passwordless off` and keep hjv-api running for 24 hours or until `report` shows 0 PCs "holding
their binding that have not synced since the last switch change": a PC that cannot reach hjv-api
keeps its trust list for up to a day.

An allowlist file has one portal ID per line; `#` starts a comment, and a byte-order mark,
zero-width characters and Windows line ends do no harm. A line that is anything else refuses the
whole file (nothing changes) and the answer names only its line number. An ID outside HJViewer's
ID rule (hbbs and the SSO, below) can be imported, but its login is refused before the SSO.

Blocking (the open login of 2026-10-14): a login puts every portal ID the SSO accepts on the
allowlist, so the way to keep an account out is a block, not taking its ID off the list. A blocked
login is told so (`이 계정은 관리자가 HJViewer 사용을 막았습니다. …`) only after the SSO accepted its
password, so that the answer tells nobody else who is blocked; its successes count against
`HJV_API_LIMIT_OK_ID`, so a known password cannot keep calling the SSO. An import does not lift a
block (an old file imported again lets nobody back in); only `unblock` does. A block holds for the
person, whichever form of the portal ID the SSO takes (whether it takes more than one is not
known, design v2 §4-1): a login is refused when the entry of the ID typed is blocked, or that of
the ID the SSO names for the person (its pcode), or that of another account the SSO named so.
`block` and `unblock` take with the ID named the other IDs the server knows for the same person
(the pcode of its account, and that person's other accounts), and say how many they took; once
one of those accounts is deleted, the server no longer knows its ID with the others, so its entry
stays blocked until it is unblocked by name. Point with `tag:<tag>` from `users` or `allow list`;
an ID that never logged in has no tag known to the operator, so it is blocked through a file
(`allow remove <file>`). The user notes each block and unblock (date and ID) outside the server:
a restored or rebuilt database does not have the later ones (Backup and restore).

`sudo hjv-api selfcheck --env-file /etc/hjviewer/hjviewer-api.env` runs the same check outside the
service, by hand (it compares the list key in use only where it can read the credentials:
`--credentials <dir>`, or in the unit); the unit's commented `ExecStartPost=` runs it with
`--log-only` after each start. `hjv-api admin selfcheck` checks the key the running service holds.

### The operator record

The service writes two lines per command to `/var/log/hjviewer-api/operator.log`: one before the
command runs (a command whose line cannot be written does not run), with its targets as tags
(an allowlist file as the tags of its IDs), and one after, with what it changed in counts or that it
failed. Each line carries the hash of the one before; `hjv-api oplog-verify` checks the chain.
One kind of line is the service's own, written without a command, under its own user ID and no
sudo user: `cmd="update device-new-address" targets=<PC's tag>,<address's tag>`, when a PC's
device key downloads a package from an address that PC did not use before (Updates below).

To make it append-only: start the service once, so that systemd creates `/var/log/hjviewer-api`
owned by `hjviewer-api` (0700) and hjv-api creates `operator.log` in it (0600); then
`sudo chattr +a /var/log/hjviewer-api/operator.log`. Do not create that directory as root or
`chown`/`chmod` it afterwards: systemd then walks it to fix the owner, fails on the append-only
file, and the unit does not start. (To create it in advance: `sudo install -d -o hjviewer-api -g
hjviewer-api -m 0700 /var/log/hjviewer-api`.) If the record cannot be opened, hjv-api still serves
logins, without the admin socket, and says so in the journal.

### Admin overview

The super operators of `HJV_API_OPERATORS` (Settings) see in the app who uses the service and on
which PCs: `GET /api/hjv/admin/overview` (Formats below, `src/overview.rs`). It shows and changes
nothing else: no connection to anyone's PC, no command; everyone else's app gets the 404 of an
unknown path. A user is an account that has logged in and is on the allowlist and not blocked,
active when one of its account keys or PCs was used within `HJV_API_ACTIVE_DAYS`; the PCs are those
bound to an account, by a login on them: a PC on which nobody logged in is unknown to hjv-api and
not in the list (the app's help line says so). The privacy policy (docs/privacy.md, sections 3 and 9)
tells the users what the super operators see and that each look is recorded.

Each look writes one line to the operator record before it is answered, under the service's own
user ID and no sudo user, with the account's tag and never its portal ID: `cmd="app overview"
acct=<tag> done="ok: 3 users (2 active), 2 PCs"`; a look whose line cannot be written is not shown
(503). The journal gets the same tag and counts. `hjv-api admin operators` shows the tag of each
super operator, the one in those lines. To make someone a super operator, or to stop it, the user
edits the line in `/etc/hjviewer/hjviewer-api.env` on the server and restarts the service
(docs/server-runbook.md 7-15).

## The App Review account

Apple's App Review asked for a demo account to sign in to the iOS app with. The user's decision
(2026-10-16): its ID and password live only in a file on the server, the password as a hash; the
account can only log in, and it ends (`src/review.rs`; docs/server-runbook.md 7-17,
docs/distribution.md).

The file, `HJV_API_REVIEW_FILE` (root:hjviewer-api 0640, as door 2's;
`deploy/review.conf.example`), has each of three lines once, a fourth at most once, and `#`
comments:

```text
ID=<the login ID>
HASH=pbkdf2-sha256$<iterations>$<salt, standard base64>$<derived key, standard base64>
UNTIL=<UTC time, 2026-11-29T15:00:00Z>
PC=<the demo PC's ID>[,<another>…]
```

`PC=` names the demo PCs, 1 to 3 different PC IDs separated by commas: the only PCs the account's
access ticket reaches (Access tickets, below). It is the one line the file may leave out; without
it the account gets no ticket, and once hbbs checks tickets it connects to nothing. An hjv-api from
before the access tickets refuses the whole file with the line: put the new hjv-api in first.

HASH is PBKDF2-HMAC-SHA256 of the password, 100 000 to 10 000 000 iterations (bounded, so that a
file cannot stall logins), a salt of 16 bytes or more and a key of 32, compared in constant time
(ring's). An ID that passes HJViewer's ID rule (hbbs and the SSO, below) refuses the whole file, so
that a staff account is never shadowed; anything else that is not as above refuses it too. hjv-api
reads the file again when it changes, as door 2's: a file missing, unreadable, refused or past its
UNTIL means there is no such account. The journal says what it read, by the account's tag:
`login: the App Review file (HJV_API_REVIEW_FILE) read: until <UTC>, <n> demo PC(s) (acct=<tag>)`,
`… is not
used: <why>` (never the ID or the hash), `… is gone`, and at the start `login: no App Review
account (…)` when there is none.

The user writes the file in a terminal of their own (Linux, macOS or WSL), not in an agent's
session, and outside the repository:

```sh
python3 -I deploy/review-conf.py --until <YYYY-MM-DD> --out ~/review.conf [--pc <demo PC ID>]...
```

It asks for the ID, and for the password twice without showing it, on the terminal only (never
from the command line, which a shell's history and the process list keep, the environment or a
standard input that is not a terminal; the password 12 to 256 printable ASCII characters), refuses
an ID of the ID rule (it takes letters, digits, `.`, `_` and `-`) and a date more than 90 days
ahead, and writes the file (0600, never over another) with 600 000 iterations, 16 random bytes of
salt and UNTIL at 0:00 KST at the start of the date, in UTC, and with each `--pc` (1 to 3
different PC IDs, which are no secret) the `PC=` line; it prints the file's path, UNTIL and the
`PC=` line only. `scripts/test_review_conf.py` tests it (`scripts/ci.sh deploy`).

While the file is active, its ID logs in (compared as the allowlist compares IDs) with every limit
and answer of any ID and nothing of the SSO's: no SSO call, no pause, no second factor. The success
limits come first, as before an SSO call; then the hash. A wrong password is the 401 of a wrong
password the SSO refused, after the same delay, counted for the ID (`admin unlock tag:<tag>` lifts
it), with the record `review-credentials` and the journal line `login: a wrong password for the App
Review account (acct=<tag>)`. The operator puts the ID on the allowlist ahead with `allow import`
in either mode (docs/server-runbook.md 7-17): in open mode an ID not on it counts under
`HJV_API_LIMIT_NEW_ID` (two failures a day) until its first login, and in list mode it logs in only
then. The right password then goes as any login, a block keeps it out; its user object is
`{"name":"<the ID, case-folded>","display_name":"App Review","status":1}`, and the journal says
`login: the App Review account logged in, without the SSO (acct=<tag>)`. It never goes into an
account it did not make: when the account of its ID has another name or pcode, or a PC (an account
of before the rule), the right password fails as a wrong one, with the record `review-existing` and
the critical journal line `login: the App Review file names the ID of an account that is not the
App Review account's (acct=<tag>); …`, and nothing of that account changes.

The account is told apart with no state of its own: its key fails the ID rule, which every other
account's passes. It never holds a PC (a login on an installed Windows PC is `not_eligible`), door 1
and the short links answer it `403 {"error":"이 계정으로는 새 버전을 받을 수 없습니다."}`, it is never a
super operator (whatever `HJV_API_OPERATORS` says), and its tokens hold only while the file is
active and names it: otherwise every request of it gets the 401 of an unknown token, and the app
logs out. Its own account (its keys, an empty device list, logout) is all it reaches. Its access
ticket (Access tickets, below) reaches the demo PCs of the file's `PC=` line only, `review`/`ctl`
until the earlier of `HJV_API_TICKET_HOURS` and UNTIL; without the line it gets none
(`POST /api/hjv/ticket` answers 403 `no_ticket`, its login's answer has no `access`). An account
whose ID is outside the rule from before the rule (2026-10-14) is held the same way; its logins were
refused already. A letter added to the rule must not begin the ID of an App Review account still
there: its key would pass the rule, and its tokens would hold without the file (delete it first).

A new HASH in the file ends no session of the old password: `hjv-api admin user revoke-all
tag:<tag>` does. To end the account: delete the file (its tokens stop at once), then `hjv-api admin
user delete tag:<tag>`, with the tag of the journal's line. hjv-api from before this account does
not know `HJV_API_REVIEW_FILE` and does not start with it, and would let the account's tokens hold
as any: before going back to it, end the account and remove the setting (docs/server-runbook.md
7-17).

## The access password

The user's decisions (2026-10-16, docs/access-tickets.md 1): someone who is not logged in uses
HJViewer only with the access password (이용 비밀번호), one password the operator sets in a file on
the server, as a hash; the app keeps neither the password nor what it brings. A controller types
it for each connection, a PC each time its app starts. Each time it is traded for one access ticket
for one PC (`src/guest.rs`; Formats, below): a controller's for the PC it connects to, a PC's for
itself, each for `HJV_API_GUEST_TICKET_HOURS`. App Review gets no access password (the App Review
account, above).

The file, `HJV_API_GUEST_FILE` (root:hjviewer-api 0640, as the App Review account's;
`deploy/guest.conf.example`), has each of two lines once, and `#` comments:

```text
HASH=pbkdf2-sha256$<iterations>$<salt, standard base64>$<derived key, standard base64>
UNTIL=<UTC time, 2026-11-29T15:00:00Z>
```

HASH is as the App Review account's. hjv-api reads the file again when it changes; a file missing,
unreadable, refused or past its UNTIL means that no access password is taken, and a try then gets
the very answer of a wrong password. The journal names the file by a tag of its HASH (the first 4
bytes of its SHA-256), never the hash: `guest: the access password's file (HJV_API_GUEST_FILE)
read: until <UTC> (hash <tag>)`, `… is not used: <why>`, `… is gone`, and at the start `guest: no
access password (…)` when there is none.

The user writes the file in a terminal of their own, outside the repository, as the App Review
account's:

```sh
python3 -I deploy/guest-conf.py --until <YYYY-MM-DD> --out ~/guest.conf            # asks for it
python3 -I deploy/guest-conf.py --until <YYYY-MM-DD> --out ~/guest.conf --random   # makes one
```

It asks for the password twice without showing it (12 to 256 printable ASCII characters, not
starting or ending with a space, which the app drops from what is typed), or with `--random` makes
one of four groups of five letters and digits that cannot be mistaken for one another and shows it
once, on a terminal only; it refuses a date more than 90 days ahead and writes the file (0600,
never over another) with 600 000 iterations. `scripts/test_guest_conf.py` tests it
(`scripts/ci.sh deploy`). A new file takes the new password from then on; the tickets handed out
before end within their hours, or at once with `NOT_BEFORE` in hbbs's access.conf
(docs/access-tickets.md 6-4). Its value goes into the development PC's `build/leak-patterns.txt`,
never into the repository, a chat, a document or an App Store or Play note.

The limits come before the hash, which runs on a blocking thread, two at a time (hjv-api has one
CPU and shares its blocking threads with every database call): the failed tries per client address
(`HJV_API_LIMIT_GUEST_IP`, an IPv6 address by its /64), the address's tickets
(`HJV_API_LIMIT_GUEST_TICKETS`) and the tries of all addresses together (`HJV_API_LIMIT_GUEST_ALL`,
in memory, one address holding a quarter); nginx limits the path per address before hjv-api, as it
does logins (`hjv_guest` in deploy/nginx-hjviewer-api-zones.conf). `admin unlock tag:<the
address's tag>` clears an address's counters and its tries of all. Every refusal leaves after the
login's failure delay. Each try leaves a record `guest` with its result (`ok-ctl`, `ok-pc`, `wrong`,
`closed`, `seal-…`, `pc-signature`, `limited-ip`, `limited-tickets`, `limited-all`, `busy`,
`wrong-pc`, `hbbs`, `error`) and the address's tag, never the address, an ID, the password or the
ticket, kept `HJV_API_EVENT_DAYS`; failures at most 20 a minute. `admin ticket status` shows the
file (on until, closed since, off) with its tag, and the last day's tries by result. Since 1.8.2 a
PC may bring a support code in the place of the password, through the same path, limits and
failure delay; such a try leaves a record `code`, not `guest` (Support codes, below).

## Support codes

The decision for 1.8.2 (2026-10-19, docs/access-tickets.md 3-7): instead of handing out the access
password, a staff member who is logged in makes a support code (지원 코드) in the app and gives it to
someone who is not. That person types it on their PC under the notice that the server refuses the
PC (P1), and the PC trades it, once and within ten minutes, for a `guest`/`pc` ticket of its own
that lives two hours (`src/support.rs`, `src/guest.rs`; Formats, below). The ticket is the access
password's PC ticket but for its times, so hbbs, the ticket's form and the shared vectors do not
change, and the staff member connects with their own ticket and the PC's password or acceptance,
as before. No nginx change: the new path is under `/api/hjv/`, and the trade goes through
`/api/hjv/guest` with its limits.

- Who makes one: `POST /api/hjv/support-code` takes the token with its account key's proof, as
  every account path, so only a device of an account on the allowlist and not blocked; the App
  Review account gets 403 `no_code`, and an app without a login has no token. A call counts with
  the account key's other calls (30 a minute).
- The code: eight symbols of Crockford's base32 from 40 bits of the operating system's random
  source, shown `XXXX-XXXX` and read as a short link's code is (Updates, below: without regard to
  case, `-` and spaces, `O` as `0`, `I` and `L` as `1`), but for the `%` escapes of a link's path,
  which a typed code never has (`support::read`). It works once, within ten minutes, for the
  PC that trades it first, and ends earlier when its account is blocked, deleted or off the
  allowlist (looked at when it is traded). Nothing takes a code back: closing the app's window
  leaves it working until its ten minutes are over.
- Kept in memory only, never in the database, a file, the journal or a record: under an HMAC-SHA256
  of the code with a key made at the start and held in memory only, with the account that made it,
  its use and its times. A code that was used or whose ten minutes are over stays ten minutes more,
  marked so, for the records to tell `used` and `expired` from `wrong`; the answer is the same. A
  restart ends them all; the tickets traded stay until their `exp`. The answer that carries a new
  code is held in memory as every signed request's that changes something, for the retry of that
  very request within `HJV_API_RETRY_WINDOW`.
- Limits: an account holds at most 5 open codes (made, neither used nor over), past which it gets
  429 `too_many_codes` with `retry_after`; hjv-api holds 1000 in all, the ended ones going first,
  past which 503 `busy`. Trading one counts as a try of the access password does, in the same
  counters: the address's failures (`HJV_API_LIMIT_GUEST_IP`), its tickets
  (`HJV_API_LIMIT_GUEST_TICKETS`) and the tries of all addresses (`HJV_API_LIMIT_GUEST_ALL`), behind
  nginx's limit of `/api/hjv/guest`; `admin unlock tag:<the address's tag>` clears them for both.
  With all addresses' tries at 60 an hour and 1000 open codes at most, a day of guessing finds one
  with a chance of about 1.3 in a million.
- The access password's file plays no part: codes work without `HJV_API_GUEST_FILE`, and with its
  file missing, refused or past its UNTIL. Without a ticket key both requests answer 503
  `unavailable`. hjv-api does not look at hbbs's mode: with `MODE=off` or `count` the other PC
  registers without a ticket and shows no P1, so a code is not needed there, and the app says so
  in the code's window (docs/access-tickets.md 3-7).
- Records `code`, kept `HJV_API_EVENT_DAYS` as every record: `made` for each code made, with the
  account's tag; for each try to trade one, its result (`ok-pc`, `wrong`, `expired`, `used`,
  `blocked`, `wrong-pc`, `hbbs`, and the access password's `seal-…`, `pc-signature`, `limited-ip`,
  `limited-tickets`, `limited-all`, `busy`, `error`), the address's tag and, once the code was
  found, the tag of the account that made it. Never the code, a tag of it, the address, an ID or
  the ticket; failures at most 20 a minute, together with the access password's. The journal gets
  the limits and the troubles as for the access password, and an account that holds its 5 open
  codes by its tag once in ten minutes; no line holds a code or a tag of one. `admin ticket status`
  shows the codes open now and held, and the last day's `code` records by result; its counts since
  the start have the tickets traded (`code pc`) and the refusals (`code refused <result>`) with the
  others.
- The numbers (the ten minutes, the two hours, 5, 1000, the ten minutes more) are constants of
  `src/support.rs`, not settings. The two hours are the user's to change: two texts of the app say
  them (docs/access-tickets.md 5-3, S2 and P3). So is the controller's side, which 1.8.2 refuses: a
  `code` with `use` `ctl` gets the 400 of a body not of the form, and the `use` of both requests is
  where it would go.

## Monitoring

While hjv-api is down, or only the PCs' syncs fail, password-less connections go on for a day on
the lists the PCs hold, and nobody notices until every PC falls back to its password at once
(design v3 §5-9). So three units watch it (`deploy/systemd/`):

- `hjviewer-api-check.timer` runs `hjv-api check` every five minutes (`src/check.rs`), as root
  with one capability and no network. It looks at `GET /api/hjv/source` on
  `/run/hjviewer-api.sock`, and at the service's `health` on its admin socket: of the PCs holding
  their binding, the ones that are on (hbbs saw them register within 90 seconds, under the key
  stored for them, at each check of the last four minutes, so that a PC that has just woken has
  time for its first sync; a PC set up again under a new key has its syncs refused until a login
  on it, needs-relogin in `report`, and is left out) and whether any of them synced in the last
  ten minutes; whether hbbs's console answers; SSO calls paused
  now or in the last ten minutes, with `invalid_client` (the credential
  `hjviewer-sso-client-secret` must be renewed, as when the WordPress plugin's credential
  changes) and a refused address told apart; with `HJV_API_LOGIN=open`, the SSO calls of the
  portal IDs not on the allowlist found full (`HJV_API_LIMIT_SSO_NEW`) in the last ten minutes
  (`sso_new`: someone may be trying passwords on many portal IDs, or many people log in for the
  first time); the SSO's user call refusing three different accounts in a row, the last in the
  last ten minutes (`sso_user`: when it refuses everyone, `HJV_API_SSO_CLIENT_ID` or the
  credential `hjviewer-sso-api-uuid` is wrong; hbbs and the SSO, below); and, with phase 2, active
  NAS relays silent for an hour. PCs that are off say nothing, so a night with every PC asleep is
  no alarm. All well: one line in the journal. A finding: an error line each, the findings in
  `/run/hjviewer-api-check/findings`, and the unit fails.
- `hjviewer-api-alert@.service` is the `OnFailure=` of `hjviewer-api.service` (each crash, also
  while systemd restarts it), of the check and of the backup: `hjv-api alert <unit>`
  (`src/alert.rs`) writes an error line to the journal. That line is the alert while no Telegram
  bot is set up; the operations agent's daily report counts them:
  `sudo journalctl --namespace=hjviewer -p warning --since yesterday -o cat | wc -l`.
- With a Telegram bot (design v3 question 12), the alert also goes to its chat: the same unit,
  ended the same way (systemd's `MONITOR_SERVICE_RESULT`, `MONITOR_EXIT_CODE`,
  `MONITOR_EXIT_STATUS`), with the same findings once in six hours, anything else at once: a crash
  and a start that fails are two alerts. The message holds the unit, how it ended, the host name,
  the time and the findings (counts and states, no person, device, address or secret, in Korean
  with the check's English line). The user puts the bot in, on the server, without an agent:

  ```sh
  { printf 'TELEGRAM_BOT_TOKEN='; systemd-ask-password -n "bot token:"; printf '\nTELEGRAM_CHAT_ID=%s\n' '<chat number>'; } |
    sudo systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-telegram - /etc/credstore.encrypted/hjviewer-api-telegram
  sudo systemctl start hjviewer-api-alert@test.service   # a test message
  ```

  The alert unit imports the credential when it is there (`ImportCredential=`, systemd 254 and
  later; nothing happens without it). Its address carries the token, so hjv-api prints neither,
  and clears the token from every error. To stop sending, delete the file.
- During planned work that stops hjv-api (a restore), stop `hjviewer-api-check.timer` first and
  start it after.

## Backup and restore

`sudo hjv-api admin backup [--keep-days N] <file>.sqlite3 | <directory>` (`src/backup.rs`): the
running service takes a consistent copy of its database (`VACUUM INTO`, one read transaction
while requests go on), makes it a standalone file (rollback journal, application ID `HJVA`),
checks it (`PRAGMA quick_check`), counts its rows and sends it over the admin socket; the command
line writes it 0600 under a temporary name, checks size, SHA-256, schema and every table's rows
against the service's counts, and only then gives it its name, which must not exist yet. The
directory must be closed to everyone but the one who runs the command (root's
`/var/lib/hjviewer-api/backup`, 0700); a missing one is made (0700) only as that directory or in a
directory of the caller's closed to others, and removed again when the backup fails; anywhere
else make it first (`install -d -m 700 <dir>`). A directory as the argument gets a name of the
time. `--keep-days N` then deletes the backups in that directory older than N days (SQLite files
with that application ID named `*.sqlite3`, nothing else) and the command's own temporary files
(`.<name>.sqlite3.<12 hex>.partial`) of runs that broke off, a day old. The output and the
operator record hold counts only.

- **Daily**: `hjviewer-api-backup.timer` (03:30 and up to 20 minutes later, and after a boot that
  missed it) runs `hjv-api admin backup --keep-days 14 /var/lib/hjviewer-api/backup`: 14 days of
  backups, upgrade snapshots included, inside the server only (design v3 §5-9; a copy off the
  server is question 8). The unit creates the directory, root's, inside the service's state
  directory, where the service's user cannot read it. The credentials,
  `/var/lib/systemd/credential.secret` and the operator record are not in a backup; the keys come
  back from their offline copies (design v3 §5-3).
- **Before an upgrade**: schema upgrades are one way (an older hjv-api refuses a newer schema), so
  take a snapshot first, `sudo hjv-api admin backup
  /var/lib/hjviewer-api/backup/before-<VERSION>.sqlite3`, and keep the env file and the old binary
  with it. To go back: the old binary, the snapshot (as below, a restore: it starts with
  password-less connections off) and the env file together.
- **Restore** brings back the tokens and account keys revoked since, the switches as they were and
  the bindings released since, so it is an incident of its own: `passwordless off`, stop the
  check timer, keep the current database as a backup in the backup directory (`admin backup
  /var/lib/hjviewer-api/backup/before-restore-<time>.sqlite3`, so that it ages out with the rest;
  only a database too damaged for that is moved aside instead, into a directory of root's, and
  deleted within 14 days), stop the socket and the service, remove the database files, put the
  backup in their place
  (`install -o hjviewer-api -g hjviewer-api -m 600 <backup> /var/lib/hjviewer-api/hjv-api.sqlite3`,
  no -wal or -shm left), start the socket and the service, `passwordless off` again,
  `user revoke-all --all`, look at the blocks (`allow list`) and the switches (`passwordless
  status`), then, with the user's approval, `passwordless on` and the check timer. A restored
  backup starts with password-less connections off for every account: hjv-api turns them off when
  it first opens a file with the backups' application ID, before it answers anything, and says so
  in the journal, since the PCs sync every two minutes and would otherwise get, between the start
  and the next command, signed lists with the backup's switch and the keys revoked since. The
  second `passwordless off` confirms it (`(no change)`). Everybody logs in again on their phones
  and PCs; the PCs stay bound, so a PC whose mark was not cleared gets its list at once, and a PC
  that cleared it meanwhile is `listed` until someone logs in on it. The restored database also
  has back the accounts deleted, the blocks as they were (an account blocked since is not, one
  unblocked since is) and the bindings released since the backup: before `passwordless on`, find
  those commands (`user delete`, `block`, `allow remove`, `unblock`, `binding revoke`, `device
  remove`) after the backup's time in the operator record and run them again with the tags it
  names (docs/server-runbook.md 7-14). A block whose tag the restored database does not have (`no
  account or allowlist entry has that tag`: an ID registered by a login after the backup, or one
  blocked through a file without an account) is made again with `allow remove <file>`, a file of
  the IDs from the operator's own notes, as is every block after a rebuild; in open mode such a
  person would otherwise log in again. A binding released in the app is in the database only, so
  the users are asked to remove again a PC that shows up in their list. The IDs a login put on the
  allowlist since the backup are put on it again at their next login (in open mode). `tests/ops.rs`
  (`the_restore_procedure`) runs these steps on the database; `docs/hjv-api-drill.md` records the
  rehearsal with systemd.
- **Host rebuilt** (the disk lost): the seal key comes from its offline copy, list key 1 and
  command key 1 were on the server only and are replaced by the offline keys 2
  (`HJV_API_LIST_KID=2`, `HJV_API_COMMAND_KID=2`; the apps and relays pin key 2 already), the log
  key is new (old records' tags no longer match). Without a backup the database starts empty:
  everyone logs in again, the PCs too (in list mode once the allowlist is imported again), the
  blocks are made again from the operator's own notes, as an ID file for `allow remove` (the old
  record's tags no longer match), and the relays say hello and are approved again.
- A restore also brings back the relays as they were: one revoked since the backup is active
  again, and only the operator record tells which. When the restored backup has active relays,
  the journal says so at the start; after a restore, `relay list`, and `relay revoke` again what
  was revoked since. (Such a relay gets wake commands of its account's PCs only, when their owner
  asks for one; it reaches no PC.)

## API

`src/api.rs` lists the paths and the rules every request follows; `src/login.rs` the login checks
in the order of v2 §9-2. Every failed login that depends on the ID gets the same 401 and body,
whatever stopped it (a wrong password, an ID off the list in list mode, a limit, a single SSO
error), so that the answer does not tell who is on the allowlist; v2 §9-1 had 503 for the SSO-call
limit and 502 for SSO errors. The exceptions are 403s with texts of their own (below): a portal ID
outside HJViewer's ID rule, which the rule makes public, and, once the SSO accepted the password, a
person HJViewer denies and a blocked account, which only someone who knows the password sees. 429
(address limit), 400 (sealing, account key), 409 (a seal's time) and 503 (SSO calls paused, too
many logins at once) do not depend on the ID. While the SSO refuses this server (its address or
its client credentials) or fails five times in a row, SSO calls pause for a few minutes, every
login gets the same 503 before the allowlist is looked at, and the journal hears of it once.

Every path that takes a token takes it only with the signature of the token's account key
(`X-HJV-Proof`, below): a token alone opens nothing. The one exception is `POST /api/logout`
without the signature, which ends that token and its key, and nothing else. `POST
/api/hjv/pc/sync` takes no token: a bound PC signs it with its device key (below).

hjv-relay's requests (`POST /api/hjv/relay/hello`, `/api/hjv/relay/poll`) take no token either:
the relay signs them with its own key, in the PC sync's envelope (relay/PROTOCOL.md, below). The
app's wake paths (`POST /api/hjv/devices/{id}/wake`, `GET /api/hjv/wake/{job}`, `GET
/api/hjv/devices/{id}/wol`) take the token with its signature as every account path does.

The packages of new app versions (`src/update.rs`, Formats below) are the one thing hjv-api
answers that is not JSON. `GET, HEAD /api/hjv/update/{windows,android}/{x.y.z}` (door 1) takes a
login, the token with its signature as every account path, or without a login the device key of a
PC in an account, for that PC's own platform; `POST /api/hjv/update/{platform}/link` takes a login
and answers a short link; `GET, HEAD /api/hjv/u/{code}` (the short link) and `GET, HEAD
/api/hjv/get/{code}/…` (door 2) take the code in their path, are what a browser opens, and so are
the only paths that take a request with an `Origin` header.

A PC leaves its account in one place only, `db::release_binding`: its row goes, its binding ID
stays 180 days as a tombstone, and the change serial goes up, in one transaction, so that the
PC's next sync gets a signed `revoked` (design v3 §4-3). The signed logout from the PC, undo,
`DELETE /api/hjv/devices/{id}`, `binding revoke` (`device remove`), `user delete` and the upkeep's
deletion of unused accounts (their PCs first), and the upkeep's release of PCs unseen for 180 days
all go through it; `tests/pcsync.rs` checks that no other SQL deletes a binding.

`POST /api/hjv/ticket` (Access tickets, below) takes the token with its signature as every account
path; the login's answer and a bound PC's sync carry the same ticket object. `POST /api/hjv/guest`
(the access password, below) takes no token: as a login, its password comes sealed, with the same
form checks, and every refusal leaves after the failure delay. `GET /api/hjv/access` (the access
check's mode, below) takes none either: it says no more than hbbs's refusals show anyone, whether
hbbs checks tickets. `POST /api/hjv/support-code` (Support codes, below) takes the token with its
signature as every account path, and the code it makes comes to `POST /api/hjv/guest` sealed in
the place of the password.

`GET /api/hjv/admin/overview` (Admin overview) takes the token with its signature too, with the
same 401 and 409 as every account path. For an account that is not a super operator the answer is
then the 404 of an unknown path, `{"error":"not found"}`, for every method and however often, so
that a normal user's app cannot tell the path from one that does not exist. A super operator gets
405 for another method than GET, 429 past 30 calls a minute of the account key, counted with its
key list's (the same limit), and 503 when the operator record does not take the look's line.

## Formats

The formats the app (client patches 0030-0031), hbbs (server patch 0004) and hjv-api must agree
on, as design v3 §3-6, §4-1, §4-2 and §5-2 define them. Each is defined in one place in the code; the test
vectors at the end are checked against the code by `tests/keys.rs`, which also checks that this
section shows them (`tests/tickets.rs` for the access tickets'). Encodings: base64 is RFC 4648 §4
with padding, base64url is §5 without padding, NUL is the byte 0x00, times are Unix seconds.

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
  that is not the strict signature, or a key another row has. The key is stored with the time and
  on or off campus, which the server saw, and with what the device said of itself, which nothing
  checks: `hjv.ver`, platform, the device name of `src/devmodel.rs` (an iPhone's model code becomes
  its name) and the device's own name. The account's older keys of the same device (the tag of
  `uuid`) are revoked with their tokens, and so is the key of the token a login from the same
  device ID replaces (`replaced`). Past 20 active keys of the account (the keys a login replaces
  do not count) the least recently used one is revoked (`evicted`): whoever knows the portal
  password now can always log in and log the other devices out.
- 200:

```text
{"type":"access_token","access_token":"hjv_<base64url of 32 bytes>",
 "user":{"name":"<portal ID>","display_name":"<name>","status":1,"hjv_role":"operator"},
 "hjv":{"ks":<the key's serial>,"binding":"<32 hex>","login_at":<time>,
        "access":{"ticket":"t1.…","use":"both","exp":<time>,"server_time":<time>}}}
```

  `name` is the person's ID as the SSO names it (its pcode, uppercased: hbbs and the SSO, below).
  `hjv_role` is there only for a super operator's account (Admin overview), as in the user object
  that `POST /api/currentUser` and `GET /api/users` answer; no other account has the member.
  `ks` is the change serial the key's registration made. `binding` (the PC's binding ID, random
  128 bits, kept while the same account logs in on the PC again) and `login_at` (the server's time
  of this login, also the device row's `login_at`) come only when this login added the device or
  found it already in the account: an installed Windows PC whose `hjv.sig` matches the key hbbs
  holds for its ID. `access` is the app's access ticket (Access tickets, below) while hjv-api has
  a ticket key: `both` for such a PC (in this account or another), `ctl` for any other device;
  the App Review account's answer has none yet. The login carries no device public key, so that
  this check is the only one a login can make: a Windows app with `ctl` here (one that runs
  without installing, or a PC hbbs does not know yet) asks `POST /api/hjv/ticket` with its PC
  part right after.
- A seal whose `t` is more than `HJV_API_SEAL_MAX_SKEW` (600 s) off the server's clock: 409
  `{"error":"기기의 날짜와 시간이 맞지 않습니다. 날짜와 시간을 자동으로 맞추게 한 뒤 다시 해 주세요.","hjv":"clock","server_time":<time>}`.
  Other sealing errors: 400 with the failure text.
- A portal ID outside HJViewer's ID rule (hbbs and the SSO, below), at once, before any SSO call
  and counted against the address only: 403
  `{"error":"연세대학교 교직원의 연세포탈 ID로만 로그인할 수 있습니다. 입력한 ID를 확인해 주세요."}`.
  The one exception is the App Review account's ID while its file names it (The App Review
  account, above).
- While SSO calls are paused: 503 `{"error":"연세 인증 서버에 지금 연결할 수 없습니다. 잠시 뒤 다시 해 주세요."}`
  for every ID.
- A person HJViewer denies once the SSO accepted the password (the SSO's user call refuses, names
  a pcode outside the ID rule, or a pcode or name whose value the contract refuses: hbbs and the
  SSO, below), before anything is written for it, counted as a success and not as a wrong
  password: 403
  `{"error":"이 계정으로는 HJViewer를 쓸 수 없습니다. 연세대학교 교직원만 쓸 수 있으며, 교직원인데도 이 문구가 나오면 관리자에게 문의하세요."}`.
  The answer is the same for each reason; the record keeps it (`denied-user-refused`,
  `denied-pcode-rule`, `denied-bad-value`, which `report` counts apart), and the journal says it
  once in ten minutes per account.
- A blocked account (`admin block`), once the SSO accepted its password, before anything is
  written for it: 403
  `{"error":"이 계정은 관리자가 HJViewer 사용을 막았습니다. 다시 쓰려면 관리자에게 문의하세요.","hjv":"blocked"}`.
  With `HJV_API_LOGIN=open` a portal ID that is not on the allowlist is put on it by the login
  that succeeds (one journal line with the account's tag).

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
- The very same request again (the same header, method, target, body and token) within
  HJV_API_RETRY_WINDOW (30 s) gets the first answer again, once the first has it, and does not run
  a second time: the app's TLS fallback sends a request again byte for byte when its first attempt
  failed, possibly after hjv-api answered it, and a 401 would log the app out. A nonce again with
  anything else, or after the window, is a nonce again. The retry of a request that ended its own
  token or key (a logout) gets the 401 of an ended token; the retry of a request that ended
  without an answer, 500.
- On every token path: currentUser, logout, users, peers, `/api/hjv/this-device` and its undo,
  `/api/hjv/devices/{id}` (and its `/wake` and `/wol`), `/api/hjv/wake/{job}`, `/api/hjv/keys…`.
- `POST /api/logout`: the token and its key are revoked. Signed by the token's key (the time may
  be off: the token ends with this request) and when the token's login added or found the PC in
  the account, the PC also leaves the list. Without the signature the PC stays: a stolen token
  cannot take anyone's PC out of the list.

### Account keys: `/api/hjv/keys` (`src/keys.rs`)

```text
GET    /api/hjv/keys                   {"total":n,"data":[{"ckid":12,"name":"iPhone 14 Pro","app_name":"iPhone15,2","platform":"ios","app_ver":"1.6.0","net":"off-campus","created":<time>,"last_login":<time>,"last_used":null,"last_used_pc":null,"use":[],"current":true,"new":false}],"ended":[…]}
POST   /api/hjv/keys/{ckid}/ack        {"ack":true}       ("내 기기 맞음" on the new-device notice)
POST   /api/hjv/keys/{ckid}/not-mine   {"revoked":b}      ("아님" on the notice: marked, and logged out if it was active)
DELETE /api/hjv/keys/{ckid}            {"revoked":true}   (the key and its token; "로그아웃시키기")
POST   /api/hjv/keys/revoke-others     {"revoked":n}      (every key of the account but the asking one)
```

The active keys of the account, newest first, in pages as `/api/peers`, and in `ended` the keys
that ended in the last 30 days (at most 50, the latest first), each with `"ended":<time>`, `"left"`
and, for `removed`, `"left_by":{"ckid","name","app_name","platform"}`, the device that logged it
out. `left` is `logout` (the device logged out, also by `DELETE` of its own key), `removed`
(another device of the account), `expired` (its login expired), `replaced` (a new login of the
same device), `evicted` (the account's 20 keys) or `admin` (the operator, or the account was
blocked). `name` is the device name the server made of what the device said, `app_name` the
device's own; neither is checked (only the time and `net` are the server's own view); `net` is
`on-campus`, `off-campus` or `""`; `use` is what the PCs reported of the key through their sync
(U6): per PC `{"pc_id","pc","last","n30"}`, the latest time the key opened it without its password
(the PC's clock, never after the server got the report) and the count of the last 30 days, `pc`
the host name; a PC's lines go when it leaves the account; `last_used` and `last_used_pc` are its
latest line, or null.

`new` is the new-device notice (design v3 §1-1, U1-a to U1-c): every key of the account but the
asking one, active or ended, whatever order the keys were made in, until it is answered, with no
time limit. "아님" from any device answers it for every device, and the answers of the key that
got it count no more. "내 기기 맞음" from a device that logged in before the key answers it for
that device and the ones that logged in after it (the oldest such answer is kept); from a device
that logged in after the key, for that device only. Whoever logs in with a leaked portal password
can thus neither answer for their own key nor have a second login of theirs answer for it on the
owner's devices, and logging the owner's devices out first does not hide it from the owner's next
login. A key a device logged out is answered for that device; a key replaced by a new login of its
device for every device (the new key gets the notice). A key of another account or none, a
revoked one (but for `ack` and `not-mine`), and `ack` or `not-mine` of the asking key: 404
`{"error":"not found"}`. At most 30 calls a minute per key (429), so that no device can use up
another's.

The key list of a super operator's account also has `"hjv_role":"operator"` at its top, beside
`total`, `data` and `ended`; no other account's has the member. The app reads the list anyway, so
a role given or taken away on the server (at its restart) shows without a new login.

### Device list: `GET /api/peers`

```text
{"total":n,"data":[{"id":"<PC ID>","user_name":"<portal ID>","info":{"device_name":"<host name>","os":"windows"},"status":1,
  "note":"<the card's second line>","hjv":{"pk":"<base64 of the PC's key>","pwless":"<state>","kinds":["remote","file","camera"],"wol":"ok"}}]}
```

`pk` is the key hbbs held when the PC was bound (the app checks the PC it reaches against it),
`kinds` is `HJV_API_PASSWORDLESS_KINDS`. `pwless` and `note`:

| `pwless` | when (the first that holds) | `note` |
|---|---|---|
| `needs-relogin` | hbbs holds another key for the PC than the one stored (design v3 §4-6) | 이 PC에서 다시 로그인 필요 |
| `disabled` | the account's switch is off | 비밀번호 없는 연결 일시 중지(관리자) |
| `listed` | no sync yet, or the last one carried no binding or another one | 이 PC에서 로그인 필요 |
| `expired` | the last sync is older than a list lives (`HJV_API_TRUST_TTL`) | 마지막 확인 <n>분/시간/일 전 |
| `off` | the PC's users switched it off there | PC에서 꺼 둠 |
| `active` | the last sync carried its binding | 비밀번호 없이 연결 |

`wol` is whether the PC can be woken (design v3 §5-2, §8-4): the verdict of its wake report
(`ok`, `warn`, `no`, `unknown` without a report; Waking a PC, below), but `no` while the account
has no active relay, since nothing could wake it, so that the app offers "켜기" on a sleeping PC
only when it is not `no`. Once the account has a relay, the note ends with " · 켜기 가능" (`ok`,
`warn`) or " · 켜기 준비 안 됨" (`no`).

`GET /api/hjv/this-device` of a login that proved it is a PC of the account also has
`"binding":"<32 hex>"`, `"pwless":"<state>"` and `"activation":"present|absent|stale"` (what the
PC's last sync carried: its binding, none, another one).

### The overview: `GET /api/hjv/admin/overview` (`src/overview.rs`)

For a super operator (Admin overview; API above for the other answers), read-only:

```text
{"v":1,"at":"2026-10-14T09:30:00Z","active_days":30,
 "users":[{"id":"<portal ID>","name":"<name>","active":true,"last_use":"<RFC 3339 or null>","keys":2,"pcs":1}],
 "pcs":[{"pc":"<PC ID>","host":"<host name or null>","owner":"<portal ID>","owner_name":"<name>",
         "os":"<OS or null>","version":"<app version or null>","last_sync":"<RFC 3339 or null>",
         "ip":"<address or null>","lan":["192.0.2.67/24"],"passwordless":"active"}]}
```

- `at` is the server's time of the answer, times are RFC 3339 in UTC, `active_days` is
  `HJV_API_ACTIVE_DAYS`.
- `users`: every account that has logged in and is on the allowlist. `last_use` is the latest of:
  a login or a signed request of one of its account keys (the account's activity, which moves at
  most once an hour), a connection without the PC's password that a PC reported for one of its
  keys, and the last time one of its PCs was seen (a sync, hbbs, a login on it); `active` when that
  is at most `active_days` days ago. `keys` is its active account keys, `pcs` the PCs in it. The
  active ones first, then by `last_use`, the newest first.
- `pcs`: every PC bound to an account, also one whose account has left the allowlist, by owner (its
  portal ID), then host name (without case). `version` and `last_sync` are of its last sync; `ip`
  is the client address that sync came from (X-Real-IP, as for every request), `lan` the IPv4
  addresses with their prefixes of its wired cards from its wake report (`[]` without one), and
  `passwordless` its state as `hjv-api admin binding list` names it (the states of the device list
  above; `disabled` also while the account is blocked).

### PC sync: `POST /api/hjv/pc/sync` (`src/pcsync.rs`, `src/envelope.rs`, `src/trust.rs`)

The request is the envelope hjv-relay uses too (relay/PROTOCOL.md §4), at most 16 KiB, signed
with the PC's device key (the key hbbs held when it was bound):

```text
{"id":"<PC ID>","t":<time>,"n":"<base64url of 16 random bytes>","p":"<payload text>","s":"<base64 signature>"}
"hjv-pc-v1" NUL "sync" NUL id NUL t NUL n NUL base64(SHA-256(p))
{"v":1,"binding":"<32 hex, or empty>","ver":"1.6.0","off":false,
 "used":[{"pk":"<account key>","day":"2026-10-20","last":<time>,"n":3}],
 "refused":{"unknown_key":0,"kind":0,"temporary_only":0,"not_secured":0,"bad_sig":0,"expired":0,"off":1},
 "wol_hash":"","wol":{…}}
```

- `t` is the PC's idea of the server's time (it learns `server_time` from the answers), `p` the
  payload as text: the signature covers its bytes as sent. `binding` is the binding the PC holds,
  the one a login on it asks for, or empty after it let one go; `used` (U6) per account key and
  UTC day the latest time the PC let it in and that day's running count, today and yesterday;
  `refused` the counts of refused proofs since the last report taken, under the seven reasons
  above (other names are left out, so that a PC adds at most seven rows a day). `wol` is the PC's
  wake report and `wol_hash` its summary (Waking a PC, below): the lowercase hex SHA-256 of the
  report as the PC serialized it without its uptime, compared for equality only. It is empty while
  the PC has no report (its first minute, a platform without one, the network adapters' registry
  keys not read since the service started); once read, the last summary goes on even when they
  cannot be read again, and a report too large for a sync stays on the PC with only its summary
  sent. A field of the wrong JSON type
  refuses the request; a line out of range (a day outside the last 30, a time not of its day, a
  count of 0, a key of no account of the PC) is left out, and a time after the server's time of
  the report is taken as that time. A `wol` object that is not a report is left out, and not asked
  for again.
- The checks, the first failure answering: size (413 `too_large`), shape (400 `bad_request`), the
  row of `id` (the PC's binding, else the ID's latest tombstone; neither: 404 `unbound`), the
  signature with that row's key (401 `bad_signature`; for a bound PC hjv-api then asks hbbs, at
  most once a minute per PC, and when hbbs signs another key the PC is `needs-relogin`), the time
  within 300 s (400 `clock` with `server_time`), the pair (`id`, `n`) not seen in 600 s (409
  `replay`), the payload (400 `bad_request`). Errors are `{"error":"<code>","server_time":<time>}`.
  As the row comes before the signature, anyone can tell by 404 or 401 whether a PC ID is bound to
  an account or was within 180 days (not which account); hbbs tells anyone whether that ID is
  registered and on anyway (design v3 §9-1).
  There is no 429: a repeat within 10 s while the change serial stayed gets the last list's
  contents again, for its own `n`.
- An accepted sync of a bound PC stores its report (it also counts as the PC seen) and the client
  address it came from, in place of the last one (Admin overview), and ends a `needs-relogin`; key
  use is merged by the larger values, so that a report cannot lower what is
  stored. A sync that carries the PC's binding keeps its `wol` with its `wol_hash` and the address
  the sync came from (the PC's egress address); without `wol` only the address moves, and when
  `wol_hash` is not empty and not the summary kept, the answer has `"send_wol":true`, which asks
  the PC for the whole report in its next sync. Then:

| row | `binding` | answer |
|---|---|---|
| bound | the PC's binding | 200, a signed list: `active` with the account's keys that are not revoked, or `disabled` without keys (the account's switch is off, or it is blocked or off the allowlist) |
| bound | empty, or one no tombstone of the ID has | 200 `{"server_time":…,"state":"listed","refresh":120}`, no list |
| bound | a released binding of the ID | 200, a signed `revoked` |
| tombstone only | a released binding of the ID | 200, a signed `revoked` |
| tombstone only | empty, or another one | 404 `{"error":"unbound","state":"unbound","server_time":…}` |

```text
{"server_time":<time>,"state":"active","refresh":120,"send_wol":false,
 "list":{"kid":1,"l":"<base64 of the list bytes>","s":"<base64 signature>"},
 "access":{"ticket":"t1.…","use":"both","exp":<time>,"server_time":<time>}}
```

- `access` is the PC's access ticket (Access tickets, below), which its service registers with
  hbbs on: in the answer with the list of a sync that carried the PC's binding, while hjv-api has
  a ticket key and the account is on the allowlist and not blocked (whatever its password-less
  switch, so a `disabled` list may come with one). A `revoked` list and `listed` carry none.

- The list, its fields in this order (the bytes are what is signed and checked):
  `{"v":1,"typ":"hjv-trust","kid":1,"binding":"<hex>","pc_id":"<ID>","pc_pk":"<base64>","req":"<n>",
  "serial":<change serial>,"iat":<time>,"exp":<iat + HJV_API_TRUST_TTL>,"refresh":<HJV_API_TRUST_REFRESH>,
  "strict":false,"state":"active","owner":"<masked name>","login_at":<time>,
  "keys":[{"pk","id","name","platform","kinds","ks"}]}`. `req` is the request's `n`, `pc_pk` the
  key that checked its signature, `binding` the one it sent; `owner` the account's name masked
  ("홍*동"), `login_at` the last login on this PC whose device signature hjv-api checked. A key's
  `id` is the device ID of its login (the proof's `my_id` must be it), `name` the server's device
  name, `kinds` `HJV_API_PASSWORDLESS_KINDS`, `ks` the change serial its registration made.
- `s` is the list key's Ed25519 signature over `"hjv-trust-v1" NUL` and the list bytes. The PC
  takes a list only with a key number it pins, at or above the highest it has taken, its own ID
  and key, the binding and `n` of its request, and `0 < exp − iat ≤ 72 hours`; it counts the
  lifetime from receipt, and drops its binding only for a signed `revoked`.

### Access tickets: `POST /api/hjv/ticket`, and the login's and the PC sync's `access` (`src/ticket.rs`)

docs/access-tickets.md is the contract of the ticket, which hjv-api signs, hbbs (server patch
0006) checks and the app carries: `t1.<kid>.<base64url payload>.<base64url signature>`, at most
1024 bytes, the ticket key's Ed25519 signature over `"hjv-ticket-v1" NUL` and the payload bytes,
the payload `{"v":1,"who","use","sub","id","pk","to","iat","exp"}` in that order. hjv-api never
signs a payload that hbbs refuses as malformed. That document is also the contract of the answers
and of the requests for a ticket (3-4, and 3-5 for the access password's); this section says the
same from hjv-api's side, and where the two differ, the document is right.

```text
POST /api/hjv/ticket                         a phone: no body, or {}
{"id":"<PC ID>","pk":"<base64 of its device key>","sig":"<base64 signature>"}    a Windows app
"hjv-ticket-pc-v1" NUL id NUL pk NUL base64(SHA-256(token))                      what sig signs
200 {"ticket":"t1.<kid>.<payload>.<signature>","use":"ctl","exp":<time>,"server_time":<time>}
```

- The ticket object, the same in the three places: `ticket` the string the app carries as it is
  and never reads; `use` what it is good for (`ctl`: the punch hole and relay requests; `both`:
  also the registration, `RegisterPk` field 1001); `exp` when hbbs stops taking it;
  `server_time` the server's clock at the answer, so that the app counts `exp − server_time` from
  receipt and renews in time without reading the ticket. Each answer has a ticket of its own.
- Who gets which ticket:

| where | ticket | lifetime |
|---|---|---|
| the login's answer, `hjv.access` | `staff`/`both` for an installed PC whose `hjv.sig` matched the key hbbs holds for its ID; `staff`/`ctl` for every other device | `HJV_API_TICKET_HOURS` |
| `POST /api/hjv/ticket`, no body or `{}` | `staff`/`ctl` | `HJV_API_TICKET_HOURS` |
| `POST /api/hjv/ticket` with the PC part | `staff`/`both` for its `id` and `pk` | `HJV_API_TICKET_HOURS` |
| the App Review account, at its login and on request | `review`/`ctl` for the `PC=` IDs of its file in their order, until its UNTIL at the latest; without the line none (403 `no_ticket`, no `access` at its login) | `HJV_API_TICKET_HOURS` |
| the access password, `POST /api/hjv/guest` (below) | `guest`/`ctl` for the one PC a controller names, or `guest`/`pc` for a PC's own ID and key | `HJV_API_GUEST_TICKET_HOURS` |
| a support code, `POST /api/hjv/guest` with `code` (Support codes, below) | `guest`/`pc` for a PC's own ID and key | two hours, a constant |
| a bound PC's sync with its binding, `access` | `staff`/`both` for its ID and the key that checked the sync, while the account is on the allowlist and not blocked | `HJV_API_PC_TICKET_HOURS` |

  `sub` is the account's tag (log.rs), never its portal ID, and empty for the access password and
  a support code;
  `to` is empty but for the App Review account and the access password's controller.
  A Windows app needs `both` to register with hbbs: whenever what it holds is not `both` (the
  login's `ctl`, a login without `access`), it asks with the PC part, and every renewal of its
  ticket carries the PC part (docs/access-tickets.md 3-3).
- The PC part: `id`, `pk` and `sig` together or none of them. `id` is a device ID (1 to 64 of
  `A-Z a-z 0-9 - _`), `pk` exactly the standard base64 of the 32-byte device key, `sig` that key's
  strict Ed25519 signature of the bytes above, with `pk` as sent and the hash of this request's
  bearer token, so that a signature goes with one login only. The ID need not be the one the
  login came from (an app whose ID changed gets the ticket of its new one): the key proves the
  PC, and hbbs takes the ticket only for a registration of that ID with that key. A phone, which
  never registers with hbbs, sends no PC part.
- Refusals, besides the 401 and 409 of every account path and 405 for another method: 400
  `{"error":"요청을 이해하지 못했습니다.","hjv":"bad_request"}` for a body that is not one of the
  two (the journal says why once in ten minutes per account, by its tag); 403
  `{"error":"이 계정으로는 출입증을 받을 수 없습니다.","hjv":"no_ticket"}` for an account that gets no
  ticket (the App Review account); 429
  `{"error":"요청이 너무 많습니다. 잠시 뒤 다시 시도해 주세요.","hjv":"rate_limited"}` past 30 calls a
  minute of the account key, counted with its key list's; 503
  `{"error":"지금은 출입증을 받을 수 없습니다. 잠시 뒤 다시 시도해 주세요.","hjv":"unavailable"}`
  without a ticket key. A blocked account's tokens end (401), and its PCs' syncs carry no ticket.
  The tickets it was handed before still work until their `exp`, up to `HJV_API_TICKET_HOURS` for
  its apps and `HJV_API_PC_TICKET_HOURS` for its bound PCs, as after a logout: hbbs checks them
  offline. To end them at once, write `NOT_BEFORE=<now>` into hbbs's access.conf
  (docs/access-tickets.md 6-4), which ends every ticket handed out before; the other apps and PCs
  get new ones. The very same request again within `HJV_API_RETRY_WINDOW` gets the first answer,
  its ticket too, as every signed request that changes something.
- Nothing stores or logs a ticket, and no ticket is recorded one by one: `admin ticket status`
  counts them since the start, by kind.

### The access password: `POST /api/hjv/guest` (`src/guest.rs`)

docs/access-tickets.md 3-5 is its contract. No token, JSON, at most 64 KiB, no `Origin`:

```text
{"use":"ctl","to":"<the PC's ID>","password":"hjv-sealed-v1.<kid>.<sealed>"}            a controller
{"use":"pc","id":"<its ID>","pk":"<base64 of its device key>","sig":"<base64>","password":…}   a PC
"hjv-guest-pc-v1" NUL id NUL pk NUL base64(SHA-256(sealed bytes))                       what sig signs
sealed: {"v":1,"p":"guest","use":"ctl"|"pc","pw":"<password>","t":<time>,"n":"<nonce>","id":"<to | id>","kid":"<kid>"}
200 {"ticket":"t1.<kid>.<payload>.<signature>","use":"ctl"|"pc","exp":<time>,"server_time":<time>}
```

- Exactly the members of its `use`, all strings, each once: a null, an array, another member or a
  repeated one gets 400 `bad_request` (a PC's `code` in the place of `password`: Support codes,
  below). `to` and `id` are PC IDs (1 to 64 of `A-Z a-z 0-9 - _`),
  `pk` exactly the standard base64 of the 32-byte device key.
- The seal is a login's (Login, above: the box, the kid, the time within
  `HJV_API_SEAL_MAX_SKEW`, a nonce used once, the same body again within `HJV_API_RETRY_WINDOW`
  answered by the first answer) with the purpose `"p":"guest"`, the request's `use`, and in `id`
  the PC the ticket reaches (`to` or `id`). A login refuses a seal with a purpose (400, record
  `seal-purpose`), and this path one without: neither seal stands in for the other.
- A PC's `sig` is its device key's strict Ed25519 signature of the bytes above, the sealed bytes
  the base64url-decoded `<sealed>` (the login signature's `h`), so that the key goes with this one
  request; when hbbs knows the ID (`hjv-pk`), the key must be the one it holds.
- Refusals, `{"error":"<text>","hjv":"<code>"}` but for the form's 403, 405, 413 and 415: 400
  `bad_request`; 401 `denied` `{"error":"이용 비밀번호가 맞지 않거나, 지금은 서버가 이용 비밀번호를 받지
  않습니다.","hjv":"denied"}` for a wrong password and for no file taken alike; 403 `wrong_pc`
  when hbbs holds another key for the PC; 409 `clock` with `server_time` for a seal's time; 429
  `rate_limited` past a limit, with `retry_after`, the seconds until that limit has room again (at
  least 1); 503 `busy` while two hashes run (or 32 tries are in hand), 503 `unavailable` without a
  ticket key or when hbbs's console does not answer about a PC.

A PC's request with the vectors' PC key (the seed `[2; 32]`, as above) and the sealed bytes of the
seal vector of Login (its `h`); the signature is the one OpenSSL makes too:

```text
device key       gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q=
SHA-256(sealed)  qIDSw48Gkb5KSslffq/QEWRaClx5ihVLTE5+jqOHQ6Q=
signed (115)     686a762d67756573742d70632d7631003132333435363738390067546c3344716839463139576f31526d7730782b7a4d754e69704730376a65695866595057342f4a7335513d0071494453773438476b62354b53736c6666712f5145575261436c78356968564c5445352b6a714f485136513d
sig              LCgw71tbHtZvAgBNOoElzNffv/sjmraHOBM/jvDp1fiGwvACfrVddPgq/FbR25j+KhE5fvd5rf2nT1KFbi0UDg==
```

### Support codes: `POST /api/hjv/support-code`, and `code` at `POST /api/hjv/guest` (`src/support.rs`, `src/guest.rs`)

docs/access-tickets.md 3-7 is their contract.

```text
POST /api/hjv/support-code  {"use":"pc"}                        a staff member's app: token and X-HJV-Proof
200 {"code":"K7M2-Q9XD","use":"pc","expires_in":600}
POST /api/hjv/guest  {"use":"pc","id":"<its ID>","pk":"<base64 of its device key>","sig":"<base64>","code":"hjv-sealed-v1.<kid>.<sealed>"}
"hjv-guest-pc-v1" NUL id NUL pk NUL base64(SHA-256(sealed bytes))   what sig signs, as for the access password
sealed: {"v":1,"p":"code","use":"pc","pw":"<what was typed>","t":<time>,"n":"<nonce>","id":"<id>","kid":"<kid>"}
200 {"ticket":"t1.<kid>.<payload>.<signature>","use":"pc","exp":<time>,"server_time":<time>}
```

- Making one: the body exactly `{"use":"pc"}`, a JSON object of that one string (`ctl` is kept
  for later), else 400 `bad_request`; 200 with the code as shown, its use and the seconds it has
  left. Refusals besides the 401 and 409 of every account path and 405 for another method: 403
  `{"error":"이 계정으로는 지원 코드를 만들 수 없습니다.","hjv":"no_code"}` for the App Review account;
  429 `rate_limited` past 30 calls a minute of the account key; 429
  `{"error":"쓰지 않은 지원 코드가 너무 많습니다. 잠시 뒤 다시 해 주세요.","hjv":"too_many_codes","retry_after":<s>}`
  while the account holds its 5 open codes, `retry_after` the seconds until the first of them ends
  (at least 1); 503
  `{"error":"지금은 지원 코드를 만들 수 없습니다. 잠시 뒤 다시 해 주세요.","hjv":"busy"}` while 1000
  open codes are held, and the same text with `unavailable` without a ticket key. The very same
  request again within `HJV_API_RETRY_WINDOW` gets the first answer, its code too, as every signed
  request that changes something.
- Trading one: the access password's PC request (above) with `code` in the place of `password`.
  Both of them, or `code` with `use` `ctl`, get 400 `bad_request` before anything is counted or
  opened. The seal is the access password's with the purpose `"p":"code"`, which only `code`
  takes (another seal is 400, the address's failure; `password` takes only `"p":"guest"`). `pw` is
  what was typed, the spaces around it dropped (1 to 256 bytes), read as a short link's code.
- The checks are the access password's in their order, with the code looked up where the hash is:
  a code that is not there, not of the form, past its ten minutes, used, or whose account is
  blocked, deleted or off the allowlist gets the one 401
  `{"error":"지원 코드가 맞지 않거나, 10분이 지났거나, 이미 쓴 코드입니다.","hjv":"denied"}`, counted
  as the address's failure and sent after the failure delay as every refusal. The key hbbs holds
  comes after (403
  `{"error":"이 PC의 키가 서버에 있는 것과 달라 지원 코드로 연결받을 수 없습니다. 관리자에게 알려 주세요.","hjv":"wrong_pc"}`,
  503 `{"error":"지금은 지원 코드를 확인할 수 없습니다. 잠시 뒤 다시 해 주세요.","hjv":"unavailable"}`,
  the same text without a ticket key), and the code is used only with the ticket: what ends
  before (a limit, the seal, `wrong_pc`, hbbs) leaves it as it was, and of two requests with one
  code at once, one gets the ticket and the other the 401 of a used code, at the lookup or, when
  both passed it, where the code is used (`used` in the records). No PBKDF2 runs for a code; the
  503 `busy` of 32 tries in hand is the same.
- The ticket: `guest`/`pc`, `sub` empty, the request's `id` and `pk`, `to` empty, `exp` two hours
  after `iat`; nothing in it names the staff member.

### The access check's mode: `GET /api/hjv/access` (`src/gate.rs`)

docs/access-tickets.md 3-6 is its contract. No token, no body, always 200:

```text
200 {"mode":"off"|"count"|"enforce"|"closed"|"unknown"}
```

- `mode` is the mode of hbbs's answer to `hjv-access` on its console (server patch 0006), without
  the counts after it; `unknown` when hbbs does not answer or does not know the command. hjv-api
  asks hbbs at most once every 30 s and gives the same answer in between, so a change of
  access.conf shows here within hbbs's 10 s and these 30 s.
- The apps word with it what a logout or taking a PC out of the list does, and tell a controller
  that is not logged in, ahead of a refusal, that the server lets in only staff. hbbs alone
  decides; the answer is only a hint.

### NAS relays: `POST /api/hjv/relay/hello`, `/api/hjv/relay/poll` (`src/relay.rs`)

relay/PROTOCOL.md is the contract; hjv-api implements its server side. The envelope is the PC
sync's with the context `hjv-relay-v1` and the operation `hello` or `poll` (a hello also carries
the relay's key, `pk`, and its ID must be that key's: `r-` and base32 of the first 10 bytes of its
SHA-256), at most 64 KiB, checked in the same order: size (413 `too_large`), shape (400
`bad_request`), the key (a relay kept under the ID keeps its key, else 401; a poll of an unknown
relay 404 `unknown_relay`), the signature (401 `bad_signature`), the time within 300 s (400
`clock` with `server_time`), the nonce within 600 s (409 `replay`; 503 `unavailable` with
`retry_after` while 100 000 nonces are kept), the payload (400 `bad_request`).

The limits (429 `rate_limited` with `retry_after`) are not among those checks, and a request they
refuse leaves nothing: no nonce, no write. A known relay's own limit, 60 hellos an hour and 120
polls a minute, is taken once a request is known to be that relay's and of this time, after the
time and before the nonce: it counts every such request, also one that is then refused as a
replay or for its payload (the latter keeps its nonce), and no request that is not the relay's.
The limits on new entries come after the payload, and a hello of a key hjv-api does not know
keeps its nonce only when it makes the entry. So the nonces kept, and the database's writes, are
bounded by hjv-api's own limits. They have to be: a pending relay is a known one, and anyone with
a key can become one (relay/PROTOCOL.md §4.3).

- hello: a new key becomes a pending entry (its egress address, versions and interfaces kept), 5
  an hour per source address (an IPv6 address by its /64), 20 in all, each for 24 hours; a code
  approved ahead makes it active at once; a known relay may say hello 60 times an hour. Answer:
  `{"server_time","state":"pending|active|revoked","code":"KZDV-VJ2U-MNDU-YAUF"}`.
- poll: the results are applied, what the relay reports is kept, `"send_ifaces":true` asks for its
  interfaces when their summary changed; a revoked relay is answered at once, a pending one when it
  is approved or revoked or after `min(wait, 20)` seconds, an active one at once with its queued
  commands, else when one is queued or after `min(wait, 20)` seconds (at most 120 polls a minute
  per relay). Answer: `{"server_time","state","commands":[{"c","s","k"}],"send_ifaces"}`. The time
  of its last poll is its liveness (online within 60 seconds).
- A command `c` is compact JSON, `{"v":1,"job","relay","iat","exp":iat+60,"net","macs","ports":[9,7],
  "repeat":3,"gap_ms":200}`, signed by the command key over `"hjv-wake-v1" NUL c` (`s`, base64; `k`
  the key's number); its text, signature and number are kept and sent as they are until a result
  names it (by `cmd`, the hex SHA-256 of `c`) or it expires. relay/PROTOCOL.md §13's vector,
  checked by `src/wake.rs`'s tests:

```text
command key       1:Kay64UG8yvCyLhqU000LxzYeUm0L/hLIl5S8kyKWbdc=   (seed 20 21 … 3f)
fingerprint       1:24f6ed6acbfe1009c030d7ca567c33ca4830911498236b5561a6c82abec5de28
c                 {"v":1,"job":"0123456789abcdef0123456789abcdef","relay":"r-kzdvvj2umnduyauf","iat":1760000000,"exp":1760000060,"net":"10.1.4.0/22","macs":["00:00:5e:00:53:01"],"ports":[9,7],"repeat":3,"gap_ms":200}
s                 91MXS5rMOB9k3ZP7XxPue9M++nXy9w4VIpjcqGJspnFI46U37TGC+st2hv6D8DxcUJOaN/2PcY7XeQ6wgfmLBw==
cmd               eefb709bfc6dae46233f75d43c02d0dfb173218d3ed9010c40dc316b4c3d7abe
```

- A result (`{"cmd","job","sent_at","iface","packets","error"}`) is matched by `cmd`, idempotently:
  no error, or `partial` with packets, makes the job `sent` at the relay's `sent_at`; `replayed`
  says nothing new; any other error fails the job with that code, and one that says the relay and
  hjv-api disagree (`unknown_key`, `bad_signature`, `bad_command`, …) goes to the journal.

### Waking a PC: `/api/hjv/devices/{id}/wake`, `/api/hjv/wake/{job}`, `/api/hjv/devices/{id}/wol` (`src/wake.rs`, `src/wolinfo.rs`)

```text
POST /api/hjv/devices/{id}/wake   {} or {"any_relay":true}
  202 {"job":"<32 hex>","state":"queued","relay":{"name":"NAS KZDV","match":"exact"},"hints":[]}
  409 {"error":"이미 켜져 있습니다.","hjv":"online"}
  409 {"error":"…","hjv":"no_wol_info"}
  409 {"error":"…","hjv":"no_relay","reason":"no_relays|other_network|different_egress|unconfirmed","any_relay":false}
  503 {"error":"…","hjv":"relay_offline","last_seen":<time or null>}
  429 {"error":"잠시 뒤 다시 시도해 주세요.","hjv":"rate_limited","retry_after":<seconds>}
  503 {"error":"…","hjv":"unavailable"}   (no command key)
GET  /api/hjv/wake/{job}
  {"job","state":"queued|sent|confirmed|not_confirmed|failed","sent_at","confirmed_at","packets","error","relay":{"name","match"},"hints":[]}
GET  /api/hjv/devices/{id}/wol
  {"ready":"ok|warn|no|unknown","hints":[],"reported_at":<time or null>,"relay":{"name","match","online"} or null}
```

- The wake request, in this order: a PC of the token's account (else 404, as for any other ID);
  on already (it synced, or hbbs saw it register, within 30 seconds; hbbs is asked 10 times a
  minute per account at most, since a refused request counts against no limit below, and past
  that a PC that has not synced counts as off): 409 `online`; no wake report: 409 `no_wol_info`;
  one job a minute per PC, of the account's own: within the minute the job under way (queued or
  sent) is the answer, 202, and one that is over 429; 20 jobs an hour per account: 429; the
  relay; 10 jobs a minute per relay: 429; the command, signed and queued, and the relay's held
  poll gets it at once. A minute after a job that still waits for its PC the packets may be sent
  again, as a new job; each job is confirmed on its own.
- The relay: of the account's active relays (a relay wakes its account's PCs only), the online ones
  with the best grade (`exact`: an interface on the PC's network with its prefix and the same
  gateway IP; `overlap`: overlapping networks and the same gateway IP or MAC; `history`: it woke
  this PC on this network before and still has an address on it); on a private or shared network
  (RFC 1918, 100.64.0.0/10) the PC's and the relay's egress addresses must be the same too. One
  that would do but is not online: 503 `relay_offline`. None: 409 `no_relay`, `any_relay` true
  when an online relay shares a network with the PC that could not be confirmed (another gateway,
  no egress address to compare); `{"any_relay":true}` then uses it (`match` `any`).
  `different_egress`: a private network behind another address, another site; the app hides
  "모든 중계기로 시도" then.
- The command names the PC's network that matched and the MACs of its cards on it, from its signed
  report only; no MAC goes to an app.
- States: `queued` → `sent` (the relay's result) → `confirmed` (the PC synced after the packets,
  or hbbs saw it register under its key within 30 seconds and after them; looked at every five
  seconds) or `not_confirmed` (five minutes after the packets); `queued` → `failed` with the
  relay's error (`no_iface`, `send_failed`, `expired`, `not_active`, `rate_limited`; others mean
  the relay and hjv-api disagree) or `no_result` (45 seconds after the command expired). A result
  after `no_result` still counts.
- The verdict and its hints (the texts go to the app as they are, so the rules change without a
  new app): no wired card with an address: `no`, the one verdict that takes "켜기" off the app's
  menu. Everything else is a hint with `warn`, shown when a wake was not confirmed: the cards'
  magic packet wake off (`"magic":"0"`, all of them or some) or not known to be on for any (a
  driver that does not tell), no card armed to wake the PC, fast startup on, on battery, USB cards
  only, modern standby without its magic packet wake.

### Updates: the packages of new app versions, `/api/hjv/update`, `/api/hjv/u`, `/api/hjv/get` (`src/update.rs`, `src/body.rs`)

The design of the minimum version and forced update, §4-6. The operator installs the packages,
then the signed policy (`hjv-release` makes both; the runbook has the steps); the apps get the
policy from nginx and its `latest` from hjv-api.

- **What is sent.** hjv-api reads the policy file again when it changes (also its owner or mode
  alone) and takes from it, for `windows` and `android`, `latest`, `source` and `package` (`file`,
  `size`, `sha256`), each by the policy's own rules (a version is three numbers; a package's file
  is a name of `[A-Za-z0-9._-]` with the platform's extension that neither ends in `install.exe`
  nor holds `-qs-`; a size is 1 byte to 200 MiB): a file that breaks one of them is no policy at
  all, and nothing is handed out. It does not check the signature (only root writes the file; the
  apps check it, and the size and SHA-256 of what they get). A policy older (by `epoch`, then
  `seq`) than one read before since the start is an error in the journal: the apps keep the newer
  one, so going back is a newer policy that asks for less. A package is the policy's
  `package.file` in `HJV_API_PACKAGE_DIR`: a plain file, no link, of exactly `package.size`; no
  name or path comes from a request. A missing policy, entry, package or file is 404.
- **How.** The file is read and sent 64 KiB at a time, as fast as its reader takes it and no
  faster than 3 840 000 bytes a second (about 30 Mb/s, the relay's limit for a session on the same
  line), within an hour; a reader that takes nothing for a minute loses the connection. With it:
  `Content-Type` by the extension (`application/vnd.microsoft.portable-executable`,
  `application/vnd.android.package-archive`, `text/plain; charset=utf-8`), `Content-Length`,
  `Content-Disposition: attachment; filename="…"`, `Cache-Control: private, no-store`,
  `X-Content-Type-Options: nosniff`, `X-Robots-Tag: noindex, nofollow, noarchive`,
  `Referrer-Policy: no-referrer`, `Accept-Ranges: none`, and for nginx `X-Accel-Buffering: no` and
  `X-Accel-Limit-Rate: 3840000`. nginx passes such an answer on as it comes and writes it to no
  file of its own; it does not slow one down (nginx 1.24 follows `X-Accel-Limit-Rate` only for an
  answer it buffers), which is why hjv-api keeps the rate itself; it also ends a download whose
  hour is over at its next write, however few bytes its reader takes at a time.
  `HJV_API_DOWNLOADS_MAX` files, packages and notice files alike, go out at once; one more gets
  `503 {"error":…,"hjv":"busy","retry_after":60}` with `Retry-After: 60`. HEAD gets the same
  head, takes no slot and counts nowhere.
- **Door 1**, `GET, HEAD /api/hjv/update/{windows,android}/{x.y.z}`: 400 for another platform or
  what is no version; then who asks; then 404 unless `x.y.z` is the platform's `latest` with a
  package. The download is named `HJViewer-<v>.exe` (never a name that makes the packer open the
  installer) or as the APK is. Who asks is one of two:
  - a login: `Authorization: Bearer hjv_…` with `X-HJV-Proof`, checked as on every account path
    (401, or 409 `clock`);
  - a PC in an account, without a login: `X-HJV-Device: <PC ID>`, `X-HJV-Time: <t>` (Unix
    seconds, decimal) and `X-HJV-Sig`, standard base64 of the Ed25519 signature by the PC's device
    key over `"hjv-update-v1" NUL <id> NUL <t> NUL <platform> NUL <version>` (`t` as the header
    has it, the platform and the version as the path has them), checked strictly with the key
    stored with the PC's binding. The PC's account must be on the allowlist, `|t − server time| ≤
    600 s`, and the platform must be the PC's own (the `os` of its binding: a PC's key gets the
    package of the app that PC runs, and no APK). Whatever of this fails, a PC in no account
    included, the answer is the same `403 {"error":"이 기기는 새 버전을 받을 수 없습니다."}`.

  The App Review account's login gets `403 {"error":"이 계정으로는 새 버전을 받을 수 없습니다."}`
  here and for a short link.
- **Short links**, `POST /api/hjv/update/{windows,android}/link` with a login and the body
  `{"installed":true}` or `{"installed":false}` (`{}` is false): `200
  {"url":"/api/hjv/u/K7M2-Q9XD","file":"HJViewer-1.6.1-install.exe","expires_in":600}`. The code
  is 40 random bits as eight symbols of Crockford's base32 (`0-9A-HJKMNP-TV-Z`), shown `XXXX-XXXX`;
  `GET, HEAD /api/hjv/u/{code}` reads it without regard to case, `-` and spaces, with `O` as `0`
  and `I` or `L` as `1`. `file` is what the download is saved as: for Windows
  `HJViewer-<v>-install.exe` for a PC with HJViewer installed (the packer opens the installer for
  that name) and `HJViewer-<v>.exe` otherwise. A link is bound to its account, platform, version
  and name; it lives ten minutes and three downloads (HEAD is free), ends when its package is no
  longer the latest or its account is blocked, and is kept in memory only (a restart
  ends it). The app shows the link under its own server name and takes `url` only in the form
  above.
- **Door 2**, `GET, HEAD /api/hjv/get/{code}/` and `…/{code}/{file}`, open while
  `HJV_API_DOOR2_FILE` is there with `CODE=<32 lowercase hex digits>` and `UNTIL=<UTC time,
  2026-12-22T15:00:00Z>` (and `#` comments; `hjv-release hbbs-conf --door2-out` writes it) and
  `UNTIL` has not come. `…/{code}/` is a page in Korean made from the policy at each request
  (`Content-Security-Policy: default-src 'none'; style-src 'unsafe-inline'`; `…/{code}` without
  its `/` is a 307 to it): the steps for an installed PC and for one that only runs HJViewer, the
  two names of the Windows package, the APK, each with its size and SHA-256, the source tag, and
  the notice files. The page names no outside site (the user's request, 2026-10-14): it says that
  the SOURCE notice next to the packages tells where to get the source of that tag, and that
  notice is the AGPL's "clear directions next to the object code" (section 6(d)). Its only link
  off the page is TestFlight's `itms-beta://`. The files are `HJViewer-<v>-install.exe` and
  `HJViewer-<v>.exe` (one file, two names), the APK under its own name, and
  `HJViewer-<v>-LICENSE.txt`, `-NOTICE.txt`, `-SOURCE.txt`, `-THIRD_PARTY_NOTICES.txt` from
  `notices-<v>/` of the package directory. A wrong code, a closed door and any other name are the
  same 404. To close door 2, delete the file (while the hbbs file's text carries door 2's link,
  put its S2 text in first: runbook 8-4); to change the code, replace it.
- **Limits.** A request that fails for who asks (no or a wrong token, proof, signature or code)
  counts against its client address; past `HJV_API_LIMIT_UPDATE_FAIL_IP` such a request gets `429
  {"error":…,"hjv":"rate_limited","retry_after":<seconds>}` with `Retry-After` in place of its
  401, 403 or 404, and is counted no further. A request with the right token, signature or code
  is never refused for its address. The HEAD without `Authorization` and without the three device
  headers, which the app's HTTP client sends first to learn how to connect, gets its 401 and
  counts nowhere. A download that begins counts under `HJV_API_LIMIT_UPDATE` for the account (all
  its logins) or for the PC (its device key), a link made under 20 a day for its account; the
  downloads of a link are the link's three, and door 2 has the download slots only.
- **Records.** Every package sent and every link made is a record of kind `update` (`token-…`,
  `device-…`, `link-made-…`, `link-…`, `door2-…` with the platform) with the account and the PC
  as tags and the client address as its tag, and a line in the journal; a code shows there as the
  first eight hex digits of its tag, a hash keyed with the log key like the other tags (a plain
  hash of a short link's 40 bits would leave a reader of the journal a few hundred codes to try).
  The first download by a PC's key from an address that PC did not use before is a record of its
  own, `device-new-address`, a notice in the journal and a line in the operator record (a key that
  someone took along with a PC's disk shows as an address the PC was never at). Door 2's records
  stop at 20 a minute.

### hbbs and the SSO

- hbbs's `hjv-pk <id>` answer, a device key signed with hbbs's key pair: `src/hbbs.rs`.
- The SSO calls and how their outcomes are sorted: `src/sso.rs`. hjv-api uses the registration of
  the 행정허브 WordPress plugin on the same server (D2) and reads the answers as that plugin does.
  Both calls are a `POST` of a form (`application/x-www-form-urlencoded`): the token call of
  exactly `username` (the portal ID, uppercased), `password` and `grant_type=password` with
  `Authorization: Basic <hjviewer-sso-client-secret>`, the user call of exactly `apiUuid` and
  `clientId` with `Authorization: Bearer <access_token>`. Each answer is HTTP 2xx, at most 1 MiB,
  JSON nested at most 32 deep, with a top-level `success`: `true`, `1`, `"true"` or `"1"` goes on,
  `false`, `0`, `"false"` or `"0"` refuses (in the token call a wrong password, in the user call
  the person is denied), anything else or none is a format error, as is an `access_token` that is
  not a string of 1 to 16384 characters without control characters or whitespace. The user call's
  fields depend on the api uuid; for this registration `data[0]` holds `pcode`, `name`, `email`
  and `deptNm`, and hjv-api reads `pcode` and `name` only. Both must be there as strings: else the
  answer's form is not the contract's, the same for everyone, and it is an upstream error (five in
  a row pause the SSO calls). Their values must be without control characters, of at most 60 and
  250 characters, and not empty once trimmed: a value that breaks this is one person's data, so
  that person is denied (`denied-bad-value`, the journal names the member and the rule, not the
  value), and one person retrying never pauses the calls of everyone. The pcode, uppercased, is the
  person's fixed ID: the account keeps it (the column `portal_id`) with the name, and a block of
  the person goes by it. hjv-api's own sorting on top of this (a refused server address by the
  SSO's `msg`, `invalid_client`, `invalid_grant`) makes no login succeed that the plugin's reading
  refuses.
- E-mail and department, on purpose: hjv-api neither reads nor checks `email` and `deptNm`, to
  keep no more of a person than it uses. The plugin checks that they are there (strings, or null),
  within their lengths and without control characters, and that an e-mail looks like one; so an
  answer that the plugin refuses for its `email` or `deptNm` alone, such as a malformed e-mail,
  logs in here.
- An account's key is the portal ID typed at login, trimmed and lowercased, not the pcode: the
  pcode is kept as the person's fixed ID, which blocks, the check of the person's other accounts
  (Admin, Blocking) and the operator's view (Admin overview) go by. Someone who logs in with two
  different IDs that the SSO names one pcode for has two accounts, each with its own devices and
  PCs; the journal line `login: the portal ID typed and the pcode the SSO names differ` shows such
  a login, and a block of either ID holds for the person. Whether the SSO takes more than one form
  of a person's ID is not known (design v2 §4-1); keying accounts by the pcode instead is the
  user's decision.
- A wrong `HJV_API_SSO_CLIENT_ID` or credential `hjviewer-sso-api-uuid` may make the user call
  refuse everyone (`success` false, access denied): then each login gets the denial text, and
  nothing pauses, as for one person. So three different accounts refused by the user call within
  ten minutes, with no user call answered between (a person found, or denied for their pcode or
  data), write one critical line to the journal (`login: the SSO's user call refused 3 different
  accounts in a row …`), and the five-minute check reports `sso_user` for ten minutes after the
  last of them (Monitoring).
- HJViewer's ID rule (`util::staff_id`, the user's decision of 2026-10-14): a portal ID, uppercased,
  is Z or Y and then 1 to 59 of A-Z, 0-9, `_` and `-`, and does not start with ZA: the faculty's
  and staff's. The plugin takes Z and B with the same tail; another first letter is one line of
  `src/util.rs`. The ID typed must pass it before any SSO call (any other gets its 403 at once and
  takes none of the SSO calls), and so must the pcode the SSO names (else the person is denied:
  Login, above).

### Test vectors

The vectors shared with the client are `tests/data/v3-vectors.json`: the client renders the file
from its `src/hjviewer_account/vectors.rs` and pins its SHA-256, and `tests/keys.rs` and
`tests/pcsync.rs` check this code against it (the request proofs, the sealed login's `ck` and `cp`,
the trust list and the sync envelope). The ones below come from the same account key and nonce
bytes.

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

The trust list of the shared vectors (`tests/pcsync.rs`): the list key 1 is the key of the seed
`[1; 32]` (key 2: `[9; 32]`), the PC's device key that of `[2; 32]`; hjv-api's list of the same
values is these bytes, and its signature this `s`.

```text
HJV_TRUST_PUBKEYS 1:iojj3XQJ8ZX9UtstPLpdcspnCb8dlBIb83SIAbQPb1w=,2:/RckOFqgx1tk+3jNYC+h2ZH96/drE8WO1wLqyDXp9hg=
list (UTF-8)     {"v":1,"typ":"hjv-trust","kid":1,"binding":"b0f1e2d3c4b5a69788796a5b4c3d2e1f","pc_id":"123456789","pc_pk":"gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q=","req":"EBESExQVFhcYGRobHB0eHw","serial":4182,"iat":1760000001,"exp":1760086401,"refresh":120,"strict":false,"state":"active","owner":"홍*동","login_at":1759999950,"keys":[{"pk":"7UkoxijRwsbq6QM4kFmVYSlZJzpcY/k2NsFGFKyHN9E=","id":"987654321","name":"iPhone 14 Pro","platform":"ios","kinds":["remote","file","camera"],"ks":4180}]}
s                Nw16DIR5leqpTGfWFU/sy14XyKbCgG8dBqeCfKiKTO3tANo/PZ24ows1wYuGH3k4x0sqYy6TMcWTj+8NkuwJCw==

the PC's sync of that request, t = 1760000000, n = EBESExQVFhcYGRobHB0eHw (bytes 10 11 … 1f)
p                {"v":1,"binding":"b0f1e2d3c4b5a69788796a5b4c3d2e1f","ver":"1.6.0","off":false,"used":[{"pk":"7UkoxijRwsbq6QM4kFmVYSlZJzpcY/k2NsFGFKyHN9E=","day":"2025-10-08","last":1759962000,"n":1},{"pk":"7UkoxijRwsbq6QM4kFmVYSlZJzpcY/k2NsFGFKyHN9E=","day":"2025-10-09","last":1759999000,"n":2}],"refused":{"unknown_key":0,"kind":0,"temporary_only":0,"not_secured":0,"bad_sig":0,"expired":0,"off":0},"wol_hash":""}
SHA-256(p)       iMyEf1q+Mr4xwo9aviy8vXD+GTlPt/W8CyuJ7y8URlk=
signed (103)     686a762d70632d76310073796e630031323334353637383900313736303030303030300045424553457851564668635947526f6248423065487700694d79456631712b4d723478776f3961766979387658442b47546c50742f57384379754a37793855526c6b3d
s                VVlqZ2wbpNnG3trz+58Y7RftEQMb1GwlsUWg7Yf4XXd94z3RiDzLQqJJPT6mKSLBA9eHEKxK/arHQ39alppkCQ==
```

The signature of a PC that downloads its package (`tests/update.rs`, `src/update.rs`): the vector
of the design's research notes, which the client checks its side against too; the device key is
the key of the seed `60 61 … 7f`.

```text
device key       F0VTtFbd38aQjsqxwQH+arIeK6oGF3lbfUOmNIKZP9U=
GET /api/hjv/update/windows/1.6.1, X-HJV-Device: 123456789, X-HJV-Time: 1793577600
signed (48)      686a762d7570646174652d76310031323334353637383900313739333537373630300077696e646f777300312e362e31
X-HJV-Sig        9SMz/B9qthKAtR/xDS6vk+fep97TTkCtFVTJRJ51ypTwmwxZMoH2EUoFrHlIN2LH5c+K5lTUfUoYMAQovYRLDg==
```

The access tickets' vectors are `tests/vectors/access-tickets.json` (docs/access-tickets.md 8),
which `tests/tickets.rs` reads, pinned by its SHA-256, as hbbs's tests read it: hjv-api makes every
valid ticket of it again byte for byte, and its signer refuses exactly the payloads hbbs refuses as
malformed. A Windows app's request for a `both` ticket (`src/ticket.rs`), with the vectors' PC key
(the seed `[2; 32]`) and the token above; the signature is the one OpenSSL makes too:

```text
device key       gTl3Dqh9F19Wo1Rmw0x+zMuNipG07jeiXfYPW4/Js5Q=
POST /api/hjv/ticket, {"id":"123456789","pk":<the device key>,"sig":<sig>}
SHA-256(token)   h6q74v9/1g0Rq8wvBTM5yQIFPZwyCOmhw1Pff+EcZ6g=
signed (116)     686a762d7469636b65742d70632d7631003132333435363738390067546c3344716839463139576f31526d7730782b7a4d754e69704730376a65695866595057342f4a7335513d00683671373476392f316730527138777642544d3579514946505a7779434f6d6877315066662b45635a36673d
sig              nG4V0N+qAT9bPvurXKhwQG3b4vvYwN+nqee9l9iA6qoRUIRsICcBdsYnaj3FWb3L4p/NC1GGMV6awpRVZazoAg==
```

These keys are public test values: never use them for anything else.

## Tests

`cargo test` runs unit tests (among them libsodium-made vectors for sealing and signatures) and
integration tests that start hjv-api in-process with the fake SSO (`src/fake_sso.rs`), a fake hbbs
console and the admin socket, and the binary under `systemd-socket-activate` when it is installed.
The test client logs in as the app does, with a new account key each time, and signs its account
requests (`tests/common/mod.rs`); `tests/allowlist.rs` has the open login of 2026-10-14 (an ID
never imported registered at its login, blocks and unblocks, a block while a login runs, list
mode); `tests/keys.rs` has the account keys, the switches and the test
vectors above and in `tests/data/v3-vectors.json`, `tests/http.rs` the request proof and its
retries (after the first answer, while the first attempt runs, after it was dropped unanswered), and
`tests/pcsync.rs` the PC sync: its checks in order, the lists as the PC checks them, the
ten-second repeat, key use, the device list's states, every way a PC leaves its account, the key
change confirmed with hbbs, the report and selfcheck. `tests/overview.rs` has the super operator's
view: its answer (counts, order, fields), the 404 of an unknown path for every other account and
method, the refusals of a request without its proof, nobody without `HJV_API_OPERATORS`, the role
in the login's answer, the user object and the key list as the setting says at each start, the
address of a PC's last sync (overwritten, gone with the PC), the record's line of each look without
a portal ID, and `admin operators`. `tests/relay.rs` has hjv-relay's requests:
the checks in their order with the cases of relay/tests/common's `check_request` and the vector of
relay/PROTOCOL.md §13, the states, approval (no address in the default output) and revocation,
approval ahead, the limits (what they refuse leaves no nonce; a relay's own counts the requests of
its that are then refused, and nobody else's), the nonce store's bound, the long-poll, the
interface reports;
`tests/wake.rs` the wake reports in the sync, the wake paths in their order, the choice of relay,
the command as the relay checks it, the jobs' states, the limits, the verdicts in the device list,
what goes with an account or past its time, and the command key's checks; `tests/relay_loop.rs`
runs hjv-relay itself (relay/'s library, its loop step by step) against hjv-api behind a TLS
terminator that stands in for nginx: enrollment, approval, a long-poll a wake ends at once, twelve
packets recorded on the PC's wire, the result, the confirmation, revocation, enrolling again, and a
relay that does not trust the key in use; and it gives the same 900 requests, well-formed and
broken in every part, and a hello whose key is no point of the curve, to relay/tests/common's
`check_request` (the relay's reference of PROTOCOL.md §4.3) and to hjv-api, which must come to
the same verdict each time (within hjv-api's limits: the reference has none). That file needs
relay/ next to server/ (a dev dependency by path). `tests/ops.rs` has the backups (the rows of
a backup's file, the places refused, the old backups deleted, a backup while requests go on), the
restore procedure on a restored database, the check (a fake report with a stopped socket and no
syncs, then the service's own counts), the alert with a stand-in for Telegram, and the units under
`systemd-analyze verify` (skipped without it). `tests/update.rs` has the packages of new app
versions: door 1 by a login (the right proof, each wrong one and none) and by a PC's device key
(the vector above, and everything that gets the one 403), short links (their forms, ten minutes,
three downloads, the same link for a request sent again), door 2 (its page from the policy, its
names, a closed door), the limits (thirty failures of an address, right requests never refused,
the HEAD without headers), a package that is not as the policy says, and that the journal and the
records hold tags only; `tests/review.rs` the App Review account (its login without the SSO, also
while SSO calls are paused or with a second factor, a wrong password and the ID's limit, the
success limits before the hash, the file missing, past its UNTIL or refused, list mode, no PC, no
account it did not make, door 1 and short links refused, no super operator, its tokens ending with
the file, the line at the start);
`tests/tickets.rs` the access tickets: the shared vectors made again and the signer's rules held
against them, `POST /api/hjv/ticket` (a phone's ticket, a Windows app's, the refusals, the retry,
the limit), the tickets of the login's answer and of a bound PC's sync (by the account's state and
the PC's binding), the App Review account's for its demo PCs (and none without the `PC=` line), a
server without a ticket key, `admin ticket status`, selfcheck (with the vectors' public test keys
refused, as outside the harness), that no ticket reaches the journal or the records, and `GET
/api/hjv/access` (hbbs's mode without its counts, kept a while, `unknown` from an hbbs that does
not know the command);
`tests/guest.rs` the access password: a controller's and a PC's ticket, the same 401 for a wrong
password and a closed file after the failure delay, the seal of this request only (a login's seal
refused, and the access password's at the login), the retry, the form, the limits before the hash
and `admin unlock` (each 429 with the seconds of its limit), a PC that hbbs knows by another key,
no ticket key, the README's vector, and that nothing of it reaches the journal or the records;
`tests/update_stream.rs` how a package goes out, on tokio's paused clock
with hjv-api on one end of a connection in memory (a slow reader past the minute a request has, a
reader that stops, the fifth download at once, the hour a download may take, 200 MiB at the rate),
and the program itself sending from a 200 MiB file without holding more memory than for a small
answer.
Their files and sockets go to the system's temporary directory (`/tmp` when `$TMPDIR` is long), so
that socket paths stay short whatever the target directory. Nothing calls the real SSO.
`hjv-fake-sso --listen 127.0.0.1:18600` serves the fake SSO for the E2E, in the 행정허브 answer
shape, with fixed test accounts (`ZZTEST01`, `ZZTEST02`, `YZTEST03`: fake IDs of HJViewer's ID rule;
passwords in `src/fake_sso.rs`) and usernames `ZZSSO-*` that play the SSO's failures and its
answers about the person. It refuses a request that is not the contract's form (`invalid_request`)
and a username that is not uppercased, and its user call refuses everyone (access denied) when the
`apiUuid` or `clientId` is not its own, as the SSO may for a wrong `HJV_API_SSO_CLIENT_ID` or
credential. `src/sso.rs`'s unit tests have the contract's answers (every form of `success`, the
token, `data[0]`, the pcode against the ID rule), and `tests/login.rs` the login's checks in order
through the fake SSO, among them an ID outside the rule (no SSO request at all), a denied person
for each reason, one person's bad value that never pauses the SSO calls, and a user call that
refuses everyone.

## Data

SQLite with `secure_delete`, the WAL truncated after deletions: the allowlist (case-folded
portal IDs, how each came, an import or a login, when, and since when the operator blocks
it, schema 5); accounts (the portal ID typed, case-folded, and the person's ID the SSO names, its
pcode uppercased; name, times, the account's password-less switch); token hashes, each with the
account key of its login; account keys (public key, the tag of the app's uuid, the device ID, the
platform, the server's device name, the device's own name, the app version, on or off campus, the
times of creation, login, confirmation and revocation, the key that confirmed it); the nonces of
signed requests for 20 minutes; devices (ID, OS, host name, public key, binding ID, times, and
from the PC's last sync: its time, the app version, whether it is switched off there, its mark of
the binding, and since when hbbs holds another key for it); released bindings (binding ID, PC ID,
the PC's public key, time, how it went); per account key, PC and UTC day the PCs' reports of
password-less use (latest time, count) and per PC, reason and day the counts of refused proofs;
the nonces of accepted syncs for 10 minutes; the change serial, the switches and the time of the
last switch change; login-limit counters under tags, and under tags too the counters of the
packages of new app versions (an address's failed requests, an account's and a PC's downloads, an
account's links); login, device and key records (tags, client address, result, time), the
records of the packages sent and links made (the account and the PC as tags, the client address as
its tag only, result, time), and since 1.8.0 the access password's counters (an address's failed
tries and tickets, under its tag) and records (`guest`: the client address as its tag only, result,
time), and since 1.8.2 the support codes' records (`code`: the tag of the account that made the
code, the client address of a try as its tag only, result, time), their tries counted in the
access password's counters. Since schema 3 (Wake-on-LAN, design v3 §10-1): per bound PC its wake report
(its wired cards' MACs, addresses, gateways and their MACs, names, drivers and wake settings, its
power facts, without its uptime), its summary and the PC's egress address; the NAS relays (ID,
public key, state, owner account, name, the source of the first hello, versions, interfaces,
egress address, times), the codes approved ahead, the relays' nonces for 10 minutes; the wake jobs
(the account and its tag, the PC, the relay, the grade, the network, the times, the result) with
their signed commands while they wait. Since schema 6 (Admin overview): per bound PC the client
address its last sync came from (X-Real-IP).

Kept: tokens 90 days unused / a year at most (a new login replaces the token of the same device,
and an account keeps at most 20); account keys while their token lasts (an hourly upkeep revokes
the keys no valid token is left for), then 30 days revoked; devices until removed or 180 days
unseen (a sync counts as seen), the address of a PC's last sync replaced by the next one's and
gone with the PC; released bindings 180 days; key use and refusals 30 days, and
they go with the PC when it leaves its account (key use also with its key); accounts until a year
unused, unless one of their PCs is in use (seen within the device time) and they are not
blocked, so that a lab PC logged in once stays listed while it is on; an account's allowlist
entry goes with it (also with `user delete`), an entry without an account (imported, never
used) a year after it came on the list, a blocked entry until it is unblocked; records six
months; limit counters for their longest
window, a day at least; backups of the database 14 days (`/var/lib/hjviewer-api/backup`, root
only); the journal namespace `hjviewer` 30 days. Of failed logins, at most 20 a minute are
recorded (the rest are counted in the daily summary of the journal), so that a flood cannot fill
the disk; successes are always recorded. A PC's wake report goes when it leaves its account; a
revoked relay keeps only its ID, key and times; a pending relay goes after 24 hours, an approval
ahead after 7 days; wake jobs, the wake record, six months (`HJV_API_EVENT_DAYS`), and an account's
go with it. No password, SSO token, e-mail, department, uuid or user name of a device is stored,
and the journal gets tags and counts only. Short links live in memory, ten minutes, never in the
database, and support codes too, as keyed hashes, twenty minutes at most (Support codes); no code
of a short link or of door 2 is stored or logged (the journal shows the first eight hex digits of
its tag, keyed with the log key), no support code or tag of one is stored or logged, and the
policy, the packages and door 2's file are the operator's files, which hjv-api only reads; so is
the App Review file, the one place of that account's ID and the hash of its password (the account
itself is kept as any other).

## Wake-on-LAN, where it is

Phase 2 plugged in as modules of its own: `src/wolinfo.rs` (the PCs' reports, the verdicts and
hints, the relays' interfaces, the grades), `src/relay.rs` (relay/PROTOCOL.md's server side and the
`relay` admin commands), `src/wake.rs` (the app's paths, the command key and commands, the choice
of relay, the jobs' ticker that `jobs.rs` runs every five seconds), routes in `api::route`, the
relays' envelope from `envelope` with the relay's `Spec`, tables through `db::MIGRATIONS` (schema
3), the PC's report through `pcsync` with the answer's `send_wol`. In front of hjv-api's own
limits nginx limits relay/hello per source (design v3 §5-8): that location of
deploy/nginx-hjviewer-api.conf and its zones file come with the packaging work (WP-B). relay/poll
needs no such limit for the nonce store, which hjv-api's own limits bound (above); one per source
there would only spare hjv-api the signature checks of a flood.
