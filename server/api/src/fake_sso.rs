// SPDX-License-Identifier: AGPL-3.0-only

//! A stand-in for the Yonsei SSO, for tests and the E2E (the real SSO answers registered server
//! addresses only, and tests never call it). It follows the contract in sso.rs: the token call
//! checks the Basic secret and the password grant, the user call the bearer token, api uuid and
//! client id. It keeps no password and writes no log; it counts calls, which `GET /_stats` reports.
//!
//! The integration tests start it in-process; the E2E runs `hjv-fake-sso` (cargo feature
//! `fake-sso`) on 127.0.0.1 with the fixed credentials and accounts below. Usernames starting with
//! `sso-` play the SSO's failures, whatever the password.

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
    pub username: &'static str,
    pub password: &'static str,
    pub persl_no: &'static str,
    pub name: &'static str,
}

/// The fixed test accounts (fake data; usernames compare without case, as portal IDs do here).
pub const ACCOUNTS: &[FakeAccount] = &[
    FakeAccount {
        username: "2026000001",
        password: "hjv-test-pass-1",
        persl_no: "2026000001",
        name: "홍길동",
    },
    FakeAccount {
        username: "2026000002",
        password: "hjv-test-pass-2",
        persl_no: "2026000002",
        name: "김연세",
    },
    FakeAccount {
        username: "Y9001",
        password: "hjv-test-pass-3",
        persl_no: "Y9001",
        name: "이교수",
    },
    // another username of 홍길동, for which the SSO names the perslNo of the first account:
    // whether the real SSO takes such forms is not known (design v2 §4-1), and a block holds
    // across them (tests/allowlist.rs)
    FakeAccount {
        username: "hong-alias",
        password: "hjv-test-pass-1",
        persl_no: "2026000001",
        name: "홍길동",
    },
];

/// The usernames that play SSO failures (sso.rs sorts each).
pub const SCENARIOS: &[&str] = &[
    "sso-address",
    "sso-address-user",
    "sso-500",
    "sso-slow",
    "sso-empty",
    "sso-no-success",
    "sso-no-token",
    "sso-html",
    "sso-invalid-grant",
    "sso-invalid-client",
    "sso-false-nomsg",
    "sso-user-500",
    "sso-no-perslno",
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

/// Serves the fake SSO on `listener`; `slow` is how long "sso-slow" takes.
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

async fn answer(fake: &Fake, req: Request<Incoming>) -> Response<Full<Bytes>> {
    let path = req.uri().path().to_owned();
    let method = req.method().clone();
    let auth = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    let body = match Limited::new(req.into_body(), 64 * 1024).collect().await {
        Ok(b) => b.to_bytes(),
        Err(_) => return reply(413, "{}"),
    };
    let form: HashMap<String, String> = form_urlencoded::parse(&body).into_owned().collect();
    match (method, path.as_str()) {
        (Method::POST, TOKEN_PATH) => {
            fake.stats.token_calls.fetch_add(1, Ordering::SeqCst);
            token(fake, &auth, &form).await
        }
        (Method::POST, USER_PATH) => {
            fake.stats.user_calls.fetch_add(1, Ordering::SeqCst);
            user(fake, &auth, &form)
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

fn issue(fake: &Fake, username: &str) -> String {
    let token = crate::util::b64url(&crate::util::random_bytes::<24>());
    fake.tokens
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(token.clone(), username.to_owned());
    json!({ "success": true, "access_token": token, "token_type": "bearer" }).to_string()
}

async fn token(fake: &Fake, auth: &str, form: &HashMap<String, String>) -> Response<Full<Bytes>> {
    let username = form.get("username").map(String::as_str).unwrap_or("");
    let password = form.get("password").map(String::as_str).unwrap_or("");
    if auth != format!("Basic {BASIC}") || username == "sso-invalid-client" {
        return reply(401, r#"{"error":"invalid_client"}"#);
    }
    if form.get("grant_type").map(String::as_str) != Some("password") {
        return reply(400, r#"{"error":"unsupported_grant_type"}"#);
    }
    match username {
        "sso-address" => reply(200, r#"{"success":false,"msg":"Not Allowed IP Address"}"#),
        "sso-500" => reply(500, "internal error"),
        "sso-slow" => {
            tokio::time::sleep(fake.slow).await;
            reply(200, issue(fake, username))
        }
        "sso-empty" => reply(200, "{}"),
        "sso-no-success" => reply(200, r#"{"access_token":"x"}"#),
        "sso-no-token" => reply(200, r#"{"success":true}"#),
        "sso-html" => reply(200, "<html><body>maintenance</body></html>"),
        "sso-invalid-grant" => reply(400, r#"{"error":"invalid_grant"}"#),
        "sso-false-nomsg" => reply(200, r#"{"success":false}"#),
        "sso-address-user" | "sso-user-500" | "sso-no-perslno" => reply(200, issue(fake, username)),
        _ => {
            let found = ACCOUNTS
                .iter()
                .find(|a| a.username.eq_ignore_ascii_case(username) && a.password == password);
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

fn user(fake: &Fake, auth: &str, form: &HashMap<String, String>) -> Response<Full<Bytes>> {
    let username = auth.strip_prefix("Bearer ").and_then(|t| {
        fake.tokens
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(t)
    });
    let Some(username) = username else {
        return reply(200, r#"{"success":false,"msg":"invalid token"}"#);
    };
    if form.get("apiUuid").map(String::as_str) != Some(API_UUID)
        || form.get("clientId").map(String::as_str) != Some(CLIENT_ID)
    {
        return reply(200, r#"{"success":false,"msg":"invalid api"}"#);
    }
    match username.as_str() {
        "sso-address-user" => reply(200, r#"{"success":false,"msg":"not allowed ip"}"#),
        "sso-user-500" => reply(500, "internal error"),
        "sso-no-perslno" => reply(200, r#"{"success":true,"data":[{"name":"x"}]}"#),
        "sso-slow" => reply(
            200,
            r#"{"success":true,"data":[{"perslNo":"sso-slow","name":"느림"}]}"#,
        ),
        _ => match ACCOUNTS.iter().find(|a| a.username == username) {
            Some(a) => reply(
                200,
                json!({
                    "success": true,
                    "data": [{
                        "perslNo": a.persl_no, "name": a.name, "email": "fake@example.ac.kr",
                        "deprtNm": "가짜학과", "divCd": "01",
                    }],
                })
                .to_string(),
            ),
            None => reply(200, r#"{"success":false,"msg":"invalid token"}"#),
        },
    }
}
