// SPDX-License-Identifier: AGPL-3.0-only

//! The PC sync (design v3 §3-6, §4-5, §5-2): `POST /api/hjv/pc/sync`, how the service of a bound
//! PC learns, every two minutes or so, which account keys may open it without its password.
//!
//! **Request**: the signed envelope of envelope.rs (relay/PROTOCOL.md §4) with the context
//! `hjv-pc-v1` and the operation `sync`, signed by the PC's device key, at most 16 KiB:
//!
//! ```text
//! {"id":"<PC ID>","t":<time>,"n":"<base64url of 16 bytes>","p":"<payload text>","s":"<base64 signature>"}
//! "hjv-pc-v1" NUL "sync" NUL id NUL t NUL n NUL base64(SHA-256(p))
//! ```
//!
//! and the payload `p`, a JSON object:
//!
//! ```text
//! {"v":1,"binding":"<32 hex or empty>","ver":"1.6.0","off":false,
//!  "used":[{"pk":"<base64 account key>","day":"2026-10-20","last":<time>,"n":3}],
//!  "refused":{"unknown_key":0,"kind":0,"temporary_only":0,"not_secured":0,"bad_sig":0,"expired":0,"off":1},
//!  "wol_hash":"<hex or empty>","wol":{…}}
//! ```
//!
//! `binding` is the binding the PC holds (or asks for, after a login on it; empty after it let
//! one go), `off` whether the PC's users switched password-less connections off there, `used`
//! (U6) per account key and UTC day the latest time the PC let it in and that day's running count,
//! `refused` the counts of refused proofs by reason since the last report the server took, under
//! the design's seven reasons (other names are left out, so that a PC adds at most seven rows a
//! day). `wol` is the PC's wake report (design v3 §8-2, wolinfo.rs) and `wol_hash` the PC's
//! summary of it, compared for equality only. Only `v` and `binding` are required; a field of the
//! wrong JSON type refuses the request, a value out of range (a day outside the last 30, a time
//! not of its day, a key of no account of the PC) is left out, and a use's time after the server's
//! time of the report is taken as that time. A `wol` object that is not a report is left out
//! rather than refusing the sync, whose trust list matters more.
//!
//! **Wake-on-LAN** (design v3 §8-2): a sync that carries the PC's binding keeps its `wol` with its
//! `wol_hash` and the address it came from (X-Real-IP), the PC's egress address for choosing a
//! relay; a sync without `wol` moves only the egress address, and when its `wol_hash` is not empty
//! and not the summary kept, the answer asks for the whole report (`send_wol: true`). An empty
//! `wol_hash` (the PC's first minute, no report on its platform) leaves the report kept as it is.
//! The report goes when the PC leaves its account (db.rs `release_binding`).
//!
//! **Checks**, in this order, the first failure answering (design v3 §5-2):
//!
//! 1. size, 16 KiB at most: 413 `too_large`;
//! 2. the envelope's shape (and a JSON body): 400 `bad_request`;
//! 3. the row of `id`: the PC's binding (`devices`), else the ID's latest tombstone; neither:
//!    404 `unbound`, unsigned;
//! 4. the signature, strictly, with that row's public key (the key hbbs held when the PC was
//!    bound): 401 `bad_signature`. For a bound PC hjv-api then asks hbbs for the PC's key, at most
//!    once a minute per PC: when hbbs signs another key than the stored one, the PC is
//!    `needs-relogin` (§4-6) and the change serial goes up;
//! 5. `|t − server time| ≤ 300 s`: 400 `clock` with `server_time`;
//! 6. (`id`, `n`) not accepted in the last 600 seconds: 409 `replay`;
//! 7. the payload: 400 `bad_request`.
//!
//! A request refused in steps 1-6 changes nothing but the needs-relogin of step 4. Errors are
//! `{"error":"<code>","server_time":<time>}`. There is no 429: a PC that asks again within ten
//! seconds while the change serial stayed the same gets the last list's contents again, signed
//! for its new `n`, without the list being read again from the database.
//!
//! **Answer.** An accepted sync of a bound PC stores its report: the time of the sync (which also
//! counts as the PC seen, so that the upkeep does not release a PC in use), the version, `off`,
//! the PC's mark (`present`: it sent its binding, `absent`: none, `stale`: another one), the
//! client address it came from (X-Real-IP, in place of the last one: the super operator's view,
//! overview.rs), the key use merged by the larger values and the refusals added under the server's
//! day; a sync signed with the stored key ends a needs-relogin. Then, by the row found and the
//! binding sent:
//!
//! | row | `binding` sent | answer |
//! |---|---|---|
//! | bound | the binding | a signed list, `active` (the account's keys that are not revoked) or `disabled` (the account's switch is off, or it is off the allowlist or blocked: no keys) |
//! | bound | empty, or one no tombstone of the ID has | 200 `listed`, no list |
//! | bound | a released binding of the ID | a signed `revoked` |
//! | tombstone only | a released binding of the ID | a signed `revoked` |
//! | tombstone only | empty, or another one | 404 `unbound`, unsigned |
//!
//! ```text
//! 200 {"server_time":<time>,"state":"active","refresh":120,"send_wol":false,
//!      "list":{"kid":1,"l":"<base64 of the list bytes>","s":"<base64 signature>"}}
//! 200 {"server_time":<time>,"state":"listed","refresh":120}
//! 404 {"error":"unbound","state":"unbound","server_time":<time>}
//! ```
//!
//! The list (trust.rs) answers this request only (`req` = its `n`), names the PC, its key (the
//! key that checked this request's signature) and the binding it sent, and lives HJV_API_TRUST_TTL
//! from its receipt on the PC. The outer `state` is not signed: the PC drops its binding only for
//! a signed `revoked`, and nothing changes there on an unsigned answer.

use crate::{
    api::{is_json, ApiRequest, BodyError, Reply},
    db::{self, Device, SyncRow, UseLine},
    envelope::{self, Envelope, Refusal, Spec},
    hbbs::{self, Lookup},
    state::AppState,
    trust::{mask_name, ListEntry, State as ListState, TrustList},
    util::{b64, b64_decode, date, day_number, hex, now, valid_device_id, DAY, HOUR, MINUTE},
    wolinfo,
};
use hyper::{header::ORIGIN, StatusCode};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

pub const PATH: &str = "/api/hjv/pc/sync";
/// The largest sync body (design v3 §5-2 step 1).
pub const MAX_BODY: usize = 16 * 1024;
/// The envelope of a PC's sync.
pub const SPEC: Spec = Spec {
    context: "hjv-pc-v1",
    op: "sync",
    max_body: MAX_BODY,
    id_ok: valid_device_id,
    with_pk: false,
};
/// A sync within this time of the last list built for the PC, with the change serial as it was,
/// gets that list's contents again (design v3 §5-2, §5-5).
pub const RESIGN_WINDOW: Duration = Duration::from_secs(10);
/// hbbs is asked about a PC's key at most this often per PC (design v3 §4-6).
pub const KEY_CHECK_EVERY: Duration = Duration::from_secs(60);
/// The largest count of one key's uses on one day taken from a report.
pub const MAX_DAY_USES: i64 = 1_000_000;
/// The reasons of the report's `refused` (design v3 §3-6), the only ones counted: a report
/// cannot add more than these rows a day to a PC's refusals, whatever names it carries.
pub const REFUSAL_REASONS: [&str; 7] = [
    "unknown_key",
    "kind",
    "temporary_only",
    "not_secured",
    "bad_sig",
    "expired",
    "off",
];
/// The largest refusal count taken from one report.
const MAX_REFUSALS: i64 = 1_000_000_000;
/// Entries kept in each of the in-memory maps below; past it they are cleared.
const MAX_ENTRIES: usize = 10_000;

// ---- the payload ----

/// What a sync's payload says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// the binding the PC holds or asks for; `None` for ""
    pub binding: Option<[u8; 16]>,
    /// the app version, '' unless it is a plain version string
    pub ver: String,
    pub off: bool,
    /// the lines of `used` that count: an account key, a day of the last USE_DAYS (or tomorrow,
    /// for a PC clock ahead), a time on that day, a count of 1 to MAX_DAY_USES; a time after the
    /// server's time of the report is taken as that time
    pub used: Vec<UseLine>,
    /// the counts of `refused` above zero, under the REFUSAL_REASONS only
    pub refused: Vec<(String, i64)>,
    /// the PC's summary of its wake report, '' for none
    pub wol_hash: String,
    /// the wake report, when the sync carried one that reads (wolinfo.rs)
    pub wol: Option<wolinfo::Report>,
    /// the sync carried a `wol` object that is not a report: not kept, and not asked for again
    pub wol_unread: bool,
}

/// A binding ID as the login answer and the PC write it: 32 lowercase hex digits.
pub fn parse_binding(text: &str) -> Option<[u8; 16]> {
    if text.len() != 32
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

/// A version as the apps write it ("1.6.0"), else ''.
fn version(text: &str) -> String {
    // digits and dots only, as a login's version (login.rs): the admin output shows it
    let plain = (1..=32).contains(&text.len())
        && text.split('.').count() <= 4
        && text
            .split('.')
            .all(|n| !n.is_empty() && n.len() <= 6 && n.bytes().all(|b| b.is_ascii_digit()));
    if plain {
        text.to_owned()
    } else {
        String::new()
    }
}

/// Reads a payload at the server time `at`; `None` when it is not one (step 7).
pub fn parse_report(p: &str, at: i64) -> Option<Report> {
    let value: Value = serde_json::from_str(p).ok()?;
    let object = value.as_object()?;
    if object.get("v").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    let binding = match object.get("binding")?.as_str()? {
        "" => None,
        text => Some(parse_binding(text)?),
    };
    let ver = match object.get("ver") {
        None => String::new(),
        Some(v) => version(v.as_str()?),
    };
    let off = match object.get("off") {
        None => false,
        Some(v) => v.as_bool()?,
    };
    let used = match object.get("used") {
        None => Vec::new(),
        Some(v) => used_lines(v.as_array()?, at)?,
    };
    let refused = match object.get("refused") {
        None => Vec::new(),
        Some(v) => refusals(v.as_object()?)?,
    };
    // Wake-on-LAN's: a summary of at most 128 characters, compared for equality only
    let wol_hash = match object.get("wol_hash") {
        None => String::new(),
        Some(v) => v.as_str()?.chars().take(128).collect(),
    };
    let (wol, wol_unread) = match object.get("wol") {
        None => (None, false),
        Some(v) if v.is_object() => {
            let wol = wolinfo::parse_report(v);
            let unread = wol.is_none();
            (wol, unread)
        }
        Some(_) => return None,
    };
    Some(Report {
        binding,
        ver,
        off,
        used,
        refused,
        wol_hash,
        wol,
        wol_unread,
    })
}

/// The lines of `used` that count; `None` when one is not of the shape.
fn used_lines(items: &[Value], at: i64) -> Option<Vec<UseLine>> {
    let today = at.div_euclid(DAY);
    let mut lines = Vec::new();
    for item in items {
        let item = item.as_object()?;
        let pk = item.get("pk")?.as_str()?;
        let day = item.get("day")?.as_str()?;
        let (last, n) = (item.get("last")?, item.get("n")?);
        if !(last.is_i64() || last.is_u64()) || !(n.is_i64() || n.is_u64()) {
            return None;
        }
        let Some(pk) = b64_decode(pk).and_then(|k| <[u8; 32]>::try_from(k).ok()) else {
            continue;
        };
        let (Some(last), Some(n)) = (last.as_i64(), n.as_i64()) else {
            continue;
        };
        let in_window =
            day_number(day).is_some_and(|d| (today - (db::USE_DAYS - 1)..=today + 1).contains(&d));
        if !in_window || last <= 0 || date(last) != day || !(1..=MAX_DAY_USES).contains(&n) {
            continue;
        }
        // no later than the report: merged by the larger value, a time to come would hide the
        // real later uses until it passed (design v3 §5-2)
        lines.push(UseLine {
            pk,
            day: day.to_owned(),
            last: last.min(at),
            n,
        });
    }
    Some(lines)
}

/// The counts of `refused` above zero under the REFUSAL_REASONS; other names are left out, whatever
/// their values. `None` when the count of a reason is not a whole number of zero or more.
fn refusals(counts: &Map<String, Value>) -> Option<Vec<(String, i64)>> {
    let mut out = Vec::new();
    for (name, count) in counts {
        if !REFUSAL_REASONS.contains(&name.as_str()) {
            continue;
        }
        let count = count.as_u64()?;
        if count == 0 {
            continue;
        }
        out.push((
            name.clone(),
            i64::try_from(count)
                .unwrap_or(MAX_REFUSALS)
                .min(MAX_REFUSALS),
        ));
    }
    Some(out)
}

// ---- the password-less state of a bound PC, as the apps show it ----

/// A bound PC's password-less state for the device list and `this-device` (design v3 §5-2), by
/// its last sync: `needs-relogin` (hbbs holds another key for it), `disabled` (its account's
/// switch is off), `listed` (it never synced, or its last sync carried no binding or another
/// one), `expired` (that sync is older than a list lives), `off` (its users switched it off) or
/// `active`.
pub fn pwless_state(d: &Device, switch_on: bool, at: i64, list_ttl: i64) -> &'static str {
    if d.key_changed_at.is_some() {
        return "needs-relogin";
    }
    if !switch_on {
        return "disabled";
    }
    match (d.last_sync_at, d.sync_state.as_deref()) {
        (Some(last), Some("present")) if at - last >= list_ttl => "expired",
        (Some(_), Some("present")) if d.off => "off",
        (Some(_), Some("present")) => "active",
        _ => "listed",
    }
}

/// "3일 전": how long ago, in minutes, hours or days.
pub fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < HOUR {
        format!("{}분 전", (secs / MINUTE).max(1))
    } else if secs < DAY {
        format!("{}시간 전", secs / HOUR)
    } else {
        format!("{}일 전", secs / DAY)
    }
}

/// What a PC's last sync said about its binding (`this-device`'s `activation`): `present`,
/// `absent` (none, or no sync yet) or `stale`.
pub fn activation(d: &Device) -> &'static str {
    match d.sync_state.as_deref() {
        Some("present") => "present",
        Some("stale") => "stale",
        _ => "absent",
    }
}

// ---- the lists ----

/// The trust list settings of the moment.
#[derive(Debug, Clone)]
pub struct ListSettings {
    pub kid: u32,
    pub ttl: i64,
    pub refresh: i64,
    pub kinds: Vec<String>,
}

/// The last list built for a PC (design v3 §5-2: a repeat within RESIGN_WINDOW with the same
/// change serial gets it again, for its own `n`).
#[derive(Debug, Clone)]
pub struct CachedList {
    pub serial: i64,
    pub binding: [u8; 16],
    pub pc_pk: [u8; 32],
    pub built: Instant,
    pub list: TrustList,
}

impl CachedList {
    /// Whether a sync of `binding` checked with `pc_pk`, while the change serial is `serial`, at
    /// `now` takes this list again.
    pub fn serves(&self, serial: i64, binding: &[u8; 16], pc_pk: &[u8; 32], now: Instant) -> bool {
        self.serial == serial
            && self.binding == *binding
            && self.pc_pk == *pc_pk
            && now.saturating_duration_since(self.built) < RESIGN_WINDOW
    }
}

/// The last list of each PC, in memory.
#[derive(Default)]
pub struct ListCache {
    lists: Mutex<HashMap<String, CachedList>>,
}

impl ListCache {
    pub fn get(&self, id: &str) -> Option<CachedList> {
        let lists = self.lists.lock().unwrap_or_else(|e| e.into_inner());
        lists.get(id).cloned()
    }

    pub fn put(&self, id: &str, list: CachedList) {
        let mut lists = self.lists.lock().unwrap_or_else(|e| e.into_inner());
        if lists.len() >= MAX_ENTRIES && !lists.contains_key(id) {
            lists.clear();
        }
        lists.insert(id.to_owned(), list);
    }

    /// Forgets the lists past their window.
    pub fn purge(&self) {
        let mut lists = self.lists.lock().unwrap_or_else(|e| e.into_inner());
        lists.retain(|_, l| l.built.elapsed() < RESIGN_WINDOW);
    }
}

/// When hbbs was last asked about each PC's key after a sync that its stored key did not sign.
#[derive(Default)]
pub struct KeyChecks {
    asked: Mutex<HashMap<String, Instant>>,
}

impl KeyChecks {
    /// Takes the PC's check if it had none in the last KEY_CHECK_EVERY.
    pub fn try_take(&self, id: &str) -> bool {
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if asked
            .get(id)
            .is_some_and(|at| now.saturating_duration_since(*at) < KEY_CHECK_EVERY)
        {
            return false;
        }
        if asked.len() >= MAX_ENTRIES {
            asked.retain(|_, at| now.saturating_duration_since(*at) < KEY_CHECK_EVERY);
            if asked.len() >= MAX_ENTRIES {
                return false;
            }
        }
        asked.insert(id.to_owned(), now);
        true
    }

    pub fn purge(&self) {
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        asked.retain(|_, at| at.elapsed() < KEY_CHECK_EVERY);
    }
}

/// What a sync came to: one value per request, moved once, so its size does not matter.
#[allow(clippy::large_enum_variant)]
enum Decided {
    /// step 3, or a tombstone only and no binding of it: 404, unsigned
    Unbound,
    /// step 4, with the bound PC whose key to compare with hbbs's
    BadSignature(Option<Device>),
    Clock,
    Replay,
    BadPayload,
    /// bound, without a list
    Listed,
    /// a list to sign: active, disabled or revoked; `cache` when it was built for a bound PC;
    /// `send_wol` when hjv-api wants the PC's whole wake report next time
    List {
        list: TrustList,
        cache: Option<CachedList>,
        send_wol: bool,
    },
}

/// The list of a bound PC whose sync carried its binding: `active` with the account's keys that
/// are not revoked, or `disabled` without keys while the account's switch is off or the account is
/// off the allowlist or blocked (design v3 §5-2).
#[allow(clippy::too_many_arguments)]
fn bound_list(
    c: &Connection,
    d: &Device,
    binding: &[u8; 16],
    pc_pk: &[u8; 32],
    serial: i64,
    at: i64,
    settings: &ListSettings,
) -> rusqlite::Result<TrustList> {
    let account = db::account(c, d.account_id)?;
    let on = db::list_on(c, d.account_id)?;
    let mut list = TrustList::new(
        settings.kid,
        if on {
            ListState::Active
        } else {
            ListState::Disabled
        },
        &hex(binding),
        &d.device_id,
        pc_pk,
        "",
        serial,
        at,
        settings.ttl,
        settings.refresh,
        db::strict(c)?,
    );
    list.owner = account.map_or_else(String::new, |a| mask_name(&a.display_name));
    list.login_at = d.login_at.unwrap_or(0);
    if on {
        list.keys = db::keys_of(c, d.account_id)?
            .into_iter()
            .map(|k| ListEntry {
                pk: b64(&k.public_key),
                id: k.ctl_id,
                name: k.model,
                platform: k.platform,
                kinds: settings.kinds.clone(),
                ks: k.key_serial,
            })
            .collect();
    }
    Ok(list)
}

/// A signed `revoked` for a released binding: no keys, the key that checked the request, the
/// binding the PC sent (design v3 §5-2), so that the PC takes it and drops its binding.
fn revoked_list(
    c: &Connection,
    id: &str,
    binding: &[u8; 16],
    pc_pk: &[u8; 32],
    at: i64,
    settings: &ListSettings,
) -> rusqlite::Result<TrustList> {
    Ok(TrustList::new(
        settings.kid,
        ListState::Revoked,
        &hex(binding),
        id,
        pc_pk,
        "",
        db::change_serial(c)?,
        at,
        settings.ttl,
        settings.refresh,
        db::strict(c)?,
    ))
}

/// A bound PC's wake report from a sync that carried its binding (design v3 §8-2): kept with its
/// summary and the sync's address; without one, the address moves and the answer asks for the
/// report when the PC's summary is not the one kept. Returns `send_wol`.
fn store_wol(
    c: &Connection,
    device_id: &str,
    report: &Report,
    egress_ip: &str,
    at: i64,
) -> rusqlite::Result<bool> {
    if let Some(wol) = &report.wol {
        db::store_wol_info(c, device_id, wol, &report.wol_hash, egress_ip, at)?;
        return Ok(false);
    }
    let kept = db::wol_egress(c, device_id, egress_ip)?;
    // a report that did not read would come back as it is: not asked for again
    Ok(!report.wol_unread
        && !report.wol_hash.is_empty()
        && kept.as_deref() != Some(report.wol_hash.as_str()))
}

/// Steps 3 to 7 and the work, in one transaction.
fn decide(
    c: &mut Connection,
    env: &Envelope,
    at: i64,
    egress_ip: &str,
    cached: Option<CachedList>,
    settings: &ListSettings,
) -> rusqlite::Result<Decided> {
    let tx = c.transaction()?;
    // 3. the PC's binding, else its latest tombstone
    let Some(row) = db::sync_row(&tx, &env.id)? else {
        return Ok(Decided::Unbound);
    };
    // 4. the signature with that row's key
    let stored = match &row {
        SyncRow::Bound(d) => &d.public_key,
        SyncRow::Released(t) => &t.public_key,
    };
    let key = <[u8; 32]>::try_from(stored.as_slice()).ok();
    let Some(pc_pk) = key.filter(|k| env.verify(&SPEC, k)) else {
        return Ok(Decided::BadSignature(match row {
            SyncRow::Bound(d) => Some(d),
            SyncRow::Released(_) => None,
        }));
    };
    // 5. the time, 6. the nonce
    if !env.fresh(at) {
        return Ok(Decided::Clock);
    }
    if !db::use_pc_nonce(&tx, &env.id, &env.nonce, at, at + envelope::NONCE_TTL)? {
        return Ok(Decided::Replay);
    }
    // 7. the payload (the nonce stays used)
    let Some(report) = parse_report(&env.p, at) else {
        tx.commit()?;
        return Ok(Decided::BadPayload);
    };
    let decided = match row {
        SyncRow::Released(_) => match report.binding {
            Some(b) if db::tombstone(&tx, &env.id, &b)?.is_some() => Decided::List {
                list: revoked_list(&tx, &env.id, &b, &pc_pk, at, settings)?,
                cache: None,
                send_wol: false,
            },
            _ => Decided::Unbound,
        },
        SyncRow::Bound(d) => {
            let (mark, released) = match report.binding {
                None => ("absent", false),
                Some(b) if Some(b) == d.binding_id => ("present", false),
                Some(b) => ("stale", db::tombstone(&tx, &d.device_id, &b)?.is_some()),
            };
            db::store_sync(
                &tx,
                &d.device_id,
                &db::SyncNote {
                    mark,
                    app_ver: &report.ver,
                    off: report.off,
                },
                at,
            )?;
            db::store_sync_address(&tx, &d.device_id, egress_ip)?;
            db::merge_use(&tx, d.account_id, &d.device_id, &report.used)?;
            db::add_refusals(&tx, &d.device_id, &report.refused, &date(at))?;
            // signed with the stored key: the PC that was bound is there (design v3 §4-6)
            db::clear_key_changed(&tx, &d.device_id)?;
            // the wake report of the PC that holds the binding (design v3 §8-2)
            let send_wol =
                mark == "present" && store_wol(&tx, &d.device_id, &report, egress_ip, at)?;
            match (report.binding, released) {
                (Some(b), true) => Decided::List {
                    list: revoked_list(&tx, &d.device_id, &b, &pc_pk, at, settings)?,
                    cache: None,
                    send_wol: false,
                },
                (Some(b), false) if mark == "present" => {
                    let serial = db::change_serial(&tx)?;
                    let now = Instant::now();
                    match cached.filter(|l| l.serves(serial, &b, &pc_pk, now)) {
                        Some(again) => Decided::List {
                            list: again.list,
                            cache: None,
                            send_wol,
                        },
                        None => {
                            let list = bound_list(&tx, &d, &b, &pc_pk, serial, at, settings)?;
                            Decided::List {
                                cache: Some(CachedList {
                                    serial,
                                    binding: b,
                                    pc_pk,
                                    built: now,
                                    list: list.clone(),
                                }),
                                list,
                                send_wol,
                            }
                        }
                    }
                }
                _ => Decided::Listed,
            }
        }
    };
    tx.commit()?;
    Ok(decided)
}

/// 404 `unbound`, unsigned: no account has the PC, as far as hjv-api knows.
fn unbound_reply() -> Reply {
    Reply::json(
        StatusCode::NOT_FOUND,
        &json!({ "error": "unbound", "state": "unbound", "server_time": now() }),
    )
}

/// Handles `POST /api/hjv/pc/sync` (the caller checked the method).
pub async fn handle(state: &Arc<AppState>, req: ApiRequest) -> Reply {
    if req.headers.contains_key(ORIGIN) {
        return envelope::error_reply(StatusCode::FORBIDDEN, "forbidden");
    }
    let body = match &req.body {
        Ok(body) => body,
        Err(BodyError::TooLarge) => return envelope::refusal_reply(Refusal::TooLarge),
        Err(BodyError::Unreadable) => {
            return envelope::error_reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    };
    // 1. size and 2. shape, with the type of the body
    if body.len() > MAX_BODY {
        return envelope::refusal_reply(Refusal::TooLarge);
    }
    if !is_json(&req.headers) {
        return envelope::refusal_reply(Refusal::BadRequest);
    }
    let env = match Envelope::parse(body, &SPEC) {
        Ok(env) => env,
        Err(refusal) => return envelope::refusal_reply(refusal),
    };
    let at = now();
    let settings = ListSettings {
        kid: state.list_key.kid(),
        ttl: state.config.trust_ttl_secs,
        refresh: state.config.trust_refresh_secs,
        kinds: state.config.passwordless_kinds.clone(),
    };
    let cached = state.lists.get(&env.id);
    let id = env.id.clone();
    let n = env.n.clone();
    let ip = req.ip.clone();
    let decided = state
        .db
        .call(move |c| decide(c, &env, at, &ip, cached, &settings))
        .await;
    match decided {
        Ok(Decided::Unbound) => unbound_reply(),
        Ok(Decided::BadSignature(device)) => {
            if let Some(device) = device {
                check_key(state, &device).await;
            }
            envelope::refusal_reply(Refusal::BadSignature)
        }
        Ok(Decided::Clock) => envelope::refusal_reply(Refusal::Clock),
        Ok(Decided::Replay) => envelope::refusal_reply(Refusal::Replay),
        Ok(Decided::BadPayload) => envelope::refusal_reply(Refusal::BadRequest),
        Ok(Decided::Listed) => Reply::ok(&json!({
            "server_time": now(),
            "state": "listed",
            "refresh": state.config.trust_refresh_secs,
        })),
        Ok(Decided::List {
            mut list,
            cache,
            send_wol,
        }) => {
            if let Some(cache) = cache {
                state.lists.put(&id, cache);
            }
            list.req = n;
            let bytes = match list.bytes() {
                Ok(bytes) => bytes,
                Err(e) => {
                    log_err!("pc sync: {e}");
                    return envelope::error_reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable");
                }
            };
            let signature = state.list_key.sign(&bytes);
            Reply::ok(&json!({
                "server_time": now(),
                "state": list.state,
                "refresh": list.refresh,
                "send_wol": send_wol,
                "list": { "kid": list.kid, "l": b64(&bytes), "s": b64(&signature) },
            }))
        }
        Err(e) => {
            log_err!("pc sync: {e}");
            envelope::error_reply(StatusCode::SERVICE_UNAVAILABLE, "unavailable")
        }
    }
}

/// After a sync of a bound PC that its stored key did not sign: asks hbbs for the PC's key, at
/// most once a minute per PC, and marks the PC needs-relogin when hbbs signs another key (design
/// v3 §4-6). The same key, or no answer, changes nothing. Only the real hbbs can change what it
/// holds for the ID, so whoever only stole hbbs's key pair cannot bring this about.
async fn check_key(state: &Arc<AppState>, d: &Device) {
    let tag = state.tags.device(&d.device_id);
    if state.first_notice(&format!("pc-sync-sig:{tag}"), Duration::from_secs(600)) {
        log_notice!(
            "pc sync: a request not signed with the PC's stored key (dev={tag}); the next ten minutes are not logged"
        );
    }
    if !state.key_checks.try_take(&d.device_id) {
        return;
    }
    let Some(server_key) = state.config.hbbs_pubkey else {
        return;
    };
    let lookup = hbbs::lookup(state.config.hbbs_console, Some(&server_key), &d.device_id).await;
    let public_key = match lookup {
        Ok(Lookup::Known { public_key, .. }) => public_key,
        Ok(Lookup::Unknown) => return,
        Err(e) => {
            if state.first_notice("pc-sync-hbbs", Duration::from_secs(600)) {
                log_warn!("pc sync: {e}; a PC's key was not compared (dev={tag})");
            }
            return;
        }
    };
    if public_key.as_slice() == d.public_key.as_slice() {
        return;
    }
    let (id, stored, at) = (d.device_id.clone(), d.public_key.clone(), now());
    let marked = state
        .db
        .call(move |c| {
            db::in_tx(c, |tx| {
                if !db::mark_key_changed(tx, &id, &stored, at)? {
                    return Ok(None);
                }
                Ok(db::device(tx, &id)?
                    .map(|d| d.account_id)
                    .map(|account_id| db::account(tx, account_id))
                    .transpose()?
                    .flatten()
                    .map(|a| a.login_key))
            })
        })
        .await;
    match marked {
        Ok(Some(login_key)) => {
            log_notice!(
                "pc sync: hbbs holds another key for a PC (dev={tag}): password-less connections to it wait for a login on it (needs-relogin)"
            );
            state
                .record(
                    "device",
                    Some(state.tags.account(&login_key)),
                    Some(tag),
                    None,
                    "key-changed",
                )
                .await;
        }
        Ok(None) => {}
        Err(e) => log_err!("pc sync: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_760_000_000; // 2025-10-09T08:53:20Z

    fn device() -> Device {
        Device {
            device_id: "1".into(),
            account_id: 1,
            public_key: vec![1; 32],
            os: "windows".into(),
            hostname: "PC".into(),
            bound_at: 0,
            last_seen_at: 0,
            binding_id: Some([7; 16]),
            login_at: Some(0),
            activated_at: None,
            last_sync_at: None,
            app_ver: None,
            off: false,
            sync_state: None,
            key_changed_at: None,
        }
    }

    #[test]
    fn bindings() {
        assert_eq!(
            parse_binding("b0f1e2d3c4b5a69788796a5b4c3d2e1f").map(|b| hex(&b)),
            Some("b0f1e2d3c4b5a69788796a5b4c3d2e1f".to_owned())
        );
        for bad in [
            "",
            "B0F1E2D3C4B5A69788796A5B4C3D2E1F",
            "b0f1e2d3c4b5a69788796a5b4c3d2e1",
            "b0f1e2d3c4b5a69788796a5b4c3d2e1f0",
            "g0f1e2d3c4b5a69788796a5b4c3d2e1f",
            "+0f1e2d3c4b5a69788796a5b4c3d2e1f",
        ] {
            assert_eq!(parse_binding(bad), None, "{bad}");
        }
    }

    #[test]
    fn payloads() {
        let pk = b64(&[3; 32]);
        let p = json!({
            "v": 1, "binding": "b0f1e2d3c4b5a69788796a5b4c3d2e1f", "ver": "1.6.0", "off": true,
            "used": [
                {"pk": pk, "day": "2025-10-08", "last": 1_759_962_000, "n": 1},
                {"pk": pk, "day": "2025-10-09", "last": 1_759_999_000, "n": 2},
                // a day too old, a time not of its day, no uses, too many, not a key: left out
                {"pk": pk, "day": "2025-09-09", "last": 1_757_404_800, "n": 1},
                {"pk": pk, "day": "2025-10-09", "last": 1_759_962_000, "n": 1},
                {"pk": pk, "day": "2025-10-09", "last": 1_759_999_000, "n": 0},
                {"pk": pk, "day": "2025-10-09", "last": 1_759_999_000, "n": MAX_DAY_USES + 1},
                {"pk": "AAAA", "day": "2025-10-09", "last": 1_759_999_000, "n": 1},
                {"pk": pk, "day": "2025-10-9", "last": 1_759_999_000, "n": 1},
            ],
            // the design's seven reasons only: other names are left out, whatever their values
            "refused": {"unknown_key": 2, "kind": 0, "off": 1, "Bad Name": 4, "not_active": 3,
                        "made_up": "x", "expired_": 5},
            "wol_hash": "", "wol": {"v": 1},
        });
        let r = parse_report(&p.to_string(), AT).unwrap();
        assert_eq!(
            r.binding.map(|b| hex(&b)).as_deref(),
            Some("b0f1e2d3c4b5a69788796a5b4c3d2e1f")
        );
        assert_eq!((r.ver.as_str(), r.off), ("1.6.0", true));
        assert_eq!(
            r.used
                .iter()
                .map(|l| (l.day.as_str(), l.last, l.n))
                .collect::<Vec<_>>(),
            vec![
                ("2025-10-08", 1_759_962_000, 1),
                ("2025-10-09", 1_759_999_000, 2)
            ]
        );
        assert_eq!(
            r.refused,
            vec![("off".to_owned(), 1), ("unknown_key".to_owned(), 2)]
        );
        // tomorrow, for a PC clock ahead; a time after the server's counts as the server's, so
        // that the larger-value merge cannot hold a time to come over the real later ones
        let ahead = json!({"v": 1, "binding": "", "used": [
            {"pk": pk, "day": "2025-10-10", "last": 1_760_054_400, "n": 1},
            {"pk": pk, "day": "2025-10-10", "last": 1_760_140_799, "n": 2},
            {"pk": pk, "day": "2025-10-09", "last": AT - 1, "n": 3}]});
        assert_eq!(
            parse_report(&ahead.to_string(), AT)
                .unwrap()
                .used
                .iter()
                .map(|l| (l.last, l.n))
                .collect::<Vec<_>>(),
            vec![(AT, 1), (AT, 2), (AT - 1, 3)]
        );
        // the least payload; a version that is not one is not kept
        let least = parse_report(r#"{"v":1,"binding":"","ver":"1.6.0 <b>"}"#, AT).unwrap();
        assert_eq!((least.binding, least.ver.as_str()), (None, ""));
        // digits and dots only: the admin output shows it
        for odd in ["1.6.0-run_it", "v1.6", "1..6", "1.2.3.4.5"] {
            let r = parse_report(&format!(r#"{{"v":1,"binding":"","ver":"{odd}"}}"#), AT).unwrap();
            assert_eq!(r.ver, "", "{odd}");
        }
        assert!(least.used.is_empty() && least.refused.is_empty() && !least.off);
        // the shape
        for bad in [
            "",
            "[]",
            r#"{"binding":""}"#,
            r#"{"v":2,"binding":""}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"binding":"B0F1E2D3C4B5A69788796A5B4C3D2E1F"}"#,
            r#"{"v":1,"binding":7}"#,
            r#"{"v":1,"binding":"","off":"no"}"#,
            r#"{"v":1,"binding":"","ver":1}"#,
            r#"{"v":1,"binding":"","used":{}}"#,
            r#"{"v":1,"binding":"","used":[{"pk":"x","day":"2025-10-09","last":"1","n":1}]}"#,
            r#"{"v":1,"binding":"","used":[{"pk":"x","day":"2025-10-09","last":1}]}"#,
            r#"{"v":1,"binding":"","used":[{"pk":"x","day":"2025-10-09","last":1.5,"n":1}]}"#,
            r#"{"v":1,"binding":"","refused":{"off":-1}}"#,
            r#"{"v":1,"binding":"","refused":{"off":"1"}}"#,
            r#"{"v":1,"binding":"","refused":[]}"#,
            r#"{"v":1,"binding":"","wol_hash":1}"#,
            r#"{"v":1,"binding":"","wol":"x"}"#,
        ] {
            assert_eq!(parse_report(bad, AT), None, "{bad}");
        }
    }

    #[test]
    fn states_and_notes() {
        let mut d = device();
        let ttl = DAY;
        // never synced: the PC has not confirmed its binding yet
        assert_eq!(pwless_state(&d, true, AT, ttl), "listed");
        assert_eq!(pwless_state(&d, false, AT, ttl), "disabled");
        assert_eq!(activation(&d), "absent");
        d.last_sync_at = Some(AT - 100);
        d.sync_state = Some("present".into());
        assert_eq!(pwless_state(&d, true, AT, ttl), "active");
        assert_eq!(activation(&d), "present");
        d.off = true;
        assert_eq!(pwless_state(&d, true, AT, ttl), "off");
        d.last_sync_at = Some(AT - ttl);
        assert_eq!(pwless_state(&d, true, AT, ttl), "expired");
        d.sync_state = Some("absent".into());
        assert_eq!(pwless_state(&d, true, AT, ttl), "listed");
        d.sync_state = Some("stale".into());
        assert_eq!(pwless_state(&d, true, AT, ttl), "listed");
        assert_eq!(activation(&d), "stale");
        d.key_changed_at = Some(AT);
        assert_eq!(pwless_state(&d, true, AT, ttl), "needs-relogin");
        assert_eq!(pwless_state(&d, false, AT, ttl), "needs-relogin");
        assert_eq!(ago(0), "1분 전");
        assert_eq!(ago(59 * MINUTE + 59), "59분 전");
        assert_eq!(ago(HOUR), "1시간 전");
        assert_eq!(ago(3 * DAY + 5), "3일 전");
    }

    #[test]
    fn the_last_list_serves_ten_seconds() {
        let list = TrustList::new(
            1,
            ListState::Active,
            "",
            "1",
            &[1; 32],
            "",
            5,
            AT,
            DAY,
            120,
            false,
        );
        let built = Instant::now();
        let cached = CachedList {
            serial: 5,
            binding: [7; 16],
            pc_pk: [1; 32],
            built,
            list,
        };
        assert!(cached.serves(5, &[7; 16], &[1; 32], built + Duration::from_secs(9)));
        assert!(!cached.serves(5, &[7; 16], &[1; 32], built + RESIGN_WINDOW));
        assert!(!cached.serves(6, &[7; 16], &[1; 32], built));
        assert!(!cached.serves(5, &[8; 16], &[1; 32], built));
        assert!(!cached.serves(5, &[7; 16], &[2; 32], built));
        let cache = ListCache::default();
        cache.put("1", cached);
        assert!(cache.get("1").is_some() && cache.get("2").is_none());
    }

    #[test]
    fn hbbs_is_asked_once_a_minute_per_pc() {
        let checks = KeyChecks::default();
        assert!(checks.try_take("1"));
        assert!(!checks.try_take("1"));
        assert!(checks.try_take("2"));
        checks.purge();
        assert!(!checks.try_take("1"));
    }
}
