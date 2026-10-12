// SPDX-License-Identifier: AGPL-3.0-only

//! The rules every request follows (design v2 §9-1): JSON answers with the security headers, no
//! Origin, JSON bodies only, no CORS, 404 for every path hjv-api does not serve.

mod common;

use common::*;
use hyper::Method;
use serde_json::json;

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
            vec![("authorization", &auth)],
            None,
            200,
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
            vec![("authorization", &auth), ("origin", "https://evil.example")],
            None,
            403,
        ),
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
    // GET requests carry only the Authorization header, as the app's list requests do
    assert_eq!(
        h.request(Method::GET, "/api/users", &[("authorization", &auth)], None)
            .await
            .status,
        200
    );
    // an empty body needs no type
    assert_eq!(
        h.request(
            Method::POST,
            "/api/hjv/this-device/undo",
            &[("authorization", &auth)],
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
    let a = h
        .request(
            Method::POST,
            "/api/currentUser",
            &[
                ("authorization", &auth),
                ("content-type", "application/json; charset=utf-8"),
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
