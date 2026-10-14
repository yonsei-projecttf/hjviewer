# hjv-api

The HJViewer account API: login with a Yonsei portal ID through the university's SSO, and the
per-account list of Windows PCs that the apps show as "accessible devices" (design v2 §9). Since
design v3 every login also registers an account key of its device, which signs every account
request after it, and each bound PC's service syncs with hjv-api for its trust list: the account
keys that may open it without its password, signed with hjv-api's list key. Its phase 2 wakes a
sleeping PC of the account (Wake-on-LAN): the PC tells in its sync how to wake it, a relay on the
PC's LAN (hjv-relay on a NAS, relay/) long-polls hjv-api, and the app's wake request becomes a
command signed with hjv-api's command key, which the relay checks before it sends magic packets on
its own network. It runs next to hbbs/hbbr on the server, behind nginx, as a systemd service that
listens only on a Unix socket systemd holds. GNU AGPL-3.0-only, like the rest of the repository.

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
  switch nginx over. With `HJV_API_LOGIN=open` (the default) everyone the Yonsei SSO accepts can
  log in from then on, and no import is needed; with `list`, import the allowlist first (`allow
  import`), or nobody can log in.
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
- For tests and the E2E only: `hjv-api serve --listen 127.0.0.1:<port> --env-file <file>
  --credentials <dir> --db <file> --admin-socket <path> --operator-log <file>`.

## Settings

Not secret, from the environment (`EnvironmentFile=/etc/hjviewer/hjviewer-api.env`, or
`--env-file`). `hjviewer-api.env.example` lists every name with its default; only the three SSO
settings are required. Unknown `HJV_API_` names stop the start.

`HJV_API_LOGIN` (the user's decision of 2026-10-14) says who may log in: `open` (the default),
everyone the Yonsei SSO accepts (the SSO takes the university's faculty and staff only), a portal
ID that is not on the allowlist being put on it at its first login; or `list`, only the IDs on the
allowlist (imported, or registered by a login before), as at first (D3): an ID off it never
reaches the SSO and no login adds one, for an incident or a later decision, without a new build.
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

`HJV_API_CAMPUS_NETS` (the university's networks, such as `192.0.2.0/24,2001:db8::/32`) lets the
account's other devices see whether a new login came from on or off campus; the address itself is
never stored. `HJV_API_PASSWORDLESS_KINDS` is the connection kinds a device of the same account may
open without the PC's password (U3: `remote,file,camera`; `terminal` can be added; port forwarding
and RDP always ask for the password). The password-less switch is not a setting but state in the
database, off when installed (`hjv-api admin passwordless`).

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

The user makes them on the server, without an agent (v2 §10-3), for example:

```sh
systemd-ask-password -n "SSO client secret:" | sudo systemd-creds encrypt --name=hjviewer-sso-client-secret - /etc/credstore.encrypted/hjviewer-sso-client-secret
hjv-api keygen seal | tee /media/usb/hjviewer-seal.key | sudo systemd-creds encrypt --name=hjviewer-api-login-key - /etc/credstore.encrypted/hjviewer-api-login-key
sudo hjv-api keygen log-key --encrypt-to /etc/credstore.encrypted/hjviewer-api-log-key
sudo hjv-api keygen list --kid 1 --encrypt-to /etc/credstore.encrypted/hjviewer-api-list-key
hjv-api keygen list --kid 2 --out /media/usb/hjviewer-list-2.key     # offline, away from agents
sudo hjv-api keygen command --kid 1 --encrypt-to /etc/credstore.encrypted/hjviewer-api-command-key
hjv-api keygen command --kid 2 --out /media/usb/hjviewer-command-2.key   # offline, as list key 2
```

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
from standard input (`systemd-creds encrypt --name=hjviewer-api-command-key - …`), set
`HJV_API_COMMAND_KID=2`, restart; commands queued before go out as they were signed (the relays
take both keys), and later relay builds drop key 1.

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
                          list is put on it blocked
unblock <id|tag:T>        lift a block: the account may log in again (its devices log in again);
                          its entry counts as put on the list now
users | devices           accounts and the devices in them
key list [<id|tag:T>]     account keys: number, account, platform, the server's device name,
                          version, on or off campus, created, state (the device's own name and ID
                          with --reveal), and a line per PC that reported the key's password-less
                          use: the last time and the count of 30 days (U6; host name with --reveal)
key revoke <ckid>         revoke an account key with its token
unlock <id|tag:T|ip:A>    clear the login counters of a portal ID (with its PCs' and successes'),
                          of a tag from a journal warning (an account's or an address's), or of a
                          client address (an IPv6 address by its /64)
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
                          results of the last day and week, the limit counters, the account keys,
                          the switches and the change serial; the PCs holding their binding and
                          those synced in the last 10 minutes, those not synced since the last
                          switch change, needs-relogin, PCs by app version, refusals, released
                          bindings by how they went, password-less connections of 30 days; the
                          relays by state and each active relay's last poll, the PCs that reported
                          how to wake them, the wake jobs of the last day and week by result
selfcheck                 DNS, TCP and TLS to the SSO and hbbs's console, from inside the sandbox;
                          the fingerprints of the pinned list keys and command keys and whether the
                          keys in use are the pinned ones; the login mode
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
keygen seal|log-key|list|command
                          as hjv-api keygen (runs locally, not in the service)
```

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
whole file (nothing changes) and the answer names only its line number.

Blocking (the open login of 2026-10-14): a login puts every portal ID the SSO accepts on the
allowlist, so the way to keep an account out is a block, not taking its ID off the list. A blocked
login is told so (`이 계정은 관리자가 HJViewer 사용을 막았습니다. …`) only after the SSO accepted its
password, so that the answer tells nobody else who is blocked; its successes count against
`HJV_API_LIMIT_OK_ID`, so a known password cannot keep calling the SSO. An import does not lift a
block (an old file imported again lets nobody back in); only `unblock` does. Point with `tag:<tag>`
from `users` or `allow list`; an ID that never logged in has no tag known to the operator, so it
is blocked through a file (`allow remove <file>`).

`sudo hjv-api selfcheck --env-file /etc/hjviewer/hjviewer-api.env` runs the same check outside the
service, by hand (it compares the list key in use only where it can read the credentials:
`--credentials <dir>`, or in the unit); the unit's commented `ExecStartPost=` runs it with
`--log-only` after each start. `hjv-api admin selfcheck` checks the key the running service holds.

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
  changes) and a refused address told apart; and, with phase 2, active NAS relays silent for an hour. PCs that
  are off say nothing, so a night with every PC asleep is no alarm. All well: one line in the
  journal. A finding: an error line each, the findings in `/run/hjviewer-api-check/findings`, and
  the unit fails.
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
    sudo systemd-creds encrypt --name=hjviewer-api-telegram - /etc/credstore.encrypted/hjviewer-api-telegram
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
  names (docs/server-runbook.md 7-14); a binding released in the app is in the database only, so
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
limit and 502 for SSO errors. The one exception is a blocked account whose password the SSO
accepted: 403 with the blocked text (below), which only someone who knows the password sees. 429
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

A PC leaves its account in one place only, `db::release_binding`: its row goes, its binding ID
stays 180 days as a tombstone, and the change serial goes up, in one transaction, so that the
PC's next sync gets a signed `revoked` (design v3 §4-3). The signed logout from the PC, undo,
`DELETE /api/hjv/devices/{id}`, `binding revoke` (`device remove`), `user delete` and the upkeep's
deletion of unused accounts (their PCs first), and the upkeep's release of PCs unseen for 180 days
all go through it; `tests/pcsync.rs` checks that no other SQL deletes a binding.

## Formats

The formats the app (client patches 0030-0031), hbbs (server patch 0004) and hjv-api must agree
on, as design v3 §3-6, §4-1, §4-2 and §5-2 define them. Each is defined in one place in the code; the test
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
- An accepted sync of a bound PC stores its report (it also counts as the PC seen) and ends a
  `needs-relogin`; key use is merged by the larger values, so that a report cannot lower what is
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
 "list":{"kid":1,"l":"<base64 of the list bytes>","s":"<base64 signature>"}}
```

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

### hbbs and the SSO

- hbbs's `hjv-pk <id>` answer, a device key signed with hbbs's key pair: `src/hbbs.rs`.
- The SSO calls and how their outcomes are sorted: `src/sso.rs`.

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
change confirmed with hbbs, the report and selfcheck. `tests/relay.rs` has hjv-relay's requests:
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
`systemd-analyze verify` (skipped without it).
Their files and sockets go to the system's temporary directory (`/tmp` when `$TMPDIR` is long), so
that socket paths stay short whatever the target directory. Nothing calls the real SSO.
`hjv-fake-sso --listen 127.0.0.1:18600` serves the fake SSO for the E2E with fixed test accounts
(`2026000001`, `2026000002`, `Y9001`; passwords in `src/fake_sso.rs`) and usernames `sso-*` that
play the SSO's failures.

## Data

SQLite with `secure_delete`, the WAL truncated after deletions: the allowlist (case-folded
portal IDs, how each came, an import or a login, when, and since when the operator blocks
it, schema 5); accounts (portal ID as the SSO sends it, name, times, the account's
password-less switch); token hashes, each with the
account key of its login; account keys (public key, the tag of the app's uuid, the device ID, the
platform, the server's device name, the device's own name, the app version, on or off campus, the
times of creation, login, confirmation and revocation, the key that confirmed it); the nonces of
signed requests for 20 minutes; devices (ID, OS, host name, public key, binding ID, times, and
from the PC's last sync: its time, the app version, whether it is switched off there, its mark of
the binding, and since when hbbs holds another key for it); released bindings (binding ID, PC ID,
the PC's public key, time, how it went); per account key, PC and UTC day the PCs' reports of
password-less use (latest time, count) and per PC, reason and day the counts of refused proofs;
the nonces of accepted syncs for 10 minutes; the change serial, the switches and the time of the
last switch change; login-limit counters under tags; login, device and key records (tags, client
address, result, time). Since schema 3 (Wake-on-LAN, design v3 §10-1): per bound PC its wake report
(its wired cards' MACs, addresses, gateways and their MACs, names, drivers and wake settings, its
power facts, without its uptime), its summary and the PC's egress address; the NAS relays (ID,
public key, state, owner account, name, the source of the first hello, versions, interfaces,
egress address, times), the codes approved ahead, the relays' nonces for 10 minutes; the wake jobs
(the account and its tag, the PC, the relay, the grade, the network, the times, the result) with
their signed commands while they wait.

Kept: tokens 90 days unused / a year at most (a new login replaces the token of the same device,
and an account keeps at most 20); account keys while their token lasts (an hourly upkeep revokes
the keys no valid token is left for), then 30 days revoked; devices until removed or 180 days
unseen (a sync counts as seen); released bindings 180 days; key use and refusals 30 days, and
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
go with it. No password, SSO token, e-mail, uuid or user name of a device is stored, and the journal
gets tags and counts only.

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
