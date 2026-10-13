# Test vectors of the signed version policy

Envelopes of HJViewer's version policy (the file apps fetch from `/api/hjv/version`) that an app
must accept or must refuse, for every implementation of the rules: this tool (`src/trust.rs`), the
client core and the server-side check script read the same bytes.

**Test keys only.** The private halves of every key here are published (`src/keys.rs`,
`TEST_KEYS`): seeds of 32 consecutive bytes. A build for the real server must never hold one of
these keys; `hjv-release keys --production` and CI refuse them.

The files are made by `tests/vectors.rs` and checked by it on every test run: a file that differs
from what the code makes fails the test. To make them again:

```sh
HJV_RELEASE_WRITE_VECTORS=1 cargo test --test vectors
```

## Files

| File | What |
|---|---|
| `manifest.json` | every case with its expected outcome (below), the keys, the domains |
| `update-keys.env` | the key file of the vectors, in the form of `client/update-keys.env`: signing keys 1 and 2, root key 101, floor `1:0` |
| `accepted/*.json` | envelopes an app accepts, in the state its case names |
| `refused/*.json` | envelopes an app refuses |
| `root-declaration-e2.json` | the root declaration of epoch 2 by itself: `{"k":101,"p":…,"s":…}`, as it is carried inside the envelopes from `e2-s1` on |
| `research/version-policy.json`, `research/vector.json` | the research vector (first draft of the format, no `epoch`): its signature is good for signing key 1, and signing its `p` again gives the same bytes; as a policy it is refused |
| `keyfile/signing-1.age` | an encrypted key file made by the age crate (not by this tool), passphrase in the manifest; holds test key `signing-1` |

Every envelope file is one line and a line feed, exactly as `hjv-release sign` writes it.

## What is signed

- The envelope: `{"v":1,"k":<key id>,"p":"<policy text>","s":"<signature>","root":{"k":…,"p":"<declaration text>","s":…}}`;
  `root` is optional. `s` is standard base64 (with padding) of the 64-byte Ed25519 signature.
- The policy's signature covers `"hjv-version-v1"`, a zero byte, and the UTF-8 bytes of `p`; the
  root declaration's covers `"hjv-version-root-v1"`, a zero byte, and the bytes of `root.p`.
  Verify first, parse afterwards, and never serialize `p` again.
- The policy text has its fields in the order of the design's table, without spaces:
  `v, epoch, seq, issued, server, contact_ko, platforms{windows, android, android_play, ios}`;
  an entry has `min, latest, grace_until, os_min, notes_ko, source, package{file, size, sha256, build}, auto_install_after, move_to_play, version_code, testflight_build`
  (optional fields are left out; `move_to_play` appears only as `true`). `v`, `epoch`, `seq`,
  `size` and `version_code` are JSON integers; `build` and `testflight_build` are strings; times
  are `YYYY-MM-DDTHH:MM:SSZ`.
- The root declaration text: `v, server, epoch, revoked, issued`; `revoked` is an ascending list
  of key ids.

## A case in `manifest.json`

| Field | Meaning |
|---|---|
| `name` | the case |
| `file` | the envelope to hand to the app |
| `after` | envelopes the app has accepted before, in this order, starting from a fresh install (no stored policy, the floor of its build). All of them are in `accepted/` |
| `floor` | the build's `HJV_POLICY_FLOOR` when it is not the key file's `1:0` |
| `expect` | `accept` or `reject` |
| `state_after` | for `accept`: the epoch, the stored policy's `seq`, the revoked key ids and the SHA-256 of the stored `p` afterwards |
| `then_accept` | for some `reject` cases: an envelope the same app must still accept afterwards (a refused envelope leaves nothing behind, not even its valid root declaration) |
| `rule` | which rule decides: `signature`, `key`, `envelope`, `version`, `server`, `size`, `root-declaration`, `revoked`, `epoch`, `seq`, `floor`, `relaxation`, `fields` (`first` for the first policy) |
| `binding` | `app`: the design says what an app does (design 1-4, 6-2). `tool`: only this tool is that strict (it reads nothing it would not write); an app may accept such an envelope |
| `why` | what the case shows |

The keys an app holds in every case are those of `update-keys.env`, and its server name is
`rd.example.com`.

## The history

`e1-s1` → `e1-s2` → `e1-s3` → `e2-s1` → `e2-s2` → `e2-s3-root-relax` → `e2-s4`:
the first policy; every `min` raised with a grace period; a new release with packages (signed by
the second signing key); signing key 1 revoked and epoch 2 started by the root key; the
unattended install date removed; a relaxation signed by the root key; `min` raised again by a
signing key. The other cases branch off this history.

Rules the cases pin down that the design leaves to the reader:

- A fresh install accepts a policy exactly at its floor (`floor-exact`) and none below it
  (`below-floor*`), whoever signed.
- A signing key's `seq` may be at most 1000 above the stored one, inclusive (`seq-step-1000`,
  `seq-step-1001`); a fresh install with floor `1:0` takes up to `seq` 1000.
- The same (epoch, seq) with the same bytes is accepted and changes nothing (`e1-s2-again`).
- Revocations are remembered without the declaration (`revoked-key-later`) and within an epoch
  (`revoke-in-the-same-epoch`, `revoked-in-the-same-epoch`).
- A policy signed by the root key may carry a higher epoch without a declaration
  (`root-policy-raises-epoch`); a signing key's may not (`signing-key-raises-epoch`). The root
  key does not go back an epoch either: a policy it signed in an earlier epoch is refused
  however high its `seq` (`root-policy-of-an-old-epoch`).
- The size limit is 16384 bytes of the file, its line feed counted (`size-at-the-limit`,
  `size-one-over`).
- Signatures are checked strictly: one whose R is the identity point satisfies the equation of
  Ed25519 and is refused (`signature-with-identity-r`). libsodium (the apps) and
  ed25519-dalek's `verify_strict` (this tool) refuse a small-order R; a checker that only
  evaluates the equation (OpenSSL's Ed25519) takes this envelope, and has to look at the first
  32 bytes of the signature itself to agree with the apps. Only the holder of the key can make
  such a signature, so the case is about agreeing, not about forgery.

## After a policy of the root key

An app compares a new policy with the policy *it holds*, and the server hands out only the
newest one. An app that was off while the root key's relaxation `e2-s3-root-relax` was served
still holds `e2-s2`:

- To that app the history's `e2-s4` is a relaxation by a signing key (it puts off the deadline
  of a `min` the app knows), so it refuses `e2-s4` and every later policy of a signing key that
  builds on it (`e2-s4-missed-the-relaxation`).
- The other way back keeps every `min` and deadline as they were before the relaxation: both
  kinds of app take it (`restore-with-the-old-deadline`,
  `restore-with-the-old-deadline-missed-the-relaxation`). The release tool checks this when a
  signing key's policy follows a root key's (`policy --held`).

Two things the rules allow a signing key although they enforce less than before, pinned here so
that every implementation agrees on them:

- a new entry with a `min` below every other (`new-entry-below-the-minimums`): no relaxation,
  since there is no old entry to compare with. hbbs's one threshold (the lowest `min`) and the
  PC's threshold for Android controllers (the lower of the two Android entries) follow it down.
  The release tool asks the root key for such a policy;
- a raised `min` with a later deadline (`min-raised-with-new-grace`): until that deadline apps
  below the *old* `min` are not blocked either. The release tool says so.
