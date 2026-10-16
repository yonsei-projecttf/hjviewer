// SPDX-License-Identifier: AGPL-3.0-only

//! Access tickets (docs/access-tickets.md, the staff-only gate of 1.8.0): hjv-api signs a ticket
//! for each logged-in app and each bound PC, hbbs (server patch 0006) checks it offline at the
//! punch hole request, the relay request and the registration, and the apps carry it without
//! reading it. This module is the signer and the places that hand tickets out:
//!
//! - `POST /api/hjv/ticket` ([`handle`]): the token with its key's `X-HJV-Proof`, counted with
//!   the account key's other calls (keys.rs `CallLimiter`, 30 a minute). A body `{}` (or none)
//!   gets a `staff`/`ctl` ticket, a phone's; `{"id","pk","sig"}` with the device key's signature
//!   of [`pc_message`] a `staff`/`both` one for that PC, a Windows app's. The App Review account
//!   gets `review`/`ctl` for the demo PCs of review.conf's `PC=` line (`review_claims`); review.rs
//!   reads no such line yet, so until it does the account gets 403 `no_ticket`.
//! - the login's answer, `hjv.access` (login.rs): the same ticket, `both` for an installed PC
//!   whose login signature matched the key hbbs holds for it, else `ctl`.
//! - the sync of a bound PC that sent its binding, `access` (pcsync.rs): `staff`/`both` for the
//!   PC's ID and the key that checked the sync, while its account is on the allowlist and not
//!   blocked, renewed with each sync.
//!
//! Each carries the ticket object `{"ticket":"t1.…","use":"ctl","exp":…,"server_time":…}`
//! ([`Issued::json`]): the app learns from `exp` and the server's time when to renew, never from
//! the ticket. Without the credential hjviewer-api-ticket-key no ticket is signed: the path
//! answers 503 and the answers carry none.
//!
//! The ticket (docs/access-tickets.md 2): `t1.<kid>.<base64url payload>.<base64url signature>`,
//! at most 1024 bytes, the signature the ticket key's Ed25519 over `"hjv-ticket-v1" NUL` and the
//! payload bytes, the payload canonical JSON of nine members in a fixed order ([`payload`]).
//! [`TicketKey::sign`] refuses claims that hbbs would refuse as malformed (2-3, 2-4), so that no
//! mistake here makes a ticket that reaches more than its kind may. Tickets are not stored or
//! logged, and nothing records them one by one: [`TicketStats`] counts them in memory for `hjv-api
//! admin ticket status`.
//!
//! The access password's `POST /api/hjv/guest` is the next step: it signs [`Claims::guest_ctl`]
//! (the one PC it connects to) and [`Claims::guest_pc`] through [`issue`].

use crate::{
    api::{session, ApiRequest, Reply},
    config::CRED_TICKET_KEY,
    devsig,
    keys::{CALLS_PER_MINUTE, TEXT_TOO_OFTEN},
    review,
    state::AppState,
    trust,
    util::{b64, b64_decode, b64url, datetime, now, valid_device_id, HOUR},
};
use ed25519_dalek::{Signer, SigningKey};
use hyper::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fmt::Write as _,
    sync::{Arc, Mutex},
    time::Duration,
};

pub const PATH: &str = "/api/hjv/ticket";
/// What the signature covers before the payload (docs/access-tickets.md 2-2).
pub const DOMAIN: &[u8] = b"hjv-ticket-v1";
/// The longest ticket hbbs reads.
pub const MAX_BYTES: usize = 1024;
/// The longest life hbbs takes, `exp − iat` (seven days).
pub const MAX_LIFETIME: i64 = 7 * 24 * HOUR;
/// The App Review account's demo PCs, at most.
pub const MAX_TO: usize = 3;
/// What a Windows app's device key signs to ask for a `both` ticket ([`pc_message`]).
pub const PC_CONTEXT: &[u8] = b"hjv-ticket-pc-v1";

const TEXT_OFF: &str = "지금은 출입증을 받을 수 없습니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_NO_TICKET: &str = "이 계정으로는 출입증을 받을 수 없습니다.";
const TEXT_BAD_REQUEST: &str = "요청을 이해하지 못했습니다.";

/// Who a ticket is for (`who`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    /// a staff account on the allowlist, not blocked, or a PC bound to one
    Staff,
    /// the App Review account
    Review,
    /// the access password (이용 비밀번호)
    Guest,
}

impl Who {
    pub fn as_str(self) -> &'static str {
        match self {
            Who::Staff => "staff",
            Who::Review => "review",
            Who::Guest => "guest",
        }
    }

    pub fn parse(text: &str) -> Option<Who> {
        match text {
            "staff" => Some(Who::Staff),
            "review" => Some(Who::Review),
            "guest" => Some(Who::Guest),
            _ => None,
        }
    }
}

/// What a ticket is good for (`use`): the punch hole and relay requests, the registration, or all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Use {
    Ctl,
    Pc,
    Both,
}

impl Use {
    pub fn as_str(self) -> &'static str {
        match self {
            Use::Ctl => "ctl",
            Use::Pc => "pc",
            Use::Both => "both",
        }
    }

    pub fn parse(text: &str) -> Option<Use> {
        match text {
            "ctl" => Some(Use::Ctl),
            "pc" => Some(Use::Pc),
            "both" => Some(Use::Both),
            _ => None,
        }
    }
}

/// A ticket's values (docs/access-tickets.md 2-3), before they are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claims {
    pub who: Who,
    pub use_: Use,
    /// the account's tag (log.rs `Tagger::account`), empty for the access password
    pub sub: String,
    /// the PC of a `pc` or `both` ticket: its ID and its device key; empty and none for `ctl`
    pub id: String,
    pub pk: Option<[u8; 32]>,
    /// the PCs it may connect to; empty, any
    pub to: Vec<String>,
    pub iat: i64,
    pub exp: i64,
}

impl Claims {
    /// A logged-in app's or a bound PC's: `both` for the PC `pc` (its ID and device key), else
    /// `ctl`.
    pub fn staff(sub: &str, pc: Option<(&str, [u8; 32])>, iat: i64, lifetime: i64) -> Claims {
        Claims {
            who: Who::Staff,
            use_: if pc.is_some() { Use::Both } else { Use::Ctl },
            sub: sub.to_owned(),
            id: pc.map(|(id, _)| id.to_owned()).unwrap_or_default(),
            pk: pc.map(|(_, pk)| pk),
            to: Vec::new(),
            iat,
            exp: iat.saturating_add(lifetime),
        }
    }

    /// The App Review account's: `ctl` for its demo PCs `to`, until `exp`.
    pub fn review(sub: &str, to: Vec<String>, iat: i64, exp: i64) -> Claims {
        Claims {
            who: Who::Review,
            use_: Use::Ctl,
            sub: sub.to_owned(),
            id: String::new(),
            pk: None,
            to,
            iat,
            exp,
        }
    }

    /// The access password's for a controller: `ctl` for the one PC it connects to.
    pub fn guest_ctl(target: &str, iat: i64, lifetime: i64) -> Claims {
        Claims {
            who: Who::Guest,
            use_: Use::Ctl,
            sub: String::new(),
            id: String::new(),
            pk: None,
            to: vec![target.to_owned()],
            iat,
            exp: iat.saturating_add(lifetime),
        }
    }

    /// The access password's for a PC: `pc` for its ID and device key.
    pub fn guest_pc(id: &str, pk: [u8; 32], iat: i64, lifetime: i64) -> Claims {
        Claims {
            who: Who::Guest,
            use_: Use::Pc,
            sub: String::new(),
            id: id.to_owned(),
            pk: Some(pk),
            to: Vec::new(),
            iat,
            exp: iat.saturating_add(lifetime),
        }
    }
}

/// The payload's members in their signed order.
#[derive(Serialize)]
struct Payload<'a> {
    v: u8,
    who: &'static str,
    #[serde(rename = "use")]
    use_: &'static str,
    sub: &'a str,
    id: &'a str,
    pk: String,
    to: &'a [String],
    iat: i64,
    exp: i64,
}

/// Whether `claims` keep the rules hbbs reads a payload by (docs/access-tickets.md 2-3, 2-4); the
/// first one broken.
fn check(c: &Claims) -> Result<(), &'static str> {
    let targets = match (c.who, c.use_) {
        (Who::Staff, _) | (Who::Guest, Use::Pc) => 0..=0,
        (Who::Guest, Use::Ctl) => 1..=1,
        (Who::Review, Use::Ctl) => 1..=MAX_TO,
        _ => return Err("no such kind of ticket"),
    };
    let tag = c.sub.len() == 16
        && c.sub
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !c.sub.is_empty() && !tag {
        return Err("sub is not an account tag");
    }
    let pc_fits = match c.use_ {
        Use::Ctl => c.id.is_empty() && c.pk.is_none(),
        Use::Pc | Use::Both => valid_device_id(&c.id) && c.pk.is_some(),
    };
    if !pc_fits {
        return Err("the PC does not fit the use");
    }
    if !targets.contains(&c.to.len()) {
        return Err("the targets do not fit the kind");
    }
    for (i, target) in c.to.iter().enumerate() {
        if !valid_device_id(target) || c.to[..i].contains(target) {
            return Err("a target is not a PC ID, or there twice");
        }
    }
    if c.iat < 0 || c.exp <= c.iat || c.exp - c.iat > MAX_LIFETIME {
        return Err("the lifetime is not 1 second to 7 days");
    }
    Ok(())
}

/// The payload bytes of `claims`: canonical JSON, the nine members in their order, no blanks;
/// `Err` when the claims break a rule of hbbs's.
pub fn payload(claims: &Claims) -> Result<Vec<u8>, &'static str> {
    check(claims)?;
    serde_json::to_vec(&Payload {
        v: 1,
        who: claims.who.as_str(),
        use_: claims.use_.as_str(),
        sub: &claims.sub,
        id: &claims.id,
        pk: claims.pk.map(|pk| b64(&pk)).unwrap_or_default(),
        to: &claims.to,
        iat: claims.iat,
        exp: claims.exp,
    })
    .map_err(|_| "the payload does not serialize")
}

/// The bytes a ticket key signs.
pub fn message(payload: &[u8]) -> Vec<u8> {
    [DOMAIN, &[0u8], payload].concat()
}

/// The ticket key in use, with its number (credential hjviewer-api-ticket-key, HJV_API_TICKET_KID).
pub struct TicketKey {
    kid: u32,
    key: SigningKey,
}

impl std::fmt::Debug for TicketKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "TicketKey({})",
            trust::entry(self.kid, &self.public_key())
        )
    }
}

impl TicketKey {
    pub fn new(kid: u32, seed: &[u8; 32]) -> TicketKey {
        TicketKey {
            kid,
            key: SigningKey::from_bytes(seed),
        }
    }

    pub fn kid(&self) -> u32 {
        self.kid
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    /// The ticket of `claims`, as the apps carry it.
    pub fn sign(&self, claims: &Claims) -> Result<String, &'static str> {
        let payload = payload(claims)?;
        let signature = self.key.sign(&message(&payload)).to_bytes();
        let ticket = format!(
            "t1.{}.{}.{}",
            self.kid,
            b64url(&payload),
            b64url(&signature)
        );
        if ticket.len() > MAX_BYTES {
            return Err("the ticket is longer than 1024 bytes");
        }
        Ok(ticket)
    }
}

/// A ticket signed for an answer.
pub struct Issued {
    pub ticket: String,
    pub use_: Use,
    pub exp: i64,
}

impl std::fmt::Debug for Issued {
    // never the ticket: it opens the service to whoever holds it
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Issued({}, exp {})", self.use_.as_str(), self.exp)
    }
}

impl Issued {
    /// The ticket object of the answers.
    pub fn json(&self, server_time: i64) -> Value {
        json!({
            "ticket": self.ticket,
            "use": self.use_.as_str(),
            "exp": self.exp,
            "server_time": server_time,
        })
    }
}

/// Why no ticket was signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoTicket {
    /// no ticket key (credential hjviewer-api-ticket-key)
    Off,
    /// the account gets none: the App Review account without demo PCs
    Account,
    /// the claims broke a rule: hjv-api's own fault, which the journal hears of
    Failed,
}

/// What was handed out and refused since the start, by how, never by whom: in memory only.
pub struct TicketStats {
    since: i64,
    counts: Mutex<BTreeMap<String, u64>>,
}

impl TicketStats {
    pub fn new(since: i64) -> TicketStats {
        TicketStats {
            since,
            counts: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn note(&self, what: &str) {
        let mut counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        *counts.entry(what.to_owned()).or_insert(0) += 1;
    }

    pub fn since(&self) -> i64 {
        self.since
    }

    pub fn counts(&self) -> Vec<(String, u64)> {
        let counts = self.counts.lock().unwrap_or_else(|e| e.into_inner());
        counts.iter().map(|(k, n)| (k.clone(), *n)).collect()
    }
}

/// Signs `claims` with the ticket key and counts the ticket as `what`.
pub fn issue(state: &AppState, claims: &Claims, what: &str) -> Result<Issued, NoTicket> {
    let key = state.ticket_key.as_ref().ok_or(NoTicket::Off)?;
    match key.sign(claims) {
        Ok(ticket) => {
            state.tickets.note(what);
            Ok(Issued {
                ticket,
                use_: claims.use_,
                exp: claims.exp,
            })
        }
        Err(e) => {
            log_err!("ticket: none signed ({what}): {e}");
            Err(NoTicket::Failed)
        }
    }
}

/// The ticket of a logged-in app of the account `login_key` (docs/access-tickets.md 2-4), for the
/// login's answer (`via` "login") and `POST /api/hjv/ticket` ("request"): `staff`/`both` for `pc`,
/// a PC whose device key the request proved, else `staff`/`ctl`, for HJV_API_TICKET_HOURS; the
/// App Review account's as [`review_claims`] says.
pub async fn for_account(
    state: &Arc<AppState>,
    login_key: &str,
    pc: Option<(&str, [u8; 32])>,
    via: &str,
    at: i64,
) -> Result<Issued, NoTicket> {
    if state.ticket_key.is_none() {
        return Err(NoTicket::Off);
    }
    let sub = state.tags.account(login_key);
    if review::is_review(login_key) {
        let claims = review_claims(state, login_key, &sub, at).await?;
        return issue(state, &claims, &format!("{via} review"));
    }
    let claims = Claims::staff(&sub, pc, at, state.config.ticket_secs);
    issue(state, &claims, &format!("{via} {}", claims.use_.as_str()))
}

/// The App Review account's claims: `review`/`ctl` for the demo PCs of its file's `PC=` line, in
/// the file's order, until the earlier of HJV_API_TICKET_HOURS and the file's UNTIL. review.rs
/// reads no `PC=` line yet ([`review::Review::demo_pcs`]): until it does, the account gets none.
async fn review_claims(
    state: &Arc<AppState>,
    login_key: &str,
    sub: &str,
    at: i64,
) -> Result<Claims, NoTicket> {
    let (app, key) = (state.clone(), login_key.to_owned());
    let found = tokio::task::spawn_blocking(move || {
        app.review
            .active(&app.config, &app.tags, at)
            .filter(|review| review.names(&key))
    })
    .await
    .ok()
    .flatten();
    let review = found.ok_or(NoTicket::Account)?;
    let exp = at
        .saturating_add(state.config.ticket_secs)
        .min(review.until());
    if review.demo_pcs().is_empty() || exp <= at {
        return Err(NoTicket::Account);
    }
    Ok(Claims::review(sub, review.demo_pcs().to_vec(), at, exp))
}

/// The ticket of a bound PC (docs/access-tickets.md 2-4) whose sync carried its binding:
/// `staff`/`both` for its ID and the key that checked the sync, under the tag of its account
/// `login_key`, for HJV_API_PC_TICKET_HOURS.
pub fn for_bound_pc(
    state: &AppState,
    login_key: &str,
    id: &str,
    pk: [u8; 32],
    at: i64,
) -> Result<Issued, NoTicket> {
    let sub = state.tags.account(login_key);
    let claims = Claims::staff(&sub, Some((id, pk)), at, state.config.pc_ticket_secs);
    issue(state, &claims, "sync both")
}

/// The bytes a Windows app's device key signs to ask for a `both` ticket: the context, the PC's
/// ID, its key as the request has it (standard base64) and the standard base64 of SHA-256 of the
/// request's bearer token, joined by single NUL bytes. The token's hash ties the signature to the
/// login it goes with.
pub fn pc_message(id: &str, pk: &str, token_hash: &[u8]) -> Vec<u8> {
    let token = b64(token_hash);
    [PC_CONTEXT, id.as_bytes(), pk.as_bytes(), token.as_bytes()].join(&0u8)
}

/// The body of a ticket request.
#[derive(Deserialize)]
struct RequestBody {
    id: Option<String>,
    pk: Option<String>,
    sig: Option<String>,
}

/// The PC of a ticket request: none for no body or `{}`, the PC of `{"id","pk","sig"}` when `sig`
/// is its key's strict signature of [`pc_message`] under the request's token; `Err` for anything
/// else, with why.
fn pc_part(body: &[u8], token_hash: &[u8]) -> Result<Option<(String, [u8; 32])>, &'static str> {
    if body.is_empty() {
        return Ok(None);
    }
    let body: RequestBody =
        serde_json::from_slice(body).map_err(|_| "not an object of id, pk and sig")?;
    let (id, pk, sig) = match (body.id, body.pk, body.sig) {
        (None, None, None) => return Ok(None),
        (Some(id), Some(pk), Some(sig)) => (id, pk, sig),
        _ => return Err("id, pk and sig go together"),
    };
    if !valid_device_id(&id) {
        return Err("id is not a PC ID");
    }
    let key = b64_decode(&pk)
        .and_then(|k| <[u8; 32]>::try_from(k).ok())
        .filter(|k| b64(k) == pk)
        .ok_or("pk is not the standard base64 of 32 bytes")?;
    if !devsig::verify(&key, &pc_message(&id, &pk, token_hash), &sig) {
        return Err("sig is not the key's signature");
    }
    Ok(Some((id, key)))
}

fn refusal(status: StatusCode, text: &str, code: &str) -> Reply {
    Reply::json(status, &json!({ "error": text, "hjv": code }))
}

/// Handles `POST /api/hjv/ticket` (the caller checked the method).
pub async fn handle(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let Some(own) = s.key else {
        // session() lets no request without its key's proof through
        return Reply::server_error();
    };
    let tag = state.tags.account(&s.account.login_key);
    if !state.key_calls.try_take(own.ckid) {
        state.tickets.note("refused too-often");
        if state.first_notice(&format!("keys-calls:{}", own.ckid), Duration::from_secs(60)) {
            log_warn!(
                "ticket: more than {CALLS_PER_MINUTE} calls a minute from one key (acct={tag}, ckid={})",
                own.ckid
            );
        }
        return refusal(
            StatusCode::TOO_MANY_REQUESTS,
            TEXT_TOO_OFTEN,
            "rate_limited",
        );
    }
    if state.ticket_key.is_none() {
        state.tickets.note("refused off");
        return refusal(StatusCode::SERVICE_UNAVAILABLE, TEXT_OFF, "unavailable");
    }
    let body = req.body.as_ref().map_or(&[][..], |b| b.as_ref());
    let pc = match pc_part(body, &s.token_hash) {
        Ok(pc) => pc,
        Err(why) => {
            state.tickets.note("refused request");
            if state.first_notice(&format!("ticket-request:{tag}"), Duration::from_secs(600)) {
                log_info!(
                    "ticket: a request not of the form ({why}; acct={tag}); the next ten minutes are not logged"
                );
            }
            return refusal(StatusCode::BAD_REQUEST, TEXT_BAD_REQUEST, "bad_request");
        }
    };
    let pc = pc.as_ref().map(|(id, pk)| (id.as_str(), *pk));
    match for_account(state, &s.account.login_key, pc, "request", now()).await {
        Ok(issued) => Reply::ok(&issued.json(now())),
        Err(NoTicket::Off) => refusal(StatusCode::SERVICE_UNAVAILABLE, TEXT_OFF, "unavailable"),
        Err(NoTicket::Account) => {
            state.tickets.note("refused account");
            refusal(StatusCode::FORBIDDEN, TEXT_NO_TICKET, "no_ticket")
        }
        Err(NoTicket::Failed) => Reply::server_error(),
    }
}

/// `hjv-api admin ticket status`: the ticket key in use and whether hbbs takes it
/// (HJV_API_TICKET_PUBKEYS), the lifetimes, and the counts since the start. Read-only; no person
/// or device in it.
pub fn status(state: &AppState) -> String {
    let config = &state.config;
    let mut out = String::new();
    match &state.ticket_key {
        None => {
            let _ = writeln!(
                out,
                "tickets: off (no credential {CRED_TICKET_KEY}): POST /api/hjv/ticket answers 503, logins and PC syncs carry no ticket"
            );
        }
        Some(key) => {
            let (kid, mine) = (key.kid(), key.public_key());
            let pinned = match config.pinned_ticket_key(kid) {
                Some(p) if p == mine => format!("the key {kid} of HJV_API_TICKET_PUBKEYS"),
                Some(_) => format!(
                    "NOT the key {kid} of HJV_API_TICKET_PUBKEYS: hbbs refuses every ticket it signs"
                ),
                None if config.ticket_pubkeys.is_empty() => {
                    "HJV_API_TICKET_PUBKEYS is not set: not compared with hbbs's access.conf"
                        .to_owned()
                }
                None => format!(
                    "no key of HJV_API_TICKET_PUBKEYS has the number {kid}: hbbs refuses every ticket it signs"
                ),
            };
            let _ = writeln!(
                out,
                "tickets: on, key {kid} ({}), {pinned}",
                trust::fingerprint(kid, &mine)
            );
            let _ = writeln!(
                out,
                "hbbs's access.conf takes it as KEY={}",
                trust::entry(kid, &mine)
            );
        }
    }
    let _ = writeln!(
        out,
        "lifetimes: logged-in apps {} h (HJV_API_TICKET_HOURS), bound PCs {} h (HJV_API_PC_TICKET_HOURS)",
        config.ticket_secs / HOUR,
        config.pc_ticket_secs / HOUR
    );
    let counts: Vec<String> = state
        .tickets
        .counts()
        .iter()
        .map(|(what, n)| format!("{what} {n}"))
        .collect();
    let _ = writeln!(
        out,
        "since {}: {}",
        datetime(state.tickets.since()),
        if counts.is_empty() {
            "none".to_owned()
        } else {
            counts.join(", ")
        }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: i64 = 1_760_000_000;
    const SUB: &str = "598891f1a14389ce";

    #[test]
    fn the_kinds_and_their_rules() {
        let pk = [2u8; 32];
        let good = [
            Claims::staff(SUB, None, AT, 86_400),
            Claims::staff(SUB, Some(("123456789", pk)), AT, 259_200),
            Claims::guest_ctl("Office-PC_7", AT, 43_200),
            Claims::guest_pc("123456789", pk, AT, 43_200),
            Claims::review(SUB, vec!["100200300".into()], AT, AT + 86_400),
            Claims::review(
                SUB,
                vec!["1".into(), "2".into(), "3".into()],
                AT,
                AT + MAX_LIFETIME,
            ),
            Claims {
                use_: Use::Pc,
                ..Claims::staff(SUB, Some(("1", pk)), AT, 1)
            },
        ];
        for c in &good {
            assert_eq!(check(c), Ok(()), "{c:?}");
        }
        let staff = Claims::staff(SUB, None, AT, 86_400);
        let both = Claims::staff(SUB, Some(("123456789", pk)), AT, 86_400);
        let guest = Claims::guest_ctl("123456789", AT, 43_200);
        let review = Claims::review(SUB, vec!["100200300".into()], AT, AT + 86_400);
        let bad = [
            // the three pairs that do not exist
            Claims {
                use_: Use::Both,
                ..Claims::guest_pc("1", pk, AT, 1)
            },
            Claims {
                use_: Use::Pc,
                id: "1".into(),
                pk: Some(pk),
                to: vec![],
                ..review.clone()
            },
            Claims {
                use_: Use::Both,
                id: "1".into(),
                pk: Some(pk),
                to: vec![],
                ..review.clone()
            },
            // targets that do not fit the kind
            Claims {
                to: vec!["1".into()],
                ..staff.clone()
            },
            Claims {
                to: vec![],
                ..guest.clone()
            },
            Claims {
                to: vec!["1".into(), "2".into()],
                ..guest.clone()
            },
            Claims {
                to: vec![],
                ..review.clone()
            },
            Claims {
                to: vec!["1".into(), "2".into(), "3".into(), "4".into()],
                ..review.clone()
            },
            Claims {
                to: vec!["1".into(), "1".into()],
                ..review.clone()
            },
            Claims {
                to: vec!["".into()],
                ..guest.clone()
            },
            Claims {
                to: vec!["a b".into()],
                ..guest.clone()
            },
            // the PC and the use
            Claims {
                id: "1".into(),
                ..staff.clone()
            },
            Claims {
                pk: Some(pk),
                ..staff.clone()
            },
            Claims {
                pk: None,
                ..both.clone()
            },
            Claims {
                id: String::new(),
                ..both.clone()
            },
            Claims {
                id: "x".repeat(65),
                ..both.clone()
            },
            // sub
            Claims {
                sub: SUB.to_uppercase(),
                ..staff.clone()
            },
            Claims {
                sub: SUB[..15].into(),
                ..staff.clone()
            },
            // the lifetime
            Claims {
                exp: AT,
                ..staff.clone()
            },
            Claims {
                exp: AT + MAX_LIFETIME + 1,
                ..staff.clone()
            },
            Claims {
                iat: -1,
                exp: 1,
                ..staff.clone()
            },
            Claims::staff(SUB, None, i64::MAX, 86_400),
        ];
        for c in &bad {
            assert!(check(c).is_err(), "{c:?}");
            assert!(payload(c).is_err(), "{c:?}");
        }
    }

    #[test]
    fn the_payload_is_canonical() {
        let key = devsig::public_key(&[2u8; 32]);
        let c = Claims::staff(SUB, Some(("123456789", key)), AT, 86_400);
        let pk = b64(&key);
        assert_eq!(
            String::from_utf8(payload(&c).unwrap()).unwrap(),
            format!(
                r#"{{"v":1,"who":"staff","use":"both","sub":"{SUB}","id":"123456789","pk":"{pk}","to":[],"iat":1760000000,"exp":1760086400}}"#
            )
        );
        assert_eq!(message(b"{}"), b"hjv-ticket-v1\0{}");
    }

    #[test]
    fn an_answer_shows_no_ticket_in_its_debug() {
        let key = TicketKey::new(1, &[0x74; 32]);
        let ticket = key.sign(&Claims::staff(SUB, None, AT, 60)).unwrap();
        assert!(ticket.starts_with("t1.1.") && ticket.len() < MAX_BYTES);
        let issued = Issued {
            ticket: ticket.clone(),
            use_: Use::Ctl,
            exp: AT + 60,
        };
        assert_eq!(format!("{issued:?}"), "Issued(ctl, exp 1760000060)");
        assert_eq!(
            issued.json(AT),
            json!({"ticket": ticket, "use": "ctl", "exp": AT + 60, "server_time": AT})
        );
        assert_eq!(
            format!("{key:?}"),
            format!("TicketKey({})", trust::entry(1, &key.public_key()))
        );
    }

    #[test]
    fn the_pc_part_of_a_request() {
        let seed = [2u8; 32];
        let pk = b64(&devsig::public_key(&seed));
        let token = [9u8; 32];
        let sig = devsig::sign(&seed, &pc_message("123456789", &pk, &token));
        let body = |v: Value| v.to_string().into_bytes();
        assert_eq!(pc_part(b"", &token), Ok(None));
        assert_eq!(pc_part(b"{}", &token), Ok(None));
        assert_eq!(
            pc_part(
                &body(json!({"id": "123456789", "pk": pk, "sig": sig})),
                &token
            ),
            Ok(Some(("123456789".into(), devsig::public_key(&seed))))
        );
        let other = devsig::sign(&[3u8; 32], &pc_message("123456789", &pk, &token));
        for bad in [
            json!([]),
            json!({"id": "123456789"}),
            json!({"id": "123456789", "pk": pk}),
            json!({"pk": pk, "sig": sig}),
            json!({"id": 123456789, "pk": pk, "sig": sig}),
            json!({"id": "123456780", "pk": pk, "sig": sig}),
            json!({"id": "1 2", "pk": pk, "sig": sig}),
            json!({"id": "123456789", "pk": pk.trim_end_matches('='), "sig": sig}),
            json!({"id": "123456789", "pk": pk.replace('+', "-"), "sig": sig}),
            json!({"id": "123456789", "pk": pk, "sig": other}),
            json!({"id": "123456789", "pk": pk, "sig": ""}),
        ] {
            assert!(pc_part(&body(bad.clone()), &token).is_err(), "{bad}");
        }
        // the signature of another token's request
        assert!(pc_part(
            &body(json!({"id": "123456789", "pk": pk, "sig": sig})),
            &[8u8; 32]
        )
        .is_err());
        assert!(pc_part(b"not json", &token).is_err());
    }

    #[test]
    fn counts_by_how() {
        let stats = TicketStats::new(AT);
        assert!(stats.counts().is_empty());
        stats.note("login ctl");
        stats.note("sync both");
        stats.note("login ctl");
        assert_eq!(
            stats.counts(),
            vec![("login ctl".to_owned(), 2), ("sync both".to_owned(), 1)]
        );
        assert_eq!(stats.since(), AT);
    }

    #[test]
    fn texts_never_say_disabled() {
        for text in [TEXT_OFF, TEXT_NO_TICKET, TEXT_BAD_REQUEST] {
            assert!(!text.to_lowercase().contains("disabled"));
        }
    }
}
