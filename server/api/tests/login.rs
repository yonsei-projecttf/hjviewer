// SPDX-License-Identifier: AGPL-3.0-only

//! POST /api/login end to end: the order of the checks of design v2 §9-2, the SSO contract of
//! §4-1 through the fake SSO, sealing, retries, limits and the second-factor hook.

mod common;

use common::*;
use hjv_api::{
    fake_sso,
    limits::{SCOPE_BOUND, SCOPE_ID, SCOPE_IP, SCOPE_OK_ID},
    login::{FAILURE_TEXT, TEXT_SEAL_CLOCK, TEXT_SSO_DOWN},
    seal,
    tfa::SecondFactor,
    util::{b64, login_key, now, random_bytes},
};
use hyper::Method;
use serde_json::json;
use std::{sync::Arc, time::Duration};

fn assert_failure(a: &Answer, status: u16) {
    assert_eq!(a.status, status, "{}", a.text);
    assert_eq!(a.json, json!({ "error": FAILURE_TEXT }), "{}", a.text);
}

/// The answer to a seal whose time is out of the window (design v3 §4-1): 409 `clock` with the
/// server's time.
fn assert_clock(a: &Answer) {
    assert_eq!(a.status, 409, "{}", a.text);
    assert_eq!(a.json["error"], TEXT_SEAL_CLOCK, "{}", a.text);
    assert_eq!(a.json["hjv"], "clock", "{}", a.text);
    let server_time = a.json["server_time"].as_i64().unwrap();
    assert!(server_time.abs_diff(now()) <= 5, "{}", a.text);
}

/// The answer of every login while SSO calls are paused (design v3 §4-1).
fn assert_sso_down(a: &Answer) {
    assert_eq!(a.status, 503, "{}", a.text);
    assert_eq!(a.json, json!({ "error": TEXT_SSO_DOWN }), "{}", a.text);
}

#[tokio::test]
async fn success_returns_the_apps_token_shape() {
    let h = Harness::start().await;
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.0.1")
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json["type"], "access_token");
    assert_eq!(
        a.json["user"],
        json!({"name": "2026000001", "display_name": "홍길동", "status": 1})
    );
    let token = a.token();
    assert!(token.starts_with("hjv_") && token.len() == 47, "{token}");
    assert_eq!(a.headers["content-type"], "application/json; charset=utf-8");
    assert_eq!((h.sso.token_calls(), h.sso.user_calls()), (1, 1));
    // only the token's hash is stored
    let stored: Vec<Vec<u8>> = h
        .state
        .db
        .call(|c| {
            let mut s = c.prepare("SELECT hash FROM tokens")?;
            let rows = s.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap();
    assert_eq!(
        stored,
        vec![hjv_api::util::sha256(token.as_bytes()).to_vec()]
    );
    // case-insensitive IDs: the allowlist and the account match "y9001" and "Y9001"
    let a = h
        .login(&h.login_body(" y9001 ", LEE.1, None), "10.0.0.1")
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json["user"]["name"], "Y9001");
}

#[tokio::test]
async fn allowlisted_and_unlisted_ids_look_the_same() {
    let h = Harness::with(Options {
        vars: vec![
            ("HJV_API_FAILURE_DELAY_MS", "600".into()),
            ("HJV_API_FAILURE_JITTER_MS", "100".into()),
        ],
        ..Default::default()
    })
    .await;
    let listed = h
        .login(&h.login_body(HONG.0, "wrong", None), "10.0.0.1")
        .await;
    let unlisted = h
        .login(&h.login_body("2026999999", "wrong", None), "10.0.0.2")
        .await;
    assert_failure(&listed, 401);
    assert_failure(&unlisted, 401);
    assert_eq!(listed.text, unlisted.text);
    for a in [&listed, &unlisted] {
        assert!(a.elapsed >= Duration::from_millis(500), "{:?}", a.elapsed);
        assert!(a.elapsed < Duration::from_millis(1500), "{:?}", a.elapsed);
    }
    // the unlisted ID never reached the SSO, and both failures count
    assert_eq!(h.sso.token_calls(), 1);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(HONG.0)).await, 1);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag("2026999999")).await, 1);
    assert_eq!(
        h.results().await,
        vec!["login:credentials", "login:not-allowed"]
    );
}

#[tokio::test]
async fn failures_wait_two_seconds_by_default() {
    let h = Harness::with(Options {
        vars: vec![
            ("HJV_API_FAILURE_DELAY_MS", "2000".into()),
            ("HJV_API_FAILURE_JITTER_MS", "500".into()),
        ],
        ..Default::default()
    })
    .await;
    let a = h
        .login(&h.login_body("2026999999", "x", None), "10.0.0.1")
        .await;
    assert_failure(&a, 401);
    assert!(a.elapsed >= Duration::from_millis(1500), "{:?}", a.elapsed);
    assert!(a.elapsed <= Duration::from_millis(3000), "{:?}", a.elapsed);
    // a success does not wait
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.0.1")
        .await;
    assert_eq!(a.status, 200);
    assert!(a.elapsed < Duration::from_millis(1000), "{:?}", a.elapsed);
}

#[tokio::test]
async fn id_limit_applies_to_every_id_and_stops_sso_calls() {
    let h = Harness::start().await;
    for (n, ip) in ["10.0.1.1", "10.0.1.2", "10.0.1.3"].iter().enumerate() {
        let a = h.login(&h.login_body(HONG.0, "wrong", None), ip).await;
        assert_failure(&a, 401);
        assert_eq!(h.sso.token_calls(), n + 1);
    }
    // the fourth try within ten minutes is refused before the SSO, even with the right password
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.1.4")
        .await;
    assert_failure(&a, 401);
    assert_eq!(h.sso.token_calls(), 3);
    // an unlisted ID is limited the same way
    for _ in 0..3 {
        assert_failure(
            &h.login(&h.login_body("unlisted1", "x", None), "10.0.2.1")
                .await,
            401,
        );
    }
    assert_failure(
        &h.login(&h.login_body("UNLISTED1", "x", None), "10.0.2.2")
            .await,
        401,
    );
    let results = h.results().await;
    assert_eq!(
        results.iter().filter(|r| *r == "login:limited-id").count(),
        2
    );
    // refused tries do not extend the lock
    assert_eq!(h.counted(SCOPE_ID, h.account_tag("unlisted1")).await, 3);
    // the admin clears it
    let out = h.admin(&["unlock", HONG.0], false, None).await.unwrap();
    assert!(out.contains("cleared 3"), "{out}");
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.1.5")
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
}

#[tokio::test]
async fn address_limit() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_IP", "4/600".into())],
        ..Default::default()
    })
    .await;
    for i in 0..4 {
        let a = h
            .login(&h.login_body(&format!("someone{i}"), "x", None), "10.0.3.1")
            .await;
        assert_failure(&a, 401);
    }
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.3.1")
        .await;
    assert_failure(&a, 429);
    assert_eq!(h.sso.token_calls(), 0);
    // other addresses are not affected
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.3.2")
        .await;
    assert_eq!(a.status, 200);
    assert_eq!(h.counted(SCOPE_IP, h.state.tags.ip("10.0.3.1")).await, 4);
}

#[tokio::test]
async fn concurrent_tries_cannot_pass_the_id_limit() {
    let h = Arc::new(Harness::start().await);
    let mut tasks = Vec::new();
    for i in 0..10 {
        let h = h.clone();
        tasks.push(tokio::spawn(async move {
            h.login(&h.login_body(HONG.0, "wrong", None), &format!("10.0.4.{i}"))
                .await
        }));
    }
    for t in tasks {
        assert_failure(&t.await.unwrap(), 401);
    }
    assert_eq!(h.sso.token_calls(), 3);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(HONG.0)).await, 3);
}

#[tokio::test]
async fn a_device_in_the_account_keeps_its_own_limit() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("111222333");
    h.hbbs.register(&pc, Some(2));
    h.login_ok(HONG, Some(&pc)).await;
    // someone who knows only the ID uses up its limit
    for i in 0..3 {
        let a = h
            .login(&h.login_body(HONG.0, "guess", None), &format!("10.0.5.{i}"))
            .await;
        assert_failure(&a, 401);
    }
    assert_failure(
        &h.login(&h.login_body(HONG.0, HONG.1, None), "10.0.5.9")
            .await,
        401,
    );
    // the owner's PC, signing its request, still logs in
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, Some(&pc)), "10.0.5.10")
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    // its own failures count apart, and a wrong signature does not get the separate limit
    let a = h
        .login(&h.login_body(HONG.0, "typo", Some(&pc)), "10.0.5.11")
        .await;
    assert_failure(&a, 401);
    let key = h.state.tags.bound(&login_key(HONG.0));
    assert_eq!(h.counted(SCOPE_BOUND, key).await, 1);
    let mut impostor = pc.clone();
    impostor.seed = [9; 32];
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, Some(&impostor)), "10.0.5.12")
        .await;
    assert_failure(&a, 401);
    assert!(h.results().await.contains(&"login:limited-id".to_owned()));
}

#[tokio::test]
async fn sealing_is_required() {
    let h = Harness::start().await;
    let ip = "10.0.6.1";
    // a plain password
    let mut body = h.login_body(HONG.0, HONG.1, None);
    body["password"] = json!(HONG.1);
    assert_failure(&h.login(&body, ip).await, 400);
    // no password at all
    body.as_object_mut().unwrap().remove("password");
    assert_failure(&h.login(&body, ip).await, 400);
    // sealed to a key the server does not have
    let other_pk = *crypto_box::SecretKey::from_bytes([1; 32])
        .public_key()
        .as_bytes();
    let mut body = h.login_body(HONG.0, HONG.1, None);
    body["password"] = json!(seal::seal_password(
        &other_pk,
        HONG.1,
        now(),
        &b64(&random_bytes::<16>()),
        ""
    ));
    assert_failure(&h.login(&body, ip).await, 400);
    // too old, too far ahead (clear of the second the server reads its clock in; the exact
    // boundary is login.rs's unit test): the clock answer, not the failure of a wrong password
    for t in [now() - 610, now() + 610] {
        let body = h.login_body_at(HONG.0, HONG.1, None, t, &b64(&random_bytes::<16>()));
        assert_clock(&h.login(&body, ip).await);
    }
    // within the ten minutes it works
    let body = h.login_body_at(
        HONG.0,
        HONG.1,
        None,
        now() - 590,
        &b64(&random_bytes::<16>()),
    );
    assert_eq!(h.login(&body, ip).await.status, 200);
    // sealed for another device than the request names
    let pc = TestDevice::windows("555");
    let mut body = h.login_body(HONG.0, HONG.1, Some(&pc));
    body["id"] = json!("556");
    assert_failure(&h.login(&body, ip).await, 400);
    assert_eq!(h.sso.token_calls(), 1);
    let results = h.results().await;
    for r in [
        "seal-missing",
        "seal-unknown-key",
        "seal-time",
        "seal-device",
    ] {
        assert!(results.contains(&format!("login:{r}")), "{r}: {results:?}");
    }
}

#[tokio::test]
async fn the_same_body_again_gets_the_first_answer() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_RETRY_WINDOW", "2".into())],
        ..Default::default()
    })
    .await;
    let body = h.login_body(HONG.0, HONG.1, None);
    // two at once and one after: one SSO call, one token
    let (a, b) = tokio::join!(h.login(&body, "10.0.7.1"), h.login(&body, "10.0.7.1"));
    let c = h.login(&body, "10.0.7.1").await;
    assert_eq!((a.status, b.status, c.status), (200, 200, 200));
    assert_eq!(a.token(), b.token());
    assert_eq!(a.token(), c.token());
    assert_eq!(h.sso.token_calls(), 1);
    // the nonce with another body is a replay
    let nonce = b64(&random_bytes::<16>());
    let t = now();
    let first = h.login_body_at(KIM.0, KIM.1, None, t, &nonce);
    let second = h.login_body_at(KIM.0, KIM.1, None, t, &nonce);
    assert_ne!(first, second);
    assert_eq!(h.login(&first, "10.0.7.2").await.status, 200);
    assert_failure(&h.login(&second, "10.0.7.2").await, 400);
    // after the window the same body is a replay too
    tokio::time::sleep(Duration::from_millis(2100)).await;
    assert_failure(&h.login(&body, "10.0.7.1").await, 400);
    assert_eq!(h.sso.token_calls(), 2);
    // a failure is answered the same way again, and counted once
    let wrong = h.login_body(LEE.0, "wrong", None);
    assert_failure(&h.login(&wrong, "10.0.7.3").await, 401);
    assert_failure(&h.login(&wrong, "10.0.7.3").await, 401);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(LEE.0)).await, 1);
}

#[tokio::test]
async fn sso_outcomes_are_sorted_and_counted_as_the_design_says() {
    let h = Harness::with(Options {
        allow: fake_sso::SCENARIOS.to_vec(),
        vars: vec![
            ("HJV_API_SSO_TIMEOUT_MS", "800".into()),
            ("HJV_API_LIMIT_ID", "1/600".into()),
            ("HJV_API_LIMIT_SSO", "100/60".into()),
            ("HJV_API_LIMIT_IP", "100/600".into()),
        ],
        sso_slow: Some(Duration::from_secs(3)),
        ..Default::default()
    })
    .await;
    // (username, record, counted against the ID); every one is the same 401 on the wire, so
    // that SSO trouble does not tell which IDs are on the list
    let cases = [
        ("sso-invalid-grant", "credentials", true),
        ("sso-address", "sso-address", false),
        ("sso-address-user", "sso-address", false),
        ("sso-500", "upstream", false),
        ("sso-empty", "upstream", false),
        ("sso-no-success", "upstream", false),
        ("sso-no-token", "upstream", false),
        ("sso-html", "upstream", false),
        ("sso-invalid-client", "sso-client", false),
        ("sso-false-nomsg", "upstream", false),
        ("sso-user-500", "upstream", false),
        ("sso-no-perslno", "upstream", false),
        ("sso-slow", "upstream", false),
    ];
    for (n, (user, result, counted)) in cases.into_iter().enumerate() {
        // each case on its own, without the pause that SSO trouble starts
        h.state.sso_breaker.reset();
        let ip = format!("10.0.8.{n}");
        let a = h.login(&h.login_body(user, "pw", None), &ip).await;
        assert_failure(&a, 401);
        assert_eq!(
            h.counted(SCOPE_ID, h.account_tag(user)).await,
            i64::from(counted),
            "{user}"
        );
        // the address counts every failure, as it does for an ID off the list
        assert_eq!(h.counted(SCOPE_IP, h.state.tags.ip(&ip)).await, 1, "{user}");
        assert_eq!(
            h.results().await.last().unwrap(),
            &format!("login:{result}"),
            "{user}"
        );
        if user == "sso-slow" {
            assert!(a.elapsed < Duration::from_millis(1500), "{:?}", a.elapsed);
        }
    }
    // with a limit of one failure, the not-counted cases still reach the SSO a second time
    h.state.sso_breaker.reset();
    let before = h.sso.token_calls();
    assert_failure(
        &h.login(&h.login_body("sso-500", "pw", None), "10.0.8.100")
            .await,
        401,
    );
    assert_failure(
        &h.login(&h.login_body("sso-invalid-grant", "pw", None), "10.0.8.100")
            .await,
        401,
    );
    assert_eq!(h.sso.token_calls(), before + 1);
    // the operator hears of the refused address at once, and of new SSO messages once
    let logs = hjv_api::log::captured().join("\n");
    assert!(logs.contains("refuses this server's address"), "{logs}");
    assert!(logs.contains("SSO message seen for the first time: Not Allowed IP Address"));
}

#[tokio::test]
async fn sso_trouble_pauses_the_calls() {
    let h = Harness::with(Options {
        allow: vec![HONG.0, KIM.0, "sso-address", "sso-500"],
        vars: vec![
            ("HJV_API_LIMIT_ID", "100/600".into()),
            ("HJV_API_LIMIT_IP", "100/600".into()),
            ("HJV_API_LIMIT_SSO", "100/60".into()),
        ],
        ..Default::default()
    })
    .await;
    // a refused address pauses every SSO call at once
    let a = h
        .login(&h.login_body("sso-address", "pw", None), "10.0.15.1")
        .await;
    assert_failure(&a, 401);
    assert_eq!(h.sso.token_calls(), 1);
    // meanwhile every ID gets the same 503 (design v3 §4-1), before the allowlist shows
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.15.2")
        .await;
    assert_sso_down(&a);
    assert_eq!(h.sso.token_calls(), 1);
    assert_eq!(h.results().await.last().unwrap(), "login:sso-paused");
    // counted neither against the ID nor the address: no password was tried
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(HONG.0)).await, 0);
    assert_eq!(h.attempts(SCOPE_IP, h.state.tags.ip("10.0.15.2")).await, 0);
    h.state.sso_breaker.reset();
    assert_eq!(
        h.login(&h.login_body(HONG.0, HONG.1, None), "10.0.15.2")
            .await
            .status,
        200
    );
    // five upstream errors in a row pause them too; an answer of the SSO in between starts over
    for _ in 0..4 {
        h.login(&h.login_body("sso-500", "pw", None), "10.0.15.3")
            .await;
    }
    h.login(&h.login_body(KIM.0, "wrong", None), "10.0.15.4")
        .await;
    for _ in 0..4 {
        h.login(&h.login_body("sso-500", "pw", None), "10.0.15.3")
            .await;
    }
    assert!(!h.state.sso_breaker.paused());
    h.login(&h.login_body("sso-500", "pw", None), "10.0.15.3")
        .await;
    assert!(h.state.sso_breaker.paused());
    let calls = h.sso.token_calls();
    assert_sso_down(
        &h.login(&h.login_body(KIM.0, KIM.1, None), "10.0.15.4")
            .await,
    );
    assert_eq!(h.sso.token_calls(), calls);
    let logs = hjv_api::log::captured().join("\n");
    assert!(logs.contains("SSO calls paused for 300 s"), "{logs}");
    assert!(logs.contains("SSO calls paused for 120 s"), "{logs}");
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(report.contains("SSO calls: paused"), "{report}");
}

#[tokio::test]
async fn sso_call_limit() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_SSO", "2/60".into())],
        ..Default::default()
    })
    .await;
    assert_eq!(
        h.login(&h.login_body(HONG.0, HONG.1, None), "10.0.9.1")
            .await
            .status,
        200
    );
    assert_failure(
        &h.login(&h.login_body(KIM.0, "wrong", None), "10.0.9.2")
            .await,
        401,
    );
    let a = h.login(&h.login_body(LEE.0, LEE.1, None), "10.0.9.3").await;
    assert_failure(&a, 401);
    assert_eq!(h.sso.token_calls(), 2);
    assert_eq!(h.results().await.last().unwrap(), "login:limited-sso");
    // not counted against the ID; against the address, as any failure is
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(LEE.0)).await, 0);
    assert_eq!(h.counted(SCOPE_IP, h.state.tags.ip("10.0.9.3")).await, 1);
}

#[tokio::test]
async fn one_account_takes_a_quarter_of_the_sso_calls() {
    let h = Harness::with(Options {
        vars: vec![
            ("HJV_API_LIMIT_SSO", "8/60".into()),
            ("HJV_API_LIMIT_ID", "100/600".into()),
        ],
        ..Default::default()
    })
    .await;
    for ip in ["10.0.16.1", "10.0.16.2"] {
        assert_failure(
            &h.login(&h.login_body(HONG.0, "wrong", None), ip).await,
            401,
        );
    }
    // the third call of the minute for Hong is refused, even with the right password
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.16.3")
        .await;
    assert_failure(&a, 401);
    assert_eq!(h.sso.token_calls(), 2);
    // the others still get their calls
    assert_eq!(
        h.login(&h.login_body(KIM.0, KIM.1, None), "10.0.16.4")
            .await
            .status,
        200
    );
    assert_eq!(h.sso.token_calls(), 3);
}

#[tokio::test]
async fn successful_logins_are_limited_too() {
    let h = Harness::with(Options {
        vars: vec![
            ("HJV_API_LIMIT_OK_ID", "2/3600".into()),
            ("HJV_API_LIMIT_OK_IP", "3/3600".into()),
        ],
        ..Default::default()
    })
    .await;
    // per account
    for ip in ["10.0.17.1", "10.0.17.2"] {
        assert_eq!(
            h.login(&h.login_body(HONG.0, HONG.1, None), ip)
                .await
                .status,
            200
        );
    }
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.17.3")
        .await;
    assert_failure(&a, 401);
    assert_eq!(h.sso.token_calls(), 2);
    assert_eq!(h.results().await.last().unwrap(), "login:limited-ok");
    assert_eq!(h.counted(SCOPE_OK_ID, h.account_tag(HONG.0)).await, 2);
    // per address: Hong's success there counts, and two more
    for account in [KIM, LEE] {
        assert_eq!(
            h.login(&h.login_body(account.0, account.1, None), "10.0.17.1")
                .await
                .status,
            200
        );
    }
    assert_failure(
        &h.login(&h.login_body(KIM.0, KIM.1, None), "10.0.17.1")
            .await,
        401,
    );
    assert_eq!(h.sso.token_calls(), 4);
    // a failed login does not use up a success
    assert_failure(
        &h.login(&h.login_body(KIM.0, "wrong", None), "10.0.17.4")
            .await,
        401,
    );
    assert_eq!(h.counted(SCOPE_OK_ID, h.account_tag(KIM.0)).await, 1);
    // the admin lifts it
    let out = h.admin(&["unlock", HONG.0], false, None).await.unwrap();
    assert!(out.contains("cleared 2"), "{out}");
    assert_eq!(
        h.login(&h.login_body(HONG.0, HONG.1, None), "10.0.17.5")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn request_form_and_types() {
    let h = Harness::start().await;
    let body = h.login_body(HONG.0, HONG.1, None).to_string().into_bytes();
    let a = h
        .request(
            Method::POST,
            "/api/login",
            &[("content-type", "text/plain")],
            Some(body.clone()),
        )
        .await;
    assert_failure(&a, 415);
    let a = h
        .request(
            Method::POST,
            "/api/login",
            &[
                ("content-type", "application/json"),
                ("origin", "https://evil.example"),
            ],
            Some(body.clone()),
        )
        .await;
    assert_failure(&a, 403);
    let a = h.request(Method::GET, "/api/login", &[], None).await;
    assert_failure(&a, 405);
    assert_eq!(a.headers["allow"], "POST");
    let a = h
        .request(
            Method::OPTIONS,
            "/api/login",
            &[("origin", "https://evil.example")],
            None,
        )
        .await;
    assert_eq!(a.status, 403);
    assert!(a.headers.get("access-control-allow-origin").is_none());
    let a = h
        .request(
            Method::POST,
            "/api/login",
            &[("content-type", "application/json")],
            Some(b"{".to_vec()),
        )
        .await;
    assert_failure(&a, 400);
    let big = vec![b' '; 70 * 1024];
    let a = h
        .request(
            Method::POST,
            "/api/login",
            &[("content-type", "application/json")],
            Some(big),
        )
        .await;
    assert_failure(&a, 413);
    let mut odd = h.login_body(HONG.0, HONG.1, None);
    odd["type"] = json!("sms_code");
    assert_failure(&h.login(&odd, "10.0.10.1").await, 400);
    // charset and other parameters are fine
    let a = h
        .request(
            Method::POST,
            "/api/login",
            &[("content-type", "application/json; charset=utf-8")],
            Some(body),
        )
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(h.sso.token_calls(), 1);
}

/// A second factor for the hook's test: "123456" is right for everyone.
struct FixedCode;

impl SecondFactor for FixedCode {
    fn required(&self) -> bool {
        true
    }

    fn verify(&self, _login_key: &str, code: &str) -> bool {
        code == "123456"
    }
}

#[tokio::test]
async fn second_factor_hook() {
    let h = Harness::with(Options {
        second_factor: Some(Arc::new(FixedCode)),
        ..Default::default()
    })
    .await;
    // every well-formed ID gets the code dialog, on the list or not
    let mut challenges = Vec::new();
    for user in [HONG.0, "2026999999"] {
        let a = h
            .login(&h.login_body(user, "anything", None), "10.0.11.1")
            .await;
        assert_eq!(a.status, 200, "{}", a.text);
        assert_eq!(a.json["type"], "email_check");
        assert_eq!(a.json["tfa_type"], "tfa_check");
        assert_eq!(a.json["user"]["name"], user);
        challenges.push(a.json["secret"].as_str().unwrap().to_owned());
    }
    assert_eq!(h.sso.token_calls(), 0);
    let code_request = |user: &str, secret: &str, code: &str| {
        json!({
            "verificationCode": code, "tfaCode": code, "secret": secret, "username": user,
            "id": "", "uuid": "dXVpZA==", "autoLogin": true, "type": "email_code",
            "deviceInfo": {"os": "ios", "type": "client", "name": "iPhone"},
        })
    };
    // off the list: the same failure as a wrong code, and no SSO call
    let a = h
        .login(
            &code_request("2026999999", &challenges[1], "123456"),
            "10.0.11.1",
        )
        .await;
    assert_failure(&a, 401);
    // the first request had a wrong password: the code step passes, the SSO refuses
    let a = h
        .login(&code_request(HONG.0, &challenges[0], "123456"), "10.0.11.1")
        .await;
    assert_failure(&a, 401);
    assert_eq!(h.sso.token_calls(), 1);
    // a challenge works once
    let a = h
        .login(&code_request(HONG.0, &challenges[0], "123456"), "10.0.11.1")
        .await;
    assert_failure(&a, 401);
    // the right password and code give the token with the user, which the app needs
    let a = h
        .login(&h.login_body(KIM.0, KIM.1, None), "10.0.11.2")
        .await;
    let secret = a.json["secret"].as_str().unwrap().to_owned();
    let wrong = h
        .login(&code_request(KIM.0, &secret, "000000"), "10.0.11.2")
        .await;
    assert_failure(&wrong, 401);
    let a = h
        .login(&h.login_body(KIM.0, KIM.1, None), "10.0.11.2")
        .await;
    let secret = a.json["secret"].as_str().unwrap().to_owned();
    let a = h
        .login(&code_request(KIM.0, &secret, "123456"), "10.0.11.2")
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json["type"], "access_token");
    assert_eq!(a.json["user"]["display_name"], "김연세");
}

#[tokio::test]
async fn code_requests_fail_while_the_second_factor_is_off() {
    let h = Harness::start().await;
    let a = h
        .login(
            &json!({"type": "email_code", "secret": "x", "username": HONG.0, "tfaCode": "1"}),
            "10.0.12.1",
        )
        .await;
    assert_failure(&a, 401);
    assert_eq!(h.sso.token_calls(), 0);
}

#[tokio::test]
async fn nothing_secret_reaches_the_logs() {
    let h = Harness::with(Options {
        allow: vec![HONG.0, KIM.0, "sso-address"],
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("777888999");
    h.hbbs.register(&pc, Some(1));
    let token = h.login_ok(HONG, Some(&pc)).await;
    for _ in 0..4 {
        let a = h
            .login(&h.login_body(KIM.0, "secret-typo-777", None), "10.0.13.1")
            .await;
        assert_failure(&a, 401);
    }
    let a = h
        .login(
            &h.login_body("sso-address", "pw-of-someone", None),
            "10.0.13.1",
        )
        .await;
    assert_failure(&a, 401);
    let logs = hjv_api::log::captured().join("\n");
    // the warnings are there, with tags
    assert!(
        logs.contains(&format!("ID limit reached (acct={}", h.account_tag(KIM.0))),
        "{logs}"
    );
    assert!(logs.contains("refuses this server's address"), "{logs}");
    for secret in [
        HONG.1,
        "secret-typo-777",
        "pw-of-someone",
        token.as_str(),
        HONG.0,
        KIM.0,
        "홍길동",
        pc.id.as_str(),
        pc.uuid.as_str(),
        "10.0.13.1",
        fake_sso::BASIC,
        fake_sso::API_UUID,
    ] {
        assert!(!logs.contains(secret), "{secret} in the logs:\n{logs}");
    }
}

#[tokio::test]
async fn an_unreachable_sso_is_an_upstream_error() {
    let (_held, refusing) = refusing_addr();
    let h = Harness::with(Options {
        vars: vec![("HJV_API_SSO_TOKEN_URL", format!("http://{refusing}/token"))],
        ..Default::default()
    })
    .await;
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.14.1")
        .await;
    assert_failure(&a, 401);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(HONG.0)).await, 0);
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains("SSO error: token call: cannot connect"),
        "{logs}"
    );
}

#[tokio::test]
async fn an_ipv6_client_counts_by_its_64() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_IP", "3/600".into())],
        ..Default::default()
    })
    .await;
    // new addresses of one /64 do not get past the address limit
    for n in 1..=3 {
        let ip = format!("2001:db8:1:2::{n:x}");
        assert_failure(
            &h.login(&h.login_body("someone", "x", None), &ip).await,
            401,
        );
    }
    assert_failure(
        &h.login(&h.login_body("someone", "x", None), "2001:db8:1:2:ffff::9")
            .await,
        429,
    );
    // another /64 is another client
    assert_eq!(
        h.login(&h.login_body(HONG.0, HONG.1, None), "2001:db8:1:3::1")
            .await
            .status,
        200
    );
    // the records keep the whole address
    let ips: Vec<String> = h
        .state
        .db
        .call(|c| {
            let mut s = c.prepare("SELECT ip FROM events ORDER BY id")?;
            let rows = s.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap();
    assert_eq!(ips[0], "2001:db8:1:2::1");
    assert!(ips.contains(&"2001:db8:1:2:ffff::9".to_owned()), "{ips:?}");
}

#[tokio::test]
async fn the_owners_pc_gets_past_a_full_address_limit() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_IP", "3/600".into())],
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("900000001");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    // someone behind the same campus address fills its limit
    let ip = "10.0.18.1";
    for n in 0..3 {
        let a = h
            .login(&h.login_body(&format!("guess{n}"), "x", None), ip)
            .await;
        assert_failure(&a, 401);
    }
    assert_failure(&h.login(&h.login_body(KIM.0, KIM.1, None), ip).await, 429);
    // the owner's PC, signing for the account it is in, still logs in, and its wrong password
    // counts under the account's PC limit, not the address
    let a = h.login(&h.login_body(HONG.0, HONG.1, Some(&pc)), ip).await;
    assert_eq!(a.status, 200, "{}", a.text);
    let a = h.login(&h.login_body(HONG.0, "typo", Some(&pc)), ip).await;
    assert_failure(&a, 401);
    let bound = h.state.tags.bound(&login_key(HONG.0));
    assert_eq!(h.counted(SCOPE_BOUND, bound).await, 1);
    assert_eq!(h.counted(SCOPE_IP, h.state.tags.ip(ip)).await, 3);
    // the same PC signing for another account, or a PC not in the account, gets 429
    assert_failure(
        &h.login(&h.login_body(KIM.0, KIM.1, Some(&pc)), ip).await,
        429,
    );
    let other = TestDevice::windows("900000002");
    h.hbbs.register(&other, Some(1));
    assert_failure(
        &h.login(&h.login_body(HONG.0, HONG.1, Some(&other)), ip)
            .await,
        429,
    );
    // the journal names the address by its tag, which unlock takes, as it takes the address
    let tag = h.state.tags.ip(ip);
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!("address limit reached (ip={tag})")),
        "{logs}"
    );
    // the address's three failures and the owner's success there
    let out = h
        .admin(&["unlock", &format!("ip:{ip}")], false, None)
        .await
        .unwrap();
    assert!(out.contains("cleared 4"), "{out}");
    assert_eq!(
        h.login(&h.login_body(KIM.0, KIM.1, None), ip).await.status,
        200
    );
    for n in 0..3 {
        h.login(&h.login_body(&format!("guess{n}"), "x", None), ip)
            .await;
    }
    let out = h
        .admin(&["unlock", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    assert!(out.contains("cleared 4"), "{out}");
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(!record.contains(ip), "{record}");
    assert!(record.contains(&format!("targets={tag}")), "{record}");
}

#[tokio::test]
async fn the_pcs_of_an_account_share_one_limit() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_BOUND", "2/600".into())],
        ..Default::default()
    })
    .await;
    let (pc1, pc2) = (
        TestDevice::windows("910000001"),
        TestDevice::windows("910000002"),
    );
    for pc in [&pc1, &pc2] {
        h.hbbs.register(pc, Some(1));
        h.login_ok(HONG, Some(pc)).await;
    }
    let calls = h.sso.token_calls();
    for (pc, ip) in [(&pc1, "10.0.19.1"), (&pc2, "10.0.19.2")] {
        assert_failure(
            &h.login(&h.login_body(HONG.0, "wrong", Some(pc)), ip).await,
            401,
        );
    }
    // a third PC, or either of them again, finds the limit full: no more SSO calls
    assert_failure(
        &h.login(&h.login_body(HONG.0, HONG.1, Some(&pc1)), "10.0.19.3")
            .await,
        401,
    );
    assert_eq!(h.sso.token_calls(), calls + 2);
    assert_eq!(h.results().await.last().unwrap(), "login:limited-bound");
}

#[tokio::test]
async fn seal_times_that_overflow_are_refused() {
    let h = Harness::start().await;
    for t in [i64::MIN, i64::MAX, now().wrapping_add(i64::MIN), -now()] {
        let body = h.login_body_at(HONG.0, HONG.1, None, t, &b64(&random_bytes::<16>()));
        assert_clock(&h.login(&body, "10.0.20.1").await);
    }
    assert_eq!(h.sso.token_calls(), 0);
    assert!(h.results().await.iter().all(|r| r == "login:seal-time"));
}

#[tokio::test]
async fn failure_records_are_capped_per_minute() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_IP", "100/600".into())],
        ..Default::default()
    })
    .await;
    for n in 0..25 {
        let a = h
            .login(
                &h.login_body(&format!("unlisted{n}"), "x", None),
                &format!("10.0.21.{n}"),
            )
            .await;
        assert_failure(&a, 401);
    }
    // the first 20 are written, the rest counted for the daily summary
    assert_eq!(h.results().await.len(), 20);
    assert_eq!(h.state.failure_records.take_skipped(), 5);
    // the limits still count every one
    assert_eq!(h.counted(SCOPE_ID, h.account_tag("unlisted24")).await, 1);
    // successes are always written
    h.login_ok(HONG, None).await;
    assert_eq!(h.results().await.last().unwrap(), "login:ok");
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains("more than 20 failed logins this minute"),
        "{logs}"
    );
}

#[tokio::test]
async fn a_slow_sso_slows_every_failure() {
    let h = Harness::with(Options {
        allow: vec![HONG.0, "sso-slow"],
        sso_slow: Some(Duration::from_millis(1000)),
        vars: vec![("HJV_API_SSO_TIMEOUT_MS", "4000".into())],
        ..Default::default()
    })
    .await;
    let quick = h
        .login(&h.login_body("2026999999", "x", None), "10.0.22.1")
        .await;
    assert_failure(&quick, 401);
    assert!(
        quick.elapsed < Duration::from_millis(700),
        "{:?}",
        quick.elapsed
    );
    // the SSO takes a second for one login
    let slow = h
        .login(&h.login_body("sso-slow", "x", None), "10.0.22.2")
        .await;
    assert_eq!(slow.status, 200, "{}", slow.text);
    // now an ID off the list, which never reaches the SSO, takes as long as one that does
    let a = h
        .login(&h.login_body("2026999998", "x", None), "10.0.22.3")
        .await;
    assert_failure(&a, 401);
    assert!(a.elapsed >= Duration::from_millis(1000), "{:?}", a.elapsed);
    assert!(a.elapsed < Duration::from_millis(2500), "{:?}", a.elapsed);
    // a success still does not wait
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.22.4")
        .await;
    assert_eq!(a.status, 200);
    assert!(a.elapsed < Duration::from_millis(900), "{:?}", a.elapsed);
}

#[tokio::test]
async fn code_dialogs_count_and_are_capped() {
    let h = Harness::with(Options {
        second_factor: Some(Arc::new(FixedCode)),
        ..Default::default()
    })
    .await;
    let ip = "10.0.23.1";
    // three dialogs from one address, each counting against it until answered
    for n in 0..3 {
        let a = h
            .login(&h.login_body(&format!("user{n}"), "x", None), ip)
            .await;
        assert_eq!(a.json["type"], "email_check", "{}", a.text);
    }
    assert_eq!(h.counted(SCOPE_IP, h.state.tags.ip(ip)).await, 3);
    // a fourth is refused like a failed login
    let a = h.login(&h.login_body("user3", "x", None), ip).await;
    assert_failure(&a, 401);
    assert_eq!(h.results().await.last().unwrap(), "login:tfa-busy");
    // three for one ID from anywhere
    for n in 0..3 {
        let a = h
            .login(
                &h.login_body(KIM.0, KIM.1, None),
                &format!("10.0.23.{}", 10 + n),
            )
            .await;
        assert_eq!(a.json["type"], "email_check", "{}", a.text);
    }
    assert_failure(
        &h.login(&h.login_body(KIM.0, KIM.1, None), "10.0.23.20")
            .await,
        401,
    );
    // a dialog answered right takes its count back
    let a = h
        .login(&h.login_body(LEE.0, LEE.1, None), "10.0.23.30")
        .await;
    let secret = a.json["secret"].as_str().unwrap().to_owned();
    assert_eq!(h.counted(SCOPE_IP, h.state.tags.ip("10.0.23.30")).await, 1);
    let code = json!({
        "verificationCode": "123456", "tfaCode": "123456", "secret": secret, "username": LEE.0,
        "id": "", "uuid": "dXVpZA==", "autoLogin": true, "type": "email_code",
    });
    let a = h.login(&code, "10.0.23.30").await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(h.attempts(SCOPE_IP, h.state.tags.ip("10.0.23.30")).await, 0);
}

#[tokio::test]
async fn a_seal_out_of_time_gets_the_clock_answer() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_IP", "3/600".into())],
        ..Default::default()
    })
    .await;
    // the seal is checked before the ID: listed or not, the same answer but the server's time
    let ip = "10.0.31.1";
    let mut answers = Vec::new();
    for user in [HONG.0, "2026999999"] {
        let body = h.login_body_at(user, "x", None, now() - 700, &b64(&random_bytes::<16>()));
        let a = h.login(&body, ip).await;
        assert_clock(&a);
        answers.push(a);
    }
    assert_eq!(answers[0].json["error"], answers[1].json["error"]);
    assert_ne!(answers[0].json["error"], FAILURE_TEXT);
    assert_eq!(h.sso.token_calls(), 0);
    // it counts against the address like any seal failure, not against an ID
    assert_eq!(h.counted(SCOPE_IP, h.state.tags.ip(ip)).await, 2);
    assert_eq!(h.attempts(SCOPE_ID, h.account_tag(HONG.0)).await, 0);
    // other seal failures keep the failure text
    let mut body = h.login_body(HONG.0, HONG.1, None);
    body["password"] = json!("plain");
    assert_failure(&h.login(&body, ip).await, 400);
    // past the address limit it is the limit's answer
    let body = h.login_body_at(
        HONG.0,
        HONG.1,
        None,
        now() + 700,
        &b64(&random_bytes::<16>()),
    );
    assert_failure(&h.login(&body, ip).await, 429);
    // a clock right again logs in (from elsewhere)
    assert_eq!(
        h.login(&h.login_body(HONG.0, HONG.1, None), "10.0.31.2")
            .await
            .status,
        200
    );
}

#[tokio::test]
async fn while_sso_calls_pause_every_id_gets_the_same_503() {
    let h = Harness::with(Options {
        allow: vec![HONG.0, KIM.0, "sso-500", "sso-invalid-client"],
        vars: vec![
            ("HJV_API_LIMIT_ID", "100/600".into()),
            ("HJV_API_LIMIT_IP", "100/600".into()),
            ("HJV_API_FAILURE_DELAY_MS", "200".into()),
            ("HJV_API_FAILURE_JITTER_MS", "20".into()),
        ],
        ..Default::default()
    })
    .await;
    // five SSO errors in a row pause the calls
    for _ in 0..5 {
        assert_failure(
            &h.login(&h.login_body("sso-500", "pw", None), "10.0.32.1")
                .await,
            401,
        );
    }
    assert!(h.state.sso_breaker.paused());
    let calls = h.sso.token_calls();
    // an ID on the list with its right password and one off the list: the same 503, before the
    // allowlist is looked at, and no SSO call
    let listed = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.32.2")
        .await;
    let unlisted = h
        .login(&h.login_body("2026999999", "x", None), "10.0.32.3")
        .await;
    assert_sso_down(&listed);
    assert_sso_down(&unlisted);
    assert_eq!(listed.text, unlisted.text);
    for a in [&listed, &unlisted] {
        assert!(
            a.elapsed >= std::time::Duration::from_millis(150),
            "{:?}",
            a.elapsed
        );
    }
    assert_eq!(h.sso.token_calls(), calls);
    // no password was tried: nothing counts, for the IDs or the addresses
    for (id, ip) in [(HONG.0, "10.0.32.2"), ("2026999999", "10.0.32.3")] {
        assert_eq!(h.attempts(SCOPE_ID, h.account_tag(id)).await, 0, "{id}");
        assert_eq!(h.attempts(SCOPE_IP, h.state.tags.ip(ip)).await, 0, "{ip}");
    }
    let results = h.results().await;
    assert_eq!(
        &results[results.len() - 2..],
        ["login:sso-paused", "login:sso-paused"]
    );
    // refused client credentials (invalid_client) pause the calls at once, as a refused address
    h.state.sso_breaker.reset();
    assert_failure(
        &h.login(&h.login_body("sso-invalid-client", "pw", None), "10.0.32.4")
            .await,
        401,
    );
    assert_eq!(h.results().await.last().unwrap(), "login:sso-client");
    assert_sso_down(
        &h.login(&h.login_body(KIM.0, KIM.1, None), "10.0.32.5")
            .await,
    );
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(report.contains("SSO calls: paused (client:"), "{report}");
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains("refuses this server's client credentials"),
        "{logs}"
    );
    // when the pause is over, logins work again
    h.state.sso_breaker.reset();
    assert_eq!(
        h.login(&h.login_body(KIM.0, KIM.1, None), "10.0.32.5")
            .await
            .status,
        200
    );
}
