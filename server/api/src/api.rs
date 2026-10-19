// SPDX-License-Identifier: AGPL-3.0-only

//! The account paths (design v2 §9-1, checked against the app's code in the design-v3 research
//! notes), and the rules every request follows.
//!
//! | path | answer |
//! |---|---|
//! | `POST /api/login` | login.rs (every failure that depends on the ID is a 401) |
//! | `POST /api/currentUser` | the user object itself, with `"hjv":{"support_code":true}` for an account that may make support codes (support.rs `may_make`); 401 only for a token that is not valid, never 400 (the app logs out on both) |
//! | `POST /api/logout` | revokes the token and its key; signed by the key, also takes the PC the login came from out of the list |
//! | `GET /api/users` | `{"total":1,"data":[<user>]}` |
//! | `GET /api/peers` | `{"total":n,"data":[{"id","user_name","info":{"device_name","os"},"status":1,"note","hjv":{"pk","pwless","kinds","wol"}}]}`: the account's PCs but the asking device, each with its password-less state (pcsync.rs) and whether it can be woken (wake.rs `peer_wol`) |
//! | `GET /api/hjv/this-device` | what the token's login did with its device, whether it can be undone, and for a PC in the account its `binding`, `pwless` and `activation` |
//! | `POST /api/hjv/this-device/undo` | takes that device out of the list again |
//! | `DELETE /api/hjv/devices/{id}` | takes a device out of the list ("내 PC에서 빼기") |
//! | `GET /api/hjv/keys`, `…/keys/{ckid}/ack`, `DELETE …/keys/{ckid}`, `POST …/keys/revoke-others` | the account keys (keys.rs) |
//! | `GET /api/hjv/admin/overview` | the super operator's view of the service's users and PCs, read-only; for every other account the 404 of an unknown path (overview.rs) |
//! | `POST /api/hjv/pc/sync` | a bound PC's sync, signed with its device key, no token (pcsync.rs) |
//! | `POST /api/hjv/relay/hello`, `POST /api/hjv/relay/poll` | hjv-relay's requests, signed with its own key, no token (relay.rs, relay/PROTOCOL.md) |
//! | `POST /api/hjv/devices/{id}/wake`, `GET /api/hjv/wake/{job}`, `GET /api/hjv/devices/{id}/wol` | waking a PC of the account through a NAS relay, and how ready it is (wake.rs) |
//! | `POST /api/hjv/ticket` | `{"ticket","use","exp","server_time"}`: the access ticket of the token's account that hbbs checks, `staff`/`ctl`, or `staff`/`both` for the PC whose device key signed the body (ticket.rs) |
//! | `POST /api/hjv/guest` | the same object for the access password: a `guest` ticket for one PC, no token (guest.rs); or for a PC's support code (`code`), `guest`/`pc` |
//! | `POST /api/hjv/support-code` | `{"code","use","expires_in","link","wait"}`: a support code the token's staff account makes for a PC, once within ten minutes, with its support link and waiting number (support.rs) |
//! | `GET /api/hjv/support-code/wait?rev=` | `{"rev","codes":[…]}`: the waiting room of the codes the request's account key made, held up to 20 seconds (support.rs) |
//! | `GET, HEAD /api/hjv/update/{platform}/{version}`, `POST /api/hjv/update/{platform}/link`, `GET, HEAD /api/hjv/u/{code}`, `GET, HEAD /api/hjv/get/{code}/[file]`, `GET, HEAD /api/hjv/s/{link}/[file]` | the packages of new app versions: to a login or a bound PC, by a short link, through door 2, and by a support code's support link (update.rs) |
//! | `GET /api/hjv/source` | licence, source and running version (AGPL section 13) |
//! | `GET /api/login-options`, `GET /api/device-group/accessible` | the static answers nginx gives as well |
//!
//! A PC leaves its account through `db::release_binding` only, which leaves a tombstone so that
//! its next sync gets a signed `revoked` (design v3 §4-3): the signed logout from the PC, undo,
//! `DELETE /api/hjv/devices/{id}`, and the admin and upkeep paths.
//!
//! Every path that takes a token takes it only with the `X-HJV-Proof` of the token's account key
//! (proof.rs, design v3 §4-2), checked in [`session`]: a token alone gets the 401 of a wrong
//! token, and a right signature whose time is more than ten minutes off gets 409
//! `{"error":…,"hjv":"clock","server_time":…}`. The one thing a token without its signature still
//! does is end itself: `POST /api/logout` revokes that token and its key, and nothing else (§4-7).
//! A signed request that changes something runs once: the very same request again within
//! HJV_API_RETRY_WINDOW, as the app's TLS fallback sends it, gets the first answer
//! ([`authenticate`]).
//!
//! Every answer is JSON in UTF-8 with `X-Content-Type-Options: nosniff`,
//! `Content-Security-Policy: default-src 'none'` and `Cache-Control: no-store`. A request with an
//! `Origin` header is refused (the app sends none, so a web page cannot use a visitor's browser);
//! a request with a body must be `application/json` (parameters such as charset allowed); there is
//! no CORS. Error texts never contain the word "disabled", which makes the app empty its list.
//! The two exceptions are update.rs's: a package, a notice file, door 2's page or a support link's
//! is sent in place of the JSON (a [`Reply`]'s `sent`, with headers of its own), and the paths a
//! browser opens, `/api/hjv/u/`, `/api/hjv/get/` and `/api/hjv/s/`, take a request with an
//! `Origin` too: the code or link in the path is their key, which no other web page has.
//!
//! The PC sync and the relays' requests answer in the envelope protocols' form
//! (`{"error":"<code>","server_time":…}`, envelope.rs) rather than with these texts. Wake-on-LAN's
//! paths are in modules of their own: wake.rs uses [`session`] for the user's token, relay.rs
//! envelope.rs for a relay's signature.

use crate::{
    body::Body,
    db::{self, Device, Session},
    envelope, gate, guest, keys,
    login::{self, BindStatus, Retry},
    overview, pcsync, proof, relay, review,
    state::AppState,
    support, ticket, update,
    util::{b64, hex, now, sha256, valid_device_id},
    wake,
};
use hyper::{
    body::Bytes,
    header::{
        HeaderMap, HeaderValue, ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY,
        CONTENT_TYPE, ORIGIN, X_CONTENT_TYPE_OPTIONS,
    },
    Method, Response, StatusCode,
};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock};
use tokio::sync::watch;

pub const MAX_BODY: usize = 64 * 1024;
const TEXT_UNAUTHORIZED: &str = "로그인이 필요합니다. 다시 로그인해 주세요.";
const TEXT_FORBIDDEN: &str = "접근이 차단되었습니다";
const TEXT_TOO_LARGE: &str = "요청이 너무 큽니다";
const TEXT_MEDIA_TYPE: &str = "JSON 요청만 받습니다";
pub const TEXT_METHOD: &str = "허용되지 않는 요청입니다";
const TEXT_INCOMPLETE: &str = "요청을 다 받지 못했습니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_SERVER: &str = "서버 오류입니다. 잠시 뒤 다시 시도해 주세요.";
const TEXT_NOT_FOUND: &str = "not found";
/// The answer to an account request signed in time by a device whose clock is off (design v3 §4-2).
pub const TEXT_CLOCK: &str =
    "기기의 날짜와 시간이 서버와 10분 넘게 다릅니다. 날짜와 시간을 자동으로 맞추게 해 주세요.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyError {
    TooLarge,
    Unreadable,
}

/// A request as the handlers see it.
pub struct ApiRequest {
    pub method: Method,
    pub path: String,
    pub query: String,
    /// the request target exactly as sent: the path, and `?` and the query when there is one
    /// (what `X-HJV-Proof` signs)
    pub target: String,
    pub headers: HeaderMap,
    pub body: Result<Bytes, BodyError>,
    /// the client's address: X-Real-IP (set by nginx), else the TCP peer, else "unknown"
    pub ip: String,
    /// where [`route`] leaves the answer for the app's retry of the very same request, when
    /// [`authenticate`] found this one the first with its nonce
    pub retry: OnceLock<watch::Sender<Option<Reply>>>,
}

/// An answer: a JSON body with the security headers.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: StatusCode,
    pub body: String,
    pub allow: Option<&'static str>,
    /// what update.rs sends with the JSON or in its place: headers of its own, a package, a page
    pub sent: Option<update::Sent>,
}

impl Reply {
    pub fn json(status: StatusCode, value: &Value) -> Reply {
        Reply {
            status,
            body: value.to_string(),
            allow: None,
            sent: None,
        }
    }

    pub fn ok(value: &Value) -> Reply {
        Reply::json(StatusCode::OK, value)
    }

    pub fn error(status: StatusCode, text: &str) -> Reply {
        Reply::json(status, &json!({ "error": text }))
    }

    pub fn not_found() -> Reply {
        Reply::error(StatusCode::NOT_FOUND, TEXT_NOT_FOUND)
    }

    pub fn server_error() -> Reply {
        Reply::error(StatusCode::INTERNAL_SERVER_ERROR, TEXT_SERVER)
    }

    pub fn into_response(self, head: bool) -> Response<Body> {
        let body = if head {
            Bytes::new()
        } else {
            Bytes::from(self.body)
        };
        let mut response = Response::new(Body::from(body));
        *response.status_mut() = self.status;
        let headers = response.headers_mut();
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        headers.insert(
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static("default-src 'none'"),
        );
        headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        if let Some(allow) = self.allow {
            headers.insert(ALLOW, HeaderValue::from_static(allow));
        }
        if let Some(sent) = self.sent {
            sent.into_response(&mut response, head);
        }
        response
    }
}

/// Whether the request says its body is JSON.
pub fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/json"))
}

/// The user object of the app (hbbs.dart `UserPayload`). No avatar: the app would send its URL
/// to every PC it connects to (research notes §8).
pub fn user_json(portal_id: &str, display_name: &str) -> Value {
    json!({ "name": portal_id, "display_name": display_name, "status": 1 })
}

/// The methods each path takes, for 405 answers.
fn allowed_methods(path: &str) -> Option<&'static str> {
    Some(match path {
        "/api/login" | "/api/currentUser" | "/api/logout" | "/api/hjv/this-device/undo" => "POST",
        ticket::PATH | support::PATH => "POST",
        support::WAIT_PATH => "GET",
        "/api/users" | "/api/peers" | "/api/hjv/this-device" | overview::PATH => "GET",
        "/api/login-options" | "/api/device-group/accessible" | "/api/hjv/source" => "GET, HEAD",
        gate::PATH => "GET, HEAD",
        p if wake::allowed_methods(p).is_some() => return wake::allowed_methods(p),
        p if update::allowed_methods(p).is_some() => return update::allowed_methods(p),
        p if p.starts_with("/api/hjv/devices/") => "DELETE",
        p if p.starts_with("/api/hjv/keys") => return keys::allowed_methods(p),
        _ => return None,
    })
}

/// Dispatches a request.
pub async fn route(state: &Arc<AppState>, req: ApiRequest) -> Reply {
    if req.path == "/api/login" {
        return login::handle(state, req).await;
    }
    // the access password: no token, the form checked as a login's
    if req.path == guest::PATH {
        return guest::handle(state, req).await;
    }
    // a PC's sync and hjv-relay's requests: no token, the envelope protocols' answers
    if req.path == pcsync::PATH || req.path == relay::HELLO_PATH || req.path == relay::POLL_PATH {
        if req.method != Method::POST {
            return Reply {
                allow: Some("POST"),
                ..envelope::error_reply(StatusCode::METHOD_NOT_ALLOWED, "method_not_allowed")
            };
        }
        if req.path == pcsync::PATH {
            return pcsync::handle(state, req).await;
        }
        return relay::handle(state, req).await;
    }
    let Some(allow) = allowed_methods(&req.path) else {
        return Reply::not_found();
    };
    if req.headers.contains_key(ORIGIN) && !update::browser_path(&req.path) {
        return Reply::error(StatusCode::FORBIDDEN, TEXT_FORBIDDEN);
    }
    let body = match &req.body {
        Ok(body) => body.clone(),
        Err(BodyError::TooLarge) => {
            return Reply::error(StatusCode::PAYLOAD_TOO_LARGE, TEXT_TOO_LARGE)
        }
        // never 400 here: the app takes a 400 from currentUser as a logout (research notes §2-2),
        // and a body cut off on the way says nothing about the token
        Err(BodyError::Unreadable) => {
            return Reply::error(StatusCode::SERVICE_UNAVAILABLE, TEXT_INCOMPLETE)
        }
    };
    if !body.is_empty() && !is_json(&req.headers) {
        return Reply::error(StatusCode::UNSUPPORTED_MEDIA_TYPE, TEXT_MEDIA_TYPE);
    }
    let get = req.method == Method::GET || req.method == Method::HEAD;
    let reply = match (req.path.as_str(), &req.method) {
        ("/api/currentUser", &Method::POST) => current_user(state, &req).await,
        ("/api/logout", &Method::POST) => logout(state, &req).await,
        ("/api/users", &Method::GET) => users(state, &req).await,
        ("/api/peers", &Method::GET) => peers(state, &req).await,
        ("/api/hjv/this-device", &Method::GET) => this_device(state, &req).await,
        ("/api/hjv/this-device/undo", &Method::POST) => undo(state, &req).await,
        (ticket::PATH, &Method::POST) => ticket::handle(state, &req).await,
        (support::PATH, &Method::POST) => support::handle(state, &req).await,
        (support::WAIT_PATH, &Method::GET) => support::wait_handle(state, &req).await,
        (overview::PATH, _) => overview::route(state, &req).await,
        (path, method) if wake::allowed_methods(path) == Some(method.as_str()) => {
            wake::route(state, &req).await
        }
        (path, method) if update::takes(path, method) => update::route(state, &req).await,
        (path, &Method::DELETE)
            if path.starts_with("/api/hjv/devices/") && wake::allowed_methods(path).is_none() =>
        {
            let id = &path["/api/hjv/devices/".len()..];
            remove_device(state, &req, id).await
        }
        (path, method) if path.starts_with("/api/hjv/keys") && allow == method.as_str() => {
            keys::route(state, &req).await
        }
        (gate::PATH, _) if get => gate::handle(state).await,
        ("/api/hjv/source", _) if get => Reply::ok(&json!({
            "license": "AGPL-3.0-only",
            "source": state.config.source_url,
            "version": env!("CARGO_PKG_VERSION"),
            "commit": option_env!("HJV_API_COMMIT").unwrap_or("unknown"),
        })),
        // nginx answers these two itself; the same answers here serve direct use and tests
        ("/api/login-options", _) if get => Reply::ok(&json!([])),
        ("/api/device-group/accessible", _) if get => Reply::ok(&json!({ "total": 0, "data": [] })),
        _ => Reply {
            allow: Some(allow),
            ..Reply::error(StatusCode::METHOD_NOT_ALLOWED, TEXT_METHOD)
        },
    };
    // the first answer of a signed request that changes something, for its retry
    if let Some(first) = req.retry.get() {
        let _ = first.send(Some(reply.clone()));
    }
    reply
}

fn unauthorized() -> Reply {
    Reply::error(StatusCode::UNAUTHORIZED, TEXT_UNAUTHORIZED)
}

/// 409 `clock`: the request is signed by the token's key, but the device's clock is off.
pub fn clock_reply(text: &str) -> Reply {
    Reply::json(
        StatusCode::CONFLICT,
        &json!({ "error": text, "hjv": "clock", "server_time": now() }),
    )
}

/// The SHA-256 of a bearer token, when the header holds one of hjv-api's tokens.
fn bearer_hash(headers: &HeaderMap) -> Option<[u8; 32]> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let token = value.strip_prefix("Bearer ")?.trim();
    let raw = token.strip_prefix("hjv_")?;
    (raw.len() == 43
        && raw
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
    .then(|| sha256(token.as_bytes()))
}

/// What the `X-HJV-Proof` of a request with a valid token came to (proof.rs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proof {
    /// signed by the token's key, in time, with a nonce not used before
    Valid,
    /// signed by the token's key, the time out of the window
    Clock,
    /// no proof
    Missing,
    /// not a proof of this request by the token's key (or the token has no key)
    Invalid,
    /// a nonce the key used within the last 20 minutes, on a request that changes something: by
    /// another request, or by this very one before the retry window
    Replayed,
}

impl Proof {
    fn as_str(self) -> &'static str {
        match self {
            Proof::Valid => "valid",
            Proof::Clock => "clock",
            Proof::Missing => "missing",
            Proof::Invalid => "invalid",
            Proof::Replayed => "replayed",
        }
    }
}

/// The session of the request's token and what its proof came to; the reply to send when the
/// token is not valid. The token's idle time moves only for a valid proof.
///
/// A signed request that changes something uses its nonce once. The very same request again
/// within HJV_API_RETRY_WINDOW ([`proof::request_hash`]) does not run again: the reply to send is
/// the first one's answer, once [`route`] has it (a 500 if that request ended without one). The
/// retry of a request that ended its own token or key (a logout) gets the 401 of an ended token.
pub async fn authenticate(
    state: &Arc<AppState>,
    req: &ApiRequest,
) -> Result<(Session, Proof), Reply> {
    let hash = bearer_hash(&req.headers).ok_or_else(unauthorized)?;
    // a header that is not text counts as there, and wrong
    let header = req
        .headers
        .get(proof::HEADER)
        .map(|v| v.to_str().unwrap_or("").to_owned());
    let (method, target) = (req.method.as_str().to_owned(), req.target.clone());
    let body_hash = sha256(req.body.as_ref().map_or(&[][..], |b| b.as_ref()));
    let changes = req.method != Method::GET && req.method != Method::HEAD;
    let (idle, max, at) = (
        state.config.token_idle_secs,
        state.config.token_max_secs,
        now(),
    );
    let app = state.clone();
    let found = state
        .db
        .call(move |c| {
            let Some(s) = db::session(c, &hash, at, idle, max)? else {
                return Ok(None);
            };
            // the App Review account's tokens hold only while its file names it (review.rs)
            if !review::admits(&app, &s.account.login_key, at) {
                return Ok(None);
            }
            let check = match &s.key {
                Some(key) => proof::check(
                    &key.public_key,
                    header.as_deref(),
                    &method,
                    &target,
                    &body_hash,
                    &hash,
                    at,
                ),
                None if header.is_none() => proof::Check::Missing,
                None => proof::Check::Invalid,
            };
            let mut retry = None;
            let proof = match (check, &s.key) {
                (proof::Check::Valid { .. }, Some(_)) if !changes => Proof::Valid,
                (proof::Check::Valid { nonce }, Some(key)) => {
                    let sent = proof::request_hash(
                        header.as_deref().unwrap_or(""),
                        &method,
                        &target,
                        &body_hash,
                        &hash,
                    );
                    let begun = app
                        .request_retries
                        .begin(&format!("{}.{}", key.ckid, hex(&nonce)), sent);
                    let proof = match &begun {
                        Retry::First(_) => {
                            let ttl = at + proof::NONCE_TTL;
                            if db::use_req_nonce(c, key.ckid, &nonce, at, ttl)? {
                                Proof::Valid
                            } else {
                                Proof::Replayed
                            }
                        }
                        // its answer is the first request's, which moved the token's idle
                        // time already
                        Retry::Again(_) => Proof::Replayed,
                        Retry::Reused => Proof::Replayed,
                    };
                    retry = Some(begun);
                    proof
                }
                (proof::Check::Clock, _) => Proof::Clock,
                (proof::Check::Missing, _) => Proof::Missing,
                _ => Proof::Invalid,
            };
            if proof == Proof::Valid {
                db::touch_session(c, &s, at)?;
            }
            Ok(Some((s, proof, retry)))
        })
        .await;
    match found {
        Ok(Some((s, proof, retry))) => {
            match retry {
                Some(Retry::Again(first)) => {
                    let tag = state.tags.account(&s.account.login_key);
                    if state.first_notice(
                        &format!("proof:again:{tag}"),
                        std::time::Duration::from_secs(600),
                    ) {
                        log_info!(
                            "request proof again (acct={tag}, {}): the first answer; the next ten minutes are not logged",
                            req.method
                        );
                    }
                    return Err(login::wait_for(first, Reply::server_error).await);
                }
                Some(Retry::First(answer)) => {
                    let _ = req.retry.set(answer);
                }
                _ => {}
            }
            if proof != Proof::Valid {
                let tag = state.tags.account(&s.account.login_key);
                if state.first_notice(
                    &format!("proof:{}:{tag}", proof.as_str()),
                    std::time::Duration::from_secs(600),
                ) {
                    // the path may hold a device ID: the method only
                    log_info!(
                        "request proof {} (acct={tag}, {}); the next ten minutes are not logged",
                        proof.as_str(),
                        req.method
                    );
                }
            }
            Ok((s, proof))
        }
        Ok(None) => Err(unauthorized()),
        Err(e) => {
            log_err!("session: {e}");
            Err(Reply::server_error())
        }
    }
}

/// The session of the request's token, signed by the token's account key (design v3 §4-2); the
/// reply to send otherwise: 401 as for a wrong token, or 409 `clock` for a right signature whose
/// time is out of the window.
pub async fn session(state: &Arc<AppState>, req: &ApiRequest) -> Result<Session, Reply> {
    match authenticate(state, req).await? {
        (s, Proof::Valid) => Ok(s),
        (_, Proof::Clock) => Err(clock_reply(TEXT_CLOCK)),
        _ => Err(unauthorized()),
    }
}

async fn current_user(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    match session(state, req).await {
        Ok(s) => {
            let mut user = overview::user_of(state, &s);
            // as the login's answer says it (login.rs): the app shows S1 only for it
            if support::may_make(&s.account.login_key) {
                user["hjv"] = json!({ "support_code": true });
            }
            Reply::ok(&user)
        }
        Err(reply) => reply,
    }
}

/// Logout (design v3 §4-7, §5-2): the token and its account key always go, so that a logged-out
/// device's key is on no trust list (§1-8). Signed by the key, the PC the login came from also
/// leaves the list ([`db::release_binding`]: its next sync gets a signed `revoked`). A signature
/// whose time is out of the window still counts here: it is this request's, and the token it is
/// bound to ends with it, so it cannot be used again. Without the key's signature the PC stays: a
/// stolen token cannot take anyone's PC out of the list, and ending the token's key gains its
/// holder nothing (a key without a login goes with the next upkeep anyway).
async fn logout(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let (s, proof) = match authenticate(state, req).await {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    let signed = matches!(proof, Proof::Valid | Proof::Clock);
    let (hash, device_id, verified, account_id, key) = (
        s.token_hash.clone(),
        s.device_id.clone(),
        s.device_verified,
        s.account.id,
        s.key,
    );
    let (reason, result_text) = if signed {
        ("logout", "revoked-logout")
    } else {
        ("logout-unsigned", "revoked-logout-unsigned")
    };
    let at = now();
    let result = state
        .db
        .call(move |c| {
            let done = db::in_tx(c, |tx| {
                db::delete_token(tx, &hash)?;
                let Some(key) = key else {
                    return Ok((false, false));
                };
                let revoked = db::revoke_key(tx, account_id, key.ckid, reason, None, at)?;
                // the device leaves the list when it logs out (v2 §5), if it proved its key at
                // login and this request is signed by the token's key
                let removed = signed
                    && verified
                    && !device_id.is_empty()
                    && db::release_binding_in(tx, &device_id, account_id, "logout", at)?;
                Ok((revoked, removed))
            })?;
            if done.0 || done.1 {
                db::checkpoint(c)?;
            }
            Ok(done)
        })
        .await;
    match result {
        Ok((revoked, removed)) => {
            let account_tag = state.tags.account(&s.account.login_key);
            if revoked {
                state
                    .record("key", Some(account_tag.clone()), None, None, result_text)
                    .await;
            }
            if removed {
                state
                    .record(
                        "device",
                        Some(account_tag),
                        Some(state.tags.device(&s.device_id)),
                        None,
                        "removed-logout",
                    )
                    .await;
            }
            Reply::ok(&json!({}))
        }
        Err(e) => {
            log_err!("logout: {e}");
            Reply::server_error()
        }
    }
}

/// `current` (from 1) and `pageSize` (1 to 100) of the app's list requests.
fn page(query: &str) -> (usize, usize) {
    let mut current = 1;
    let mut size = 100;
    for (k, v) in form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "current" => current = v.parse().ok().filter(|n| *n >= 1).unwrap_or(1),
            "pageSize" => size = v.parse().ok().filter(|n| *n >= 1).unwrap_or(100).min(100),
            _ => {}
        }
    }
    (current, size)
}

pub(crate) fn paged(items: Vec<Value>, query: &str) -> Value {
    let (current, size) = page(query);
    let total = items.len();
    let data: Vec<Value> = items
        .into_iter()
        .skip((current - 1).saturating_mul(size))
        .take(size)
        .collect();
    json!({ "total": total, "data": data })
}

async fn users(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    match session(state, req).await {
        Ok(s) => Reply::ok(&paged(vec![overview::user_of(state, &s)], &req.query)),
        Err(reply) => reply,
    }
}

/// The second line of a PC's card (design v3 §5-2): `note` of a password-less state; `expired`
/// has [`expired_note`].
pub fn pwless_note(state: &str) -> &'static str {
    match state {
        "active" => "비밀번호 없이 연결",
        "listed" => "이 PC에서 로그인 필요",
        "off" => "PC에서 꺼 둠",
        "disabled" => "비밀번호 없는 연결 일시 중지(관리자)",
        "needs-relogin" => "이 PC에서 다시 로그인 필요",
        _ => "",
    }
}

/// The note of `expired`: when the PC last synced, "마지막 확인 3일 전".
pub fn expired_note(secs_ago: i64) -> String {
    format!("마지막 확인 {}", pcsync::ago(secs_ago))
}

/// The note of a PC's state at `at`.
fn note_of(pwless: &str, device: &Device, at: i64) -> String {
    match (pwless, device.last_sync_at) {
        ("expired", Some(last)) => expired_note(at - last),
        _ => pwless_note(pwless).to_owned(),
    }
}

/// A PC of the account as the app's PeerPayload reads it, with its password-less state (design
/// v3 §5-2, pcsync.rs): `hjv.pk` is the key hbbs held when it was bound, which the app compares
/// with the key of the PC it reaches (key continuity, §3-4); `wol` is whether it can be woken
/// (`ok`, `warn`, `no`, `unknown`; wake.rs `peer_wol`), which also ends the note once the account
/// has a relay (" · 켜기 가능", " · 켜기 준비 안 됨").
fn peer_json(
    device: &Device,
    portal_id: &str,
    pwless: &str,
    at: i64,
    kinds: &[String],
    wol: (&str, &str),
) -> Value {
    json!({
        "id": device.device_id,
        "user_name": portal_id,
        "info": { "device_name": device.hostname, "os": device.os },
        "status": 1,
        "note": format!("{}{}", note_of(pwless, device, at), wol.1),
        "hjv": {
            "pk": b64(&device.public_key),
            "pwless": pwless,
            "kinds": kinds,
            "wol": wol.0,
        },
    })
}

async fn peers(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let account_id = s.account.id;
    let found = state
        .db
        .call(move |c| {
            Ok((
                db::devices_of(c, account_id)?,
                db::passwordless_on(c, account_id)?,
                db::wol_infos_of(c, account_id)?,
                !db::active_relays_of(c, account_id)?.is_empty(),
            ))
        })
        .await;
    match found {
        Ok((devices, on, infos, has_relay)) => {
            let (kinds, at) = (&state.config.passwordless_kinds, now());
            let items = devices
                .iter()
                .filter(|d| d.device_id != s.device_id)
                .map(|d| {
                    let pwless = pcsync::pwless_state(d, on, at, state.config.trust_ttl_secs);
                    let info = infos.iter().find(|i| i.device_id == d.device_id);
                    let wol = wake::peer_wol(info, has_relay);
                    peer_json(d, &s.account.portal_id, pwless, at, kinds, wol)
                })
                .collect();
            Reply::ok(&paged(items, &req.query))
        }
        Err(e) => {
            log_err!("peers: {e}");
            Reply::server_error()
        }
    }
}

fn status_text(status: BindStatus) -> &'static str {
    match status {
        BindStatus::Added => "이 PC를 내 PC 목록에 추가했습니다.",
        BindStatus::Already => "이 PC는 이미 내 PC 목록에 있습니다.",
        BindStatus::OtherAccount => "다른 계정에 등록된 PC입니다. 관리자에게 문의하세요.",
        BindStatus::NotEligible => "설치된 Windows PC만 내 PC 목록에 추가합니다.",
        BindStatus::CheckFailed => {
            "이 PC를 확인하지 못해 내 PC 목록에 추가하지 않았습니다. 잠시 뒤 다시 로그인해 주세요."
        }
        BindStatus::Removed => "이 PC를 내 PC 목록에서 뺐습니다.",
        BindStatus::None => "",
    }
}

/// The token's device now: the login's outcome, unless the device left the list since, whether
/// [취소] can undo it, and the PC with its password-less state when the token's login proved that
/// it is this PC of the account.
struct DeviceState {
    status: BindStatus,
    undo: bool,
    pc: Option<(Device, &'static str)>,
}

async fn device_state(state: &Arc<AppState>, s: &Session) -> Result<DeviceState, Reply> {
    let recorded = BindStatus::parse(&s.bind_status);
    if s.device_id.is_empty() {
        return Ok(DeviceState {
            status: recorded,
            undo: false,
            pc: None,
        });
    }
    let (id, account_id) = (s.device_id.clone(), s.account.id);
    let (device, on) = state
        .db
        .call(move |c| Ok((db::device(c, &id)?, db::passwordless_on(c, account_id)?)))
        .await
        .map_err(|e| {
            log_err!("this-device: {e}");
            Reply::server_error()
        })?;
    let device = device.filter(|d| d.account_id == s.account.id);
    let in_account = device.is_some();
    let status = match recorded {
        BindStatus::Added | BindStatus::Already if !in_account => BindStatus::Removed,
        other => other,
    };
    let pc = device.filter(|_| s.device_verified).map(|d| {
        let pwless = pcsync::pwless_state(&d, on, now(), state.config.trust_ttl_secs);
        (d, pwless)
    });
    // [취소] undoes what this login did; a PC that was in the list before leaves it through
    // DELETE /api/hjv/devices/{id}
    Ok(DeviceState {
        status,
        undo: in_account && s.device_verified && recorded == BindStatus::Added,
        pc,
    })
}

/// `this-device` (design v3 §4-3): for a PC of the account whose login proved its device key,
/// also its binding ID, its password-less state and what its last sync said of the binding.
fn device_reply(s: &Session, found: &DeviceState) -> Reply {
    let mut answer = json!({
        "id": s.device_id,
        "status": found.status.as_str(),
        "undo": found.undo,
        "message": status_text(found.status),
    });
    if let Some((d, pwless)) = &found.pc {
        if let Some(binding) = d.binding_id {
            answer["binding"] = json!(hex(&binding));
        }
        answer["pwless"] = json!(pwless);
        answer["activation"] = json!(pcsync::activation(d));
    }
    Reply::ok(&answer)
}

async fn this_device(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    match device_state(state, &s).await {
        Ok(found) => device_reply(&s, &found),
        Err(reply) => reply,
    }
}

async fn undo(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let found = match device_state(state, &s).await {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    if !found.undo {
        return device_reply(&s, &found);
    }
    let (hash, id, account_id, at) = (
        s.token_hash.clone(),
        s.device_id.clone(),
        s.account.id,
        now(),
    );
    let removed = state
        .db
        .call(move |c| {
            let removed = db::in_tx(c, |tx| {
                let removed = db::release_binding_in(tx, &id, account_id, "undo", at)?;
                db::set_bind_status(tx, &hash, BindStatus::Removed.as_str())?;
                Ok(removed)
            })?;
            db::checkpoint(c)?;
            Ok(removed)
        })
        .await;
    match removed {
        Ok(_) => {
            state
                .record(
                    "device",
                    Some(state.tags.account(&s.account.login_key)),
                    Some(state.tags.device(&s.device_id)),
                    None,
                    "removed-undo",
                )
                .await;
            device_reply(
                &s,
                &DeviceState {
                    status: BindStatus::Removed,
                    undo: false,
                    pc: None,
                },
            )
        }
        Err(e) => {
            log_err!("undo: {e}");
            Reply::server_error()
        }
    }
}

async fn remove_device(state: &Arc<AppState>, req: &ApiRequest, id: &str) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    if !valid_device_id(id) {
        return Reply::not_found();
    }
    let (device_id, account_id, at) = (id.to_owned(), s.account.id, now());
    let removed = state
        .db
        .call(move |c| {
            let removed = db::in_tx(c, |tx| {
                db::release_binding_in(tx, &device_id, account_id, "remove", at)
            })?;
            if removed {
                db::checkpoint(c)?;
            }
            Ok(removed)
        })
        .await;
    match removed {
        Ok(true) => {
            state
                .record(
                    "device",
                    Some(state.tags.account(&s.account.login_key)),
                    Some(state.tags.device(id)),
                    None,
                    "removed-by-user",
                )
                .await;
            Reply::ok(&json!({ "id": id, "removed": true }))
        }
        // not in this account, whether it exists elsewhere or not
        Ok(false) => Reply::not_found(),
        Err(e) => {
            log_err!("remove device: {e}");
            Reply::server_error()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_content_types() {
        let mut h = HeaderMap::new();
        assert!(!is_json(&h));
        for (v, ok) in [
            ("application/json", true),
            ("application/json; charset=utf-8", true),
            ("Application/JSON;charset=UTF-8", true),
            ("text/plain", false),
            ("application/jsonx", false),
            ("multipart/form-data", false),
        ] {
            h.insert(CONTENT_TYPE, HeaderValue::from_static(v));
            assert_eq!(is_json(&h), ok, "{v}");
        }
    }

    #[test]
    fn paging() {
        let items: Vec<Value> = (0..150).map(|i| json!(i)).collect();
        let first = paged(items.clone(), "current=1&pageSize=100&accessible=&status=1");
        assert_eq!(first["total"], 150);
        assert_eq!(first["data"].as_array().unwrap().len(), 100);
        let second = paged(items.clone(), "current=2&pageSize=100");
        assert_eq!(second["data"].as_array().unwrap().len(), 50);
        assert_eq!(second["data"][0], 100);
        let none = paged(items.clone(), "current=3&pageSize=100");
        assert_eq!(none["data"].as_array().unwrap().len(), 0);
        assert_eq!(
            paged(items, "pageSize=0&current=x")["data"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
    }

    #[test]
    fn bearer_tokens() {
        let mut h = HeaderMap::new();
        assert!(bearer_hash(&h).is_none());
        let token = format!("hjv_{}", "a".repeat(43));
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert_eq!(bearer_hash(&h), Some(sha256(token.as_bytes())));
        for bad in [
            "Bearer hjv_short",
            "Basic abc",
            "Bearer xyz_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ] {
            h.insert(AUTHORIZATION, HeaderValue::from_static(bad));
            assert!(bearer_hash(&h).is_none(), "{bad}");
        }
    }

    #[test]
    fn texts_never_say_disabled() {
        for text in [
            TEXT_UNAUTHORIZED,
            TEXT_FORBIDDEN,
            TEXT_TOO_LARGE,
            TEXT_MEDIA_TYPE,
            TEXT_METHOD,
            TEXT_INCOMPLETE,
            TEXT_SERVER,
            TEXT_NOT_FOUND,
            TEXT_CLOCK,
            login::FAILURE_TEXT,
            login::TEXT_SEAL_CLOCK,
            login::TEXT_SSO_DOWN,
            pwless_note("active"),
            pwless_note("listed"),
            pwless_note("off"),
            pwless_note("disabled"),
            pwless_note("needs-relogin"),
            &expired_note(3 * crate::util::DAY),
            &expired_note(90),
            status_text(BindStatus::Added),
            status_text(BindStatus::Already),
            status_text(BindStatus::OtherAccount),
            status_text(BindStatus::NotEligible),
            status_text(BindStatus::CheckFailed),
            status_text(BindStatus::Removed),
        ] {
            assert!(!text.to_lowercase().contains("disabled"));
        }
        assert_eq!(expired_note(3 * crate::util::DAY + 5), "마지막 확인 3일 전");
    }
}
