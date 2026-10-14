// SPDX-License-Identifier: AGPL-3.0-only

//! A stand-in for the Yonsei SSO, for tests and the E2E (the real SSO answers registered server
//! addresses only, and tests never call it). It answers as sso.rs says the SSO answers the
//! registration of the 행정허브 plugin: the token call checks the Basic secret and a form of
//! exactly `username`, `password` and `grant_type=password`, the username uppercased; the user
//! call checks the bearer token and a form of exactly `apiUuid` and `clientId`, and answers
//! `data[0]` with `pcode`, `name`, `email` and `deptNm`. A request that breaks the contract gets
//! 400 `{"error":"invalid_request"}` (hjv-api takes it for an upstream error, so that a test sees
//! it), an unknown bearer token 401 `{"error":"invalid_token"}`. It keeps no password and writes
//! no log; it counts calls, which `GET /_stats` reports.
//!
//! The integration tests start it in-process; the E2E runs `hjv-fake-sso` (cargo feature
//! `fake-sso`) on 127.0.0.1 with the fixed credentials and accounts below. The usernames of
//! [`SCENARIOS`] play the SSO's failures and its answers about the person, whatever the password;
//! like the accounts' they pass HJViewer's ID rule (util.rs `staff_id`), as no other ID reaches
//! the SSO.

use http_body_util::{BodyExt, Full, Limited};
use hyper::{
    body::{Bytes, Incoming},
    header::{AUTHORIZATION, CONTENT_TYPE},
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::rt::TokioIo;
use serde_json::json;
use std::{
    collections::HashMap,
    convert::Infallible,
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{net::TcpListener, sync::oneshot};

pub const TOKEN_PATH: &str = "/oauth/token.do";
pub const USER_PATH: &str = "/api/findByApiUuid.do";
/// base64 of "hjv-fake-client:hjv-fake-secret", as the SSO issues client secrets
pub const BASIC: &str = "aGp2LWZha2UtY2xpZW50Omhqdi1mYWtlLXNlY3JldA==";
pub const API_UUID: &str = "hjv-fake-api-uuid";
pub const CLIENT_ID: &str = "hjv-fake-client";

pub struct FakeAccount {
    /// as the token call takes it: uppercased
    pub username: &'static str,
    pub password: &'static str,
    /// as the user call answers it
    pub pcode: &'static str,
    pub name: &'static str,
}

/// The fixed test accounts (fake data, of HJViewer's ID rule: Z or Y, not ZA).
pub const ACCOUNTS: &[FakeAccount] = &[
    FakeAccount {
        username: "ZZTEST01",
        password: "hjv-test-pass-1",
        pcode: "ZZTEST01",
        name: "홍길동",
    },
    FakeAccount {
        username: "ZZTEST02",
        password: "hjv-test-pass-2",
        pcode: "ZZTEST02",
        name: "김연세",
    },
    // a Y code, which the SSO names in lower case: hjv-api keeps it uppercased
    FakeAccount {
        username: "YZTEST03",
        password: "hjv-test-pass-3",
        pcode: "yztest03",
        name: "이교수",
    },
    // another username of 홍길동, for which the SSO names the pcode of the first account:
    // whether the real SSO takes such forms is not known (design v2 §4-1), and a block holds
    // across them (tests/allowlist.rs)
    FakeAccount {
        username: "ZZTEST01-ALIAS",
        password: "hjv-test-pass-1",
        pcode: "ZZTEST01",
        name: "홍길동",
    },
];

/// The usernames that play the SSO's failures and its answers about the person (sso.rs sorts
/// each).
pub const SCENARIOS: &[&str] = &[
    "ZZSSO-ADDRESS",
    "ZZSSO-ADDRESS-USER",
    "ZZSSO-500",
    "ZZSSO-SLOW",
    "ZZSSO-EMPTY",
    "ZZSSO-NO-SUCCESS",
    "ZZSSO-NO-TOKEN",
    "ZZSSO-HTML",
    "ZZSSO-HUGE",
    "ZZSSO-INVALID-GRANT",
    "ZZSSO-INVALID-CLIENT",
    "ZZSSO-FALSE-NOMSG",
    "ZZSSO-USER-500",
    "ZZSSO-NO-PCODE",
    "ZZSSO-DENIED",
    "ZZSSO-ZA",
    "ZZSSO-B",
    "ZZSSO-STUDENT",
    "ZZSSO-STRINGS",
];

#[derive(Default)]
pub struct Stats {
    pub token_calls: AtomicUsize,
    pub user_calls: AtomicUsize,
}

struct Fake {
    stats: Arc<Stats>,
    /// issued tokens: token → username
    tokens: Mutex<HashMap<String, String>>,
    slow: Duration,
}

pub struct FakeSso {
    pub addr: SocketAddr,
    pub stats: Arc<Stats>,
    stop: Option<oneshot::Sender<()>>,
}

impl FakeSso {
    pub fn token_url(&self) -> String {
        format!("http://{}{TOKEN_PATH}", self.addr)
    }

    pub fn user_url(&self) -> String {
        format!("http://{}{USER_PATH}", self.addr)
    }

    pub fn token_calls(&self) -> usize {
        self.stats.token_calls.load(Ordering::SeqCst)
    }

    pub fn user_calls(&self) -> usize {
        self.stats.user_calls.load(Ordering::SeqCst)
    }
}

impl Drop for FakeSso {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

/// Serves the fake SSO on `listener`; `slow` is how long "ZZSSO-SLOW" takes.
pub fn start(listener: TcpListener, slow: Duration) -> FakeSso {
    let addr = listener.local_addr().expect("a bound listener");
    let fake = Arc::new(Fake {
        stats: Arc::new(Stats::default()),
        tokens: Mutex::new(HashMap::new()),
        slow,
    });
    let stats = fake.stats.clone();
    let (stop, mut stopped) = oneshot::channel();
    tokio::spawn(async move {
        loop {
            let stream = tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => match accepted {
                    Ok((s, _)) => s,
                    Err(_) => continue,
                },
            };
            let fake = fake.clone();
            tokio::spawn(async move {
                let service = service_fn(move |req| {
                    let fake = fake.clone();
                    async move { Ok::<_, Infallible>(answer(&fake, req).await) }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    FakeSso {
        addr,
        stats,
        stop: Some(stop),
    }
}

fn reply(status: u16, body: impl Into<String>) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(body.into())));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r.headers_mut().insert(
        CONTENT_TYPE,
        "application/json;charset=UTF-8"
            .parse()
            .expect("a header value"),
    );
    r
}

/// The answer to a request that breaks the contract.
fn invalid_request() -> Response<Full<Bytes>> {
    reply(400, r#"{"error":"invalid_request"}"#)
}

/// The form of a request, when it is a form of exactly `keys`, each once.
fn exact_form(form: bool, body: &[u8], keys: &[&str]) -> Option<HashMap<String, String>> {
    let pairs: Vec<(String, String)> = form_urlencoded::parse(body).into_owned().collect();
    let mut names: Vec<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
    names.sort_unstable();
    let mut want = keys.to_vec();
    want.sort_unstable();
    (form && names == want).then(|| pairs.into_iter().collect())
}

async fn answer(fake: &Fake, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = req.uri().path().to_owned();
    let method = req.method().clone();
    let header = |name| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned()
    };
    let auth = header(AUTHORIZATION);
    let form = header(CONTENT_TYPE).starts_with("application/x-www-form-urlencoded");
    let body = match Limited::new(req.into_body(), 64 * 1024).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return reply(413, "{}"),
    };
    match (method, path.as_str()) {
        (Method::POST, TOKEN_PATH) => {
            fake.stats.token_calls.fetch_add(1, Ordering::SeqCst);
            match exact_form(form, &body, &["username", "password", "grant_type"]) {
                Some(form) => token(fake, &auth, &form).await,
                None => invalid_request(),
            }
        }
        (Method::POST, USER_PATH) => {
            fake.stats.user_calls.fetch_add(1, Ordering::SeqCst);
            match exact_form(form, &body, &["apiUuid", "clientId"]) {
                Some(form) => user(fake, &auth, &form),
                None => invalid_request(),
            }
        }
        (Method::GET, "/_stats") => reply(
            200,
            json!({
                "token_calls": fake.stats.token_calls.load(Ordering::SeqCst),
                "user_calls": fake.stats.user_calls.load(Ordering::SeqCst),
            })
            .to_string(),
        ),
        _ => reply(404, r#"{"error":"not found"}"#),
    }
}

/// A new access token for `username`, kept for its user call.
fn new_token(fake: &Fake, username: &str) -> String {
    let token = crate::util::b64url(&crate::util::random_bytes::<24>());
    fake.tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), username.to_owned());
    token
}

fn issue(fake: &Fake, username: &str) -> String {
    json!({ "success": true, "access_token": new_token(fake, username), "token_type": "bearer" })
        .to_string()
}

async fn token(fake: &Fake, auth: &str, form: &HashMap<String, String>) -> Response<Full<Bytes>> {
    let username = form.get("username").map(String::as_str).unwrap_or("");
    let password = form.get("password").map(String::as_str).unwrap_or("");
    if auth != format!("Basic {BASIC}") || username == "ZZSSO-INVALID-CLIENT" {
        return reply(401, r#"{"error":"invalid_client"}"#);
    }
    if form.get("grant_type").map(String::as_str) != Some("password") {
        return reply(400, r#"{"error":"unsupported_grant_type"}"#);
    }
    // the contract sends the portal ID uppercased
    if username != username.to_ascii_uppercase() {
        return invalid_request();
    }
    match username {
        "ZZSSO-ADDRESS" => reply(200, r#"{"success":false,"msg":"Not Allowed IP Address"}"#),
        "ZZSSO-500" => reply(500, "internal error"),
        "ZZSSO-SLOW" => {
            tokio::time::sleep(fake.slow).await;
            reply(200, issue(fake, username))
        }
        "ZZSSO-EMPTY" => reply(200, "{}"),
        "ZZSSO-NO-SUCCESS" => reply(200, r#"{"access_token":"x"}"#),
        "ZZSSO-NO-TOKEN" => reply(200, r#"{"success":true}"#),
        "ZZSSO-HTML" => reply(200, "<html><body>maintenance</body></html>"),
        // a token answer past the 1 MiB that hjv-api reads (the contract's limit, written out
        // here), which would log in if it were read
        "ZZSSO-HUGE" => reply(
            200,
            json!({
                "success": true,
                "access_token": new_token(fake, username),
                "pad": "a".repeat(1024 * 1024),
            })
            .to_string(),
        ),
        "ZZSSO-INVALID-GRANT" => reply(400, r#"{"error":"invalid_grant"}"#),
        "ZZSSO-FALSE-NOMSG" => reply(200, r#"{"success":false}"#),
        // the contract's other forms of success: a string here, a number in the user call
        "ZZSSO-STRINGS" => reply(
            200,
            json!({ "success": "true", "access_token": new_token(fake, username) }).to_string(),
        ),
        "ZZSSO-ADDRESS-USER" | "ZZSSO-USER-500" | "ZZSSO-NO-PCODE" | "ZZSSO-DENIED"
        | "ZZSSO-ZA" | "ZZSSO-B" | "ZZSSO-STUDENT" => reply(200, issue(fake, username)),
        _ => {
            let found = ACCOUNTS
                .iter()
                .find(|a| a.username == username && a.password == password);
            match found {
                Some(a) => reply(200, issue(fake, a.username)),
                None => reply(
                    200,
                    json!({ "success": false, "msg": "아이디 또는 비밀번호가 일치하지 않습니다." })
                        .to_string(),
                ),
            }
        }
    }
}

/// A user answer of the 행정허브 shape.
fn person(pcode: &str, name: &str) -> String {
    json!({
        "success": true,
        "data": [{
            "pcode": pcode, "name": name, "email": "fake@example.com", "deptNm": "가짜학과",
        }],
    })
    .to_string()
}

fn user(fake: &Fake, auth: &str, form: &HashMap<String, String>) -> Response<Full<Bytes>> {
    let username = auth.strip_prefix("Bearer ").and_then(|t| {
        fake.tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(t)
    });
    let Some(username) = username else {
        return reply(401, r#"{"error":"invalid_token"}"#);
    };
    if form.get("apiUuid").map(String::as_str) != Some(API_UUID)
        || form.get("clientId").map(String::as_str) != Some(CLIENT_ID)
    {
        return invalid_request();
    }
    match username.as_str() {
        "ZZSSO-ADDRESS-USER" => reply(200, r#"{"success":false,"msg":"not allowed ip"}"#),
        "ZZSSO-USER-500" => reply(500, "internal error"),
        // the answer of another registration, which names a perslNo: hjv-api does not read it
        "ZZSSO-NO-PCODE" => reply(
            200,
            r#"{"success":true,"data":[{"perslNo":"ZZSSO-NO-PCODE","name":"x","email":null,"deptNm":null}]}"#,
        ),
        // the user call refuses: access denied
        "ZZSSO-DENIED" => reply(200, r#"{"success":false,"msg":"access denied"}"#),
        // codes outside HJViewer's ID rule: ZA, B (which the 행정허브 plugin takes), a number
        "ZZSSO-ZA" => reply(200, person("ZATEST01", "가짜")),
        "ZZSSO-B" => reply(200, person("BZTEST01", "가짜")),
        "ZZSSO-STUDENT" => reply(200, person("1234567890", "가짜")),
        "ZZSSO-SLOW" => reply(200, person("ZZSSO-SLOW", "느림")),
        "ZZSSO-HUGE" => reply(200, person("ZZSSO-HUGE", "큼")),
        "ZZSSO-STRINGS" => reply(
            200,
            json!({
                "success": 1,
                "data": [{ "pcode": "zzsso-strings", "name": "문자열", "email": null, "deptNm": null }],
            })
            .to_string(),
        ),
        _ => match ACCOUNTS.iter().find(|a| a.username == username) {
            Some(a) => reply(200, person(a.pcode, a.name)),
            None => reply(401, r#"{"error":"invalid_token"}"#),
        },
    }
}
