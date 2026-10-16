# Wave 1 notes (agent reports, 2026-10-16)

These are the wave-1 agents' own reports, kept for the next waves. Paths under build/wt/ were local worktrees; their work is now in this branch as commits (spec, hjv-api) and as patch files (server/patches 0006-0007, client/patches/rustdesk 0058-0059, client/patches/hbb_common 0003).

## SPEC

### summary

I wrote docs/access-tickets.md in Korean: the single spec that hjv-api (signer), hbbs 0006 (checker) and the 1.8.0 app (carrier) all implement against. One commit, 0707e18, also adds the test vectors and their generator. It is not pushed.

What the doc defines:
- **Ticket on the wire:** `t1.<kid>.<payload b64url>.<sig b64url>`, at most 1024 bytes, strict base64url, kid 1 to 65535. The signature is Ed25519 over `"hjv-ticket-v1" NUL` followed by the payload bytes.
- **Payload:** canonical JSON with the nine members v, who, use, sub, id, pk, to, iat, exp, in that order. The doc gives each member's type and encoding. There are six valid (who, use) pairs, a rule for how many entries `to` has, and a cap of 7 days on exp - iat.
- **Kinds:** a table of the seven ticket kinds, where each is obtained, and the 24 h / 72 h / 12 h lifetimes. The review ticket is limited by review.conf's PC= line and UNTIL.
- **Carriers:** PunchHoleRequest.token (field 5), RequestRelay.token (field 8), and RegisterPk field 1001 `hjv_ticket`, which hbbs reads as an unknown field following `get(1001)`.
- **Decision order and reason codes:** missing, oversize, malformed, unknown_kid, bad_signature, wrong_use, expired, not_yet_valid, revoked, wrong_pc, wrong_target. The doc spells out each check point:
  - punch hole: licence key, then minimum version, then ticket.
  - RequestRelay: ticket only.
  - RegisterPk: the key is stored, the answer is NOT_DEPLOYED, nothing else changes.
  - Registered connections are closed on the 10 s heartbeat when their ticket expires, is revoked or loses its key. UDP registration is refused while enforcing.
  - A MODE table covers off, count, enforce and closed.
- **Refusals:** answers carry TEXT containing the marker `access not allowed`, with the 0005 TEXT rules adapted. The doc gives the built-in bilingual text and the app's replacement texts in Korean and English, using 이용 비밀번호.
- **access.conf:** MODE, one or two KEY lines, NOT_BEFORE, TEXT. Parsing follows 0005's rules but fails closed. The doc includes key rotation and the emergency NOT_BEFORE.
- **Retention:** what hbbs, hjv-api and the app keep or log.

The vectors file (server/api/tests/vectors/access-tickets.json) holds:
- 84 ticket cases, each with the expected reason at each check point. They cover valid staff, both, bound-PC, guest-ctl, guest-pc and review tickets, plus boundary cases and every invalid class you listed.
- 7 RegisterPk and RendezvousMessage hex encodings that carry field 1001.
- 39 access.conf parse cases.

The test keys are repeated-byte seeds, also listed in scripts/public-test-keys.txt. The generator, access-tickets.py, uses only the standard library plus PyNaCl if installed, otherwise the openssl CLI. It checks every expected verdict against its own model of the doc. The verdicts were cross-checked against a strict Ed25519 implementation, rust-protobuf 3.7.2 and serde_json; all agreed.

### tests

After the commit, each once:
- `python3 -I -B server/api/tests/vectors/access-tickets.py --check`: OK. The file is byte-identical to what the script makes: 84 tickets, 7 RegisterPk messages, 39 access.conf files. PyNaCl is not installed, so it used the openssl backend (OpenSSL 3.0.13, after its RFC 8032 self-test). Every hand-written expected decision agrees with the script's model of the doc.
- `scripts/leak-check.py --base main`: no hits in 483 tracked files, the staged changes, or the 1 new commit.
- `scripts/test_check_build_keys.py`: 47 tests OK, 1 skipped (the build/ci hjv-release target is missing). This covers the new lines in public-test-keys.txt.

Scratch interop checks, outside the repo (scratchpad only, offline crates from the local registry):
- All 64 signature verdicts made through OpenSSL match the strict pure-Python Ed25519 in tools/hjv-release/tests/vectors/generate.py. The 14 valid tickets re-sign to the same bytes.
- rust-protobuf 3.7.2:
  - A RegisterPk with `string hjv_ticket = 1001` declared last encodes exactly to the vector's register_pk_hex.
  - `unknown_fields().get(1001)` on all 7 messages behaves as the spec says: last length-delimited value wins, a varint makes it malformed, empty or absent means missing.
- serde_json 1.0.81 and serde 1.0.229 (hbbs's lock versions), with `deny_unknown_fields`, re-serialize and byte compare: same canonical verdict as the generator on all 80 decodable payloads.

No cargo builds of hbbs, hjv-api or the client, and no E2E: this package has no Rust code. The main worktree and both upstream trees were not touched (main is still 5b6546a and clean).

### open

- Deviation from the brief: at RequestRelay hbbs checks only the ticket, not licence_key. The upstream client's request_relay never sets licence_key on the RequestRelay it sends to hbbs, so checking the key would refuse every relay. The brief said 'key and ticket'.
- `to` is a JSON array, not the brief's string: empty for staff and pc, exactly one entry for a guest controller, 1 to 3 for review. With an array the review account holds one ticket for all its demo PCs, and the app never needs to know which ticket goes with which target.
- Skew: 60 s tolerance on iat only, none on exp (exp > now strictly). The brief said no slack at all; the task asked for a skew. Please confirm.
- New rules not in the brief, please confirm: (1) hbbs refuses exp - iat > 7 days as malformed, so hjv-api must refuse lifetimes over 168 h. (2) When MODE is count or enforce, the NOT_BEFORE line must be present (empty means none), as a guard against a truncated file, like 0005's FROM=. (3) TEXT is optional, with a built-in Korean and English default that is also used in the closed state. (4) With MODE=off the values of other lines are not read, but line syntax and counts are.
- Fail-closed details: an access.conf that disappears or breaks while hbbs runs keeps the last good rule. A missing or broken file at start means closed (everything refused). Switching off to enforce drops, within 10 s, PCs that registered while off (no ticket on record). That may cut their sessions, so do it in a quiet hour.
- Client caution: in upstream client.rs a non-empty token sets legacy_secure. The punch hole then skips the wait for the UDP NAT test. secure_tcp is a no-op on WebSocket. The C1 package should check whether this hurts connection quality.
- The app texts A1, A2 and P1 in doc 5-3 (Korean and English, using 이용 비밀번호) are proposed wording. The user may want to review them before the wave-2 UI.
- The doc is not yet linked from docs/architecture.md or the Test vectors section of server/api/README.md. That belongs to the docs package (brief section 6).
- The worktree build/wt/staff-spec already existed on branch staff-only at main 5b6546a when I started. I used it as it was.

### hooks_for_wave2

- hjv-api POST /api/hjv/guest: issue guest/ctl tickets with to=[that target] (exactly one) and guest/pc tickets whose id and pk are checked by device signature and, when hbbs knows the ID, by hjv-pk. Lifetime 12 h, sub empty. Give the same 401 for a wrong password and for a closed guest file. Keep guest records with the address tag only, for 6 months. Seal the password with the purpose field `guest`.
- review.conf PC= line: review/ctl ticket with to = the PC= IDs in file order (1 to 3) and exp = min(iat + 24 h, UNTIL). Issue no ticket when the PC= line is missing or UNTIL has passed. hbbs already refuses a review ticket with an empty to.
- Every hjv-api ticket answer should carry exp and server_time beside the ticket string, since the app never parses the ticket. hjv-api should refuse to start with any lifetime over 7 days. The format for this is doc 2-4.
- App UI (doc 5-3): treat a refusal as an access refusal when the lowercased text contains 'access not allowed'. Put this check next to hjviewer_update::on_refusal, and map RelayResponse.refuse_reason the same way. Show the texts A1 (dialog with [연세포탈 로그인] [이용 비밀번호로 연결] [취소]), A2 (logged-in, still refused after one renewal) and P1 (PC home and the quick-support window on NOT_DEPLOYED). Keep the marker at the end of A1 and A2 so auto-reconnect stays off.
- Client storage (doc 7): the staff or review ticket lives in LocalConfig and is cleared at logout; the bound PC ticket lives in the service config folder. Guest tickets stay only in memory: a target-to-ticket map for controllers, and for a guest PC the app's memory or the service's via IPC. Port forwarding re-reads the ticket on every local connection. A held ticket stays in use until its exp while hjv-api is down.
- hbbs ops: console line `HJVACCESS1 mode=<off|count|enforce|closed> <ph|rr|reg|udp>.<reason>=<n>`; log lines limited to 30 per 10 s and never containing the ticket, sub or TEXT; option --hjv-access-file / HJV_ACCESS_FILE. Still to write: deploy/access.conf.example (comments on whole lines only), a runbook section, and a deploy or selfcheck step that refuses public test keys (now in scripts/public-test-keys.txt) in a production access.conf or ticket key.
- Tests: hbbs tests/access.rs and the hjv-api ticket tests should read server/api/tests/vectors/access-tickets.json, and may pin its SHA-256. Each decision function should take now and the rule as arguments. The E2E access stage (HJV_E2E_TICKETS) and tools/hjv-load will need tickets once enforce is on.

## S: build

### summary

I wrote hbbs patch 0006 in the server tree: commit c44e975 on branch staff-only (worktree build/wt/staff-server), exported as build/wt/staff-server-patch/0006-hbbs-admit-only-holders-of-an-access-ticket-off-by-d.patch. It follows docs/access-tickets.md exactly, and all its tests pass.

**Code:**
- A new module, src/hjv_access.rs.
- Thin hooks in rendezvous_server.rs.
- PeerMap::store_pk in peer.rs: update_pk without the address and the registration time.
- The --hjv-access-file option in main.rs.
- Visibility only in 0005's module (read_file/Seen, parse_from, head and the TEXT helpers are now pub(crate)), so they are reused instead of copied.

**Ticket check (4-1):**
- Order: size, form (t1, kid 1-65535, strict base64url), the key the kid names, the libsodium Ed25519 signature over `hjv-ticket-v1` NUL and the payload.
- Only then the payload: serde with deny_unknown_fields, re-serialized and compared byte for byte, then the six kinds and their `to` counts and the 7-day lifetime.
- Then use, exp, iat (+60 s), NOT_BEFORE, and wrong_pc or wrong_target.
- Field 1001 is read through `special_fields.unknown_fields().get(1001)`.
- The decision functions take now and the rule as arguments.

**Check points:**
- **Punch hole:** after the key and the minimum version. A refusal is other_failure = TEXT, and the peer never hears of it.
- **RequestRelay:** before the peer lookup. A refusal is RelayResponse.refuse_reason = TEXT on the same connection.
- **RegisterPk over TCP/WS:** the checks of old pass, then the key is stored. Without a good ticket the answer is NOT_DEPLOYED (keep_alive 30), and the ID's address, signaling channel and registration time stay untouched. hjv-pk then answers the stored key with age `-`.
- **Admitted registrations:** the connection keeps the ticket's kid, iat and exp. Under enforce the 10 s heartbeat closes it on expiry, NOT_BEFORE, a removed key, or no kept ticket. A newer ticket registered on the same connection extends it.
- **UDP:** registrations are dropped unanswered while enforcing or closed; test_hbbs's probe is the one exception.

**access.conf:**
- Re-read every 10 s like 0005's file, but fail-closed: with the option and no usable file at the start, everything is refused (built-in bilingual text). A file that breaks or disappears later leaves the last good rule in force, with an hourly error.
- MODE=off, or no option, leaves the wire behaviour exactly as before.
- count decides and counts without refusing; enforce refuses.

**Ops:**
- The console command `hjv-access` answers `HJVACCESS1 mode=<off|count|enforce|closed> <ph|rr|reg|udp>.<reason>=<n>`.
- Refusals and closings are logged with the address, point, quoted ID and reason, at most 30 lines per 10 s. No ticket, sub or TEXT ever reaches the log.

**Tests:**
- 18 unit tests read the copied vectors, pinned by SHA-256 or taken from HJV_ACCESS_VECTORS:
  - all 84 tickets and their checks under each mode
  - all 7 RegisterPk byte strings
  - all 39 access.conf cases
- tests/access.rs has 18 tests that run real hbbs processes, with tickets signed at run time by the vectors' test keys. One cross-check: the test signer reproduces the vectors' staff-ctl ticket byte for byte.

### tests

After the commit, once, the CI server stage's command: `cargo test --release --locked --lib --bins --tests`. The log is build/wt/staff-server-patch/0006-server-tests.log; its header names commit c44e975, tree c16507bd, 0 status lines, hbb_common 229b904 and rustc 1.99.0, and it ends `EXIT 0`.
- lib: 45 passed (27 existing + 18 new hjv_access)
- hbbr: 5
- tests/access.rs: 18 (new, 32 s)
- hjv_pk: 4
- min_version: 16
- protobuf_recursion: 1
- relay_pairing: 1
- tcp_signaling: 2

All existing server tests pass unchanged.

During development, before the commit:
- **Debug run of the whole suite**: all passed.
- **Mutation checks**: each mutation made at least one test fail:
  - signature check off
  - skew 61 s
  - varint field 1001 read as missing
  - canonical-JSON check off
  - NOT_BEFORE line not required
  - no reply to test_hbbs's probe (hbbs then exits after about 26 s, and the probe test fails)
  - heartbeat close off (5 tests fail)
  - refused registration taken online
  - relay check removed
- **Formatting and lint**: rustfmt-clean new files. My hunks in the edited files add no rustfmt drift. Clippy shows 90 warnings with or without the change, none in my files.

After the commit:
- **Patch file**: `git apply` on a clean copy of the ee2081c tree reproduces c44e975 exactly (diff -r empty).
- **Canonical vectors**: the unit tests pass with HJV_ACCESS_VECTORS pointed at the spec worktree's file. A one-byte-altered copy is refused by the SHA-256 pin.

Not run: no E2E, no scripts/ci.sh stages, no client builds. Nothing contacted the production server or the network. The hbb_common submodule was checked out from the local clone, and the shared git config is unchanged.

### open

- Author name: I committed as hjviewer <haejin.cho@yonsei.ac.kr>, not ysfaculty as the task text said. The user's commit-identity memory says patch headers carry `hjviewer`, and server patches 0001-0005 all do. If ysfaculty is wanted: `git -c user.name=ysfaculty -c user.email=haejin.cho@yonsei.ac.kr commit --amend --reset-author --no-edit`, then run format-patch again.
- Deviation from 4-5 (UDP): while enforcing, or closed, hbbs still answers test_hbbs's own probe, a UDP RegisterPeer with the ID `(:test_hbbs:)`. The answer is RegisterPeerResponse request_pk=true, what an unknown ID gets; the peer map is not touched and the probe is not counted. Without this, hbbs stops itself after about 26 s whenever TEST_HBBS is not `no`. The systemd unit sets it, but the Docker deploy paths do not. A test covers this.
- Gap in the spec's console format: two choices made here.
- Closings at the heartbeat count under `reg.<reason>`, with the same names as refused registrations: missing, expired, revoked, unknown_kid, closed. The log tells them apart (`access closed reg ...` against `access refused reg ...`).
- A UDP registration dropped while enforcing counts as `udp.ignored`, a 14th reason name not in 4-1. While closed it counts as `udp.closed`.

Wave 2 may prefer a separate point, such as `drop.`.
- The generator's model differs from 6-2 on one case. hbbs refuses a non-comment line without `=` (a bare `KEY`, `NOT_BEFORE` or `TEXT`), also under MODE=off, as 6-2 (NAME=value) and 0005 do. access-tickets.py's parse_conf instead takes a bare name as that name with an empty value: it accepts `MODE=off\nKEY\n` and reads a bare `NOT_BEFORE` as empty. No vector covers this; the spec package may want to align the model or add a vector.
- Unlike 0005, the access check does not depend on the server key check (-k). With KEY= empty, hbbs still enforces: TEXT holds no secret, and access control must not switch itself off.
- Without the option, `hjv-access` answers `HJVACCESS1 mode=off`, the same as MODE=off. Tools tell the two apart by `hbbs --help`, which lists --hjv-access-file, or by the `access off` log line. The console's `h` list gains a line (hjv-access) even without the option; nothing on the wire changes.
- Count mode (fail-closed reading of 4-6): a registration whose ticket is not ok goes online, but its connection then keeps no ticket, even if an earlier ok one was kept. Switching to enforce therefore closes it.
- For the R (review) package: upstream's key rules still apply to refused registrations, as 4-4 says. A client that knows a PC's uuid and comes from the same IP can register without a ticket and replace the stored key (store_pk). The admitted connection stays the signaling channel, but controllers get a key the real PC does not have until it registers again. Before 0006 the same client could take over the whole channel. A rule that a registration without a good ticket never changes an online peer's key would close this.
- Cost: every punch hole, relay request or registration whose kid is known now costs one Ed25519 verification (libsodium) before any lookup. Punch holes still have no rate limit, as before. This should be fine: an unknown ID already costs a database lookup.
- The SPEC package's still-unconfirmed choices are implemented as written. If the user changes any, hbbs's constants and parser change together with the vectors:
- 60 s skew on iat only, none on exp
- exp - iat over 7 days is malformed
- NOT_BEFORE line required under count and enforce
- TEXT optional, with the built-in text
- Copied vectors: tests/vectors/access-tickets.json is pinned by SHA-256 (7b9fa3fd…). When the spec's vectors change, both the copy and VECTORS_SHA256 in src/hjv_access.rs must change.

### hooks_for_wave2

- scripts/ci.sh server stage: run the hbbs tests with HJV_ACCESS_VECTORS="$ROOT/server/api/tests/vectors/access-tickets.json", so the pinned copy in the server tree is checked against hjv-api's file. Also require tests/access.rs once server/patches contains 0006, as is done for hjv_pk.rs.
- E2E and ci.sh detection: `hbbs --help` lists `--hjv-access-file` (as with 0005's option). The env name is HJV_ACCESS_FILE. Log lines to grep:
- `access <mode>: keys 1,2[, not before <UTC>]`
- `access refused <ph|rr|reg> <addr> for "<id>": <reason>`
- `access closed reg <addr> for "<id>": <reason>`
- `access file <path>: <why>; <kept rule or 'access closed: every request is refused'>`
- `access refused or closed N more in the last 10 seconds`
- Deploy and runbook:
- Write access.conf with MODE=off before adding HJV_ACCESS_FILE to hjviewer.env. The option with a missing or broken file means closed: every controller is refused and every PC gets NOT_DEPLOYED.
- deploy/access.conf.example should use the order MODE, KEY, KEY, NOT_BEFORE, TEXT, with whole-line comments only.
- After switching to enforce, PCs that came in while off are closed within 10 s of the change taking effect, which is up to 20 s after the file is written.
- App (C1):
- Write field 1001 as wire type 2 after the known fields: key bytes ca 3e, as the vectors and tests/access.rs's register_pk() do.
- On NOT_DEPLOYED, keep the connection and register again on it as soon as a ticket arrives.
- Renew on the same connection before half the ticket's lifetime has passed.
- A heartbeat close arrives as a plain close with no message, so re-register on a new connection with the current ticket.
- Refusals carry `access not allowed` in PunchHoleResponse.other_failure and RelayResponse.refuse_reason. The built-in text is in BUILTIN_TEXT of src/hjv_access.rs (doc 5-3).
- hjv-api: a PC refused for its ticket still has its key in hbbs. `hjv-pk` answers the signed key with age `-`, so binding and PC sync (key only) work, while wake, jobs and check, which go by age, see the PC offline.
- Tickets for E2E and tools/hjv-load once enforce is on: the vectors' test keys sign valid tickets, seed 0x74 for kid 1 and 0x75 for kid 2. tests/access.rs's Ticket::sign is a 20-line reference for a test-only signer.

### S: review (good)

- [minor] src/rendezvous_server.rs:1463 (check_ip_blocker inside handle_register_pk, which runs before the ticket verdict from :567 is applied): Under enforce, PCs without a good ticket keep using up the per-address registration budget that ticket holders behind the same address need. Before 0006 a PC on the WS path sent one RegisterPk per connection. Now every refused PC re-sends one every 30 s for as long as it runs. In the upstream app (client tree 398fa05), NOT_DEPLOYED sets key_confirmed false (rendezvous_mediator.rs:527), start_tcp re-registers while it is false (:637), and register_pk throttles that to DEPLOY_RETRY_INTERVAL, 30 s (:1251). Every retry passes check_ip_blocker (:1013-1041). That limiter blocks an address after about 31 registrations with no 60 s pause, and blocked attempts do not restart the window. One refused PC alone on an address therefore blocks it for 60 s every ~16 min, and ten behind one NAT block it ~40% of the time. During a block, a bound, staff or guest PC with a valid ticket at that address gets TOO_FREQUENT and comes online up to a minute later. hjv-access never counts TOO_FREQUENT, so the operator cannot see this. The same happens after a NOT_BEFORE emergency, when all bound PCs retry with revoked tickets until their next sync. Contract 4-4 step 1 keeps 주소 차단 unchanged, so S is as specified: this is a gap in the contract, not a coding error. Fix: Give registrations refused for their ticket a budget of their own: pass `online` into check_ip_blocker and key IP_BLOCKER by address plus a no-ticket marker (e.g. `format!("{ip}/noticket")` when !online). With MODE=off, count or no option, online is always true, so nothing changes. Under enforce, unticketed traffic stays limited (the 300-IDs-a-day cap still bounds the DB) but cannot lock out ticket holders. Add one sentence to docs/access-tickets.md 4-4. Add a tests/access.rs case: 32 refused registrations from one address, then a ticketed registration from the same address answers OK.
- [minor] src/rendezvous_server.rs:1530-1534 (`else if changed { self.pm.store_pk(...) }`), src/peer.rs:146: Confirms the author's open note. A client without a ticket that knows a PC's uuid and comes from the same public address passes the old checks: the ip/pk mismatch check at :1473 fires only on a different address. store_pk then overwrites the stored pk (and info.ip) of a PC that is online through an admitted connection. That connection stays the signaling channel, but controllers now get a key the PC does not hold, and connections fail until the PC registers again (with renewal at half-life, up to 12-36 h). hjv-pk also hands that key to hjv-api, which trusts it when binding an ID to an account. So someone with a staff login and the victim's uuid could bind the ID and then register as it with a valid ticket. This is narrower than before 0006, when the same client took the whole channel, and 4-4 says to store the key. It needs a contract decision (R package), not a code fix alone. Fix: In handle_register_pk, when !online and the stored peer has a live tcp_tx (online through an admitted connection), skip store_pk if rk.pk differs, and answer NOT_DEPLOYED with nothing stored. Keys of offline or new IDs are still stored, so first-time binding keeps working. Record the rule in 4-4. Add a tests/access.rs case: same uuid and address, OTHER_PK, no ticket → hjv-pk still answers PC_PK.
- [minor] src/hjv_access.rs:204-208 (Admitted keeps kid, iat, exp) and :526 (lapse only checks that the kid is still listed): Replacing a key under the same kid does not close what the old key let in. Say key 1 is stolen and the operator writes `KEY=1:<new key>`, rather than adding kid 2 and removing kid 1 as 6-4 says. Registrations admitted with forged kid-1 tickets then stay online until their own exp: up to 7 days, chosen by the attacker. Nothing is logged. Only a NOT_BEFORE line would close them. Fix: Keep the 32-byte public key (not a secret) in Admitted. In lapse, close with unknown_kid when `rule.key(admitted.kid) != Some(&admitted.key)`. Add a case to an_admitted_registration_lapses_with_its_ticket. In contract 4-4/7, "kid, iat, exp" becomes "kid, key, iat, exp".
- [minor] src/rendezvous_server.rs:591 (`conn.hjv_ticket = access.admitted()` inside `if res == OK`), tests/access.rs: Test gap on 4-4's "같은 연결의 앞선 ok 등록도 그대로 둡니다". No test has an admitted connection send a refused re-registration (missing ticket, or another PC's ticket) and then survive a heartbeat. Reading the tests, moving line 591 out of the `if res == OK` block would make no test fail. That change would let a failed renewal clear the kept ticket, so the next heartbeat closes the PC with `missing`. Fix: Add a tests/access.rs case. Admit a PC, then re-register on the same connection with no ticket and with Ticket::pc(OTHER_ID); both answer NOT_DEPLOYED. Then expect_silence_for(PERIOD + 1 s), and check that a staff punch hole still reaches the PC.
- [minor] src/hjv_access.rs:718-722 (Access::start uses get_arg and treats a blank value as no option): An empty HJV_ACCESS_FILE silently means "off". get_arg cannot tell unset from set-but-blank, so a `HJV_ACCESS_FILE=` line in hjviewer.env (a template left blank, or a deploy step that failed to fill it in) turns the gate off without a word. The module's own rule (Watcher::look: "the way to turn the check off is MODE=off, not taking the file away") and contract 6-3 treat an option with no usable file as closed. Fix: Use crate::common::get_arg_opt(OPTION). For Some(blank), log an error ("--hjv-access-file is set but empty; access closed: every request is refused") and start in Policy::Closed without a watcher, or at least log the error. Keep None as off. Add a unit or integration case.
- [minor] report hooks_for_wave2 (C1: "On NOT_DEPLOYED, keep the connection and register again on it"); src/rendezvous_server.rs:491-492 and :1390: The C1 hook says more than hbbs guarantees. hbbs sends no heartbeat to a connection without an admitted registration. It closes that connection once 30 s pass without a frame (TCP_IDLE_TIMEOUT_MS, checked every 10 s), and at once if the client sends an empty frame (handle_tcp returns false when peer_id is None). The upstream retry throttle re-sends RegisterPk exactly every 30 s, which races that timeout. A renewal can also be answered TOO_FREQUENT (first finding), which leaves the old ticket in force until exp. Fix: Correct the hook for C1. After NOT_DEPLOYED, expect hbbs to drop the idle connection and reconnect. Never send empty frames on it. Send the ticketed RegisterPk on whichever connection is current. Retry a renewal answered TOO_FREQUENT well before exp.
- [minor] docs/access-tickets.md (staff-spec 0707e18) sections 3-2, 4-5, 5-2/6-1, 7; code at src/rendezvous_server.rs:395-411, src/hjv_access.rs:117-118 and :817: Choices made in S that the contract does not state, plus one sentence that is not exactly true. (1) 4-5: under enforce and closed, hbbs still answers test_hbbs's UDP probe `(:test_hbbs:)`. Without that, hbbs exits whenever TEST_HBBS is not `no` (only the systemd unit sets it). (2) 7: a 14th reason, `ignored` (udp.ignored), and heartbeat closings counted under `reg.<reason>` in the same counter as refusals. (3) 3-2 says a field 1001 in any other wire type is malformed. That holds only for varint and fixed fields: rust-protobuf 3.7.2 skips a group-typed field without storing it (rt/unknown_or_group.rs:44), so a group plus a length-delimited value reads as that value. Harmless, since the value is still signature-checked. (4) 5-2/6-1: unlike 0005's TEXT, the access TEXT reaches anyone, because RequestRelay is refused before any key check (4-3). 0005 requires -k precisely because its TEXT may carry a link meant for app users only. Fix: In the spec package (D/wave 2), add these lines to docs/access-tickets.md. Have 3-2 say the get(1001) result is what counts. In 5-2 and the runbook, add "TEXT is shown to anyone; put nothing in it that is meant for app users only."
- [minor] staff-spec server/api/tests/vectors/access-tickets.py:544 (`name, _, value = line.partition("=")` in parse_conf): Confirms the author's note. The vectors' model takes a line without '=' (`KEY`, `NOT_BEFORE`, `TEXT`) as that name with an empty value. It therefore accepts `MODE=off\nKEY\n` and reads a bare `NOT_BEFORE` as "none". 6-2 and hbbs (hjv_access.rs:603, unit test a_line_without_its_equals_sign_is_an_error) refuse such a line. No vector covers this, so hjv-api or a tool built on the model could accept a file that hbbs refuses. For hbbs that fails closed, but it would confuse whoever wrote the file. Fix: In parse_conf, return an error for a non-comment line without '='. Add vectors such as bare-key-under-off and bare-not-before (expect error). Then update tests/vectors/access-tickets.json and VECTORS_SHA256 in S together.

## S: fixes

### summary

I checked all 8 review findings against the code; every one holds. I applied the five code findings (1-5) in commit 349b8dd, `hbbs: review fixes`, and exported it as 0007-hbbs-review-fixes.patch. I corrected the C1 hook text (finding 6). I passed findings 7 and 8 on to the spec package, because they change files in its worktree. All server tests pass once after the commit.

Applied:
1. **IP blocker budget.** Registrations without a good ticket now have their own per-address budget. In `handle_register_pk` the blocker key is `<ip>/noticket` when `!online`. With no option, MODE=off or count, `online` is always true, so nothing changes. Old apps refused with NOT_DEPLOYED retry every 30 s, which could get the address blocked, so ticketed PCs behind the same NAT got TOO_FREQUENT. Test `refused_registrations_leave_the_address_budget_of_ticket_holders_alone`: unticketed registrations until TOO_FREQUENT, then a ticketed one from the same address answers OK while the unticketed ones stay blocked.
2. **Key overwrite.** An unticketed registration no longer stores anything for an ID that is online through a ticketed TCP/WS connection. The check is `else if changed && peer.read().await.tcp_tx.is_none()`. Keys of offline and new IDs are stored as before, so first-login binding still works. The existing push-out test gains a case: same uuid and address, OTHER_PK, no ticket gives NOT_DEPLOYED, and hjv-pk still signs PC_PK.
3. **Key replaced under its kid.** `Admitted` now keeps the public key its kid named (it is not secret). `lapse` closes with `unknown_kid` when `rule.key(kid) != Some(&key)`, so a stolen key replaced in place closes what it let in. The unit test covers a replaced key, and that a kept key goes with its number, not with any line.
4. **Test gap.** New `a_refused_registration_leaves_its_connection_as_it_was_admitted`: an admitted PC re-registers with no ticket and with another PC's ticket, gets NOT_DEPLOYED twice, survives a heartbeat, and a staff punch hole still reaches it. Without this, moving the `hjv_ticket` assignment out of `if res == OK` broke no test.
5. **Blank option.** `Access::start` uses `get_arg_opt`, so a set-but-blank HJV_ACCESS_FILE or --hjv-access-file means closed, not off. The watcher logs `access file : --hjv-access-file (HJV_ACCESS_FILE) is set but empty; access closed: every request is refused` and repeats it hourly. There is a unit test and an integration test (`a_blank_option_refuses_everything_too`).

Other findings:
- **Finding 6:** the corrected C1 hook is below.
- **Findings 7 and 8:** declined here and passed to wave 2. They touch the staff-spec worktree's contract doc and vector generator.

### tests

After the commit, once: the CI server stage's command, `cargo test --release --locked --lib --bins --tests`, with the warm target dir build/wt/minver-server/target. The log is build/wt/staff-server-patch/0007-server-tests.log. Its header names commit 349b8dd, tree 264d920, 0 status lines, hbb_common 229b904 and rustc 1.99.0, and it ends `EXIT 0`.
- lib: 46 (one new)
- hbbr: 5
- tests/access.rs: 21 (three new, 31.8 s)
- hjv_pk: 4
- min_version: 16
- protobuf_recursion: 1
- relay_pairing: 1
- tcp_signaling: 2

No hbbs process or temp directory was left behind.

Before the commit:
- **Targeted runs:** only the new and changed tests: the hjv_access unit tests and 4 tests from tests/access.rs.
- **Mutation checks:** I undid each fix in turn and ran its test. Each test failed, and the tree was restored byte for byte afterwards.
  - m1, one address budget: ticketed registration got TOO_FREQUENT, not OK.
  - m2, store_pk unconditional: hjv-pk returned OTHER_PK.
  - m3, lapse checks only the kid: a replaced key was not closed.
  - m4, `conn.hjv_ticket` set outside `if res == OK`: hbbs closed the connection at the heartbeat.
  - m5, `get_arg` with a blank value meaning off: no closed line.
- **rustfmt:** clean for src/hjv_access.rs and tests/access.rs. rendezvous_server.rs keeps its 7 pre-existing drift hunks, none new.
- **Clippy:** one run, `--lib --tests -- -A clippy::unused_io_amount`, to get past upstream's deny-level lint. No lint in hjv_access.rs, tests/access.rs or the changed lines of rendezvous_server.rs.
- **Compiler warnings:** none in new code; all are pre-existing upstream ones.

After the commit:
- 0006 still equals `git format-patch --zero-commit --no-signature` of c44e975.
- Applying 0006 and 0007 to ee2081c in a temporary index gives tree 264d920, the same as HEAD.

Not run: E2E, ci.sh stages, client builds. The 1.75 check applies to the client only. Nothing contacted the network or the production server. The main worktrees are untouched.

### open

- Commit name: I kept `hjviewer <haejin.cho@yonsei.ac.kr>`, not ysfaculty as the task text says. The user's commit-identity memory says patch headers use `hjviewer`. c44e975 and server/patches 0001-0005 do the same, and the reviewer confirmed it.
- Finding 2 goes slightly beyond the reviewer's fix. The reviewer suggested skipping the store only when the pk differs. I store nothing for an unticketed registration of an ID whose signaling channel is a live TCP/WS connection (`peer.tcp_tx.is_some()`). Under enforce that means admitted. Why: the narrower rule still let a same-pk registration from another IP move info.ip, and that later lets the attacker swap the key from their own IP once the PC is offline. Contract 4-4 step 2 says 'key storage unchanged', so this is a contract change. The R package and the user should confirm it, and the spec should record it.
- Still open for R (upstream's uuid trust model): an OFFLINE ID's key can still be replaced by any client that knows its uuid and comes from its last IP. hjv-api then trusts that key when it binds the ID to an account. This cannot be closed without breaking the first-time binding that 4-4 requires.
- Still open after finding 1: unticketed registrations still share one budget per address with each other. A new 1.8.0 PC waiting for its first binding behind a NAT full of refused old PCs may get TOO_FREQUENT for a minute at a time before its key is stored. Until then hjv-api's bind sees no key (check_failed). This was no better before the split.
- Patch series: the review fixes are a separate commit and patch (0007, header [PATCH]), as the task asked. The integrator can squash them into 0006, or regenerate the server series with scripts/export-patches.sh (then [PATCH n/7]). Both patch files use --zero-commit --no-signature, like server/patches.
- Findings 7 and 8 were declined in S, not fixed. They change docs/access-tickets.md and server/api/tests/vectors/access-tickets.py in the staff-spec worktree, which this package must not edit. They are listed in hooks_for_wave2.

### hooks_for_wave2

- C1 (app), corrected per finding 6:
- Write field 1001 as wire type 2 after the known fields, key bytes ca 3e, as the vectors and tests/access.rs register_pk() do.
- After NOT_DEPLOYED, do not count on keeping the connection. hbbs sends no heartbeat on a connection without an admitted registration. It closes that connection once 30 s pass without a frame from the client (checked every 10 s), so the upstream 30-s retry races it. It closes it at once on an empty frame from the client.
- So: never send empty frames on an unadmitted connection, expect the drop and reconnect.
- As soon as a ticket arrives, send the ticketed RegisterPk on whichever connection is current, without waiting for the 30-s throttle.
- Renew on the admitted connection before half the ticket's lifetime has passed.
- A renewal answered NOT_DEPLOYED or TOO_FREQUENT leaves the old ticket in force until its exp, and the connection stays admitted. Retry well before exp.
- A heartbeat close arrives as a plain close with no message: register again on a new connection with the current ticket.
- Refusals carry `access not allowed` in PunchHoleResponse.other_failure and RelayResponse.refuse_reason. The built-in text is BUILTIN_TEXT in src/hjv_access.rs (5-3).
- Spec (D), lines for docs/access-tickets.md (staff-spec 0707e18):
- **3-2:** the result of get(1001) is what counts. protobuf 3.7.2 skips a group-typed field 1001 without storing it, so only varint and fixed values make it malformed.
- **4-4 step 1:** registrations without a good ticket count against their own per-address budget (IP blocker key `<ip>/noticket`).
- **4-4 step 2:** nothing is stored for an ID that is online through a ticketed TCP/WS registration. Keys of offline and new IDs are still stored.
- **4-4 and 7:** the connection keeps kid, key, iat and exp. A key replaced under its kid closes the connection like a removed one (unknown_kid).
- **4-5:** under enforce and closed, hbbs still answers test_hbbs's UDP probe `(:test_hbbs:)` with RegisterPeerResponse request_pk, uncounted.
- **5-2 / 6-1 and the runbook:** TEXT is shown to anyone, because RequestRelay is refused before any key check. Put nothing in it that is meant for app users only.
- **6-3:** an option set but blank is closed, with an hourly error.
- **7:** the 14th reason `ignored` (udp.ignored) and `closed` (udp.closed). Heartbeat closings count under reg.<reason> with the refusal names.
- Spec (D), vector generator per finding 8: parse_conf in server/api/tests/vectors/access-tickets.py must refuse a non-comment line without '=', as 6-2 and hbbs do. Add vectors such as a bare KEY under MODE=off and a bare NOT_BEFORE, both expecting an error. Then update S's tests/vectors/access-tickets.json and VECTORS_SHA256 in src/hjv_access.rs (now 7b9fa3fd...) together.
- scripts/ci.sh server stage:
- Run hbbs's tests with HJV_ACCESS_VECTORS="$ROOT/server/api/tests/vectors/access-tickets.json", so the pinned copy is checked against hjv-api's file.
- Require tests/access.rs once server/patches contains the access patch(es), as is done for hjv_pk.rs.
- E2E and ci.sh detection: `hbbs --help` lists `--hjv-access-file`, and the env name is HJV_ACCESS_FILE. Log lines to grep:
- `access <mode>: keys 1,2[, not before <UTC>]`
- `access refused <ph|rr|reg> <addr> for "<id>": <reason>`
- `access closed reg <addr> for "<id>": <reason>`
- `access file <path>: <why>; <kept rule or 'access closed: every request is refused'>`
- for a blank option: `access file : --hjv-access-file (HJV_ACCESS_FILE) is set but empty; access closed: every request is refused`
- `access refused or closed N more in the last 10 seconds`
- Deploy and runbook:
- Write access.conf with MODE=off before adding HJV_ACCESS_FILE to hjviewer.env. Never leave `HJV_ACCESS_FILE=` blank: blank now means closed. Leave the line out to keep the check off.
- deploy/access.conf.example should use the order MODE, KEY, KEY, NOT_BEFORE, TEXT, with whole-line comments only.
- After switching to enforce, PCs that came in while off are closed within 10 s of the change taking effect (up to 20 s after the write).
- The console's `ib` list now shows `<ip>/noticket` entries for unticketed registrations. Clear one with `ib <ip>/noticket -`.
- Replacing a stolen key in place (same kid) now closes what it admitted within 10 s. The documented rotation (add kid 2, move hjv-api, remove kid 1) is unchanged.
- hjv-api (P): a PC refused for its ticket still has its key in hbbs. `hjv-pk` answers the signed key with age `-`, so binding and PC sync (key only) work, while wake, jobs and check, which go by age, see the PC offline. An ID that is online through a ticketed registration keeps its key: unticketed registrations from elsewhere no longer change what hjv-pk answers.
- Tickets for E2E and tools/hjv-load once enforce is on: the vectors' test keys sign valid tickets, seed 0x74 for kid 1 and 0x75 for kid 2. tests/access.rs's Ticket::sign is a 20-line reference for a test-only signer. Hbbs::stored_pk shows how to read and verify hjv-pk's signed answer.
- Integration: server patches 0006 (c44e975) and 0007 (349b8dd) apply in order on ee2081c and reproduce tree 264d920. Squash them, or regenerate the series with scripts/export-patches.sh.

## P: build

### summary

I built package P, hjv-api's access tickets, as one commit 957ecbd on branch staff-api in build/wt/staff-api, branched from staff-only. It is not pushed. Its tickets match docs/access-tickets.md byte for byte: the tests make all 14 valid vectors again exactly. They also check the signer against hbbs's verdict on the 58 vector payloads hbbs judges; all agree.

- **Signer (`src/ticket.rs`):**
  - The six ticket kinds as `Claims`, with constructors for the staff, review, guest-controller and guest-PC kinds.
  - The canonical nine-member payload, the `"hjv-ticket-v1" NUL` signature and the `t1.<kid>.<payload>.<sig>` form, capped at 1024 bytes.
  - The signer refuses any claims hbbs would call malformed: wrong pairs, target counts, PC fields, sub format, a lifetime over 7 days.
  - Tickets are never stored or logged. They are counted in memory only.
- **Key and settings:**
  - A new credential `hjviewer-api-ticket-key`, made with `hjv-api keygen ticket --kid N`, which prints `N:<base64>` (the access.conf KEY= value) and a fingerprint.
  - New settings: `HJV_API_TICKET_KID`, `HJV_API_TICKET_PUBKEYS` (compared by selfcheck and at start-up), `HJV_API_TICKET_HOURS=24` and `HJV_API_PC_TICKET_HOURS=72`. Hours are capped at 168, and hjv-api will not start with more.
  - Without the key, no tickets are signed and the endpoint answers 503.
  - The systemd unit now requires the key. The tests' Secrets literals, both of scripts/ci.sh's credential loops and the drill all make it.
- **`POST /api/hjv/ticket`:**
  - Takes a session with its X-HJV-Proof, counted against the key's shared limit of 30 calls a minute.
  - An empty body or `{}` gets a staff/ctl ticket (phones). A body `{"id","pk","sig"}` gets staff/both for that PC, where the device key signs `"hjv-ticket-pc-v1" NUL id NUL pk NUL base64(SHA-256(token))` (Windows apps).
  - Blocked accounts get 401 through the session check. The review account gets 403 `no_ticket`, behind a clean hook (`Review::demo_pcs`, empty until wave 2 parses the PC= line).
- **Login answer:** `hjv.access` carries the same ticket object. It is `both` for an installed PC whose key hbbs verified, `ctl` for every other device, and absent for the review account for now.
- **PC sync answer:** `access` carries staff/both for the PC's ID and the key that verified the sync, 72 h. It comes only with a sync that carried the binding, and only while the account is on the allowlist and not blocked.
- **Answer shape:** every ticket comes as `{"ticket","use","exp","server_time"}`, so the app never parses one.
- **Admin:** a read-only `hjv-api admin ticket status` command.
- **Docs:** server/api README (formats with a vector OpenSSL reproduces, settings, credentials, admin, vectors, tests) and the env example are updated.
- **Hooks for wave 2:** `ticket::issue` plus `Claims::guest_ctl` / `Claims::guest_pc` for the guest endpoint, and the review.rs accessors for the PC= line.
- **Checks on the commit:** fmt and clippy are clean; 407 tests pass with 0 skipped; the leak check is clean.

**Main open items:**
- Because the unit now requires the ticket credential, the runbook and drill docs must have it created before the new unit is installed, or hjv-api will not start.
- A Windows app run without installing gets `ctl` at login and needs one POST with its device signature to get `both`.
- The client team needs the new request and answer formats, which are written in the README.

### tests

Run once on commit 957ecbd (tree b3dc5b16, worktree clean), with CARGO_TARGET_DIR=build/wt-target/staff-api, a warm copy of the review-account package's target dir. Log: scratchpad/api-tests.log.
- `cargo fmt --check`: ok.
- `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `cargo test --locked -- --show-output` (what scripts/ci.sh's server stage runs for hjv-api): 407 passed, 0 failed, 10 ignored (doc tests that were ignored before). No `skipped:` lines, so the systemd-socket-activate and systemd-analyze tests ran too. About 60 s.
- New tests:
  - `tests/tickets.rs` (14 tests): the vectors file pinned at SHA-256 7b9fa3fd…; its keys, tags, limits and lifetimes; all 14 valid tickets made again byte for byte (payload, signed_hex, ticket, bytes); the constructors give the vectors' kinds.
  - The signer was checked against hbbs's verdict on every vector payload that hbbs judges: 58 payloads, 17 signed and 41 refused, all matching. An independent Python recount gave the same 58/41/17. A mutation run (letting a staff ticket hold a target) made this test fail.
  - The endpoint: phone ctl, Windows both, a renamed ID, lifetimes; every 400 case, 401 without the proof, 405, the retry returns the same ticket, a blocked account gets 401, 429 at the shared limit of 30 a minute.
  - Without a key: 503, no access in login or sync answers, the ticket status line, selfcheck FAIL and `[ -- ]`.
  - The review account gets 403 `no_ticket`.
  - Login answers: phone ctl, installed bound PC both, portable or unconfirmed key ctl.
  - Sync answers: disabled list with a ticket, active, repeat, listed without, PC lifetime setting, blocked without, unblocked with, revoked without.
  - `ticket status` and selfcheck, and no ticket in the journal or the records.
  - Unit tests in ticket.rs, config.rs and keygen.rs; `tests/binary.rs` `keygen_ticket_and_selfcheck_of_the_ticket_key`.
- `scripts/leak-check.py --base main`: none in 485 tracked files, the staged changes, or the 2 unpushed commits.
- `bash -n` on scripts/ci.sh and tools/e2e/hjv-api-drill.sh: ok.
- Not run: the ci.sh deploy stage (both credential loops changed), the drill, any E2E, the release build, and anything on the client. The main worktree and both upstream trees were not touched.

### open

- Deploy hazard for the docs package: deploy/systemd/hjviewer-api.service now has `LoadCredentialEncrypted=hjviewer-api-ticket-key`, so systemd will not start hjv-api until that credential exists. Runbook 7-4/7-5 ("여섯 자격 증명" becomes seven) and docs/hjv-api-drill.md need the same note they carry for the command key. The ticket key must be made before the new unit goes in (brief 4-1 step 1).
- selfcheck fails when a ticket key is held but HJV_API_TICKET_PUBKEYS is empty (`[FAIL] HJV_API_TICKET_PUBKEYS is not set`), as it does for the command key. The server env must carry access.conf's KEY= values. The drill does not set it; it tolerates selfcheck failures, but its log will show that FAIL line.
- Design choice to confirm: the login answer gives `both` only to an installed PC whose login signature matched the key hbbs holds, because that is the only device check a login makes today. A Windows app run without installing gets `ctl` at login and needs one POST /api/hjv/ticket with its PC part to get `both`. Letting the login body carry the device public key would remove that extra request.
- Design choice to confirm: POST /api/hjv/ticket picks `ctl` or `both` by whether the request proves a device key, not by the platform the login claimed, since nothing checks that platform. A phone that sent a PC part would get `both`, which is harmless because phones never register. The PC part's ID need not be the session's device ID, because a RustDesk ID can change while the app runs; only the device key's signature counts, as spec 2-4 says.
- New wire format that C1 has to implement (documented in server/api/README.md Formats, with a vector that OpenSSL reproduces): the PC part is `{"id","pk","sig"}`, where sig is the device key's Ed25519 signature over `"hjv-ticket-pc-v1" NUL id NUL pk NUL base64(SHA-256(bearer token))`.
- Answer shape that C1 has to read: the ticket object `{"ticket","use","exp","server_time"}` is the whole POST /api/hjv/ticket answer, `hjv.access` in the login answer, and `access` in the pc/sync answer. Error codes are in `hjv`: 400 bad_request, 403 no_ticket, 429 rate_limited, 503 unavailable.
- A bound PC gets its ticket only with a list for a sync that carried its binding (mark `present`). This includes a `disabled` list when only the password-less switch is off, but not for a blocked or off-allowlist account. A sync without the binding (`listed`) gets no ticket, so a service that lost its binding stays offline until someone logs in on it.
- Error texts are Korean and use "출입증" ("지금은 출입증을 받을 수 없습니다…", "이 계정으로는 출입증을 받을 수 없습니다."). The app probably never shows them, but the user may want different wording.
- Not done here, for later packages:
- the guest file and its settings (HJV_API_GUEST_FILE, HJV_API_GUEST_TICKET_HOURS);
- a production check of HJV_API_TICKET_PUBKEYS in scripts/check-api-settings.py;
- a selfcheck or deploy check that refuses the public test ticket keys listed in scripts/public-test-keys.txt (the test harness signs with vector key 0x74, so the check must exempt tests);
- ticket counts in `report` and `health`; `admin ticket status` counts in memory since the start only.
- Docs still to change (docs package, brief 6-1): README's Monitoring section and the nginx/Caddy/Apache 503 bodies still say connections go on while hjv-api is down. With the gate, staff keep the tickets they hold for 24 h, bound PCs for 72 h, and access-password users stop at once.
- Each sync of a present-binding PC runs one extra pair of indexed queries (account, allowlist) inside the sync transaction, even when no ticket key is set. This is negligible, but it could be skipped when tickets are off.

### hooks_for_wave2

- Guest endpoint (POST /api/hjv/guest): after the sealed password, limits and delay, sign with `ticket::issue(state, &Claims::guest_ctl(target, now, guest_secs), "guest ctl")` or `Claims::guest_pc(id, pk, now, guest_secs)` ("guest pc"), and answer `Issued::json(now())`. `NoTicket::Off` means 503 unavailable. Steps for wave 2:
- Add HJV_API_GUEST_FILE and HJV_API_GUEST_TICKET_HOURS (at most 168 hours; reuse the `ticket_hours` closure in config.rs) to SETTINGS and the env example.
- The path takes no token, so route it in api.rs before `allowed_methods` and session, as /api/login and pc/sync are.
- The signer already refuses a guest ctl ticket without exactly one target and a guest `both`.
- review.conf PC= line: make `Review::parse` (review.rs) accept `PC=<id>[,<id>…]` with 1 to 3 distinct `valid_device_id` IDs in file order, and fill the private `pcs` field. `Review::demo_pcs()` and `Review::until()` already exist. `ticket::review_claims` then issues review/ctl automatically, with exp = min(iat + HJV_API_TICKET_HOURS, UNTIL), both at login (hjv.access) and on POST /api/hjv/ticket. Today both answers are 403 `no_ticket` or no access. Deploy order: today's parser refuses an unknown line, so put the new hjv-api in before adding PC=.
- C1 client:
- Read `hjv.access` from the login answer and `access` from the pc/sync answer, each `{ticket,use,exp,server_time}`. Count `exp - server_time` from receipt and renew below 12 h (or half-life).
- A Windows app whose login gave `ctl` (not installed, or hbbs did not confirm its key) POSTs /api/hjv/ticket with `{id,pk,sig}` signed as in the README vector (seed [2;32], the README token, signature nG4V0N+q…oAg==). A phone POSTs `{}` or no body.
- POST /api/hjv/ticket is a signed POST: same X-HJV-Proof, single-use nonce, and the retry window returns the same ticket.
- hbbs S and E2E: the hjv-api test harness signs with the vectors' ticket_1 seed (0x74×32, kid 1) and has HJV_API_TICKET_PUBKEYS = KEY lines 1 and 2 of the vectors' rule, so harness-issued tickets verify under the vectors' access.conf. For the E2E access stage (HJV_E2E_TICKETS):
- put `hjv-api keygen ticket --kid 1` output into "$RUN/api/creds/hjviewer-api-ticket-key";
- put its first stderr line into both HJV_API_TICKET_PUBKEYS and hbbs's access.conf `KEY=`.
Without the credential, hjv-api's behaviour is unchanged apart from the 503 on the ticket path, so the existing E2E is unaffected.
- Ops and docs:
- `hjv-api keygen ticket --kid N` prints `N:<base64>` (the access.conf KEY= value) and `fingerprint: N:<hex>`.
- `hjv-api admin ticket status` prints: the key, its fingerprint, the KEY= line, whether HJV_API_TICKET_PUBKEYS takes it, the lifetimes, and counts since the start (`login ctl|both`, `request ctl|both|review`, `sync both`, `refused off|account|request|too-often`). Wave 2 adds `guest ctl|pc`.
- selfcheck has two ticket-key lines.
- The ready journal line now ends with `ticket key <fingerprint|none>`.
- Key rotation steps are in README Credentials (docs/access-tickets.md 6-4).

### P: review (needs-fixes)

- [major] server/api/src/login.rs:1789 (the `pc` match on `check`), with check_device at login.rs:1818; contract docs/access-tickets.md:87 on staff-only: The code and the shared contract disagree on what a Windows app that runs without installing gets at login.
- Contract 2-4, row "교직원 Windows 앱(설치하지 않은 실행본 포함)": it gets staff/both from "같음" (POST /api/hjv/ticket and the login answer) "기기 서명과 함께".
- Code: the login answer gives `both` only when check_device returns Verified. Verified requires `device.installed` and HJV_API_BIND_OS before any hbbs lookup (login.rs:1818). So a portable app's login gets `ctl`, and tests/tickets.rs:658-668 pins that.
- server/api/README.md documents the code's behaviour, but docs/access-tickets.md, which C1 implements against in parallel, still says otherwise.
- If C1 follows the contract, a portable app will not ask for `both` after logging in. It cannot register as a PC under enforce until its first renewal POST. Fix: Settle this before integration, in one of two ways:
- (a), the minimal one: amend docs/access-tickets.md 2-4 (the table row and "hjv-api가 지킬 것"). It should say the login answer gives `both` only to an installed PC whose hjv.sig matches hbbs's key, and a portable Windows app gets `both` by POSTing /api/hjv/ticket with its {id,pk,sig} right after login. Tell the C1 package.
- (b): in login.rs, for a device that sent hjv.sig but is not eligible for binding, run the same hbbs::lookup and device.signed_by check without binding, and pass Some((id, pk)) to ticket::for_account.
- [major] deploy/systemd/hjviewer-api.service:64 vs docs/server-runbook.md:856-867 and docs/hjv-api-drill.md:61: The unit now has LoadCredentialEncrypted=hjviewer-api-ticket-key, so systemd will not start hjv-api until that credential exists. That takes down login, PC sync, door 1 and wake.
- Runbook 7-4 still makes only the log, list and command keys.
- Its line 867 still says "여섯 자격 증명(SSO 둘, 봉인 키, 기록 키, 목록 키, 명령 키)이 모두 있어야 6단계에서 서비스가 시작합니다".
- No step sets HJV_API_TICKET_PUBKEYS, so 7-5's `admin selfcheck` would show `[FAIL] HJV_API_TICKET_PUBKEYS is not set`.
- docs/hjv-api-drill.md has the "key needed before start" note only for the command key.
- Anyone who follows the runbook with this unit gets a service that does not start.
The author flagged this for the docs package, but this commit introduces the requirement. Fix: Ship the runbook change together with this unit change, in this commit or a docs commit merged before it:
- 7-4: add `sudo bash -c 'set -o pipefail; /opt/hjviewer/bin/hjv-api keygen ticket --kid 1 | systemd-creds encrypt --tpm2-pcrs= --name=hjviewer-api-ticket-key - /etc/credstore.encrypted/hjviewer-api-ticket-key' || echo 'STOP: 출입증 키 1번을 넣지 못함'`.
- 7-4: change "키 세 줄" to 네 줄 and "여섯 자격 증명" to 일곱, adding 출입증 키.
- 7-3/7-5: write the `1:<base64>` line that keygen shows into HJV_API_TICKET_PUBKEYS (and later into access.conf KEY=).
- Add the same note to docs/hjv-api-drill.md that it has for the command key.
- Keep brief 4-1 step 1's order: make the key before installing the new unit.
- [minor] server/api/src/ticket.rs:491-510 (RequestBody, pc_part): The README (Formats, refusals) says 400 "for a body that is not one of the two": no body or {}, or {id,pk,sig}. RequestBody has no deny_unknown_fields, and serde's derive also accepts nulls and the sequence form. I checked this in a scratch crate with the lock file's serde 1.0.229 and serde_json 1.0.154:
- `{"foo":1}`, `{"id":null,"pk":null,"sig":null}` and `[null,null,null]` all parse as no PC part and get a 200 staff/ctl ticket.
- `["<id>","<pk>","<sig>"]` is accepted as a PC part.
This is not a security issue, since the device signature still decides `both`, but the endpoint accepts more than its documented format. Fix: Add #[serde(deny_unknown_fields)] to RequestBody. Before deserializing, require the body to be a JSON object (parse into serde_json::Map or check is_object()), and refuse explicit null members with 400. Add {"x":1}, {"id":null,"pk":null,"sig":null} and [null,null,null] to the bad bodies in tests/tickets.rs the_ticket_path_refuses.
- [minor] server/api/src/pcsync.rs:660 (ticket_holder at :475): Every sync of a present-binding PC now runs db::account and db::allowed inside the sync transaction.
- This happens even when hjv-api holds no ticket key.
- It also happens on the cached repeat path, which pcsync's module docs say reads nothing again from the database.
- A rusqlite error in these reads now fails the sync with 503, even with tickets off.
Otherwise the answers without a key are unchanged, so this is small: extra work and a new failure point on the no-ticket path. Fix: Pass `tickets: state.ticket_key.is_some()` in the ListSettings that handle() already gives decide(), and call ticket_holder only when it is true. Optionally reuse the account row that bound_list already reads.
- [minor] tools/e2e/hjv-api-drill.sh:261 and :518: The drill makes the ticket key with `2> /dev/null`, which drops the `1:<base64>` line, and it never sets HJV_API_TICKET_PUBKEYS.
- Every rehearsal therefore logs `[FAIL] HJV_API_TICKET_PUBKEYS is not set`, which the drill hides with `|| true`.
- It never rehearses the fingerprint comparison that its command-key steps do (:283-287).
- Host B gets a fresh ticket key instead of A's (list and command keys are moved with cred_from). A real host move done this way would invalidate every ticket in circulation unless access.conf's KEY= changes too. Fix: Capture the first stderr line of `keygen ticket --kid 1` as TICKET_PUBKEY_1, and set HJV_API_TICKET_PUBKEYS in the env edit next to COMMAND_PUBKEYS (:189-200). Assert that `admin selfcheck` shows `[ OK ] ticket key in use (HJV_API_TICKET_KID 1)`. On B, either move A's ticket key with cred_from or say explicitly that the move makes a new key and needs access.conf's KEY= changed.
- [minor] server/api/README.md:1051 and :333: "A blocked account's tokens end (401), and its PCs' syncs carry no ticket" reads as an immediate cut-off. In fact hbbs keeps taking tickets already handed out until their exp: up to 24 h for the account's apps and 72 h for its bound PCs. The same holds after a logout. Also, the `ticket status` kinds list (login ctl|both, request ctl|both|review, …) leaves out `login review`, which for_account counts with via "login" (ticket.rs:431). Fix: Add one sentence: tickets already handed out keep working until their exp; to stop them at once, write NOT_BEFORE=<now> into hbbs's access.conf (docs/access-tickets.md 6-4). Add `login review` to the kinds list.
- [minor] server/api/src/selfcheck.rs ticket_keys / server/api/src/main.rs check_ticket_key: The vector ticket seeds 0x74, 0x75 and 0x76 are public: scripts/public-test-keys.txt lines 37-38 and tests/common/mod.rs TICKET_SEED. scripts/check-build-keys.py guards only app builds, and the ticket key is pinned on the server, not in a build. Nothing refuses a held ticket key or an HJV_API_TICKET_PUBKEYS entry equal to one of these keys. A server set up from the vectors' values (for example access.conf KEY=1:VV9GnCIY…) would trust tickets anyone can sign. The author lists this as later work; I note it so it gets an owner. Fix: Make selfcheck FAIL and start-up log_crit when the held ticket key's public key, or any HJV_API_TICKET_PUBKEYS entry, is a repeated-byte-seed key or is listed in scripts/public-test-keys.txt. Exempt the test harness explicitly, for example with a test-only Options flag. Or put the same check into the docs package's access.conf/deploy check.
- [minor] server/api/src/ticket.rs:408 (issue) and :440 (review_claims): Two risks in the wave-2 hooks:
- issue() logs log_err! with no first_notice throttle on every refused claim. Today every input is validated before issue(), but POST /api/hjv/guest will pass a target chosen by an unauthenticated client. A malformed target would then give a 500 and one error line per request.
- review_claims (exp capped at UNTIL, `to` in file order) has no test, because Review::parse cannot fill `pcs` yet. Fix: In wave 2:
- Validate the guest target and PC ID with valid_device_id and answer 400 before calling issue(), or throttle issue()'s log line with state.first_notice.
- Once PC= parses, add tests for exp = min(iat + HJV_API_TICKET_HOURS, UNTIL), `to` in file order, and an UNTIL less than a second away.

## P: fixes

### summary

I fixed all 8 review findings for package P (hjv-api access tickets) in one commit, 292b031 "server: review fixes", on branch staff-api in build/wt/staff-api. It is not pushed. Each finding was checked against the code first, and all of them held.

- **Major 1, the login answer vs the contract:** applied option (a).
  - docs/access-tickets.md 2-4 (the table row and a new "hjv-api가 지킬 것" bullet) now matches the code. The login carries no device key, so `both` goes only to an installed PC whose hjv.sig matched hbbs's key, and `ctl` to everything else.
  - 3-3 now tells the app what to do: a Windows app holding anything but `both` POSTs /api/hjv/ticket with its PC part right after the login, and every Windows renewal carries the PC part.
  - The README and the ticket.rs docs say the same.
  - Option (b) was declined; the reason is under open.
- **Major 2, the unit's new credential:**
  - Runbook 7-4 makes ticket key 1 next to the other server keys ("네 줄", "일곱 파일/자격 증명", the unit needs it). Its (다) list names HJV_API_TICKET_PUBKEYS.
  - 7-5 writes TICKET into HJV_API_TICKET_PUBKEYS (three public-key lines) and its selfcheck notes cover the ticket key.
  - Section 6 (update of path B), step 4 now refuses to install a unit whose credentials are missing, and gives the steps to make the ticket key and set the setting. This keeps brief 4-1's order: the key is made before the new unit goes in.
  - The host rebuild (7-14) and docs/hjv-api-drill.md say a rebuilt host makes a new ticket key.
- **Minor 3, the request body:** RequestBody now has deny_unknown_fields, and the body must be a JSON object whose members are all strings. Arrays, nulls, unknown members and repeated members get 400. Unit and integration tests cover these cases.
- **Minor 4, PC sync reads:** ListSettings.tickets means a sync reads the account for a ticket only while hjv-api holds a ticket key.
- **Minor 5, the drill:**
  - New helpers `ticket_key_on` and `ticket_key_shown` take the public key and fingerprint from `keygen ticket` (which goes straight into systemd-creds).
  - `settings` sets HJV_API_TICKET_PUBKEYS, and both hosts check the selfcheck `[ OK ] ticket key in use` line against the fingerprint keygen showed.
  - Host B makes its own key, with the reason in a comment.
- **Minor 6, README:** tickets of a blocked account (or after a logout) keep working until their exp (up to HJV_API_TICKET_HOURS or HJV_API_PC_TICKET_HOURS), and NOT_BEFORE=<now> in access.conf ends them at once. `login review` is added to the counted kinds.
- **Minor 7, public test keys:** `ticket::public_test_key` (seeds of one repeated byte or 32 consecutive bytes) and `test_keys_pinned`. selfcheck FAILs and the start-up logs crit for each such key in HJV_API_TICKET_PUBKEYS. The test harness is exempted through Config::test_ticket_keys, which no setting can turn on. The README, the env example and access-tickets.md 8 mention it.
- **Minor 8, the wave-2 hooks:**
  - issue()'s error line now logs once per ten minutes per kind.
  - The App Review account's ticket rule is now a pure, unit-tested function, `review_ticket`.
  - The module doc says the guest path must check IDs with valid_device_id and answer 400 before calling issue().

**Checks, run once after the commit:** fmt and clippy are clean, and 409 tests passed with 0 failed and 0 skipped. `bash -n` passes on the drill and ci.sh, and the leak check found nothing. The drill and E2E were not run.

**Main open item:** C1 must be told about the contract change. It works from staff-only 0707e18, which does not have it.

### tests

Run once on commit 292b031 (tree cde9f803, worktree clean), with CARGO_TARGET_DIR=build/wt-target/staff-api (warm). Log: scratchpad/api-tests-fixes.log.
- `cargo fmt --check`: ok.
- `cargo clippy --locked --all-targets -- -D warnings`: clean.
- `cargo test --locked -- --show-output` (what ci.sh's server stage runs for hjv-api): 409 passed, 0 failed, 10 ignored (the same doc tests as before). That is 407 + 2 new unit tests. No `skipped` lines anywhere, so the systemd-socket-activate tests ran too.
- New or extended tests:
  - ticket.rs `the_pc_part_of_a_request`: arrays, nulls, unknown members and a repeated member are refused.
  - ticket.rs `the_review_accounts_ticket_ends_with_its_file`: exp is the earlier of the lifetime and UNTIL, `to` keeps the file's order, an UNTIL 1 s away gives a 1 s ticket, a passed UNTIL or no PCs gives none.
  - ticket.rs `the_vectors_keys_are_public_test_keys`.
  - tests/tickets.rs `the_ticket_path_refuses`: 5 more 400 bodies.
  - `ticket_status_and_selfcheck`: with `refuse_test_keys` the harness's selfcheck FAILs on keys 1 and 2.
  - tests/binary.rs `keygen_ticket_and_selfcheck_of_the_ticket_key`: the next key is now made by `keygen ticket --kid 2`, and the real binary's selfcheck FAILs when the vectors' key 2 is pinned.
- `bash -n` on tools/e2e/hjv-api-drill.sh and scripts/ci.sh: ok.
- `scripts/leak-check.py --base main`: none in 485 tracked files, the staged changes, or the 3 unpushed commits.
- Not run: the drill (changed, syntax only), any E2E, the ci.sh deploy stage, the release build, and anything on the client. No exported patch files belong to this package (server/api is in the main repo), so nothing to refresh. The main worktree and both upstream trees were not touched.

### open

- C1 must be told about the contract change, which lives on staff-api only. C1 implements against staff-only 0707e18, so it has not seen it. docs/access-tickets.md now says:
- 2-4: the login answer gives `both` only to an installed PC whose hjv.sig matched hbbs's key, and `ctl` to every other device.
- 3-3: a Windows app holding anything but `both` (the login's `ctl`, or a login without `access`) POSTs /api/hjv/ticket with {id,pk,sig} right after the login, and every Windows renewal carries the PC part. Phones send no PC part.
If C1 skips this, a portable Windows app cannot register under enforce until its first renewal.
- Finding 1, option (b) declined (a login-time hbbs lookup for portable apps). C1 needs the ctl-then-POST fallback anyway: hbbs may not know the ID yet, or the lookup can fail. So (b) would only save one request, at the cost of an extra hbbs console lookup on every portable login and a second device-check path in login.rs. Option (a), the contract change, was needed either way.
- Finding 4: the optional reuse of the account row that bound_list reads was declined. With a ticket key, a present-binding sync still does the two indexed reads (account, allowlist), including on the cached repeat path. Without a key it now does none.
- Upgrading an existing server: runbook 6 (update of path B), step 4 now starts with a generic guard. It reads every LoadCredentialEncrypted= name in the package's unit and STOPs before installing anything if one is missing. A paragraph and a short block follow:
- The user makes the ticket key with the package's own bin/hjv-api; the installed old binary has no `keygen ticket`.
- The agent re-runs the block, then adds or sets HJV_API_TICKET_PUBKEYS, restarts and checks the two selfcheck ticket lines.
- Two restarts happen; the first only logs the `not set` warning.
The docs package's new 1.8.0 runbook section should build on this, including the access.conf KEY= order. 7-3 itself was not changed: the setting goes in at 7-5, like the other key settings.
- The public-test-key refusal has limits:
- It covers Ed25519 keys from seeds of one repeated byte or 32 consecutive bytes (wrapping), the same rule as check-build-keys.py. That covers every Ed25519 key in scripts/public-test-keys.txt; the file itself is not read at runtime, so a future test key in another form would need adding to the rule.
- It sees only HJV_API_TICKET_PUBKEYS. hjv-api cannot read access.conf, so hbbs S or a deploy check of access.conf should refuse the same keys there.
- The exemption is the new pub field Config::test_ticket_keys. from_vars always sets it false; only tests/common's Harness sets it true (Options::refuse_test_keys turns it off).
- The drill script and its record doc changed but were not run (no drill or E2E per the rules). docs/hjv-api-drill.md says so; its results table is still 3813e40's. The next drill run should confirm both hosts' `ticket key in use` fingerprint checks.
- Still open from the author's list, unchanged here:
- the refusal texts' word "출입증";
- a production check of HJV_API_TICKET_PUBKEYS in check-api-settings.py;
- ticket counts in report and health;
- README Monitoring and the nginx/Caddy/Apache 503 bodies, which still say connections go on while hjv-api is down (docs package).

### hooks_for_wave2

- C1, from the amended contract (docs/access-tickets.md 2-4, 3-3, on staff-api):
- Read `use` from hjv.access in the login answer and from `access` in the pc/sync answer.
- On Windows, when the held ticket is not `both`, POST /api/hjv/ticket with {id,pk,sig} right after the login; every Windows renewal sends the PC part.
- Phones POST `{}` or no body.
- The body must be exactly one of those forms: an object of strings, with no other or repeated member. Anything else is 400 bad_request.
- Guest endpoint (POST /api/hjv/guest):
- Check the target and PC IDs with `valid_device_id` and answer 400 before calling `ticket::issue`. issue() treats a bad claim as hjv-api's own fault: a 500, with a journal line once in ten minutes per kind (state.first_notice key `ticket-failed:<what>`).
- Sign `Claims::guest_ctl` or `Claims::guest_pc` as before.
- `NoTicket::Off` means 503.
- review.conf PC= line: once `Review::parse` fills `pcs`, `ticket::review_claims` gives review/ctl through the pure `review_ticket(sub, pcs, until, at, HJV_API_TICKET_HOURS secs)`. Its unit test already pins exp = min(at + lifetime, UNTIL), the file's order of `to`, and none with 0 s left. Add an integration test of the login (hjv.access) and of POST /api/hjv/ticket with a PC= file, and enforce 1 to 3 distinct IDs in the parser; the signer refuses 0 or 4+ IDs as a 500.
- E2E access stage and hbbs S:
- Use keys from `hjv-api keygen ticket --kid 1`. The real binary's selfcheck FAILs, and its start logs crit, when HJV_API_TICKET_PUBKEYS holds a public test key such as the vectors' 0x74/0x75/0x76 keys.
- Only the in-process harness exempts them (Config::test_ticket_keys, which tests/common sets unless Options::refuse_test_keys).
- hbbs or the deploy check should refuse the same keys in access.conf itself.
- Docs package: runbook 6 (update of path B), step 4 now holds the generic missing-credential guard and the ticket-key steps for an existing server (make the key with the package's bin/hjv-api, then set HJV_API_TICKET_PUBKEYS after the new binary is in, then restart). The new 1.8.0 runbook section can point there and add access.conf's KEY= line from the same value.

## C1: build

### summary

C1 is done: the client now carries access tickets to hbbs. Two commits, not pushed:
- 861fe2a in the worktree's hbb_common, exported as `0003`.
- 36da587 in build/wt/staff-client, exported as `0058`.

Both patches are in build/wt/staff-client-patch/. The full stable lib suite, the HJViewer tests in a pinned account build and both Rust 1.75 checks pass. One open item needs the integrator: the hjv-api answer and request shapes are my proposal, because P had not written them yet. They are listed under open.

**What the change does**
- **hbb_common 0003:** adds `string hjv_ticket = 1001;` at the end of `RegisterPk`. An empty ticket writes no bytes, so a registration without one is byte for byte 1.7.0's.
- **New module `src/hjviewer_access.rs`:** all ticket logic, joined to existing code by thin hooks in `Client::start`, `on_establish_connection_error`, the mediator, the account module (`api.rs` login and logout, `pc.rs` sync), `flutter_ffi.rs`, `hjviewer.rs` and `hjviewer_e2e.rs`. The app never reads a ticket; it only uses the `exp` and `server_time` sent beside it.

**Controller**
- `Client::start` takes the token for every attempt: the access-password ticket for that target if one is held (memory only, filled by wave 2), else the login's ticket. Reconnects and each local port-forward connection therefore get the current ticket, and the RequestRelay to hbbr carries none.
- The login's ticket comes from the login answer (`hjv.ticket`) or from `POST /api/hjv/ticket`, sent with door 1's signed request (`send_by_login`, now shared). A desktop app adds its ID and device key, signed by that key, so the ticket can also register it.
- A background thread renews past half-life. Without a valid ticket, a logged-in app fetches one before connecting (5 s at most). All requests share one limiter.
- The ticket stays in LocalConfig until logout.
- If hbbs refuses with "access not allowed", a logged-in app fetches a new ticket once and retries. A refusal that stays reaches Flutter as msgbox type `hjv-access-error` with the app's own text (A1 or A2). Today's screens show it as a normal error box.

**PC**
- `RegisterPk.hjv_ticket` carries, in order of preference:
  - a bound PC's ticket from the sync answer (kept by the service, plus a file copy for restarts while hjv-api is down);
  - the access-password PC ticket (wave 2);
  - the login's ticket on a portable app.
- The PC re-registers at once when a new ticket arrives after a refusal, and when the registered ticket is past half its lifetime.
- When it has no ticket left, it reconnects instead, because a registration without a ticket would not end the one hbbs already accepted.
- Whether hbbs accepted the registration goes into the option `hjv-access-pc` for the screens.

**Other**
- Pinned builds now fix direct IP access and LAN discovery off.
- New E2E command `--hjv-e2e-ticket [refresh]`; an access refusal ends the E2E connect commands with exit code 5.
- Builds without account features, and apps holding no ticket, send exactly what 1.7.0 sent, so MODE=off and older servers keep working.
- The SPEC's worry about `legacy_secure` does not apply: pinned builds always use WebSocket, where it changes nothing.

### tests

After the last commit (client 36da587, tree 420378e3; hbb_common 861fe2a, tree 0a2c097e). Each ran once, and each log header names these commits.

- Stable `cargo test --locked --features linux-pkg-config,hjv-e2e --lib -- --skip test_get_cursor_pos --skip a_punch_that_meets_the_peers_syn_connects_both_ends`: 571 passed, 0 failed, 5 ignored, 74 s. The 2 filtered out are the two tests known to fail on this WSL2 PC.
- Pinned account build, `--lib hjv` (as the e2e stage runs it, but debug and without update keys): 254 passed, 0 failed, 2 ignored. It used HJV_SERVER=hjv.test plus the public test keys that src/hjviewer_account/vectors.rs already holds for HJV_LOGIN_SEAL_PUBKEY and HJV_TRUST_PUBKEYS.
- `cargo +1.75.0 check --locked --features linux-pkg-config,hjv-e2e` (CI's command, target build/ci/target-175): OK.
- `cargo +1.75.0 check --locked --features linux-pkg-config,flutter`: OK. This is also the build without hjv-e2e. It used the gitignored bridge_generated files copied from upstream/rustdesk, whose src/ matches HEAD.
- No warning in any of these runs points at the new module or at a changed line.
- 11 new tests, all passing:
  - 10 in hjviewer_access::tests:
    - RegisterPk bytes equal the vectors' register_pk "ticket" and "no-field" hex (JSON SHA-256 7b9fa3fd…63f9eb).
    - A parser without field 1001 reads id and pk, and `unknown_fields().get(1001)` is the ticket.
    - Ticket timing by this device's clock and renewal past half-life.
    - Which ticket a controller and a PC choose.
    - When the PC registers again or reconnects.
    - The refusal mark, and that check_if_retry stays off for A1, A2 and the msgbox type.
    - The device signature in the ticket request.
    - Access-password tickets stay in memory.
  - 1 in client::access_ticket_tests: against loopback stand-in servers, RequestRelay to hbbs carries the ticket and comes back as a recognised refusal; the RequestRelay to hbbr carries no token.
- `git apply --check`: 0058 applies to 398fa05's files, 0003 to b86989e. Exit 0 for both.
- Not run: E2E, scripts/ci.sh stages, hbb_common's own tests (proto-only change), Flutter/Dart (no UI change), Android/iOS/Windows compiles (no targets on this PC; I checked the cfg gates by hand).

### open

- API contract with the P package (hjv-api). Doc 2-4 leaves the answer shape to server/api/README.md, which P had not written yet; staff-api was still at 0707e18 and no other session was reachable. The client expects the following; each is one function to change if P chose otherwise (`Held::issued`, `request_body`):
- The ticket object `{"ticket":"t1.…","exp":<time>,"server_time":<time>}`. It is the whole 200 answer of POST /api/hjv/ticket, `hjv.ticket` in the login answer, and a top-level `ticket` beside `list` in the pc/sync answer.
- The POST /api/hjv/ticket body: `{}` from a phone (gets `ctl`). From a desktop: `{"id":"<device ID>","pk":"<std base64 device key>","sig":"<std base64>"}`, where sig is the device key's Ed25519 signature over `"hjv-ticket-dev-v1" NUL id NUL pk NUL base64(SHA-256(bearer token))`. It is sent like door 1's requests (token plus X-HJV-Proof).
- The client ignores an answer whose ticket is empty, longer than 4096 bytes, or has exp <= server_time. It caps a lifetime above 7 days.
- Sync answers: a 200 answer that keeps a list of the binding but carries no `ticket` drops the PC's ticket, and the PC then goes offline through a reconnect. hjv-api must include the ticket in every such answer; blocked accounts get none by design. Old hjv-api versions send none, and the old hbbs accepts that.
- Deviation from the brief, same effect: the brief suggested changing rendezvous_token and port_forward.rs. Instead, one hook in Client::start (`controller_token`, then `renewed`) supplies the ticket on every attempt. That covers sessions, reconnects, each local port-forward connection and the E2E commands. port_forward.rs and the call sites are unchanged, and rendezvous_token still blanks the upstream token.
- New rule not in the doc: per doc 4-4, a RegisterPk without a ticket does not end a registration hbbs took. So when the PC registered with a ticket and holds none any more, it restarts its rendezvous connection and registers without one. Causes are a logout, unbind or revoke, a blocked account, the ticket expiring, or Flutter's own logout after a 401. Otherwise it re-registers on the same connection: at once after NOT_DEPLOYED (skipping the 30 s wait) when a new ticket arrives, after a registration without a ticket, and past half the registered ticket's lifetime.
- SPEC caution about legacy_secure, checked: no effect in pinned builds. Pinned builds fix allow-websocket=Y, so use_ws() is true. secure_tcp and secure_tcp_required then return at once, and force_relay = policy_relay || use_ws() is always true, so the UDP NAT test never runs and there is no wait to skip. Unpinned builds behave as upstream, and HealthCheck still gets the empty io_loop token.
- Patch author: the commits are 'ysfaculty <haejin.cho@yonsei.ac.kr>', as the task said. The existing series uses 'hjviewer <haejin.cho@yonsei.ac.kr>', so the integrator may want to re-author on import. The patches use --zero-commit like the series. The hbb_common patch is 0003; apply it before rustdesk 0058, which uses `hjv_ticket`.
- Incident, fixed: `git -C upstream/rustdesk worktree add build/wt/staff-client` resolved the relative path inside upstream/rustdesk, under its gitignored build/. I moved it with `git worktree move` to build/wt/staff-client and removed the two empty directories it had left. Nothing else in the main working copy changed: upstream/rustdesk hjviewer is still 398fa05 and the hjviewer main is still 5b6546a, clean. The worktree's hbb_common was cloned from the local upstream/rustdesk/libs/hbb_common, so its hjviewer branch has the same commits; the shared config's submodule URL is unchanged.
- Known limits:
- Switch sides from a bound PC whose UI user is not logged in is refused. The connection runs in the UI process, which cannot read the service's bound ticket.
- A1 and A2 are Korean-only Rust constants, like R1 and R2, using the SPEC's proposed wording, which the user may still review.
- After a service restart, hjv-access-pc shows the previous value until the first registration answer.
- Upstream's "Server requires deployment. Run rustdesk --deploy" warning still appears in the log on NOT_DEPLOYED.
- A renewal finishing exactly during a re-login can leave the previous login's (valid) ticket until the next renewal.
- Typing an IP on the controller still tries a direct connection; only the PC's direct server is pinned off.
- Storage, for the D package (docs/privacy):
- LocalConfig `hjv-access-ticket` holds the login's ticket as `{"t":…,"until":…,"life":…}` in device-clock seconds; it is cleared at logout and dropped login, and unused while access_token is empty.
- The service writes `HJViewer_ticket.json` in its config folder: 0600 on Unix, rewritten about hourly rather than per sync, read at service start only if the PC is bound, deleted when the service starts unbound.
- Access-password tickets live in process memory only.
- No ticket is logged; the E2E command prints only the first 8 bytes of its SHA-256.
- Housekeeping: the builds used the warm build/ci/target-client and build/ci/target-175, which now also hold artifacts for this worktree's path. The gitignored bridge_generated*.rs copied into the worktree for the flutter check are still there.

### hooks_for_wave2

- Dart, session refusal dialog (A1/A2): in handleMsgBox, before the default branch, match `type == 'hjv-access-error'`.
- Not logged in: open the A1 dialog with [연세포탈 로그인] [이용 비밀번호로 연결] [취소], modelled on _connectDialog.
- Logged in: show it as an error. Rust has already fetched a new ticket once and retried.
- The text arrives as A1 or A2 from Rust (hjviewer_access A1/A2, ending with '(access not allowed)'), the title is 'Connection Error', and hasRetry is false.
- Without the dialog, today's screens already show it as a plain error box (OK closes the session).
- Dart, PC home and quick-support window (P1): show P1 when `bind.mainGetOption('hjv-access-pc') == 'refused'`. Rust writes 'ok' or 'refused' on each RegisterPkResponse (OK or NOT_DEPLOYED). It is the service's Config2 option on an installed PC and the app's own on a portable one; the E2E reads it with `--hjv-e2e-option hjv-access-pc`.
- Dart, app lifecycle: call `bind.mainSetCommon('hjv-access-refresh','')` at app start and on return to the foreground; it wakes the renewing thread. `bind.mainGetCommon('hjv-access-state')` returns `{"ticket":bool,"until":<unix, device clock>}` for the login's ticket.
- Guest controller exchange (Rust, wave 2): after POST /api/hjv/guest succeeds, call `crate::hjviewer_access::keep_guest(target_id, &answer)`, where answer is the ticket object `{ticket,exp,server_time}`, then `sessionReconnect`. Client::start then sends it for that target in preference to the login's ticket. Call `forget_guest(target_id)` when the session ends; where exactly is still to be decided, because tabs to the same ID share the entry. Never persist it.
- Guest PC (Rust, wave 2): `keep_guest_pc(&answer)` must run in the process that registers. On an installed PC that is the service, through a new IPC message (not a config write) whose handler calls it. `forget_guest_pc()` serves [끝내기]. registration_tick then re-registers at once after a refusal, and reconnects without a ticket once it is gone.
- hjv-api (P): implement the shapes in 'open':
- the ticket object in the login answer `hjv.ticket`, the POST /api/hjv/ticket answer and the pc/sync answer;
- the desktop request body with the 'hjv-ticket-dev-v1' device signature bound to the token's SHA-256;
- the same ticket object from POST /api/hjv/guest.
Either change the client's two functions or document these in server/api/README.md.
- E2E (E):
- `<app> --hjv-e2e-ticket [refresh]` prints `hjv-e2e: ticket {"login","ticket":<8-byte sha256 hex>,"valid","left","life"}`. With refresh it first sends one POST /api/hjv/ticket outside the limits; exit 1 if that brought no ticket.
- --hjv-e2e-screenshot, -stream and -forward exit with code 5 on an access refusal, after the one renewal and retry.
- `--hjv-e2e-guest` (brief 3-4) is for wave 2.
- Docs (D):
- app-side names: LocalConfig `hjv-access-ticket`, service file `HJViewer_ticket.json`, option `hjv-access-pc`, msgbox type `hjv-access-error`;
- pinned builds fix `direct-server` and `enable-lan-discovery` to N; today's settings show them as fixed checkboxes, and hiding them is still UI work;
- renewal: past half the lifetime, with all requests at least 10 s apart and backing off to 30 min, and a 5 s wait before connecting when no ticket holds.

### C1: review (needs-fixes)

- [major] client src/hjviewer_account/api.rs:720, src/hjviewer_account/pc.rs:850, src/hjviewer_access.rs:77 (also tests.rs:237) vs hjv-api 957ecbd (worktree build/wt/staff-api): server/api/src/login.rs:1794, server/api/src/pcsync.rs:782, server/api/src/ticket.rs:67, server/api/README.md:1003-1032: The client's ticket API shapes disagree with what hjv-api (package P) committed in 957ecbd at 22:03 and documented in server/api/README.md. Doc 2-4 makes that README the authority for the answer shape, and the staff-api working copy keeps the same names.
(a) Login answer: hjv-api sends the ticket as `hjv.access`, but api.rs:720 reads `hjv.ticket`. So `login_answered(None)` runs and clears any earlier ticket.
(b) Sync answer: hjv-api sends a top-level `access`, but pc.rs:850 reads `ticket`. Every 200 answer that keeps the list therefore counts as having no ticket, and the bound PC drops its ticket and reconnects without one.
(c) Device signature: hjv-api verifies over "hjv-ticket-pc-v1" NUL id NUL pk NUL base64(SHA-256(token)), and hashes the token the same way the client does. The client signs "hjv-ticket-dev-v1". So `pc_part` fails, and POST /api/hjv/ticket answers 400 bad_request to every desktop app.
Integrated under MODE=enforce:
- No Windows, macOS or Linux app ever holds a ticket. Its controllers get A2, and a portable PC stays offline.
- Every bound PC goes offline at its first sync.
- Only phones work, after a fetch of up to 5 s before their first connection.
The author's open note says each change sits in `Held::issued` or `request_body`, but (a) and (b) are in api.rs and pc.rs. Fix: Code changes:
- Read `h.get("access")` at api.rs:720.
- Rename `Answer.ticket` to `access` (or add `#[serde(rename = "access")]`) at pc.rs:850.
- Set `DEVICE_CONTEXT` to `b"hjv-ticket-pc-v1"`.
- Update the doc comments (hjviewer_access.rs:20, 378, 650; api.rs:203; pc.rs:849) and the commit message.
Tests:
- In the api.rs Fake (line 926), record `login_ticket`, and check that a login answer with `hjv.access` reaches it.
- In the pc/tests.rs Fake (line 1047), record `keep_ticket`, and check it for a 200 answer with `access`.
- Pin the request signature to bytes both sides share, such as a `ticket_request` case in access-tickets.json or P's `pc_message` test values, instead of the client's own constant.
- [major] src/hjviewer_access.rs:116 (Held::issued), :223 (pc_choice), :250 (reregister), :706 (renew); hjv-api 957ecbd server/api/src/login.rs:1786-1795 and check_device (requires device.installed): The app drops the `use` value that hjv-api sends with every ticket, and a refused registration never leads to a new ticket. hjv-api's login answer gives `staff/both` only to an installed PC that hbbs's key verified, and `staff/ctl` to every other device (see the README table).
Once the first finding is fixed, a portable (not installed) desktop app that logs in therefore holds a `ctl` ticket, and this follows:
1. It registers with that ticket, and hbbs refuses it (`wrong_use`, so NOT_DEPLOYED).
2. `reregister` waits, because the ticket did not change.
3. The 30 s NOT_DEPLOYED retries resend the same ticket.
4. `renew()` sends POST /api/hjv/ticket only past half the lifetime. With the PC part, that request would bring a `both` ticket.
The PC is therefore offline for about 12 h after every login. That contradicts doc 2-4 and brief 3-4 ("로그인한, 설치하지 않은 실행본: 앱의 출입증(both)으로 등록"). The same 12 h gap follows a NOT_BEFORE emergency (doc 6-4) or an ID change, because NOT_DEPLOYED never prompts a renewal. Fix: In the client:
- Keep `use` in `Held`, with `#[serde(default)]` so stored values still parse.
- In a process that registers with the login's ticket (desktop, device key present, not installed), treat a `ctl` ticket from the login answer as due at once. The renewing thread then sends the PC-part request right after login.
- A `ctl` answer to that request waits for half-life as it does now, so the 10 s limiter sees no request loop.
- Let `pc_choice` take only `pc` and `both` tickets.
Alternatively, agree with P that the login answer gives `both` to a portable app whose login signature it can verify. One of the two sides has to make this change.
- [major] src/hjviewer_account/pc.rs:1647-1654 (Syncer::keep_ticket), src/hjviewer_access.rs:422 (bound_answered): A 200 sync answer that keeps the list but carries no ticket drops the bound PC's still-valid ticket and its file copy, and the PC then reconnects without one.
hjv-api leaves the ticket out in more cases than a blocked account:
- whenever it has no ticket key (ticket.rs: "Without the credential ... the answers carry none"; the working copy adds `tickets: state.ticket_key.is_some()`);
- when it is rolled back as brief 4-3 describes ("새 HJV_API_* 줄을 먼저 뺍니다").
Under MODE=enforce, a lost credential or that rollback takes every bound PC offline within one sync (2–5 min).
The documents say otherwise:
- Doc 3-3: the app keeps the ticket it holds until `exp` when it gets no new one.
- Brief section 2 and 3-7: bound PCs last up to 72 h when hjv-api fails, and a blocked account's tickets live out their lifetime. NOT_BEFORE is the emergency lever. Fix: Replace the ticket only when a new one came:
- Under `Kept`, `Some(Some(ticket))` calls `keep_ticket(Some(ticket))`; anything else returns without a change.
- Keep `None` (drop the ticket) for `Mode::Unbound`, a signed `Revoked` and the unbind path. Those are what end a binding.
This removes code, adds no state, and matches doc 3-3. If an immediate cut-off for blocked accounts is wanted, have hjv-api send an explicit marker, and drop the ticket only on that marker.
- [minor] src/hjviewer.rs:24-36 (pinned OVERWRITE_SETTINGS); libs/hbb_common/src/tls.rs:84-98; flutter/lib/desktop/pages/desktop_setting_page.dart:1987: The ticket is a bearer credential: 24 h for staff, 72 h for a bound PC, and a `both` ticket also works as a controller's ticket for any PC. It now travels in PunchHoleRequest, RequestRelay and RegisterPk on the wss connection to hbbs.
Pinned builds still let the user set `allow-insecure-tls-fallback`, which the desktop network settings show. With it on, hbb_common's WebSocket continues with an invalid certificate after a failed check. A TLS interceptor on the path can then read the ticket and replay it until `exp`.
Before 1.8.0 this channel carried no credential (`rendezvous_token` blanks the token for exactly that reason), and hjv-api requests use the strict client. Fix: Either:
- pin `keys::OPTION_ALLOW_INSECURE_TLS_FALLBACK` to "N" in the pinned OVERWRITE_SETTINGS, beside direct-server (one line); or
- send no ticket while `get_cached_tls_accept_invalid_cert` for the rendezvous URL is `Some(true)`.
- [minor] src/hjviewer_access.rs:103 (Held), 147 (Sent), 192 (login_ticket), 585-587 (fetch), 407-409 (restore_bound); src/hjviewer_account/pc.rs:276-304 (forget_pc_binding, uninstall_line): Tickets outlive the state that should end them, and they print in Debug output.
(a) Flutter's own logout after a 401 or 400 (UserModel.reset, which sends no /api/logout) clears `access_token` but leaves a valid `hjv-access-ticket` in LocalConfig. A renewal that finishes between `log_out` and Flutter clearing `access_token` also writes the ticket back, because `fetch` compares against the still-old token. The stored ticket is unused, but doc 7 says the login's ticket goes at logout.
(b) `--uninstall` forgets the binding but leaves HJViewer_ticket.json, valid for up to 72 h, until a reinstalled service starts. `restore_bound(false)` also ignores a failed removal silently.
(c) `Held` and `Sent` derive Debug with the ticket string. hjv-api's `Issued` deliberately has a Debug that leaves it out. Fix: (a) Clear ACCOUNT_TICKET where it is read while `logged_in()` is false (`login_ticket`, `renew`). That is a two-line change with no new state.
(b) Remove the file in `uninstall_line` (or `forget_pc_binding`), and log a failed removal in `restore_bound`.
(c) Write Debug for `Held` and `Sent` by hand, without `t`.
- [minor] src/client.rs:5994-6066 (access_ticket_tests); src/hjviewer_access/tests.rs; fakes at src/hjviewer_account/api.rs:926 and pc/tests.rs:1047: The client test calls upstream's `request_relay` with a token argument, and `create_relay`. C1 changes neither function, so apart from the `is_refusal` call the test passes on 398fa05 and does not pin C1's change.
No test reaches the hooks:
- `Client::start` sending `controller_token`'s value;
- `register_pk` filling field 1001;
- the login and sync answers reaching hjviewer_access.
`Env::login_ticket` and `Service::keep_ticket` are no-ops by default in both fakes, so the account and sync tests never see a ticket. That is how the first finding's wrong member names went unnoticed.
Brief 6-2 also asked to turn `rendezvous_token_only_kept_in_unpinned_builds` into a ticket test, which was not done. Fix: Add the two fake-based wiring tests from the first finding. They fail on the parent commit and with the wrong names. Then drop the relay test, or reword it as a guard of the upstream behaviour it actually pins.
- [minor] cross-package: src/rendezvous_mediator.rs:1261-1269 (30 s NOT_DEPLOYED retry); hbbs package build/wt/staff-server c44e975 src/rendezvous_server.rs:563-570 and 1463 (check_ip_blocker runs before the ticket decision and counts refused registrations): Under enforce, every PC without a ticket resends RegisterPk every 30 s indefinitely. That covers unbound 1.8.0 PCs and all 1.6.1 and 1.7.0 PCs. hbbs counts each resend against the per-address limiter, which allows about 31 registrations and resets only after 60 s with no allowed one.
Behind one public address, such as campus Wi-Fi NAT, about 16 refused PCs keep the limiter full. Bound PCs at that address then get TOO_FREQUENT when they reconnect or re-register at half-life. The client logs that as "unknown RegisterPkResponse" and retries, so their registration is delayed.
Old apps behave the same way, so this is not C1's defect alone, but C1 relies on NOT_DEPLOYED's retry. Fix: Preferred, in S: count registrations that the ticket check refuses separately, so they cannot crowd out admitted PCs at the same address.
Alternative, in C1: lengthen the NOT_DEPLOYED retry while the PC holds no ticket. It already re-registers at once when a ticket arrives. The cost is slower recovery after MODE=off.

## C1: fixes

### summary

I fixed C1's review findings in commit 8f973c2 and exported it as patch 0059. Nothing is pushed. All three majors held and are fixed. The package's tests and both Rust 1.75 checks pass.

**Majors**
1. **The client didn't match hjv-api's answer shapes.** Checked against the staff-api branch at 957ecbd and 292b031:
   - the login's ticket is now read from `hjv.access`;
   - the sync answer's member is now `access`;
   - the device key now signs "hjv-ticket-pc-v1".

   Two new tests check that the login and sync answers reach the access module. The request signature is pinned to the vector in hjv-api's README.
2. **The `use` of a ticket was dropped.**
   - The app now keeps it, and a PC registers only with a `pc` or `both` ticket.
   - A desktop app whose login gave it `ctl` now asks for a `both` ticket right after login. P's 292b031 makes this the contract (doc 3-3).
   - A ticket that came from that request waits for half its lifetime, so the App Review account's `ctl` does not loop.
3. **A sync answer without a ticket no longer drops the bound PC's ticket.** Only a signed revoked or an unbind does.

**Minors**
- **TLS fallback:** pinned builds now fix "Allow insecure TLS fallback" off. The settings screen shows it as a fixed checkbox, and the E2E check K2 needs rewording.
- **Leftover ticket after logout:** the review's suggested fix (clear the ticket whenever it is read while logged out) would erase the new ticket at every login. Rust sees the login answer before Flutter stores the token. I added a one-line hook in ui_interface::set_local_option instead: clearing the token drops the ticket, and storing it wakes the renewing thread.
- **Ticket file at uninstall:** the uninstaller now deletes HJViewer_ticket.json. This is not tested on Windows. A failed removal at service start is now logged.
- **Debug output:** it no longer shows the ticket.
- **Relay test:** reworded to say it guards upstream code that already sends the ticket to hbbs and nothing to hbbr.

**Declined**
- The NOT_DEPLOYED retry finding: S already fixed it in 349b8dd.
- Rewriting the old token test as a ticket test: it would write the test user's real config.

**Known limit:** a portable PC refused after a NOT_BEFORE emergency or an ID change waits up to 12 h, or for a new login, to come back online. Fixing it needs new state.

Patch 0058's commit message still names the old values; the integrator may squash 0058 and 0059.

### tests

Each ran once after the last commit: client 8f973c2 (tree 7081b10a), hbb_common 861fe2a (tree 0a2c097e). Each log header names these commits.

- **Pinned account build** `--lib hjv` (HJV_SERVER=hjv.test plus the public test keys from vectors.rs): 257 passed, 0 failed, 2 ignored. Before: 254, plus 3 new tests.
- **Stable** `cargo test --locked --features linux-pkg-config,hjv-e2e --lib -- --skip test_get_cursor_pos --skip a_punch_that_meets_the_peers_syn_connects_both_ends`: 574 passed, 0 failed, 5 ignored, 74.5 s. Before: 571. The 2 filtered out are the two known WSL failures.
- **Rust 1.75** `cargo +1.75.0 check --locked --features linux-pkg-config,hjv-e2e` (build/ci/target-175): OK.
- **Rust 1.75** `cargo +1.75.0 check --locked --features linux-pkg-config,flutter`: OK. This is also the build without hjv-e2e. flutter_ffi.rs is unchanged, so the copied gitignored bridge files still match.
- **Warnings**: none points at a changed line. The ones at api.rs 845–877 and ui_interface.rs 1077/1666/1670 are older and also appear in the review's logs.
- **rustfmt --check**: clean for hjviewer_access.rs and its tests, api.rs, and pc.rs and its tests.
- **Patches**: on 398fa05's src (from git archive), git apply of 0058 and then 0059 reproduces all 15 changed files of HEAD byte for byte.

New and changed tests:
- api.rs Fake records `login_ticket` and `forget_login_ticket`. The test checks that `hjv.access` reaches the access module and that log_out and drop_login forget it. It fails with the old member name.
- pc tests Fake records `keep_ticket`, and FakeApi sends `access`. A sync-task test checks that a bound PC takes the ticket of each list and keeps it through a list that has none; the old code would record [first, None, second]. It also parses the README's sync answer to pin the member name.
- The request signature is now pinned to the vector in hjv-api's README (seed 0x02, token hjv_AAECA…, 116 signed bytes as hex, sig nG4V0N…). The body is checked as exactly {id, pk, sig}.
- `use` parsing; pc_choice skips a `ctl` ticket; a desktop app asks at once in place of the login's `ctl`, but not on a phone and not for what its own request brought.
- The uninstall line test now includes the ticket file.

Not run: E2E, scripts/ci.sh stages, Flutter/Dart, and Android/iOS/Windows builds.

### open

- 0058's commit message still says hjv.ticket and "hjv-ticket-dev-v1". 0059 corrects the code and says so in its own message; I did not amend 0058, as the task asked for a separate commit. The integrator may squash 0058 and 0059 or reword 0058 on import. Authors are ysfaculty, as the task said; the existing series uses 'hjviewer'.
- Finding 5(a), different from the review's fix. The suggested clear-on-read while logged out races the login. Rust stores the login's ticket when it sees the login answer, but Flutter writes access_token only afterwards (login.dart handleLoginResponse), and the renewing thread is woken inside that gap. The clear would therefore erase the fresh ticket. Instead, one hook in ui_interface::set_local_option, implemented in hjviewer_access::local_option_set:
- clearing access_token drops the login's ticket. That covers Flutter's own logout after a 401, and a renewal that finished just after a logout;
- storing access_token wakes the renewing thread.
The second part also fixes timing: before, the first wake found no login and the thread slept 10 min, which would have delayed finding 2's request after login.
- Finding 2, broader than suggested. The immediate request with the PC part covers every desktop app that has a device key, as doc 3-3 words it, not only apps that are not installed. An installed PC's UI process whose login gave `ctl` therefore sends one extra request and gets a `both` ticket. This is harmless.
Known limit, not fixed: a refused registration (NOT_DEPLOYED) still does not trigger a renewal. After a NOT_BEFORE emergency or an ID change, a portable PC stays offline until its next renewal (at most half the lifetime, 12 h) or a new login. Bound PCs recover at their next sync. Fixing it needs new state ('the last ticket hbbs accepted') to avoid a request loop.
- Finding 4 changes behaviour. Pinned builds now fix 'Allow insecure TLS fallback' off; the desktop settings show it as a fixed checkbox, like direct-server.
- Strict TLS already trusts the OS certificate store (rustls_native_certs), so managed SSL inspection keeps working.
- Under enforce, a network that needs the fallback cannot get tickets anyway, because hjv-api requests are strict.
- Cost: HTTPS proxies or interceptors with untrusted certificates no longer connect, even with MODE=off.
- In firewall-e2e.sh, K2 ('with the insecure TLS fallback allowed…') still passes but no longer exercises the fallback in pinned builds. The E package should reword it.
The integrator or the user may want to confirm this, given that firewall traversal is the top priority.
- Finding 5(b). The uninstaller's elevated batch now deletes %WINDIR%\ServiceProfiles\LocalService\AppData\Roaming\<app>\config\HJViewer_ticket.json. That is the folder Config::path gives LocalSystem after hbb_common's patch(). It uses the same `if exist … del /f /q` style as the batch's other lines, but I could not run it on Windows here. forget_pc_binding could not do this: it runs unelevated and in the user's config folder.
- Declined, with reasons:
- Finding 7: package S already fixed it in 349b8dd. Refused registrations now count against their own '<ip>/noticket' budget. Lengthening the client's 30 s retry would slow recovery after MODE=off and change upstream's mediator, and older apps retry every 30 s anyway.
- Part of finding 6: I did not turn rendezvous_token_only_kept_in_unpinned_builds into a ticket test. controller_token reads this process's real LocalConfig, so a test would write the test user's config. controller_choice's test covers the choice, and the hook is one line. The relay test stays, reworded as a guard of upstream request_relay and create_relay: the ticket goes to hbbs and none to hbbr (doc 3-1).
- Contract. The client now follows server/api/README.md and docs/access-tickets.md as of staff-api 292b031:
- the ticket object {ticket, use, exp, server_time};
- the login's ticket in hjv.access, and the top-level `access` in a sync answer;
- the context "hjv-ticket-pc-v1";
- the PC part on every desktop renewal.
If P changes any of these again, the places to change are Held::issued, api.rs login_answered, pc.rs Answer.access and DEVICE_CONTEXT.
- Housekeeping:
- The gitignored bridge_generated*.rs copied earlier are still in the worktree.
- The runs used the warm build/ci/target-client and build/ci/target-175.
- The main trees are untouched: hjviewer is still 5b6546a and clean, upstream/rustdesk hjviewer is still 398fa05.

### hooks_for_wave2

- E2E (E):
- `<app> --hjv-e2e-ticket [refresh]` now prints `hjv-e2e: ticket {"login","ticket":<8-byte sha256 hex>,"use","valid","left","life"}`.
- A portable desktop client's login ticket is `use: ctl` until its first ticket request, which carries the PC part and brings `both`.
- `--hjv-e2e-login` writes access_token straight into LocalConfig, not through ui_interface, so the renewing thread is not woken in that process. Use `--hjv-e2e-ticket refresh` to get `both` there.
- Reword K2 (the TLS-fallback case) in firewall-e2e.sh: pinned builds now fix the fallback off.
- Exit code 5 on an access refusal is unchanged.
- Guest exchange (Rust, wave 2): `keep_guest(target_id, &answer)` and `keep_guest_pc(&answer)` take the ticket object {ticket, use, exp, server_time}. A guest PC ticket registers only if its `use` is `pc` (or `both`), so POST /api/hjv/guest must send `use`, as hjv-api's other answers do. `forget_guest` and `forget_guest_pc` are unchanged; never persist these tickets.
- Dart (C2): unchanged.
- msgbox type `hjv-access-error` with the A1/A2 texts.
- P1 when `bind.mainGetOption('hjv-access-pc') == 'refused'`.
- `bind.mainSetCommon('hjv-access-refresh','')` at app start and on return to the foreground.
The app's write of access_token at login now also wakes the renewing thread, and clearing it drops the login's ticket. No Dart change is needed for either.
- Docs (D):
- LocalConfig `hjv-access-ticket` and the service's `HJViewer_ticket.json` hold `{"t","use","until","life","from_login"}` (device-clock seconds).
- The login's ticket goes at the explicit logout, and whenever the app clears access_token, including its own logout after a 401.
- A bound PC keeps its ticket through sync answers that carry none, until exp. It drops it on a signed revoked or an unbind.
- The uninstaller deletes HJViewer_ticket.json.
- Pinned builds fix direct-server, enable-lan-discovery and allow-insecure-tls-fallback to N.
- docs/architecture.md:167, which says what the uninstall batch removes, can name the ticket file too.
- hjv-api (P): nothing more for wave 1. The client follows README 292b031:
- the ticket object including `use`;
- `hjv.access` in the login answer and `access` in the sync answer;
- the device signature context "hjv-ticket-pc-v1";
- a desktop app given `ctl` asks POST /api/hjv/ticket with its PC part right after the login, and every desktop renewal carries the PC part.
The README's request vector is pinned in the client's tests.

