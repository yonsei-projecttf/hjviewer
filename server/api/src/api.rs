// SPDX-License-Identifier: AGPL-3.0-only

//! The account paths (design v2 §9-1, checked against the app's code in the design-v3 research
//! notes), and the rules every request follows.
//!
//! | path | answer |
//! |---|---|
//! | `POST /api/login` | login.rs (every failure that depends on the ID is a 401) |
//! | `POST /api/currentUser` | the user object itself; 401 only for a token that is not valid, never 400 (the app logs out on both) |
//! | `POST /api/logout` | revokes the token and takes the device it came from out of the list |
//! | `GET /api/users` | `{"total":1,"data":[<user>]}` |
//! | `GET /api/peers` | `{"total":n,"data":[{"id","user_name","info":{"device_name","os"},"status":1}]}`: the account's PCs but the asking device |
//! | `GET /api/hjv/this-device` | what the token's login did with its device, and whether it can be undone |
//! | `POST /api/hjv/this-device/undo` | takes that device out of the list again |
//! | `DELETE /api/hjv/devices/{id}` | takes a device out of the list ("내 목록에서 빼기") |
//! | `GET /api/hjv/source` | licence, source and running version (AGPL section 13) |
//! | `GET /api/login-options`, `GET /api/device-group/accessible` | the static answers nginx gives as well |
//!
//! Every answer is JSON in UTF-8 with `X-Content-Type-Options: nosniff`,
//! `Content-Security-Policy: default-src 'none'` and `Cache-Control: no-store`. A request with an
//! `Origin` header is refused (the app sends none, so a web page cannot use a visitor's browser);
//! a request with a body must be `application/json` (parameters such as charset allowed); there is
//! no CORS. Error texts never contain the word "disabled", which makes the app empty its list.
//!
//! Later phases add their paths in [`route`]: the password-less connection proof of design v3
//! (for example `POST /api/hjv/connect-ticket`) and the Wake-on-LAN paths of v2 §7
//! (`/api/hjv/device/poll`, `/api/hjv/device/wol-info`, `/api/hjv/devices/{id}/wake`, …), each in
//! a module of its own that uses [`session`] for the user's token.

use crate::{
    db::{self, Device, Session},
    login::{self, BindStatus},
    state::AppState,
    util::{now, sha256, valid_device_id},
};
use http_body_util::Full;
use hyper::{
    body::Bytes,
    header::{
        HeaderMap, HeaderValue, ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_SECURITY_POLICY,
        CONTENT_TYPE, ORIGIN, X_CONTENT_TYPE_OPTIONS,
    },
    Method, Response, StatusCode,
};
use serde_json::{json, Value};
use std::sync::Arc;

pub const MAX_BODY: usize = 64 * 1024;
const TEXT_UNAUTHORIZED: &str = "로그인이 필요합니다. 다시 로그인해 주세요.";
const TEXT_FORBIDDEN: &str = "접근이 차단되었습니다";
const TEXT_TOO_LARGE: &str = "요청이 너무 큽니다";
const TEXT_MEDIA_TYPE: &str = "JSON 요청만 받습니다";
const TEXT_METHOD: &str = "허용되지 않는 요청입니다";
const TEXT_INCOMPLETE: &str = "요청을 다 받지 못했습니다. 잠시 뒤 다시 시도해 주세요";
const TEXT_SERVER: &str = "서버 오류입니다. 잠시 뒤 다시 시도해 주세요";
const TEXT_NOT_FOUND: &str = "not found";

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
    pub headers: HeaderMap,
    pub body: Result<Bytes, BodyError>,
    /// the client's address: X-Real-IP (set by nginx), else the TCP peer, else "unknown"
    pub ip: String,
}

/// An answer: a JSON body with the security headers.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: StatusCode,
    pub body: String,
    pub allow: Option<&'static str>,
}

impl Reply {
    pub fn json(status: StatusCode, value: &Value) -> Reply {
        Reply {
            status,
            body: value.to_string(),
            allow: None,
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

    pub fn into_response(self, head: bool) -> Response<Full<Bytes>> {
        let body = if head {
            Bytes::new()
        } else {
            Bytes::from(self.body)
        };
        let mut response = Response::new(Full::new(body));
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
        "/api/users" | "/api/peers" | "/api/hjv/this-device" => "GET",
        "/api/login-options" | "/api/device-group/accessible" | "/api/hjv/source" => "GET, HEAD",
        p if p.starts_with("/api/hjv/devices/") => "DELETE",
        // later phases: their paths here and in `route` (module docs)
        _ => return None,
    })
}

/// Dispatches a request.
pub async fn route(state: &Arc<AppState>, req: ApiRequest) -> Reply {
    if req.path == "/api/login" {
        return login::handle(state, req).await;
    }
    let Some(allow) = allowed_methods(&req.path) else {
        return Reply::not_found();
    };
    if req.headers.contains_key(ORIGIN) {
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
    match (req.path.as_str(), &req.method) {
        ("/api/currentUser", &Method::POST) => current_user(state, &req).await,
        ("/api/logout", &Method::POST) => logout(state, &req).await,
        ("/api/users", &Method::GET) => users(state, &req).await,
        ("/api/peers", &Method::GET) => peers(state, &req).await,
        ("/api/hjv/this-device", &Method::GET) => this_device(state, &req).await,
        ("/api/hjv/this-device/undo", &Method::POST) => undo(state, &req).await,
        (path, &Method::DELETE) if path.starts_with("/api/hjv/devices/") => {
            let id = &path["/api/hjv/devices/".len()..];
            remove_device(state, &req, id).await
        }
        // later phases dispatch here to modules of their own, which take the user's token
        // with `session` and a device's signature with `devsig::verify` (README.md)
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
    }
}

fn unauthorized() -> Reply {
    Reply::error(StatusCode::UNAUTHORIZED, TEXT_UNAUTHORIZED)
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

/// The session of the request's token; the reply to send when there is none.
pub async fn session(state: &Arc<AppState>, req: &ApiRequest) -> Result<Session, Reply> {
    let hash = bearer_hash(&req.headers).ok_or_else(unauthorized)?;
    let (idle, max, at) = (
        state.config.token_idle_secs,
        state.config.token_max_secs,
        now(),
    );
    let found = state
        .db
        .call(move |c| {
            let s = db::session(c, &hash, at, idle, max)?;
            if let Some(s) = &s {
                db::touch_session(c, s, at)?;
            }
            Ok(s)
        })
        .await;
    match found {
        Ok(Some(s)) => Ok(s),
        Ok(None) => Err(unauthorized()),
        Err(e) => {
            log_err!("session: {e}");
            Err(Reply::server_error())
        }
    }
}

async fn current_user(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    match session(state, req).await {
        Ok(s) => Reply::ok(&user_json(&s.account.portal_id, &s.account.display_name)),
        Err(reply) => reply,
    }
}

async fn logout(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let (hash, device_id, verified, account_id) = (
        s.token_hash.clone(),
        s.device_id.clone(),
        s.device_verified,
        s.account.id,
    );
    let result = state
        .db
        .call(move |c| {
            db::delete_token(c, &hash)?;
            // the device leaves the list when it logs out (v2 §5), if it proved its key at login
            let removed =
                verified && !device_id.is_empty() && db::unbind(c, &device_id, account_id)?;
            if removed {
                db::checkpoint(c)?;
            }
            Ok(removed)
        })
        .await;
    match result {
        Ok(removed) => {
            if removed {
                state
                    .record(
                        "device",
                        Some(state.tags.account(&s.account.login_key)),
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

fn paged(items: Vec<Value>, query: &str) -> Value {
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
        Ok(s) => Reply::ok(&paged(
            vec![user_json(&s.account.portal_id, &s.account.display_name)],
            &req.query,
        )),
        Err(reply) => reply,
    }
}

fn peer_json(device: &Device, portal_id: &str) -> Value {
    json!({
        "id": device.device_id,
        "user_name": portal_id,
        "info": { "device_name": device.hostname, "os": device.os },
        "status": 1,
    })
}

async fn peers(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let account_id = s.account.id;
    match state.db.call(move |c| db::devices_of(c, account_id)).await {
        Ok(devices) => {
            let items = devices
                .iter()
                .filter(|d| d.device_id != s.device_id)
                .map(|d| peer_json(d, &s.account.portal_id))
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
        BindStatus::Added => "이 PC를 내 기기 목록에 추가했습니다.",
        BindStatus::Already => "이 PC는 이미 내 기기 목록에 있습니다.",
        BindStatus::OtherAccount => "다른 계정에 등록된 PC입니다. 관리자에게 문의하세요.",
        BindStatus::NotEligible => "설치된 Windows PC만 내 기기 목록에 추가합니다.",
        BindStatus::CheckFailed => {
            "이 PC를 확인하지 못해 목록에 추가하지 않았습니다. 잠시 뒤 다시 로그인해 보세요."
        }
        BindStatus::Removed => "이 PC를 내 기기 목록에서 뺐습니다.",
        BindStatus::None => "",
    }
}

/// The state of the token's device now: the login's outcome, unless the device left the list
/// since.
async fn device_state(state: &Arc<AppState>, s: &Session) -> Result<(BindStatus, bool), Reply> {
    let recorded = BindStatus::parse(&s.bind_status);
    if s.device_id.is_empty() {
        return Ok((recorded, false));
    }
    let id = s.device_id.clone();
    let device = state
        .db
        .call(move |c| db::device(c, &id))
        .await
        .map_err(|e| {
            log_err!("this-device: {e}");
            Reply::server_error()
        })?;
    let in_account = device.is_some_and(|d| d.account_id == s.account.id);
    let status = match recorded {
        BindStatus::Added | BindStatus::Already if !in_account => BindStatus::Removed,
        other => other,
    };
    // [취소] undoes what this login did; a PC that was in the list before leaves it through
    // DELETE /api/hjv/devices/{id}
    Ok((
        status,
        in_account && s.device_verified && recorded == BindStatus::Added,
    ))
}

fn device_reply(s: &Session, status: BindStatus, undo: bool) -> Reply {
    Reply::ok(&json!({
        "id": s.device_id,
        "status": status.as_str(),
        "undo": undo,
        "message": status_text(status),
    }))
}

async fn this_device(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    match device_state(state, &s).await {
        Ok((status, undo)) => device_reply(&s, status, undo),
        Err(reply) => reply,
    }
}

async fn undo(state: &Arc<AppState>, req: &ApiRequest) -> Reply {
    let s = match session(state, req).await {
        Ok(s) => s,
        Err(reply) => return reply,
    };
    let (status, can_undo) = match device_state(state, &s).await {
        Ok(found) => found,
        Err(reply) => return reply,
    };
    if !can_undo {
        return device_reply(&s, status, false);
    }
    let (hash, id, account_id) = (s.token_hash.clone(), s.device_id.clone(), s.account.id);
    let removed = state
        .db
        .call(move |c| {
            let removed = db::unbind(c, &id, account_id)?;
            db::set_bind_status(c, &hash, BindStatus::Removed.as_str())?;
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
            device_reply(&s, BindStatus::Removed, false)
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
    let (device_id, account_id) = (id.to_owned(), s.account.id);
    let removed = state
        .db
        .call(move |c| {
            let removed = db::unbind(c, &device_id, account_id)?;
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
            login::FAILURE_TEXT,
            status_text(BindStatus::Added),
            status_text(BindStatus::Already),
            status_text(BindStatus::OtherAccount),
            status_text(BindStatus::NotEligible),
            status_text(BindStatus::CheckFailed),
            status_text(BindStatus::Removed),
        ] {
            assert!(!text.to_lowercase().contains("disabled"));
        }
    }
}
