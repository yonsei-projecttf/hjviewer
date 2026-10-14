// SPDX-License-Identifier: AGPL-3.0-only

//! The super operator's view (최고운영자): who uses the service and on which PCs, read-only.
//!
//! The super operators are the portal IDs of HJV_API_OPERATORS, which only the server's own env
//! file holds (no real portal ID is in the repository); unset or empty, nobody is one, and every
//! other account is a normal user. A super operator's account carries `"hjv_role":"operator"` in
//! its user object (the login's answer, `currentUser`, `users`) and at the top of its key list
//! (`GET /api/hjv/keys`, which the app reads anyway), so that a role given or taken away on the
//! server shows in the app at its next look, without a new login; no other account has the member.
//! The setting is read at start: a change takes effect at the next restart.
//!
//! `GET /api/hjv/admin/overview` is an account request like the others: the token with its account
//! key's `X-HJV-Proof` (api.rs `session`: 401 without them or with a wrong one, 409 `clock`). For
//! an account that is not a super operator the answer is then the 404 of an unknown path, whatever
//! the method and however often, so that its app cannot tell the path from one that does not
//! exist. A super operator gets 405 for another method than GET, 429 past
//! [`CALLS_PER_MINUTE`](crate::keys::CALLS_PER_MINUTE) calls a minute of its account key, counted
//! together with its key list's (the same limit), and otherwise, changing nothing:
//!
//! ```text
//! {"v":1,"at":"2026-10-14T09:30:00Z","active_days":30,
//!  "users":[{"id":"<portal ID>","name":"<name>","active":true,"last_use":"<RFC 3339 or null>","keys":2,"pcs":1}],
//!  "pcs":[{"pc":"<PC ID>","host":"<host name or null>","owner":"<portal ID>","owner_name":"<name>",
//!          "os":"windows","version":"1.6.0","last_sync":"<RFC 3339 or null>","ip":"<address or null>",
//!          "lan":["192.0.2.67/24"],"passwordless":"active"}]}
//! ```
//!
//! - `users`: every account that has logged in (a first login makes the account) and is on the
//!   allowlist. `last_use` is the latest of: a login or a signed request of one of its account keys
//!   (the account's activity, which moves at most once an hour), a connection without the PC's
//!   password that a PC reported for one of its keys, and the last time one of its PCs was seen (a
//!   sync, hbbs, a login on it); `active` when that is at most HJV_API_ACTIVE_DAYS (30) days ago.
//!   `keys` is its active account keys, `pcs` its PCs. The active ones first, then by `last_use`,
//!   the newest first.
//! - `pcs`: every PC bound to an account, also one whose account left the allowlist: the installed
//!   PCs as hjv-api knows them, which is from a login on them (a PC nobody logged in on is unknown
//!   to hjv-api and not listed). By owner, then host name. `version` and `last_sync` are of its last
//!   sync, `ip` the client address that sync came from (schema 5, db.rs `store_sync_address`:
//!   X-Real-IP, overwritten by each sync, gone with the binding), `lan` the IPv4 addresses with
//!   their prefixes of its wired cards from its wake report (none without one), `passwordless` its
//!   state as `hjv-api admin binding list` names it.
//!
//! Every answer with the data first writes one line to the operator record (oplog.rs
//! `append_look`: the account's tag, never its portal ID, and the counts), then one to the journal;
//! when the line cannot be written (no admin socket, the record failing) nothing is shown (503).
//! `hjv-api admin operators` shows which configured IDs have an account (masked unless
//! `--reveal`), each with the tag that its lines in the record carry.

use crate::{
    api::{session, user_json, ApiRequest, Reply, TEXT_METHOD},
    db::{self, Session},
    keys::TEXT_TOO_OFTEN,
    pcsync,
    state::AppState,
    util::{date, datetime, login_key, mask, now, valid_portal_id, DAY},
    wolinfo::Report,
};
use hyper::{Method, StatusCode};
use rusqlite::Connection;
use serde_json::{json, Value};
use std::{collections::HashMap, fmt::Write as _, sync::Arc, time::Duration};

pub const PATH: &str = "/api/hjv/admin/overview";
/// The value of `hjv_role` for a super operator's account.
pub const ROLE: &str = "operator";
/// The command a look writes into the operator record.
pub const COMMAND: &str = "app overview";
/// HJV_API_ACTIVE_DAYS at most.
pub const MAX_ACTIVE_DAYS: i64 = 365;
/// The answer to a super operator's look that the operator record did not take.
pub const TEXT_NOT_RECORDED: &str =
    "열람 기록을 남기지 못해 보여 드리지 않았습니다. 잠시 뒤 다시 시도해 주세요.";

/// HJV_API_OPERATORS: portal IDs, comma-separated, as the allowlist compares them (trimmed and
/// case-folded), each once. An entry that is not a portal ID stops the start; the error names its
/// place only, so that no ID goes to the journal.
pub fn parse_operators(text: &str) -> Result<Vec<String>, String> {
    let mut keys: Vec<String> = Vec::new();
    for (n, entry) in text.split(',').enumerate() {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        if !valid_portal_id(entry) {
            return Err(format!(
                "HJV_API_OPERATORS takes portal IDs, comma-separated: entry {} is not one",
                n + 1
            ));
        }
        let key = login_key(entry);
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    Ok(keys)
}

/// HJV_API_ACTIVE_DAYS: 1 to [`MAX_ACTIVE_DAYS`].
pub fn parse_active_days(days: i64) -> Result<i64, String> {
    if (1..=MAX_ACTIVE_DAYS).contains(&days) {
        Ok(days)
    } else {
        Err(format!(
            "HJV_API_ACTIVE_DAYS is 1 to {MAX_ACTIVE_DAYS} days"
        ))
    }
}

/// Whether the account of `login_key` (the case-folded portal ID) is a super operator.
pub fn is_operator(state: &AppState, login_key: &str) -> bool {
    state.config.operators.iter().any(|k| k == login_key)
}

/// `answer` with `"hjv_role":"operator"` when the account of `login_key` is a super operator.
pub fn mark_role(state: &AppState, login_key: &str, answer: &mut Value) {
    if is_operator(state, login_key) {
        answer["hjv_role"] = json!(ROLE);
    }
}

/// The app's user object of the account of `login_key`, with its role when it has one.
pub fn with_role(state: &AppState, login_key: &str, mut user: Value) -> Value {
    mark_role(state, login_key, &mut user);
    user
}

/// The user object of a session's account (`currentUser`, `users`).
pub fn user_of(state: &AppState, s: &Session) -> Value {
    with_role(
        state,
        &s.account.login_key,
        user_json(&s.account.portal_id, &s.account.display_name),
    )
}

/// An account on the allowlist as the view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserLine {
    pub id: String,
    pub name: String,
    /// the latest use, Unix seconds (0: none known)
    pub last_use: i64,
    pub active: bool,
    pub keys: i64,
    pub pcs: i64,
}

/// A PC in an account as the view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcLine {
    pub pc: String,
    pub host: String,
    pub owner: String,
    pub owner_name: String,
    pub os: String,
    pub version: String,
    pub last_sync: Option<i64>,
    pub ip: Option<String>,
    pub lan: Vec<String>,
    pub passwordless: &'static str,
}

/// What a look shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub at: i64,
    pub active_days: i64,
    pub users: Vec<UserLine>,
    pub pcs: Vec<PcLine>,
}

impl View {
    pub fn active(&self) -> usize {
        self.users.iter().filter(|u| u.active).count()
    }

    /// What the operator record and the journal say of it.
    pub fn counts(&self) -> String {
        format!(
            "{} users ({} active), {} PCs",
            self.users.len(),
            self.active(),
            self.pcs.len()
        )
    }

    pub fn json(&self) -> Value {
        let text = |t: &str| if t.is_empty() { Value::Null } else { json!(t) };
        let time = |t: Option<i64>| {
            t.filter(|t| *t > 0)
                .map_or(Value::Null, |t| json!(datetime(t)))
        };
        json!({
            "v": 1,
            "at": datetime(self.at),
            "active_days": self.active_days,
            "users": self.users.iter().map(|u| json!({
                "id": u.id,
                "name": u.name,
                "active": u.active,
                "last_use": time(Some(u.last_use)),
                "keys": u.keys,
                "pcs": u.pcs,
            })).collect::<Vec<Value>>(),
            "pcs": self.pcs.iter().map(|p| json!({
                "pc": p.pc,
                "host": text(&p.host),
                "owner": p.owner,
                "owner_name": p.owner_name,
                "os": text(&p.os),
                "version": text(&p.version),
                "last_sync": time(p.last_sync),
                "ip": p.ip,
                "lan": p.lan,
                "passwordless": p.passwordless,
            })).collect::<Vec<Value>>(),
        })
    }
}

/// The IPv4 addresses with their prefixes of a wake report's cards, each once, in its order.
fn lan_of(report: &Report) -> Vec<String> {
    let mut lan: Vec<String> = Vec::new();
    for (_, address) in report.addresses() {
        let text = address.to_string();
        if !lan.contains(&text) {
            lan.push(text);
        }
    }
    lan
}

/// Reads the view at `at`: the accounts on the allowlist with their latest use, and every PC in an
/// account; `list_ttl` is a trust list's lifetime (the password-less state's `expired`).
pub fn read(c: &Connection, at: i64, active_days: i64, list_ttl: i64) -> rusqlite::Result<View> {
    let mut stmt = c.prepare(
        "SELECT a.portal_id, a.display_name,
                MAX(a.last_active_at,
                    COALESCE((SELECT MAX(d.last_seen_at) FROM devices d WHERE d.account_id = a.id), 0),
                    COALESCE((SELECT MAX(u.last_used_at) FROM key_use u
                              JOIN ctl_keys k ON k.ckid = u.ckid WHERE k.account_id = a.id), 0)),
                (SELECT COUNT(*) FROM ctl_keys k WHERE k.account_id = a.id AND k.revoked_at IS NULL),
                (SELECT COUNT(*) FROM devices d WHERE d.account_id = a.id)
         FROM accounts a JOIN allowlist l ON l.login_key = a.login_key",
    )?;
    let since = at - active_days * DAY;
    let mut users = stmt
        .query_map([], |r| {
            let last_use: i64 = r.get(2)?;
            Ok(UserLine {
                id: r.get(0)?,
                name: r.get(1)?,
                last_use,
                active: last_use > 0 && last_use >= since,
                keys: r.get(3)?,
                pcs: r.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    users.sort_by(|a, b| {
        b.active
            .cmp(&a.active)
            .then(b.last_use.cmp(&a.last_use))
            .then_with(|| a.id.cmp(&b.id))
    });

    let owners: HashMap<i64, db::Account> =
        db::accounts(c)?.into_iter().map(|a| (a.id, a)).collect();
    let mut stmt = c.prepare("SELECT device_id, last_sync_ip FROM devices")?;
    let addresses: HashMap<String, Option<String>> = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let mut switches: HashMap<i64, bool> = HashMap::new();
    let mut pcs = Vec::new();
    for d in db::all_devices(c)? {
        let Some(owner) = owners.get(&d.account_id) else {
            continue;
        };
        let on = match switches.get(&d.account_id) {
            Some(on) => *on,
            None => {
                let on = db::list_on(c, d.account_id)?;
                switches.insert(d.account_id, on);
                on
            }
        };
        let lan = db::wol_info(c, &d.device_id)?
            .map(|w| lan_of(&w.report))
            .unwrap_or_default();
        pcs.push(PcLine {
            passwordless: pcsync::pwless_state(&d, on, at, list_ttl),
            ip: addresses.get(&d.device_id).cloned().flatten(),
            pc: d.device_id,
            host: d.hostname,
            owner: owner.portal_id.clone(),
            owner_name: owner.display_name.clone(),
            os: d.os,
            version: d.app_ver.unwrap_or_default(),
            last_sync: d.last_sync_at,
            lan,
        });
    }
    pcs.sort_by(|a, b| {
        a.owner
            .cmp(&b.owner)
            .then_with(|| a.host.to_lowercase().cmp(&b.host.to_lowercase()))
            .then_with(|| a.pc.cmp(&b.pc))
    });
    Ok(View {
        at,
        active_days,
        users,
        pcs,
    })
}

/// Handles `GET /api/hjv/admin/overview`, whatever the method (api.rs sends every method here).
pub async fn route(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let tag = state.tags.account(&s.account.login_key);
    if !is_operator(state, &s.account.login_key) {
        if state.first_notice(&format!("overview:other:{tag}"), Duration::from_secs(600)) {
            log_info!(
                "overview: asked for by an account that is not a super operator (acct={tag}): the 404 of an unknown path; the next ten minutes are not logged"
            );
        }
        return Reply::not_found();
    }
    if req.method != Method::GET {
        return Reply {
            allow: Some("GET"),
            ..Reply::error(StatusCode::METHOD_NOT_ALLOWED, TEXT_METHOD)
        };
    }
    let Some(own) = s.key else {
        // session() lets no request without its key's proof through
        return Reply::server_error();
    };
    if !state.key_calls.try_take(own.ckid) {
        if state.first_notice(&format!("keys-calls:{}", own.ckid), Duration::from_secs(60)) {
            log_warn!(
                "overview: more than {} calls a minute from one key, with its key list's (acct={tag}, ckid={})",
                crate::keys::CALLS_PER_MINUTE,
                own.ckid
            );
        }
        return Reply::error(StatusCode::TOO_MANY_REQUESTS, TEXT_TOO_OFTEN);
    }
    // a look that cannot be recorded is not made
    let Some((record, uid)) = state.operator_record.get().cloned() else {
        if state.first_notice("overview:no-record", Duration::from_secs(600)) {
            log_err!(
                "overview: no operator record (the admin socket is off): a super operator's look is refused (acct={tag})"
            );
        }
        return Reply::error(StatusCode::SERVICE_UNAVAILABLE, TEXT_NOT_RECORDED);
    };
    let (at, days, ttl) = (now(), state.config.active_days, state.config.trust_ttl_secs);
    let view = match state.db.call(move |c| read(c, at, days, ttl)).await {
        Ok(view) => view,
        Err(e) => {
            log_err!("overview: {e}");
            return Reply::server_error();
        }
    };
    let (who, done) = (tag.clone(), format!("ok: {}", view.counts()));
    let written =
        tokio::task::spawn_blocking(move || record.append_look(uid, COMMAND, &who, &done)).await;
    if !matches!(written, Ok(Ok(()))) {
        log_err!(
            "overview: the operator record did not take the line of a look (acct={tag}); nothing was shown"
        );
        return Reply::error(StatusCode::SERVICE_UNAVAILABLE, TEXT_NOT_RECORDED);
    }
    log_notice!(
        "overview: shown to a super operator (acct={tag}): {}; the operator record has its line",
        view.counts()
    );
    Reply::ok(&view.json())
}

/// `hjv-api admin operators`: the IDs of HJV_API_OPERATORS, each with its account's tag, whether
/// it has an account and is on the allowlist, and its last login; the IDs masked and no name unless
/// `reveal`.
pub async fn operators(state: &Arc<AppState>, reveal: bool) -> Result<String, String> {
    let configured = state.config.operators.clone();
    if configured.is_empty() {
        return Ok(
            "super operators (HJV_API_OPERATORS): none, so nobody sees the overview\n".to_owned(),
        );
    }
    let found = state
        .db
        .call(move |c| {
            let mut rows = Vec::new();
            for key in configured {
                let account = db::account_by_key(c, &key)?;
                let allowed = db::allowed(c, &key)?;
                rows.push((key, account, allowed));
            }
            Ok(rows)
        })
        .await
        .map_err(|e| e.to_string())?;
    let yes = |b: bool| if b { "yes" } else { "no" };
    let accounts = found.iter().filter(|(_, a, _)| a.is_some()).count();
    let mut out = format!(
        "super operators (HJV_API_OPERATORS): {} configured, {accounts} with an account\n",
        found.len()
    );
    let _ = writeln!(
        out,
        "  {:<16}  {:<16}  {:<7}  {:<9}  last login{}",
        "tag",
        "portal ID",
        "account",
        "allowlist",
        if reveal { "  name" } else { "" }
    );
    for (key, account, allowed) in found {
        let id = account
            .as_ref()
            .map_or(key.as_str(), |a| a.portal_id.as_str());
        let _ = writeln!(
            out,
            "  {:<16}  {:<16}  {:<7}  {:<9}  {}{}",
            state.tags.account(&key),
            if reveal { id.to_owned() } else { mask(id) },
            yes(account.is_some()),
            yes(allowed),
            account
                .as_ref()
                .map_or_else(|| "-".to_owned(), |a| date(a.last_login_at)),
            match (&account, reveal) {
                (Some(a), true) => format!("  {}", a.display_name),
                _ => String::new(),
            }
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        [
            ("HJV_API_SSO_TOKEN_URL", "https://sso.example.ac.kr/token"),
            ("HJV_API_SSO_USERINFO_URL", "https://sso.example.ac.kr/user"),
            ("HJV_API_SSO_CLIENT_ID", "client"),
        ]
        .iter()
        .chain(pairs)
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
    }

    #[test]
    fn the_settings() {
        // unset, empty or only commas: nobody; thirty days
        for text in [None, Some(""), Some(" , ,")] {
            let c = match text {
                Some(t) => Config::from_vars(&vars(&[("HJV_API_OPERATORS", t)])),
                None => Config::from_vars(&vars(&[])),
            }
            .unwrap();
            assert!(c.operators.is_empty(), "{text:?}");
            assert_eq!(c.active_days, 30);
        }
        // trimmed, case-folded as the allowlist compares IDs, each once
        let c = Config::from_vars(&vars(&[
            ("HJV_API_OPERATORS", " Y9001 ,2026000001,y9001"),
            ("HJV_API_ACTIVE_DAYS", "7"),
        ]))
        .unwrap();
        assert_eq!(c.operators, vec!["y9001", "2026000001"]);
        assert_eq!(c.active_days, 7);
        // an entry that is not an ID stops the start, and the error does not repeat it
        for bad in ["2026000001,홍길동", "2026000001,a b", &"1".repeat(65)] {
            let err = Config::from_vars(&vars(&[("HJV_API_OPERATORS", bad)])).unwrap_err();
            assert!(
                err.starts_with("HJV_API_OPERATORS takes portal IDs"),
                "{err}"
            );
            assert!(
                !err.contains("홍길동") && !err.contains("a b") && !err.contains("2026000001"),
                "{err}"
            );
        }
        for bad in ["0", "366", "-1", "x"] {
            assert!(
                Config::from_vars(&vars(&[("HJV_API_ACTIVE_DAYS", bad)])).is_err(),
                "{bad}"
            );
        }
        assert_eq!(parse_active_days(365), Ok(365));
        // the app empties its list on an error text with this word (api.rs)
        assert!(!TEXT_NOT_RECORDED.to_lowercase().contains("disabled"));
    }

    /// A database with three accounts on the allowlist and one off it, their keys, PCs, syncs and
    /// a wake report, at the time `at`.
    fn sample(at: i64) -> Connection {
        let mut c = db::open_in_memory().unwrap();
        let keys: Vec<String> = ["2026000001", "2026000002", "y9001"]
            .iter()
            .map(|k| k.to_string())
            .collect();
        db::allow_add(&mut c, &keys, at).unwrap();
        let hong =
            db::upsert_account(&c, "2026000001", "2026000001", "홍길동", at - 3 * DAY).unwrap();
        let kim = db::upsert_account(&c, "2026000002", "2026000002", "김철수", at - DAY).unwrap();
        let lee = db::upsert_account(&c, "y9001", "Y9001", "이영희", at - 40 * DAY).unwrap();
        let off = db::upsert_account(&c, "2026000009", "2026000009", "박민수", at).unwrap();
        for (account, n) in [(hong, 1u8), (hong, 2), (hong, 5), (kim, 3), (lee, 4)] {
            let key = [n; 32];
            let new = db::NewKey {
                account_id: account,
                public_key: &key,
                dev_tag: "",
                ctl_id: "",
                platform: "ios",
                model: "iPhone",
                app_name: "",
                app_ver: "1.6.0",
                login_net: "",
            };
            db::add_key(&c, &new, at - 50 * DAY, 20).unwrap();
        }
        // one of HONG's keys ended: it is not counted
        c.execute(
            "UPDATE ctl_keys SET revoked_at = ?1, revoke_reason = 'logout' WHERE pk = ?2",
            rusqlite::params![at - 2 * DAY, vec![5u8; 32]],
        )
        .unwrap();
        // HONG: two PCs, one seen an hour ago that synced from an address with a wake report; KIM:
        // one PC never synced; the account off the allowlist: one PC
        db::bind(
            &c,
            hong,
            "700000002",
            &[12; 32],
            "windows",
            "pc-b",
            at - 10 * DAY,
        )
        .unwrap();
        db::bind(
            &c,
            hong,
            "700000001",
            &[11; 32],
            "windows",
            "PC-A",
            at - 10 * DAY,
        )
        .unwrap();
        db::bind(&c, kim, "700000003", &[13; 32], "", "", at - 5 * DAY).unwrap();
        db::bind(&c, off, "700000009", &[19; 32], "windows", "PC-X", at).unwrap();
        let note = db::SyncNote {
            mark: "present",
            app_ver: "1.6.0",
            off: false,
        };
        db::store_sync(&c, "700000001", &note, at - 3600).unwrap();
        db::store_sync_address(&c, "700000001", "198.51.100.7").unwrap();
        let report = crate::wolinfo::parse_report(&json!({"v": 1, "adapters": [
            {"mac": "00:00:5e:00:53:10", "ipv4": ["192.0.2.67/24", "192.0.2.67/24", "10.1.4.9/22"]},
            {"mac": "00:00:5e:00:53:11", "ipv4": ["192.0.2.68/24"]}]}))
        .unwrap();
        db::store_wol_info(&c, "700000001", &report, "hash", "198.51.100.7", at - 3600).unwrap();
        db::store_sync(&c, "700000009", &note, at).unwrap();
        db::store_sync_address(&c, "700000009", "unknown").unwrap();
        db::set_passwordless_global(&c, true).unwrap();
        c
    }

    #[test]
    fn what_a_look_shows() {
        let at = 1_790_000_000;
        let c = sample(at);
        let view = read(&c, at, 30, 86_400).unwrap();
        // the accounts on the allowlist: the active ones first, newest use first
        let users: Vec<(&str, bool, i64, i64, i64)> = view
            .users
            .iter()
            .map(|u| (u.id.as_str(), u.active, at - u.last_use, u.keys, u.pcs))
            .collect();
        assert_eq!(
            users,
            vec![
                ("2026000001", true, 3600, 2, 2),
                ("2026000002", true, DAY, 1, 1),
                ("Y9001", false, 40 * DAY, 1, 0),
            ]
        );
        // every PC in an account, by owner and host name; the one off the allowlist's too
        let pcs: Vec<(&str, &str, &str)> = view
            .pcs
            .iter()
            .map(|p| (p.owner.as_str(), p.host.as_str(), p.passwordless))
            .collect();
        assert_eq!(
            pcs,
            vec![
                ("2026000001", "PC-A", "active"),
                ("2026000001", "pc-b", "listed"),
                ("2026000002", "", "listed"),
                ("2026000009", "PC-X", "disabled"),
            ]
        );
        let a = &view.pcs[0];
        assert_eq!(a.ip.as_deref(), Some("198.51.100.7"));
        assert_eq!(a.lan, vec!["192.0.2.67/24", "10.1.4.9/22", "192.0.2.68/24"]);
        assert_eq!(
            (a.version.as_str(), a.last_sync),
            ("1.6.0", Some(at - 3600))
        );
        assert_eq!(view.pcs[3].ip, None);
        assert_eq!(view.counts(), "3 users (2 active), 4 PCs");
        // the answer: times in RFC 3339, empty texts as null
        let j = view.json();
        assert_eq!(j["v"], 1);
        assert_eq!(j["at"], datetime(at));
        assert_eq!(j["active_days"], 30);
        assert_eq!(j["users"][2]["last_use"], datetime(at - 40 * DAY));
        assert_eq!(j["users"][0]["name"], "홍길동");
        assert_eq!(j["pcs"][0]["last_sync"], datetime(at - 3600));
        assert_eq!(j["pcs"][0]["owner_name"], "홍길동");
        let kim = &j["pcs"][2];
        for field in ["host", "os", "version", "last_sync", "ip"] {
            assert!(kim[field].is_null(), "{field}: {kim}");
        }
        assert_eq!(kim["lan"], json!([]));
        assert_eq!(kim["pc"], "700000003");
        // a shorter window: KIM's use of a day ago is still in it, HONG's of an hour too
        let view = read(&c, at, 1, 86_400).unwrap();
        assert_eq!(view.active(), 2);
        // a PC's reported connection of a key counts as its account's use
        c.execute(
            "INSERT INTO key_use (ckid, device_id, day, last_used_at, n)
             SELECT ckid, '700000001', ?1, ?2, 1 FROM ctl_keys WHERE pk = ?3",
            rusqlite::params![date(at), at - 60, vec![4u8; 32]],
        )
        .unwrap();
        let view = read(&c, at, 30, 86_400).unwrap();
        let lee = view.users.iter().find(|u| u.id == "Y9001").unwrap();
        assert!(lee.active && lee.last_use == at - 60, "{lee:?}");
        assert_eq!(view.users[0].id, "Y9001");
    }
}
