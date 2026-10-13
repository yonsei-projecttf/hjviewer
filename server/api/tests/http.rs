// SPDX-License-Identifier: AGPL-3.0-only

//! The rules every request follows (design v2 §9-1): JSON answers with the security headers, no
//! Origin, JSON bodies only, no CORS, 404 for every path hjv-api does not serve; and the account
//! key's signature on every request with a token (design v3 §4-2, proof.rs).

mod common;

use common::*;
use hjv_api::{
    proof,
    util::{now, random_bytes},
};
use hyper::Method;
use serde_json::json;
use std::time::Duration;

/// method, path, headers, body, expected status
type Case<'a> = (
    Method,
    &'a str,
    Vec<(&'a str, &'a str)>,
    Option<Vec<u8>>,
    u16,
);

fn assert_security_headers(a: &Answer) {
    assert_eq!(
        a.headers["content-type"], "application/json; charset=utf-8",
        "{}",
        a.text
    );
    assert_eq!(a.headers["x-content-type-options"], "nosniff");
    assert_eq!(a.headers["content-security-policy"], "default-src 'none'");
    assert_eq!(a.headers["cache-control"], "no-store");
    assert!(a.headers.get("access-control-allow-origin").is_none());
    assert!(a.headers.get("set-cookie").is_none());
}

#[tokio::test]
async fn every_answer_is_json_with_the_security_headers() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let auth = format!("Bearer {token}");
    let proof = h.proof_for("GET", "/api/users", b"", &token).unwrap();
    let cases: Vec<Case> = vec![
        (Method::GET, "/api/login-options", vec![], None, 200),
        (
            Method::GET,
            "/api/device-group/accessible?current=1&pageSize=100",
            vec![("authorization", &auth)],
            None,
            200,
        ),
        (Method::GET, "/api/hjv/source", vec![], None, 200),
        (
            Method::GET,
            "/api/users",
            vec![("authorization", &auth), ("x-hjv-proof", &proof)],
            None,
            200,
        ),
        // the token without its key's signature
        (
            Method::GET,
            "/api/users",
            vec![("authorization", &auth)],
            None,
            401,
        ),
        (Method::GET, "/api/peers", vec![], None, 401),
        (
            Method::POST,
            "/api/currentUser",
            vec![
                ("authorization", "Bearer x"),
                ("content-type", "application/json"),
            ],
            Some(b"{}".to_vec()),
            401,
        ),
        (Method::GET, "/api/heartbeat", vec![], None, 404),
        (
            Method::POST,
            "/api/sysinfo",
            vec![("content-type", "application/json")],
            Some(b"{}".to_vec()),
            404,
        ),
        (Method::GET, "/api/x.php", vec![], None, 404),
        (Method::POST, "/api/audit/conn", vec![], None, 404),
        (Method::GET, "/api/ab", vec![], None, 404),
        (Method::GET, "/api/hjv/devices", vec![], None, 404),
        (Method::GET, "/api/hjv/nothing", vec![], None, 404),
        (Method::GET, "/", vec![], None, 404),
        (
            Method::GET,
            "/api/users",
            vec![
                ("authorization", &auth),
                ("x-hjv-proof", &proof),
                ("origin", "https://evil.example"),
            ],
            None,
            403,
        ),
        (
            Method::DELETE,
            "/api/hjv/keys/revoke-others",
            vec![("authorization", &auth)],
            None,
            405,
        ),
        (
            Method::GET,
            "/api/hjv/keys/x",
            vec![("authorization", &auth)],
            None,
            405,
        ),
        (Method::GET, "/api/hjv/keysx", vec![], None, 404),
        (
            Method::POST,
            "/api/users",
            vec![("authorization", &auth)],
            None,
            405,
        ),
        (
            Method::PUT,
            "/api/hjv/devices/1",
            vec![("authorization", &auth)],
            None,
            405,
        ),
        (Method::OPTIONS, "/api/peers", vec![], None, 405),
    ];
    for (method, path, headers, body, status) in cases {
        let a = h.request(method.clone(), path, &headers, body).await;
        assert_eq!(a.status, status, "{method} {path}: {}", a.text);
        assert_security_headers(&a);
        assert!(
            a.json.is_object() || a.json.is_array(),
            "{path}: {}",
            a.text
        );
    }
    assert_eq!(
        h.request(Method::GET, "/api/heartbeat", &[], None)
            .await
            .json,
        json!({"error": "not found"})
    );
    let a = h
        .request(Method::GET, "/api/login-options", &[], None)
        .await;
    assert_eq!(a.json, json!([]));
    let a = h.request(Method::POST, "/api/logout", &[], None).await;
    assert_eq!(a.status, 401);
    let a = h.request(Method::GET, "/api/logout", &[], None).await;
    assert_eq!(
        (a.status, a.headers["allow"].to_str().unwrap()),
        (405, "POST")
    );
}

#[tokio::test]
async fn bodies_must_be_json() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let auth = format!("Bearer {token}");
    let a = h
        .request(
            Method::POST,
            "/api/currentUser",
            &[("authorization", &auth), ("content-type", "text/plain")],
            Some(b"{}".to_vec()),
        )
        .await;
    assert_eq!(a.status, 415);
    let a = h
        .request(
            Method::POST,
            "/api/currentUser",
            &[
                ("authorization", &auth),
                ("content-type", "application/x-www-form-urlencoded"),
            ],
            Some(b"id=1".to_vec()),
        )
        .await;
    assert_eq!(a.status, 415);
    // GET requests carry the Authorization and X-HJV-Proof headers only, as the app's list
    // requests do
    let proof = h.proof_for("GET", "/api/users", b"", &token).unwrap();
    assert_eq!(
        h.request(
            Method::GET,
            "/api/users",
            &[("authorization", &auth), ("x-hjv-proof", &proof)],
            None
        )
        .await
        .status,
        200
    );
    // an empty body needs no type
    let proof = h
        .proof_for("POST", "/api/hjv/this-device/undo", b"", &token)
        .unwrap();
    assert_eq!(
        h.request(
            Method::POST,
            "/api/hjv/this-device/undo",
            &[("authorization", &auth), ("x-hjv-proof", &proof)],
            None
        )
        .await
        .status,
        200
    );
    let big = vec![b'x'; 65 * 1024];
    let a = h
        .request(
            Method::POST,
            "/api/currentUser",
            &[
                ("authorization", &auth),
                ("content-type", "application/json"),
            ],
            Some(big),
        )
        .await;
    assert_eq!(a.status, 413);
    let proof = h
        .proof_for("POST", "/api/currentUser", b"{\"id\":\"\"}", &token)
        .unwrap();
    let a = h
        .request(
            Method::POST,
            "/api/currentUser",
            &[
                ("authorization", &auth),
                ("content-type", "application/json; charset=utf-8"),
                ("x-hjv-proof", &proof),
            ],
            Some(b"{\"id\":\"\"}".to_vec()),
        )
        .await;
    assert_eq!(a.status, 200);
}

#[tokio::test]
async fn head_and_source() {
    let h = Harness::start().await;
    let a = h
        .request(Method::HEAD, "/api/login-options", &[], None)
        .await;
    assert_eq!((a.status, a.text.as_str()), (200, ""));
    let a = h.request(Method::GET, "/api/hjv/source", &[], None).await;
    assert_eq!(a.json["license"], "AGPL-3.0-only");
    assert_eq!(
        a.json["source"],
        "https://github.com/yonsei-projecttf/hjviewer"
    );
    assert_eq!(a.json["version"], env!("CARGO_PKG_VERSION"));
    assert!(a.json["commit"].is_string());
}

#[tokio::test]
async fn a_forged_address_header_is_only_an_address() {
    // nginx overwrites X-Real-IP; anything else in it counts as one address, "invalid"
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_IP", "2/600".into())],
        ..Default::default()
    })
    .await;
    for ip in ["1.2.3.4, 5.6.7.8", "not an address"] {
        let a = h.login(&h.login_body("nobody", "x", None), ip).await;
        assert_eq!(a.status, 401);
    }
    let a = h
        .login(&h.login_body("nobody2", "x", None), "garbage")
        .await;
    assert_eq!(a.status, 429);
}

#[tokio::test]
async fn a_body_cut_off_is_not_a_logout() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // the app logs out on a 400 from currentUser as on a 401: a body that does not arrive whole
    // says nothing about the token
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let mut s = tokio::net::TcpStream::connect(h.api).await.unwrap();
    let head = format!(
        "POST /api/currentUser HTTP/1.1\r\nHost: rd.example.com\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: 100\r\n\r\n{{\"id\":"
    );
    s.write_all(head.as_bytes()).await.unwrap();
    s.shutdown().await.unwrap();
    let mut answer = Vec::new();
    s.read_to_end(&mut answer).await.unwrap();
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
    assert!(answer.contains("\"error\""), "{answer}");
    // the token still works
    assert_eq!(
        h.post("/api/currentUser", &token, Some(json!({"id": ""})))
            .await
            .status,
        200
    );
}

/// The paths that take a token, with a body for the ones that change something.
const TOKEN_PATHS: &[(&str, &str, Option<&str>)] = &[
    (
        "POST",
        "/api/currentUser",
        Some(r#"{"id":"","uuid":"dXVpZA=="}"#),
    ),
    (
        "GET",
        "/api/users?current=1&pageSize=100&accessible=&status=1",
        None,
    ),
    (
        "GET",
        "/api/peers?current=1&pageSize=100&accessible=&status=1",
        None,
    ),
    ("GET", "/api/hjv/this-device", None),
    ("POST", "/api/hjv/this-device/undo", None),
    ("DELETE", "/api/hjv/devices/123456789", None),
    ("GET", "/api/hjv/keys", None),
    ("POST", "/api/hjv/keys/1/ack", None),
    ("DELETE", "/api/hjv/keys/1", None),
    ("POST", "/api/hjv/keys/revoke-others", None),
];

/// A request with the token and an `X-HJV-Proof` value of the test's choice.
async fn with_proof(
    h: &Harness,
    method: &str,
    target: &str,
    token: &str,
    proof: Option<&str>,
    body: Option<&str>,
) -> Answer {
    let auth = format!("Bearer {token}");
    let mut headers = vec![("authorization", auth.as_str())];
    if body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    if let Some(p) = proof {
        headers.push(("x-hjv-proof", p));
    }
    h.request(
        Method::from_bytes(method.as_bytes()).unwrap(),
        target,
        &headers,
        body.map(|b| b.as_bytes().to_vec()),
    )
    .await
}

fn assert_unauthorized(a: &Answer, what: &str) {
    assert_eq!(a.status, 401, "{what}: {}", a.text);
    assert_eq!(
        a.json,
        json!({"error": "로그인이 필요합니다. 다시 로그인해 주세요."}),
        "{what}"
    );
}

#[tokio::test]
async fn a_token_alone_opens_nothing() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    let token = h.login_ok(HONG, Some(&pc)).await;
    let phone = h.login_ok(HONG, None).await;
    let ckid: i64 = h.get("/api/hjv/keys", &phone).await.json["data"][0]["ckid"]
        .as_i64()
        .unwrap();
    for (method, target, body) in TOKEN_PATHS {
        let target = target.replace("/keys/1", &format!("/keys/{ckid}"));
        for t in [&token, &phone] {
            let a = with_proof(&h, method, &target, t, None, *body).await;
            assert_unauthorized(&a, &format!("{method} {target}"));
        }
    }
    // nothing changed: the PC is in the list, both keys are active, the tokens work
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 1);
    assert_eq!(h.get("/api/hjv/keys", &phone).await.json["total"], 2);
    assert_eq!(h.get("/api/users", &token).await.status, 200);
    // and every path answers the app as before once the request is signed
    for (method, target, body) in TOKEN_PATHS {
        if target.contains("/keys/") || *method == "DELETE" {
            continue;
        }
        let proof = h
            .proof_for(method, target, body.unwrap_or("").as_bytes(), &phone)
            .unwrap();
        let a = with_proof(&h, method, target, &phone, Some(&proof), *body).await;
        assert_eq!(a.status, 200, "{method} {target}: {}", a.text);
    }
}

#[tokio::test]
async fn the_request_proof() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let other = h.login_ok(HONG, None).await;
    let seed = h.key_of(&token).unwrap();
    let body = r#"{"id":"","uuid":"dXVpZA=="}"#;
    let sign = |method: &str, target: &str, time: i64, body: &str, token: &str| {
        proof::header_value(
            &seed,
            method,
            target,
            time,
            &random_bytes::<16>(),
            body.as_bytes(),
            token,
        )
    };
    // right
    let good = sign("POST", "/api/currentUser", now(), body, &token);
    let a = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &token,
        Some(&good),
        Some(body),
    )
    .await;
    assert_eq!(a.status, 200, "{}", a.text);
    // the same nonce again on another request that changes something: refused like a wrong
    // token (the very same request again gets the first answer: see below)
    let used = proof::parse(&good).unwrap();
    let reused = proof::header_value(
        &seed,
        "POST",
        "/api/currentUser",
        used.time,
        &used.nonce,
        b"{}",
        &token,
    );
    let a = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &token,
        Some(&reused),
        Some("{}"),
    )
    .await;
    assert_unauthorized(&a, "nonce again");
    // a signature over this token's hash sent with another token, and another token's key
    // signing this token's request
    let paired = sign("POST", "/api/currentUser", now(), body, &token);
    let a = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &other,
        Some(&paired),
        Some(body),
    )
    .await;
    assert_unauthorized(&a, "with another token");
    let foreign = proof::header_value(
        &h.key_of(&other).unwrap(),
        "POST",
        "/api/currentUser",
        now(),
        &random_bytes::<16>(),
        body.as_bytes(),
        &token,
    );
    let a = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &token,
        Some(&foreign),
        Some(body),
    )
    .await;
    assert_unauthorized(&a, "another key");
    // a body, a path or a query other than the signed one
    let signed = sign("POST", "/api/currentUser", now(), body, &token);
    let a = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &token,
        Some(&signed),
        Some(r#"{"id":"1","uuid":"dXVpZA=="}"#),
    )
    .await;
    assert_unauthorized(&a, "body changed");
    let signed = sign("GET", "/api/users", now(), "", &token);
    let a = with_proof(&h, "GET", "/api/peers", &token, Some(&signed), None).await;
    assert_unauthorized(&a, "another path");
    let signed = sign("GET", "/api/users?current=1", now(), "", &token);
    let a = with_proof(
        &h,
        "GET",
        "/api/users?current=2",
        &token,
        Some(&signed),
        None,
    )
    .await;
    assert_unauthorized(&a, "another query");
    let a = with_proof(&h, "GET", "/api/users", &token, Some(&signed), None).await;
    assert_unauthorized(&a, "query left out");
    // not the format
    for bad in ["", "v1", "v1.x.y.z", &good.replacen("v1.", "v2.", 1)] {
        let a = with_proof(&h, "GET", "/api/users", &token, Some(bad), None).await;
        assert_unauthorized(&a, bad);
    }
    // the right signature from a clock ten minutes off: 409 clock with the server's time, which
    // the app does not take for a logout; the token still works
    for skew in [-700, 700] {
        let late = sign("GET", "/api/users", now() + skew, "", &token);
        let a = with_proof(&h, "GET", "/api/users", &token, Some(&late), None).await;
        assert_eq!(a.status, 409, "{}", a.text);
        assert_eq!(a.json["hjv"], "clock");
        assert_eq!(
            a.json["error"],
            "기기의 날짜와 시간이 서버와 10분 넘게 다릅니다. 날짜와 시간을 자동으로 맞추게 해 주세요."
        );
        assert!(a.json["server_time"].as_i64().unwrap().abs_diff(now()) <= 5);
    }
    // a wrong signature with a wrong time is a wrong signature
    let late = sign("GET", "/api/peers", now() - 700, "", &token);
    let a = with_proof(&h, "GET", "/api/users", &token, Some(&late), None).await;
    assert_unauthorized(&a, "wrong signature and time");
    // a GET may come again with the same proof (the app retries list requests)
    let list = sign("GET", "/api/users", now(), "", &token);
    for _ in 0..3 {
        let a = with_proof(&h, "GET", "/api/users", &token, Some(&list), None).await;
        assert_eq!(a.status, 200, "{}", a.text);
    }
    assert_eq!(h.get("/api/users", &token).await.status, 200);
    // the journal hears of refused proofs by the account's tag, without the path or the token
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!(
            "request proof invalid (acct={}",
            h.account_tag(HONG.0)
        )),
        "{logs}"
    );
    assert!(!logs.contains(&token), "{logs}");
}

#[tokio::test]
async fn the_very_same_request_again_gets_the_first_answer() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_RETRY_WINDOW", "2".into())],
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let phone = h.login_ok(HONG, None).await;
    let seed = h.key_of(&phone).unwrap();
    let (t, nonce) = (now(), random_bytes::<16>());
    let sign = |method: &str, target: &str, t: i64, body: &str| {
        proof::header_value(&seed, method, target, t, &nonce, body.as_bytes(), &phone)
    };
    let body = r#"{"id":"","uuid":"dXVpZA=="}"#;
    let user = sign("POST", "/api/currentUser", t, body);
    let first = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &phone,
        Some(&user),
        Some(body),
    )
    .await;
    assert_eq!(first.status, 200, "{}", first.text);
    // the app's TLS fallback sends a request again byte for byte when its first attempt failed,
    // possibly after hjv-api answered it: the first answer again, not the 401 that logs it out
    for _ in 0..2 {
        let a = with_proof(
            &h,
            "POST",
            "/api/currentUser",
            &phone,
            Some(&user),
            Some(body),
        )
        .await;
        assert_eq!((a.status, &a.text), (200, &first.text));
    }
    // the nonce with anything else is a replay, refused like a wrong token: another body, path,
    // query or time, each signed by the key, or a signature that is not the first's (another
    // key's over the same bytes)
    let other = r#"{"id":"","uuid":"b3RoZXI="}"#;
    let undo = "/api/hjv/this-device/undo";
    let query = "/api/currentUser?current=1";
    let foreign = proof::header_value(
        &random_bytes::<32>(),
        "POST",
        "/api/currentUser",
        t,
        &nonce,
        body.as_bytes(),
        &phone,
    );
    for (what, target, proof, body) in [
        (
            "body",
            "/api/currentUser",
            sign("POST", "/api/currentUser", t, other),
            other,
        ),
        ("path", undo, sign("POST", undo, t, body), body),
        ("query", query, sign("POST", query, t, body), body),
        (
            "time",
            "/api/currentUser",
            sign("POST", "/api/currentUser", t + 1, body),
            body,
        ),
        ("signature", "/api/currentUser", foreign, body),
    ] {
        let a = with_proof(&h, "POST", target, &phone, Some(&proof), Some(body)).await;
        assert_unauthorized(&a, what);
    }
    // which leaves the first answer to the very same request
    let a = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &phone,
        Some(&user),
        Some(body),
    )
    .await;
    assert_eq!((a.status, &a.text), (200, &first.text));
    // a request that changes something runs once: two at once and one after all get the first
    // answer, where a second run would find no PC (404); the PC left the list once
    let serial = h.change_serial().await;
    let pc_path = format!("/api/hjv/devices/{}", pc.id);
    let remove = proof::header_value(
        &seed,
        "DELETE",
        &pc_path,
        now(),
        &random_bytes::<16>(),
        b"",
        &phone,
    );
    let (a, b) = tokio::join!(
        with_proof(&h, "DELETE", &pc_path, &phone, Some(&remove), None),
        with_proof(&h, "DELETE", &pc_path, &phone, Some(&remove), None)
    );
    let c = with_proof(&h, "DELETE", &pc_path, &phone, Some(&remove), None).await;
    for x in [&a, &b, &c] {
        assert_eq!(x.status, 200, "{}", x.text);
        assert_eq!(x.json, json!({"id": pc.id, "removed": true}));
    }
    assert_eq!(h.change_serial().await, serial + 1);
    let removed = h.results().await;
    assert_eq!(
        removed
            .iter()
            .filter(|r| *r == "device:removed-by-user")
            .count(),
        1,
        "{removed:?}"
    );
    // past the window the very same request is a replay too, and the token works on
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let a = with_proof(
        &h,
        "POST",
        "/api/currentUser",
        &phone,
        Some(&user),
        Some(body),
    )
    .await;
    assert_unauthorized(&a, "the same request past the window");
    let a = with_proof(&h, "DELETE", &pc_path, &phone, Some(&remove), None).await;
    assert_unauthorized(&a, "the same removal past the window");
    let again = h.post(
        "/api/currentUser",
        &phone,
        Some(json!({"id": "", "uuid": "dXVpZA=="})),
    );
    assert_eq!(again.await.status, 200);
    // the journal hears of it by the account's tag
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!(
            "request proof again (acct={}, POST)",
            h.account_tag(HONG.0)
        )),
        "{logs}"
    );
    assert!(!logs.contains(&phone), "{logs}");
}

/// How long the retry tests hold the database, and when they send the retry or drop the first
/// attempt: the first attempt reaches the database well before.
const HOLD_MS: u64 = 1000;
const FIRST_SENT_MS: u64 = 300;

/// Holds the database from when this returns for `ms` milliseconds, as a slow disk would: the
/// requests that need it wait.
async fn hold_db(h: &Harness, ms: u64) -> tokio::task::JoinHandle<()> {
    let (held, holding) = tokio::sync::oneshot::channel();
    let state = h.state.clone();
    let busy = tokio::spawn(async move {
        state
            .db
            .call(move |_| {
                let _ = held.send(());
                std::thread::sleep(Duration::from_millis(ms));
                Ok(())
            })
            .await
            .unwrap();
    });
    holding.await.unwrap();
    busy
}

/// A signed request as the bytes of a connection of its own.
fn raw_request(method: &str, target: &str, token: &str, proof: &str, body: &str) -> String {
    let content = if body.is_empty() {
        String::new()
    } else {
        format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        )
    };
    format!(
        "{method} {target} HTTP/1.1\r\nHost: rd.example.com\r\nAuthorization: Bearer {token}\r\nX-HJV-Proof: {proof}\r\n{content}Connection: close\r\n\r\n{body}"
    )
}

/// The request nonces used so far.
async fn used_nonces(h: &Harness) -> i64 {
    h.state
        .db
        .call(|c| c.query_row("SELECT COUNT(*) FROM req_nonces", [], |r| r.get(0)))
        .await
        .unwrap()
}

#[tokio::test]
async fn the_retry_of_a_request_dropped_unanswered_is_no_logout() {
    use tokio::io::AsyncWriteExt;
    // The app gives up on an attempt that takes too long and sends the very same request again.
    // When the first attempt's connection went before hjv-api answered it, hyper dropped its
    // handler: the retry gets 500, not the 401 that logs the app out, at once, not after the
    // request time limit, and the request did not run.
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let phone = h.login_ok(HONG, None).await;
    let serial = h.change_serial().await;
    let pc_path = format!("/api/hjv/devices/{}", pc.id);
    let user = r#"{"id":"","uuid":"dXVpZA=="}"#;
    for (method, target, body) in [
        ("POST", "/api/currentUser", user),
        ("DELETE", pc_path.as_str(), ""),
    ] {
        let proof = h
            .proof_for(method, target, body.as_bytes(), &phone)
            .unwrap();
        let used = used_nonces(&h).await;
        let busy = hold_db(&h, HOLD_MS).await;
        let mut first = tokio::net::TcpStream::connect(h.api).await.unwrap();
        first
            .write_all(raw_request(method, target, &phone, &proof, body).as_bytes())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(FIRST_SENT_MS)).await;
        drop(first);
        busy.await.unwrap();
        // the first attempt's check of its proof had begun, so it still ran and took the nonce;
        // the retry comes after it, so the retry is the one that finds the nonce taken
        for _ in 0..100 {
            if used_nonces(&h).await > used {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            used_nonces(&h).await,
            used + 1,
            "{method} {target}: the first attempt did not reach the check of its proof"
        );
        let body = (!body.is_empty()).then_some(body);
        let a = with_proof(&h, method, target, &phone, Some(&proof), body).await;
        assert_eq!(a.status, 500, "{method} {target}: {}", a.text);
        assert!(a.json["error"].is_string(), "{method} {target}: {}", a.text);
        assert!(
            a.elapsed < Duration::from_secs(5),
            "{method} {target}: {:?}",
            a.elapsed
        );
    }
    // nothing ran: the PC is in the list; and the token and its key work on
    assert_eq!(h.change_serial().await, serial);
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 1);
    let a = h.delete(&pc_path, &phone).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json, json!({"id": pc.id, "removed": true}));
    assert_eq!(h.change_serial().await, serial + 1);
}

#[tokio::test]
async fn a_retry_while_the_first_attempt_runs_gets_its_answer() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    // The retry may come while the first attempt still runs (its path was slow, not gone): both
    // get the first answer, and the request runs once.
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let phone = h.login_ok(HONG, None).await;
    let serial = h.change_serial().await;
    let pc_path = format!("/api/hjv/devices/{}", pc.id);
    let proof = h.proof_for("DELETE", &pc_path, b"", &phone).unwrap();
    let busy = hold_db(&h, HOLD_MS).await;
    let mut first = tokio::net::TcpStream::connect(h.api).await.unwrap();
    first
        .write_all(raw_request("DELETE", &pc_path, &phone, &proof, "").as_bytes())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(FIRST_SENT_MS)).await;
    // the first attempt waits for the database, and the retry comes
    let retry = with_proof(&h, "DELETE", &pc_path, &phone, Some(&proof), None);
    let (retry, busy) = tokio::join!(retry, busy);
    busy.unwrap();
    let mut answer = Vec::new();
    first.read_to_end(&mut answer).await.unwrap();
    let answer = String::from_utf8_lossy(&answer);
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert_eq!(retry.status, 200, "{}", retry.text);
    assert_eq!(retry.json, json!({"id": pc.id, "removed": true}));
    assert!(
        answer.ends_with(&format!("\r\n\r\n{}", retry.text)),
        "{answer}"
    );
    assert_eq!(h.change_serial().await, serial + 1);
    let removed = h.results().await;
    assert_eq!(
        removed
            .iter()
            .filter(|r| *r == "device:removed-by-user")
            .count(),
        1,
        "{removed:?}"
    );
}
