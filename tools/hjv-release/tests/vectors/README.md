# Test vectors of the signed version policy

Envelopes of HJViewer's version policy (the file apps fetch from `/api/hjv/version`) that an app
must take or must refuse, for every implementation of the trust rules of 1.6.0: this tool
(`src/trust.rs`), the client core and the server-side check script read the same bytes. The rules
themselves are the design's (not part of this repository); `model.py` is their executable form,
and these vectors are what every implementation is held to.

**Test keys only.** The private halves of every key here are published (`src/keys.rs`,
`TEST_KEYS`): seeds of 32 consecutive bytes. A build for the real server must never hold one of
these keys; `hjv-release keys --production` and CI refuse them.

## How they are made

`generate.py` makes every file here from the cases of spec 7-4 (the design's list), the cases
decided on 2026-10-14 (`cases_of_2026_10_14`) and the App Store member of 2026-10-26
(`cases_of_2026_10_26`: `ios-appstore-member`, `ios-appstore-as-number`, auto-deploy design 3-7
T3 and T4). `model.py` is the executable reference model of the
rules, a **byte-identical copy** of the design's model (`generate.py` refuses to run when its
SHA-256 is not the one it names). The verdicts are the
model's: `generate.py` reads every envelope back from its bytes as an app does (spec 2-2, steps A
to C: size, UTF-8, the JSON rules, strict Ed25519 with the pinned test keys, the member rules),
runs the model's `accept()` over the case's history and the case's file, and writes what the model
says; it stops when that differs from the model's own result for the same case of 7-4.

```sh
python3 -I -B generate.py          # writes the files
python3 -I -B generate.py --check  # compares the files with what it makes, and runs the model
                                   # over every file on disk against manifest.json
```

`cargo test --test vectors` runs the check, and also runs the tool's own reading of the rules over
every case (see "Who checks what").

## Files

| File | What |
|---|---|
| `manifest.json` | every case with what an app does with it (below), the keys, the domains |
| `update-keys.env` | the key file of the vectors in the form of `client/update-keys.env`: signing keys 1 and 2, root keys 101 and 102, floor `1:0` |
| `accepted/*.json` | envelopes an app takes (`binding: app`) |
| `refused/*.json` | envelopes an app refuses (`binding: app`) |
| `tool/*.json` | envelopes an app takes and the tool (and the server check) refuse (`binding: tool`) |
| `covers.json` | `COVERS(new, old)` of spec 2-3 for all 256 pairs of 16 entries (versions 0.0.0, 1.5.0, 1.6.0, 1.6.1; no deadline, 2026-10-16T15:00:00Z, 2026-10-23T15:00:00Z) |
| `root-declaration-e2.json` | the root part of the declaration of epoch 2 by itself, `{"k":101,"p":…,"s":…}`, as `e2-s1` and `e2-s2` carry it |
| `research/` | the research vector (first draft of the format, no `epoch`): its signature is good for signing key 1; as a policy it is refused |
| `keyfile/signing-1.age` | an encrypted key file made by the age crate (not by this tool), passphrase in the manifest; holds test key `signing-1` |
| `generate.py`, `model.py` | the generator and the model (above) |

Every envelope file is one line and a line feed, exactly as `hjv-release sign` writes it, unless
its case says `canonical: false`.

## What is signed

- The envelope: `{"v":1,"k":<key number>,"p":"<policy text>","s":"<signature>","root":{"k":…,"p":"<declaration text>","s":…}}`;
  `root` is optional. `s` is the canonical standard base64 (with padding) of the 64-byte Ed25519
  signature: 88 characters, `==` at the end, the character before it one of `A`, `Q`, `g`, `w`.
- The policy's signature covers `"hjv-version-v1"`, a zero byte, and the UTF-8 bytes of `p`; the
  declaration's covers `"hjv-version-root-v1"`, a zero byte, and the bytes of `root.p`. Verify
  first, parse afterwards, never serialize a text again. Verification is strict (libsodium,
  `verify_strict`): a small-order R or an S at or above the group order is refused.
- Policy text members, in this order, without spaces: `v, epoch, seq, issued, server, contact_ko,
  platforms{windows, android, android_play, ios}`; an entry: `min, latest, grace_until,
  grace_min, os_min, notes_ko, source, package{file, size, sha256, build}, auto_install_after,
  move_to_play, version_code, testflight_build, appstore` (optional members left out;
  `move_to_play` only as `true`; `grace_until` and `grace_min` both or neither, `grace_min`
  below `min`). `appstore` (the tool of 1.9.1 on, iOS only: the App Store's version) is a member
  the apps of 1.6.0 on do not know and skip; `ios-appstore-member` holds every implementation to
  taking such a policy, and the tool and the server check to reading it as canonical;
  `ios-appstore-as-number` (`binding: tool`) holds the tool and the server check to refusing the
  member as a number (not canonical), which the apps take as any unknown member.
- Declaration text: `v, server, epoch, revoked, issued, base{windows, android, android_play,
  ios}`, a base entry `min, latest, grace_until, grace_min`; `epoch` at least 2; `revoked`
  ascending and cumulative.

## A case in `manifest.json`

Every field describes what an **app** does (the client core of 1.6.0), unless it says otherwise.

| Field | Meaning |
|---|---|
| `name` | the case (the names of spec 7-4; `version-*` and `package-*` are expanded) |
| `file` | the envelope to hand to the app |
| `after` | envelopes the app has taken before, in this order, from a fresh install (no stored state, the floor of its build); each is `Taken` or `Unchanged` |
| `floor` | the build's `HJV_POLICY_FLOOR` when it is not the key file's `1:0` |
| `binding` | `app`: every implementation gives this case's outcome. `tool`: the app does what the case says, and the tool and the server check refuse the envelope (canonical form, or a rule only they keep: URLs in texts, a root key number in `revoked`) |
| `canonical` | whether the envelope, its `p` and its `root.p` are in the form the tool's strict reader keeps (below) |
| `expect` | `accept` or `reject` |
| `outcome` | for `accept`: `taken` (the state changed) or `unchanged` (the same policy again) |
| `reason` | for `reject`: the code of spec 2-4 (`Field(<member>)` written `Field:<member>`) |
| `state_after` | for `accept`: the state afterwards (below) |
| `state_after_refusal` | for a `reject` that changed the state (a declaration taken in step B, kept although the policy part is refused): the state afterwards. A `reject` without it leaves the state exactly as it was |
| `app_expect` | for `binding: tool`: what the app does (`accept` for all of them; the same as `expect`) |
| `then_accept`, `then_outcome` | an envelope the same app takes next, **from the state the case left** (its `state_after_refusal`, or the unchanged state), and with which outcome |
| `why` | what the case shows |

A state: `epoch` is the device epoch `E` of spec 1-5 (`max(floor epoch, held declaration's
epoch)`); `seq` the held policy's (or null); `revoked` the held declaration's list; `root_sha256`
the SHA-256 of the held declaration's text `root.p` (null without one; the declared epoch is that
declaration's, or 1); `root_k` the key number its root part names; `p_sha256` the SHA-256 of the
held policy's text `p` (null without one); `enforces` what `EFF` is (`policy`, `base`: the held
declaration's base read as a policy, `nothing`); `may_install` is `MAY_INSTALL` (spec 1-5: the
held policy is of the device epoch, at or above the floor, and not by a revoked key).

The keys an app holds in every case are those of `update-keys.env`; its server name is
`rd.example.com`.

### Canonical

`canonical: true`: the file is exactly what the tool writes, with every member the tool always
writes and only values it can write: one line and a line feed; no member is given twice and none
is `null`; only the members of spec 1-3 appear, in 1-3's order, each with its JSON type (unsigned
integers up to 2^64-1, `move_to_play` only `true`, `package.build` a string); strings escaped as
serde_json writes them; the envelope's `k` is 1-65535 and every `s` is the canonical base64 of 64
bytes; a policy text has its required members (`v, epoch, seq, issued, server, platforms`), a
declaration text all six of its members (`v, server, epoch, revoked, issued, base`). Value rules
are not part of it: an envelope or text of version 2, a version or time of the wrong form or year,
a text too long, another server, a declaration of epoch 1 is canonical, and the tool gives the
app's reason for it. Not canonical: `key-number-zero`, `key-number-as-text`, `signature-63-bytes`,
`signature-trailing-bits`, `body-not-utf8`, `duplicate-field`, `grace-min-as-number`,
`policy-without-epoch`, `package-build-1e400`, `declaration-without-base`,
`lone-surrogate-in-known-member`, and eight of the `binding: tool` cases (among them
`declaration-without-revoked`).

The definition was settled on 2026-10-14: a key number 0, a policy without `epoch` and a
declaration without one of its six members are not canonical. Nothing but the server check's
`Canonical` answer depends on it: the app refuses those cases for the reasons given, or reads past
a missing `revoked` as no revocation.

Times: an app reads the years 0001-9999 (`issued-year-0000` is refused for its `issued`); the
tool writes and signs only 2000-2199, a rule of its own (`issued-1999`, `binding: tool`).

### Who checks what

- **The client** runs every case and must give `expect`, `outcome` or `reason`, and the state
  (`state_after`, `state_after_refusal`, or unchanged), then `then_accept`; and reads
  `covers.json`. It alone decides the non-canonical cases (spec 4-1, INV-13).
- **The tool** (`tests/vectors.rs`) gives the same for every canonical case, and refuses every
  non-canonical one and every `binding: tool` one.
- **The server check** (`deploy/hjv-update-verify.sh`) answers `Canonical` for every
  non-canonical case and the app's outcome, reason and state for the others; for `binding: tool`
  cases that are canonical, `app_expect`.

## The history

`e1-s1` → `e1-s2` → `e1-s3` → `e2-s1` → `e2-s2` → `e3-s1-relax` → `e3-s2`:

- `e1-s1` (signing key 1): the first policy, 1.6.0 out, every min 1.5.0, nothing enforced;
- `e1-s2` (key 1): every min 1.6.0 until 2026-10-16T15:00:00Z with `grace_min` 1.5.0;
- `e1-s3` (key 2): 1.6.1 for Windows and Android with packages, an unattended install date, the
  Google Play entry at 1.6.0 without a deadline;
- `e2-s1` (key 2): the declaration of epoch 2 by root key 101, revoking signing key 1, base =
  `e1-s3`'s requirements; `seq` starts again at 1;
- `e2-s2` (key 2): the same declaration; `auto_install_after` removed (the emergency stop);
- `e3-s1-relax` (key 2): the declaration of epoch 3 by root key 101, revoked `[1]`, every min
  1.5.0 without deadlines (base = this policy's requirements);
- `e3-s2` (key 2): 1.6.1 (iOS 1.6.0) until 2026-10-23T15:00:00Z with `grace_min` 1.5.0.

The other cases branch off this history.

## Rules the cases pin down

- A policy is taken only if it asks no less than the device's basis at every time, entry by
  entry and for the Android pair (`raise-*`, `android-play-*`, `min-raised-with-new-grace`,
  `new-entry-below-the-minimums`); only a declaration asks less, and it opens a new epoch
  (`relax-*-by-declaration`, `e3-s1-relax`).
- A root key's policy has no special rights: it may not relax, jump or move the epoch
  (`relax-*-root`, `root-seq-jump`, `root-policy-raises-epoch`); it is taken like any other when
  it asks no less (`root-signed-policy-taken`).
- Revocations come only with a new epoch: a declaration of epoch 1 is refused
  (`revoke-in-the-same-epoch`, `declaration-of-epoch-1`); a revoked key stays revoked for an
  envelope of its epoch (`revoked-key-later`).
- Every policy of epoch 2 or later travels with its epoch's declaration (`epoch-2-without-declaration`,
  `e2-s2-stripped`, `epoch-differs-from-declaration`).
- A fresh install takes a policy exactly at its floor (`floor-exact`) and none below it
  (`below-floor*`), whoever signed it. A policy may be at most 1000 above the stored `seq`,
  inclusive (`seq-step-1000`, `seq-step-1001`); a fresh install with floor `1:0` takes up to 1000.
- The same position with the same bytes changes nothing (`e1-s2-again`, outcome `unchanged`);
  other bytes there are a fork (`same-position-other-bytes`).
- The size limit is 16384 bytes of the file, its line feed counted (`size-at-the-limit`,
  `size-one-over`). The body must be UTF-8 before anything is read (`body-not-utf8`).
- Signatures are checked strictly (`signature-with-identity-r`: OpenSSL's Ed25519 takes it,
  libsodium and `verify_strict` do not; `signature-trailing-bits`: a lenient base64 decoder reads
  the same 64 bytes).

## After a root declaration

A declaration is judged before the policy part (step B before C). A device takes it and keeps it
whatever happens to the policy in the same envelope, and of the two signed parts it holds, the
later wins:

- a good declaration with a policy part that is refused is kept: `good-declaration-bad-policy`,
  `revoked-key-with-declaration`, `unknown-key-with-declaration`, `epoch-differs-from-declaration`,
  `below-floor-in-epoch-2`, `issued-before-declaration`, `policy-below-its-base` and
  `relaxation-declaration-alone` all leave `enforces: base` and `may_install: false`, and the next
  good envelope is taken from there (`then_accept`);
- a device that missed a relaxation takes the next envelope, declaration first, and compares its
  policy with the declaration's base (`e3-s2-missed-the-relaxation`);
- a forged policy (a stolen key's `forged-minimum`) is lifted by the declaration that revokes the
  key, also when only the declaration gets through (`forgery-*`);
- an older declaration is refused (`declaration-epoch-back`), another text for the device's epoch
  too (`declaration-fork`); either pinned root key signs a declaration (`declaration-by-second-root`),
  and no declaration may revoke a pinned root key (`declaration-revokes-*-root`).
