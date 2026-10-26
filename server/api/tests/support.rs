// SPDX-License-Identifier: AGPL-3.0-only

//! Support codes (support.rs, guest.rs, docs/access-tickets.md 3-7) end to end: a staff member's
//! app makes a code, a PC trades it once within ten minutes for a two-hour `guest`/`pc` ticket;
//! wrong, expired, used and blocked codes get one 401 after the failure delay; one of two trades
//! at once; the access password's limits; the caps; who may not make one; nothing of a code in
//! the journal or the records. TV-W6 (1.9.0): the support link (its page, three downloads, the
//! link dying with its code), the waiting room (the hold, `rev`, the maker key's codes only, the
//! limits) and the support grant (its bytes against tests/data/support-grant-vectors.json, only
//! for a traded code and its PC, no name).

mod common;

use common::*;
use hjv_api::{
    devsig, grant, guest,
    limits::SCOPE_UPDATE_FAIL_IP,
    seal, support, ticket,
    trust::ListKey,
    util::{b64, b64_decode, b64url_decode, datetime, hex, now, random_bytes, sha256, DAY, HOUR},
};
use hyper::Method;
use serde_json::{json, Value};
use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

fn nonce() -> String {
    b64(&random_bytes::<16>())
}

/// `POST /api/hjv/support-code` as the app sends it.
async fn make(h: &Harness, token: &str, body: Value) -> Answer {
    h.post(support::PATH, token, Some(body)).await
}

/// A code made by the account of `token`, as shown.
async fn made(h: &Harness, token: &str) -> String {
    let a = make(h, token, json!({"use": "pc"})).await;
    assert_eq!(a.status, 200, "{}", a.text);
    a.json["code"].as_str().unwrap().to_owned()
}

fn sealed_hash(field: &str) -> [u8; 32] {
    sha256(&b64url_decode(field.rsplit('.').next().unwrap()).unwrap())
}

/// A PC's request with `field` (a sealed code) in `member` ("code" as the app sends it).
fn signed_body(pc: &TestDevice, member: &str, field: &str) -> Value {
    let pk = b64(&pc.public_key());
    let sig = devsig::sign(
        &pc.seed,
        &guest::pc_message(&pc.id, &pk, &sealed_hash(field)),
    );
    let mut body = json!({"use": "pc", "id": pc.id, "pk": pk, "sig": sig});
    body[member] = json!(field);
    body
}

/// A PC's trade of what was typed, sealed now as the app seals it.
fn code_body(h: &Harness, typed: &str, pc: &TestDevice) -> Value {
    let field = seal::seal_code(&h.seal_pk, typed, now(), &nonce(), &pc.id);
    signed_body(pc, "code", &field)
}

async fn trade(h: &Harness, body: &Value, ip: &str) -> Answer {
    h.request(
        Method::POST,
        guest::PATH,
        &[("content-type", "application/json"), ("x-real-ip", ip)],
        Some(body.to_string().into_bytes()),
    )
    .await
}

fn denied() -> Value {
    json!({"error": support::TEXT_DENIED, "hjv": "denied"})
}

/// The `code` records: result, the account's tag, the address's tag.
async fn code_records(h: &Harness) -> Vec<(String, String, String)> {
    h.state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT result, COALESCE(account_tag, ''), COALESCE(ip, '') FROM events
                 WHERE kind = 'code' ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
        .await
        .unwrap()
}

/// The payload of a ticket object's ticket, once the harness's ticket key's signature holds.
fn payload_of(object: &Value) -> Value {
    // a code's answer has the support grant besides (TV-W6)
    let members = object.as_object().unwrap();
    assert_eq!(
        members.keys().filter(|k| *k != "staff").count(),
        4,
        "{object}"
    );
    let parts: Vec<&str> = object["ticket"].as_str().unwrap().split('.').collect();
    assert_eq!((parts.len(), parts[0], parts[1]), (4, "t1", "1"));
    let payload = b64url_decode(parts[2]).unwrap();
    let sig = b64url_decode(parts[3]).unwrap();
    assert!(devsig::verify(
        &devsig::public_key(&TICKET_SEED),
        &ticket::message(&payload),
        &b64(&sig)
    ));
    let p: Value = serde_json::from_slice(&payload).unwrap();
    assert_eq!((&object["use"], &object["exp"]), (&p["use"], &p["exp"]));
    assert!((object["server_time"].as_i64().unwrap() - now()).abs() <= 5);
    p
}

/// A staff member's phone makes a code; a PC that is not logged in trades it, as typed, for a
/// `guest`/`pc` ticket of its own ID and key that lives two hours and names no one; no access
/// password's file is needed. The records keep the maker's tag and the address's tag.
#[tokio::test]
async fn a_staff_member_makes_a_code_and_a_pc_trades_it() {
    let h = Harness::start().await;
    assert!(h.state.config.guest_file.is_none());
    let token = h.login_ok(HONG, None).await;
    let a = make(&h, &token, json!({"use": "pc"})).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json.as_object().unwrap().len(), 5, "{}", a.text);
    assert_eq!(
        (&a.json["use"], &a.json["expires_in"]),
        (&json!("pc"), &json!(600))
    );
    let code = a.json["code"].as_str().unwrap();
    let symbols = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    assert_eq!((code.len(), &code[4..5]), (9, "-"), "{code}");
    assert!(
        code.chars()
            .filter(|c| *c != '-')
            .all(|c| symbols.contains(c)),
        "{code}"
    );
    // typed as a person may: lower case, without the dash, O for 0, I or L for 1
    let typed = code
        .replace('-', " ")
        .to_lowercase()
        .replace('0', "o")
        .replace('1', "l");
    let pc = TestDevice::windows("300000081");
    h.hbbs.register(&pc, Some(1));
    let a = trade(&h, &code_body(&h, &typed, &pc), "10.8.0.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let p = payload_of(&a.json);
    assert_eq!(
        (&p["who"], &p["use"], &p["sub"], &p["id"], &p["pk"], &p["to"]),
        (
            &json!("guest"),
            &json!("pc"),
            &json!(""),
            &json!(pc.id),
            &json!(b64(&pc.public_key())),
            &json!([])
        )
    );
    assert_eq!(
        p["exp"].as_i64().unwrap() - p["iat"].as_i64().unwrap(),
        2 * HOUR
    );
    assert_eq!(support::TICKET_SECS, 2 * HOUR);
    // a PC hbbs does not know yet trades one too
    let fresh = TestDevice::windows("Office-PC_8");
    let code = made(&h, &token).await;
    let a = trade(&h, &code_body(&h, &code, &fresh), "10.8.0.2").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let hong = h.account_tag(HONG.0);
    assert_eq!(
        code_records(&h).await,
        vec![
            ("made".to_owned(), hong.clone(), String::new()),
            (
                "ok-pc".to_owned(),
                hong.clone(),
                h.state.tags.ip("10.8.0.1")
            ),
            ("made".to_owned(), hong.clone(), String::new()),
            (
                "ok-pc".to_owned(),
                hong.clone(),
                h.state.tags.ip("10.8.0.2")
            ),
        ]
    );
    // no record of the access password's
    assert!(!h.results().await.iter().any(|r| r.starts_with("guest:")));
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(status.contains("code pc 2"), "{status}");
    assert!(
        status.contains("support codes: 0 open, 2 held in memory"),
        "{status}"
    );
    assert!(
        status.contains("support codes, the last day's records: made 2, ok-pc 2"),
        "{status}"
    );
    // the very same request to make one again: the first answer, the same code
    let body = br#"{"use":"pc"}"#.to_vec();
    let proof = h.proof_for("POST", support::PATH, &body, &token).unwrap();
    let auth = format!("Bearer {token}");
    let headers = [
        ("authorization", auth.as_str()),
        ("content-type", "application/json"),
        ("x-hjv-proof", proof.as_str()),
    ];
    let first = h
        .request(Method::POST, support::PATH, &headers, Some(body.clone()))
        .await;
    let again = h
        .request(Method::POST, support::PATH, &headers, Some(body))
        .await;
    assert_eq!(first.status, 200, "{}", first.text);
    assert_eq!(again.json, first.json);
    assert_eq!(h.state.support.counts(now()).0, 1);
}

/// A wrong code, one of another form, an expired one, a used one and one whose maker is blocked
/// get the very same 401, no sooner than the failure delay; each is the address's failure, and
/// the records tell them apart, with the maker's tag once the code was found.
#[tokio::test]
async fn wrong_expired_used_and_blocked_get_the_same_answer() {
    let h = Harness::with(Options {
        vars: vec![
            ("HJV_API_FAILURE_DELAY_MS", "300".into()),
            ("HJV_API_FAILURE_JITTER_MS", "50".into()),
        ],
        ..Default::default()
    })
    .await;
    let expect_denied = |a: &Answer| {
        assert_eq!((a.status, &a.json), (401, &denied()), "{}", a.text);
        assert!(a.elapsed.as_millis() >= 250, "{:?}", a.elapsed);
    };
    let pc = TestDevice::windows("300000082");
    let hong = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    // used: once, then the same answer as a wrong one
    let code = made(&h, &hong).await;
    assert_eq!(
        trade(&h, &code_body(&h, &code, &pc), "10.8.1.1")
            .await
            .status,
        200
    );
    expect_denied(&trade(&h, &code_body(&h, &code, &pc), "10.8.1.2").await);
    // wrong, and not of the form
    expect_denied(&trade(&h, &code_body(&h, "ZZZZ-ZZZZ", &pc), "10.8.1.3").await);
    expect_denied(&trade(&h, &code_body(&h, "K7M2-Q9X", &pc), "10.8.1.4").await);
    // the maker's account blocked: its code is over
    let blocked = made(&h, &kim).await;
    h.admin(
        &["block", &format!("tag:{}", h.account_tag(KIM.0))],
        false,
        None,
    )
    .await
    .unwrap();
    expect_denied(&trade(&h, &code_body(&h, &blocked, &pc), "10.8.1.5").await);
    // past its ten minutes
    let late = made(&h, &hong).await;
    h.state.support.age_by(support::CODE_SECS);
    expect_denied(&trade(&h, &code_body(&h, &late, &pc), "10.8.1.6").await);
    let (hong_tag, kim_tag) = (h.account_tag(HONG.0), h.account_tag(KIM.0));
    let records: Vec<(String, String)> = code_records(&h)
        .await
        .into_iter()
        .filter(|(result, _, _)| result != "made")
        .map(|(result, account, _)| (result, account))
        .collect();
    assert_eq!(
        records,
        vec![
            ("ok-pc".to_owned(), hong_tag.clone()),
            ("used".to_owned(), hong_tag.clone()),
            ("wrong".to_owned(), String::new()),
            ("wrong".to_owned(), String::new()),
            ("blocked".to_owned(), kim_tag),
            ("expired".to_owned(), hong_tag),
        ]
    );
    for ip in ["10.8.1.2", "10.8.1.3", "10.8.1.4", "10.8.1.5", "10.8.1.6"] {
        assert_eq!(h.counted("guest-ip", h.state.tags.ip(ip)).await, 1, "{ip}");
    }
    assert_eq!(h.counted("guest-ip", h.state.tags.ip("10.8.1.1")).await, 0);
}

/// A code ends for good with its maker's login: blocked, it stays over once the block is lifted
/// (the block ended the key that made it); `user revoke-all`, a logout and the account's own
/// "로그아웃시키기" end it too, and an ended code holds no place of the five.
#[tokio::test]
async fn a_code_ends_with_its_makers_login() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("300000091");
    let expect_denied =
        |a: &Answer| assert_eq!((a.status, &a.json), (401, &denied()), "{}", a.text);
    // blocked, then unblocked within the code's ten minutes: still over
    let kim = h.login_ok(KIM, None).await;
    let code = made(&h, &kim).await;
    let kim_tag = format!("tag:{}", h.account_tag(KIM.0));
    h.admin(&["block", &kim_tag], false, None).await.unwrap();
    expect_denied(&trade(&h, &code_body(&h, &code, &pc), "10.8.9.1").await);
    h.admin(&["unblock", &kim_tag], false, None).await.unwrap();
    expect_denied(&trade(&h, &code_body(&h, &code, &pc), "10.8.9.2").await);
    // blocked and unblocked with no try between: over all the same
    let kim = h.login_ok(KIM, None).await;
    let code = made(&h, &kim).await;
    h.admin(&["block", &kim_tag], false, None).await.unwrap();
    h.admin(&["unblock", &kim_tag], false, None).await.unwrap();
    expect_denied(&trade(&h, &code_body(&h, &code, &pc), "10.8.9.3").await);
    // the operator's revoke-all of the account
    let kim = h.login_ok(KIM, None).await;
    let code = made(&h, &kim).await;
    h.admin(&["user", "revoke-all", &kim_tag], false, None)
        .await
        .unwrap();
    expect_denied(&trade(&h, &code_body(&h, &code, &pc), "10.8.9.4").await);
    // the maker's logout
    let hong = h.login_ok(HONG, None).await;
    let code = made(&h, &hong).await;
    let out = h.post("/api/logout", &hong, Some(json!({}))).await;
    assert_eq!(out.status, 200, "{}", out.text);
    expect_denied(&trade(&h, &code_body(&h, &code, &pc), "10.8.9.5").await);
    // logged out by the account's other device; the codes of that device's key end, not those of
    // the device that is still logged in
    let phone = h.login_ok(HONG, None).await;
    let other = h.login_ok(HONG, None).await;
    let theirs = made(&h, &other).await;
    let mine = made(&h, &phone).await;
    let pk = devsig::public_key(&h.key_of(&other).unwrap()).to_vec();
    let other_key = h
        .state
        .db
        .call(move |c| {
            c.query_row("SELECT ckid FROM ctl_keys WHERE pk = ?1", [pk], |r| {
                r.get::<_, i64>(0)
            })
        })
        .await
        .unwrap();
    let a = h
        .delete(&format!("/api/hjv/keys/{other_key}"), &phone)
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    expect_denied(&trade(&h, &code_body(&h, &theirs, &pc), "10.8.9.6").await);
    let a = trade(&h, &code_body(&h, &mine, &pc), "10.8.9.7").await;
    assert_eq!(a.status, 200, "{}", a.text);
    // five codes, a logout, a new login: the five ended hold no place
    let first = h.login_ok(HONG, None).await;
    for _ in 0..support::OPEN_PER_ACCOUNT {
        made(&h, &first).await;
    }
    assert_eq!(make(&h, &first, json!({"use": "pc"})).await.status, 429);
    h.post("/api/logout", &first, Some(json!({}))).await;
    let again = h.login_ok(HONG, None).await;
    made(&h, &again).await;
    let results: Vec<String> = code_records(&h)
        .await
        .into_iter()
        .map(|(r, _, _)| r)
        .filter(|r| r != "made")
        .collect();
    assert_eq!(
        results,
        ["blocked", "revoked", "revoked", "revoked", "revoked", "revoked", "ok-pc"]
    );
}

/// The app's resend of a trade's very body gets the first answer, the ticket, even when the
/// address's failures filled up since (the code was used by the first); a resend of a body that
/// the address's limit refused is looked at afresh.
#[tokio::test]
async fn a_resend_gets_the_first_answer_past_the_limit() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_GUEST_IP", "2/600".into())],
        ..Default::default()
    })
    .await;
    let token = h.login_ok(HONG, None).await;
    let pc = TestDevice::windows("300000092");
    let ip = "10.8.10.1";
    let wrong = |h: &Harness| code_body(h, "ZZZZ-ZZZZ", &pc);
    assert_eq!(trade(&h, &wrong(&h), ip).await.status, 401);
    let body = code_body(&h, &made(&h, &token).await, &pc);
    let first = trade(&h, &body, ip).await;
    assert_eq!(first.status, 200, "{}", first.text);
    assert_eq!(trade(&h, &wrong(&h), ip).await.status, 401);
    // the limit is full now
    assert_eq!(trade(&h, &wrong(&h), ip).await.status, 429);
    let again = trade(&h, &body, ip).await;
    assert_eq!(
        (again.status, &again.json),
        (200, &first.json),
        "{}",
        again.text
    );
    // a body refused by the limit, resent once the address is unlocked: looked at afresh
    let later = code_body(&h, &made(&h, &token).await, &pc);
    let refused = trade(&h, &later, ip).await;
    assert_eq!(refused.status, 429, "{}", refused.text);
    let tag = format!("tag:{}", h.state.tags.ip(ip));
    h.admin(&["unlock", tag.as_str()], false, None)
        .await
        .unwrap();
    let a = trade(&h, &later, ip).await;
    assert_eq!(a.status, 200, "{}", a.text);
    let results: Vec<String> = code_records(&h)
        .await
        .into_iter()
        .map(|(r, _, _)| r)
        .filter(|r| r != "made")
        .collect();
    assert_eq!(results, ["wrong", "ok-pc", "wrong", "limited-ip", "ok-pc"]);
}

/// Of two PCs that trade one code at once, one gets the ticket and the other the 401 of a used
/// code.
#[tokio::test]
async fn one_of_two_trades_at_once_gets_the_ticket() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    for round in 0..3 {
        let code = made(&h, &token).await;
        let (one, two) = (
            TestDevice::windows(&format!("30000009{round}")),
            TestDevice::windows(&format!("30000019{round}")),
        );
        let (body_one, body_two) = (code_body(&h, &code, &one), code_body(&h, &code, &two));
        let (a, b) = tokio::join!(
            trade(&h, &body_one, "10.8.2.1"),
            trade(&h, &body_two, "10.8.2.2"),
        );
        let mut statuses = [a.status, b.status];
        statuses.sort_unstable();
        assert_eq!(statuses, [200, 401], "{} / {}", a.text, b.text);
        let refused = if a.status == 401 { &a } else { &b };
        assert_eq!(refused.json, denied());
    }
    let results: Vec<String> = code_records(&h)
        .await
        .into_iter()
        .map(|(result, _, _)| result)
        .filter(|r| r != "made")
        .collect();
    assert_eq!(results.iter().filter(|r| *r == "ok-pc").count(), 3);
    assert_eq!(results.iter().filter(|r| *r == "used").count(), 3);
}

/// A code's tries count in the access password's limits: the address's failures before the
/// lookup (a code refused there is still open), and `admin unlock` clears them.
#[tokio::test]
async fn the_access_passwords_limits() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIMIT_GUEST_IP", "2/600".into())],
        ..Default::default()
    })
    .await;
    let token = h.login_ok(HONG, None).await;
    let code = made(&h, &token).await;
    let pc = TestDevice::windows("300000083");
    let ip = "10.8.3.1";
    for _ in 0..2 {
        let a = trade(&h, &code_body(&h, "ZZZZ-ZZZZ", &pc), ip).await;
        assert_eq!(a.status, 401, "{}", a.text);
    }
    assert_eq!(h.counted("guest-ip", h.state.tags.ip(ip)).await, 2);
    let a = trade(&h, &code_body(&h, &code, &pc), ip).await;
    assert_eq!(a.status, 429, "{}", a.text);
    assert_eq!(a.json["hjv"], "rate_limited");
    assert!(a.json["retry_after"].as_u64().unwrap() > 500, "{}", a.text);
    // the code was not used: another address trades it
    assert_eq!(h.state.support.counts(now()).0, 1);
    let tag = format!("tag:{}", h.state.tags.ip(ip));
    let unlocked = h
        .admin(&["unlock", tag.as_str()], false, None)
        .await
        .unwrap();
    assert!(
        unlocked.ends_with(", and 2 tries of the access password and support codes"),
        "{unlocked}"
    );
    let a = trade(&h, &code_body(&h, &code, &pc), ip).await;
    assert_eq!(a.status, 200, "{}", a.text);
    let results: Vec<String> = code_records(&h)
        .await
        .into_iter()
        .map(|(result, _, _)| result)
        .collect();
    assert_eq!(results, ["made", "wrong", "wrong", "limited-ip", "ok-pc"]);
}

/// Five open codes per account: the sixth gets 429 `too_many_codes` with the seconds until the
/// first ends, and the journal hears of it once, by the account's tag; a used code makes room.
#[tokio::test]
async fn five_open_codes_per_account() {
    let h = Harness::start().await;
    // an account no other test of this file makes codes with: the journal is the binary's
    let token = h.login_ok(LEE, None).await;
    let first = made(&h, &token).await;
    for _ in 1..support::OPEN_PER_ACCOUNT {
        made(&h, &token).await;
    }
    for _ in 0..2 {
        let a = make(&h, &token, json!({"use": "pc"})).await;
        assert_eq!(a.status, 429, "{}", a.text);
        assert_eq!(
            (&a.json["error"], &a.json["hjv"]),
            (&json!(support::TEXT_TOO_MANY), &json!("too_many_codes"))
        );
        let wait = a.json["retry_after"].as_u64().unwrap();
        assert!((590..=600).contains(&wait), "{}", a.text);
    }
    let tag = h.account_tag(LEE.0);
    let lines: Vec<String> = hjv_api::log::captured()
        .into_iter()
        .filter(|l| l.contains("open codes") && l.contains(&tag))
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    // another account makes its own
    let kim = h.login_ok(KIM, None).await;
    made(&h, &kim).await;
    // one used: room for one more
    let pc = TestDevice::windows("300000084");
    assert_eq!(
        trade(&h, &code_body(&h, &first, &pc), "10.8.4.1")
            .await
            .status,
        200
    );
    made(&h, &token).await;
    assert_eq!(make(&h, &token, json!({"use": "pc"})).await.status, 429);
}

/// Who may not make a code: the App Review account (403 `no_code`), an app without a login or
/// with a ticket in the place of a token (401); and what is not a request to make one.
#[tokio::test]
async fn who_may_not_make_one() {
    const HASH: &str =
        "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";
    let dir = temp_dir("support-review");
    let file = dir.join("review.conf");
    std::fs::write(
        &file,
        format!(
            "ID=APPREVIEW-TEST\nHASH={HASH}\nUNTIL={}\nPC=100200300\n",
            datetime(now() + 2 * DAY)
        ),
    )
    .unwrap();
    let h = Harness::with(Options {
        vars: vec![("HJV_API_REVIEW_FILE", file.display().to_string())],
        ..Default::default()
    })
    .await;
    let a = h
        .login(
            &h.login_body("APPREVIEW-TEST", "correct horse battery staple", None),
            "10.8.5.1",
        )
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    let review = a.token();
    let a = make(&h, &review, json!({"use": "pc"})).await;
    assert_eq!(
        (a.status, &a.json),
        (
            403,
            &json!({"error": support::TEXT_NO_CODE, "hjv": "no_code"})
        ),
        "{}",
        a.text
    );
    // no token, and a ticket in the place of one: a PC that came in with a code or the password
    let body = br#"{"use":"pc"}"#.to_vec();
    let a = h
        .request(
            Method::POST,
            support::PATH,
            &[("content-type", "application/json")],
            Some(body.clone()),
        )
        .await;
    assert_eq!(a.status, 401, "{}", a.text);
    let staff = h.login_ok(HONG, None).await;
    let code = made(&h, &staff).await;
    let pc = TestDevice::windows("300000085");
    let traded = trade(&h, &code_body(&h, &code, &pc), "10.8.5.2").await;
    let guest_ticket = traded.json["ticket"].as_str().unwrap().to_owned();
    let auth = format!("Bearer {guest_ticket}");
    let a = h
        .request(
            Method::POST,
            support::PATH,
            &[
                ("authorization", auth.as_str()),
                ("content-type", "application/json"),
            ],
            Some(body),
        )
        .await;
    assert_eq!(a.status, 401, "{}", a.text);
    // not a request to make one: 400 (a controller's code is not taken yet); GET 405
    for bad in [
        json!({"use": "ctl"}),
        json!({"use": "ctl", "to": "123456789"}),
        json!({}),
        json!({"use": "pc", "x": "y"}),
    ] {
        let a = make(&h, &staff, bad.clone()).await;
        assert_eq!(a.status, 400, "{bad}: {}", a.text);
        assert_eq!(a.json["hjv"], "bad_request");
    }
    let a = h.get(support::PATH, &staff).await;
    assert_eq!(a.status, 405, "{}", a.text);
    assert_eq!(a.headers["allow"], "POST");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Whether the app shows S1: a staff login's answer and its currentUser carry
/// `"hjv":{"support_code":true}`, on a phone and on a PC added to the account; the App Review
/// account's never do, nor does the ticket a guest's PC trades a code for, and a guest has no
/// currentUser.
#[tokio::test]
async fn the_apps_learn_who_may_make_codes() {
    const HASH: &str =
        "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";
    let dir = temp_dir("support-flag");
    let file = dir.join("review.conf");
    std::fs::write(
        &file,
        format!(
            "ID=APPREVIEW-TEST\nHASH={HASH}\nUNTIL={}\nPC=100200300\n",
            datetime(now() + 2 * DAY)
        ),
    )
    .unwrap();
    let h = Harness::with(Options {
        vars: vec![("HJV_API_REVIEW_FILE", file.display().to_string())],
        ..Default::default()
    })
    .await;
    let flag = json!({"support_code": true});
    // staff: a phone, and an installed PC the login adds to the account
    let pc = TestDevice::windows("300000089");
    h.hbbs.register(&pc, Some(1));
    for (device, ip) in [(None, "10.8.9.1"), (Some(&pc), "10.8.9.2")] {
        let a = h.login(&h.login_body(HONG.0, HONG.1, device), ip).await;
        assert_eq!(a.status, 200, "{}", a.text);
        assert_eq!(a.json["hjv"]["support_code"], true, "{}", a.text);
        assert!(a.json["user"].get("hjv").is_none(), "{}", a.text);
        let token = a.token();
        let me = h.post("/api/currentUser", &token, Some(json!({}))).await;
        assert_eq!((me.status, &me.json["hjv"]), (200, &flag), "{}", me.text);
        assert_eq!(make(&h, &token, json!({"use": "pc"})).await.status, 200);
    }
    // the App Review account: neither, and it cannot make one
    let a = h
        .login(
            &h.login_body("APPREVIEW-TEST", "correct horse battery staple", None),
            "10.8.9.3",
        )
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(a.json["hjv"].get("support_code").is_none(), "{}", a.text);
    let review = a.token();
    let me = h.post("/api/currentUser", &review, Some(json!({}))).await;
    assert_eq!(me.status, 200, "{}", me.text);
    assert!(me.json.get("hjv").is_none(), "{}", me.text);
    assert_eq!(make(&h, &review, json!({"use": "pc"})).await.status, 403);
    // a guest: a PC that came in with a code has no flag and no currentUser
    let staff = h.login_ok(HONG, None).await;
    let code = made(&h, &staff).await;
    let guest_pc = TestDevice::windows("300000090");
    let traded = trade(&h, &code_body(&h, &code, &guest_pc), "10.8.9.4").await;
    assert_eq!(traded.status, 200, "{}", traded.text);
    assert!(traded.json.get("support_code").is_none(), "{}", traded.text);
    assert!(traded.json.get("hjv").is_none(), "{}", traded.text);
    let ticket = traded.json["ticket"].as_str().unwrap().to_owned();
    let auth = format!("Bearer {ticket}");
    let a = h
        .request(
            Method::POST,
            "/api/currentUser",
            &[
                ("authorization", auth.as_str()),
                ("content-type", "application/json"),
            ],
            Some(b"{}".to_vec()),
        )
        .await;
    assert_eq!(a.status, 401, "{}", a.text);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A controller's code is not taken: 400 before anything counts or opens, and the code still
/// works. Each seal is only its own field's: the access password's seal in `code`, a code's in
/// `password` and at the login are refused.
#[tokio::test]
async fn a_controllers_code_and_the_seals() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let code = made(&h, &token).await;
    let bad = json!({"error": "요청을 이해하지 못했습니다.", "hjv": "bad_request"});
    let ip = "10.8.6.1";
    let ctl_field = seal::seal_code(&h.seal_pk, &code, now(), &nonce(), "123456789");
    let a = trade(
        &h,
        &json!({"use": "ctl", "to": "123456789", "code": ctl_field}),
        ip,
    )
    .await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    assert_eq!(h.counted("guest-ip", h.state.tags.ip(ip)).await, 0);
    assert!(code_records(&h).await.iter().all(|(r, _, _)| r == "made"));
    // both fields
    let pc = TestDevice::windows("300000086");
    let mut both = code_body(&h, &code, &pc);
    both["password"] = both["code"].clone();
    let a = trade(&h, &both, ip).await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    // the access password's seal in code, a code's seal in password: seal-purpose
    let guest_field = seal::seal_guest(&h.seal_pk, &code, now(), &nonce(), "pc", &pc.id);
    let a = trade(&h, &signed_body(&pc, "code", &guest_field), ip).await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    let code_field = seal::seal_code(&h.seal_pk, &code, now(), &nonce(), &pc.id);
    let a = trade(&h, &signed_body(&pc, "password", &code_field), "10.8.6.2").await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    // and at the login
    let mut login = h.login_body(KIM.0, KIM.1, None);
    login["password"] = json!(seal::seal_code(
        &h.seal_pk,
        KIM.1,
        now(),
        &nonce(),
        "123456789"
    ));
    login["id"] = json!("123456789");
    assert_eq!(h.login(&login, "10.8.6.3").await.status, 400);
    let results: Vec<String> = code_records(&h)
        .await
        .into_iter()
        .map(|(r, _, _)| r)
        .collect();
    assert_eq!(results, ["made", "seal-purpose"]);
    assert!(h.results().await.contains(&"guest:seal-purpose".to_owned()));
    // the code is still open, and works
    let a = trade(&h, &code_body(&h, &code, &pc), "10.8.6.4").await;
    assert_eq!(a.status, 200, "{}", a.text);
}

/// A PC hbbs knows by another key: 403 `wrong_pc` with the code's text, and the code stays open;
/// without hbbs's answer 503; without a ticket key neither request signs or makes anything.
#[tokio::test]
async fn the_pcs_key_and_the_ticket_key() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let code = made(&h, &token).await;
    let pc = TestDevice::windows("300000087");
    h.hbbs
        .register_key(&pc.id, TestDevice::windows("300000088").public_key());
    let a = trade(&h, &code_body(&h, &code, &pc), "10.8.7.1").await;
    assert_eq!(
        (a.status, &a.json),
        (
            403,
            &json!({"error": support::TEXT_WRONG_PC, "hjv": "wrong_pc"})
        ),
        "{}",
        a.text
    );
    assert_eq!(h.state.support.counts(now()).0, 1);
    assert_eq!(h.counted("guest-ip", h.state.tags.ip("10.8.7.1")).await, 0);
    let other = TestDevice::windows("300000089");
    h.hbbs.unpatched.store(true, Ordering::SeqCst);
    let a = trade(&h, &code_body(&h, &code, &other), "10.8.7.2").await;
    assert_eq!(a.status, 503, "{}", a.text);
    assert_eq!(
        a.json,
        json!({"error": support::TEXT_UNAVAILABLE, "hjv": "unavailable"})
    );
    assert_eq!(h.state.support.counts(now()).0, 1);
    let results: Vec<String> = code_records(&h)
        .await
        .into_iter()
        .map(|(r, _, _)| r)
        .collect();
    assert_eq!(results, ["made", "wrong-pc", "hbbs"]);

    let h = Harness::with(Options {
        no_ticket_key: true,
        ..Default::default()
    })
    .await;
    let token = h.login_ok(HONG, None).await;
    let a = make(&h, &token, json!({"use": "pc"})).await;
    assert_eq!(
        (a.status, &a.json),
        (
            503,
            &json!({"error": support::TEXT_CANNOT_MAKE, "hjv": "unavailable"})
        ),
        "{}",
        a.text
    );
    let a = trade(&h, &code_body(&h, "K7M2-Q9XD", &pc), "10.8.7.3").await;
    assert_eq!(a.status, 503, "{}", a.text);
    assert_eq!(a.json["hjv"], "unavailable");
    assert!(code_records(&h).await.is_empty());
}

/// No code, in any form, goes to the journal or the records; nor the ticket, the address or the
/// PC's ID; nor (TV-W6) a support link, a waiting number, the waiting room's answer or a grant.
#[tokio::test]
async fn nothing_of_a_code_is_logged_or_recorded() {
    // with a Windows package, so that one support-link download is journaled
    let files = Files::new();
    let h = Harness::with(Options {
        vars: files.vars(),
        ..Default::default()
    })
    .await;
    let token = h.login_ok(HONG, None).await;
    let (first, link, wait) = made_all(&h, &token).await;
    let codes = [first, made(&h, &token).await];
    // the link's page and file, and the waiting room
    for (path, status) in [
        (link.clone(), 307),
        (format!("{link}/"), 200),
        (format!("{link}/HJViewer-1.9.0.exe"), 200),
    ] {
        let a = h
            .request(Method::GET, &path, &[("x-real-ip", "10.8.8.2")], None)
            .await;
        assert_eq!(a.status, status, "{path}");
    }
    let waited = h.get(&wait_path(None), &token).await;
    assert_eq!(waited.status, 200, "{}", waited.text);
    let pc = TestDevice::windows("300000090");
    let a = trade(&h, &code_body(&h, &codes[0], &pc), "10.8.8.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let payload = a.json["ticket"]
        .as_str()
        .unwrap()
        .split('.')
        .nth(2)
        .unwrap()
        .to_owned();
    let grant = a.json["staff"].clone();
    let waited = h.get(&wait_path(None), &token).await;
    // two codes made in the same second come in either order
    assert!(
        waited.json["codes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["pc"] == json!(pc.id)),
        "{}",
        waited.text
    );
    // the used link, used, wrong, and the caps' journal line
    h.request(
        Method::GET,
        &format!("{link}/"),
        &[("x-real-ip", "10.8.8.2")],
        None,
    )
    .await;
    trade(&h, &code_body(&h, &codes[0], &pc), "10.8.8.1").await;
    trade(&h, &code_body(&h, "ZZZZ-ZZZZ", &pc), "10.8.8.1").await;
    for _ in 0..support::OPEN_PER_ACCOUNT {
        make(&h, &token, json!({"use": "pc"})).await;
    }
    let lines = hjv_api::log::captured();
    // the download's journal line is there, and is checked with the rest
    assert!(
        lines.iter().any(|l| l.contains("by a support link (acct=")),
        "{lines:?}"
    );
    let records: Vec<String> = h
        .state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT kind || ' ' || result || ' ' || COALESCE(account_tag, '') || ' '
                        || COALESCE(device_tag, '') || ' ' || COALESCE(ip, '')
                 FROM events",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap();
    let value = link.strip_prefix(support::LINK_PATH).unwrap().to_owned();
    let mut secrets = vec![
        payload,
        "10.8.8.1".to_owned(),
        "10.8.8.2".to_owned(),
        pc.id.clone(),
        b64(&pc.public_key()),
        value,
        wait,
        text_of(&grant, "g").to_owned(),
        text_of(&grant, "s").to_owned(),
    ];
    for code in &codes {
        secrets.push(code.clone());
        secrets.push(code.replace('-', ""));
    }
    for line in lines.iter().chain(records.iter()) {
        for secret in &secrets {
            assert!(!line.contains(secret.as_str()), "{line}");
        }
    }
}

// ---- TV-W6: the support grant, the support link, the waiting room ----

/// The shared vectors of the support grant, which the client renders from its
/// src/hjviewer_account/vectors.rs (`support_json()`) and checks there with libsodium.
const VECTORS: &str = include_str!("data/support-grant-vectors.json");
/// The client's vectors.rs SUPPORT_JSON_SHA256: the file is its rendering, byte for byte.
const VECTORS_SHA256: &str = "d5c2a45bcd30dfdb2d974ead325cd5b20a9b8fa1eef4d056abec0b658a21d184";

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn key32(text: &Value) -> [u8; 32] {
    b64_decode(text.as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap()
}

fn seed_of(key: &Value) -> [u8; 32] {
    unhex(key["seed_hex"].as_str().unwrap()).try_into().unwrap()
}

/// What hjv-api signs for the grant bytes `text` (another PC, another life): the same bytes.
fn grant_of(text: &str) -> (Value, String, [u8; 32], [u8; 32], String, i64, i64) {
    let g: Value = serde_json::from_str(text).unwrap();
    (
        g.clone(),
        g["pc_id"].as_str().unwrap().to_owned(),
        key32(&g["pc_pk"]),
        key32(&g["ck"]),
        g["ctl_id"].as_str().unwrap().to_owned(),
        g["iat"].as_i64().unwrap(),
        g["exp"].as_i64().unwrap(),
    )
}

/// The grant hjv-api makes of the vectors' inputs is the vectors' grant, byte for byte, and so is
/// its signature (Ed25519 signs deterministically); the refused ones are signed as the client
/// expects, and the altered bytes do not hold. The vector's list key is the one the harness signs
/// with.
#[test]
fn the_grant_is_the_vectors_byte_for_byte() {
    assert_eq!(hex(&sha256(VECTORS.as_bytes())), VECTORS_SHA256);
    let v: Value = serde_json::from_str(VECTORS).unwrap();
    let keys = &v["keys"];
    for name in ["list_1", "pc_device", "account", "other_account"] {
        assert_eq!(
            b64(&devsig::public_key(&seed_of(&keys[name]))),
            keys[name]["pk"].as_str().unwrap(),
            "{name}"
        );
    }
    assert_eq!(seed_of(&keys["list_1"]), LIST_SEED);
    assert_eq!(
        (v["context"].as_str().unwrap().as_bytes(), &v["max_life"]),
        (grant::CONTEXT, &json!(grant::MAX_LIFE))
    );
    let list = ListKey::new(1, &LIST_SEED);
    let sign = |text: &str| b64(&list.sign_with(grant::CONTEXT, text.as_bytes()).unwrap());
    let holds = |text: &str, s: &Value| {
        devsig::verify(
            &list.public_key(),
            &grant::message(text.as_bytes()),
            s.as_str().unwrap(),
        )
    };
    // the grant: from its inputs, the bytes, the signed bytes and the staff member
    let g = &v["grant"];
    let made = grant::Grant {
        pc_id: g["pc_id"].as_str().unwrap(),
        pc_pk: key32(&g["pc_pk"]),
        ck: key32(&g["ck"]),
        ctl_id: g["ctl_id"].as_str().unwrap(),
        iat: g["iat"].as_i64().unwrap(),
        exp: g["exp"].as_i64().unwrap(),
    };
    let bytes = grant::bytes(1, &made).unwrap();
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), g["bytes"]);
    assert_eq!(hex(&grant::message(&bytes)), g["signed_hex"]);
    assert_eq!(grant::staff(&list, &made).unwrap(), g["staff"]);
    assert!(holds(g["bytes"].as_str().unwrap(), &g["staff"]["s"]));
    assert_eq!(made.ck, key32(&keys["account"]["pk"]));
    assert_eq!(made.pc_pk, key32(&keys["pc_device"]["pk"]));
    // the trust list's purpose does not sign it
    assert!(!devsig::verify(
        &list.public_key(),
        &hjv_api::trust::message(&bytes),
        g["staff"]["s"].as_str().unwrap()
    ));
    // no name, ever
    let parsed: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["name"], "");
    assert_eq!(parsed.as_object().unwrap().len(), 10);
    // another PC and a life of 7201 s: what hjv-api would sign for those inputs
    for name in ["other_pc", "life_7201"] {
        let r = &v["refused"][name];
        let text = r["grant"].as_str().unwrap();
        let (_, pc_id, pc_pk, ck, ctl_id, iat, exp) = grant_of(text);
        let grant = grant::Grant {
            pc_id: &pc_id,
            pc_pk,
            ck,
            ctl_id: &ctl_id,
            iat,
            exp,
        };
        assert_eq!(grant::bytes(1, &grant).unwrap(), text.as_bytes(), "{name}");
        assert_eq!(grant::staff(&list, &grant).unwrap(), r["staff"], "{name}");
    }
    // a member the format does not have: signed with list key 1 all the same
    let r = &v["refused"]["unknown_member"];
    assert_eq!(sign(r["grant"].as_str().unwrap()), r["staff"]["s"]);
    assert_eq!(
        r["staff"]["g"].as_str().unwrap(),
        b64(r["grant"].as_str().unwrap().as_bytes())
    );
    // bytes changed after signing: the grant's signature does not hold for them
    let r = &v["refused"]["altered_bytes"];
    assert_eq!(r["staff"]["s"], g["staff"]["s"]);
    assert!(!holds(r["grant"].as_str().unwrap(), &r["staff"]["s"]));
    // the maker's proof to the PC, and another account key's valid proof (the PC's mark refuses
    // that one for its key)
    let proof = &v["conn_proof"];
    for p in [proof, &v["refused"]["other_key_proof"]] {
        assert!(devsig::verify(
            &key32(&p["ctl_pk"]),
            &unhex(p["transcript_hex"].as_str().unwrap()),
            p["signature"].as_str().unwrap()
        ));
    }
    assert_eq!(proof["ctl_pk"], g["ck"]);
    assert_ne!(v["refused"]["other_key_proof"]["ctl_pk"], g["ck"]);
}

/// The member `name` of an answer as text.
fn text_of<'a>(a: &'a Value, name: &str) -> &'a str {
    a[name]
        .as_str()
        .unwrap_or_else(|| panic!("no {name} in {a}"))
}

/// A code made by the account of `token`, with its link and waiting number.
async fn made_all(h: &Harness, token: &str) -> (String, String, String) {
    let a = make(h, token, json!({"use": "pc"})).await;
    assert_eq!(a.status, 200, "{}", a.text);
    (
        text_of(&a.json, "code").to_owned(),
        text_of(&a.json, "link").to_owned(),
        text_of(&a.json, "wait").to_owned(),
    )
}

fn is_value(text: &str) -> bool {
    text.len() == 22
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && b64url_decode(text).is_some_and(|b| b.len() == 16)
}

/// A trade of a code made by a staff member's phone carries the support grant: list key 1's
/// signature with its own purpose over the bytes in their order, naming that PC (the trade's ID
/// and key), the account key that made the code and the phone's ID, the ticket's times, and no
/// name. A resend gets the same; the used code, a wrong one, another PC get no grant.
#[tokio::test]
async fn a_traded_code_carries_the_grant_for_its_pc_only() {
    let h = Harness::start().await;
    let phone = TestDevice::phone("987650001");
    let token = h.login_ok(HONG, Some(&phone)).await;
    let ck = devsig::public_key(&h.key_of(&token).unwrap());
    let (code, link, wait) = made_all(&h, &token).await;
    let value = link.strip_prefix(support::LINK_PATH).unwrap();
    assert!(
        is_value(value) && is_value(&wait) && value != wait,
        "{link} {wait}"
    );
    let pc = TestDevice::windows("300000101");
    h.hbbs.register(&pc, Some(1));
    let body = code_body(&h, &code, &pc);
    let a = trade(&h, &body, "10.8.10.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let p = payload_of(&a.json);
    let staff = &a.json["staff"];
    assert_eq!(staff.as_object().unwrap().len(), 3, "{staff}");
    assert_eq!((&staff["kid"], text_of(staff, "s").len()), (&json!(1), 88));
    let g = b64_decode(text_of(staff, "g")).unwrap();
    let list = devsig::public_key(&LIST_SEED);
    assert!(devsig::verify(
        &list,
        &grant::message(&g),
        text_of(staff, "s")
    ));
    assert!(!devsig::verify(
        &list,
        &hjv_api::trust::message(&g),
        text_of(staff, "s")
    ));
    assert_eq!(
        String::from_utf8(g).unwrap(),
        format!(
            r#"{{"v":1,"typ":"hjv-support","kid":1,"pc_id":"{}","pc_pk":"{}","ck":"{}","ctl_id":"{}","name":"","iat":{},"exp":{}}}"#,
            pc.id,
            b64(&pc.public_key()),
            b64(&ck),
            phone.id,
            p["iat"],
            p["exp"]
        )
    );
    // the very same body again within the retry window: the first answer, the same grant
    let again = trade(&h, &body, "10.8.10.1").await;
    assert_eq!(
        (again.status, &again.json),
        (200, &a.json),
        "{}",
        again.text
    );
    // the code is used: another PC, and the same PC with a new seal, get the one 401, no grant
    let other = TestDevice::windows("300000102");
    for (device, ip) in [(&other, "10.8.10.2"), (&pc, "10.8.10.3")] {
        let b = trade(&h, &code_body(&h, &code, device), ip).await;
        assert_eq!((b.status, &b.json), (401, &denied()), "{}", b.text);
    }
    // a code made without a device ID names none
    let bare = h.login_ok(KIM, None).await;
    let code = made(&h, &bare).await;
    let a = trade(&h, &code_body(&h, &code, &other), "10.8.10.4").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let g: Value =
        serde_json::from_slice(&b64_decode(text_of(&a.json["staff"], "g")).unwrap()).unwrap();
    assert_eq!(
        (&g["pc_id"], &g["ctl_id"], &g["name"]),
        (&json!(other.id), &json!(""), &json!(""))
    );
    assert_eq!(
        g["ck"],
        json!(b64(&devsig::public_key(&h.key_of(&bare).unwrap())))
    );
}

/// The operator's files of a test: a policy with Windows 1.9.0, its package and two notices.
struct Files {
    dir: std::path::PathBuf,
    exe: Vec<u8>,
}

const EXE: &str = "HJViewer-1.9.0-windows-x86_64.exe";

impl Files {
    fn new() -> Files {
        let dir = temp_dir("support-link");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join("notices-1.9.0")).unwrap();
        // text, so that the harness reads the download as it reads every answer
        let exe: Vec<u8> = (0..70_001u32).map(|i| b'a' + (i * 7 % 26) as u8).collect();
        std::fs::write(pkg.join(EXE), &exe).unwrap();
        std::fs::write(pkg.join("notices-1.9.0/LICENSE.txt"), "AGPL-3.0").unwrap();
        std::fs::write(pkg.join("notices-1.9.0/SOURCE.txt"), "client-1.9.0").unwrap();
        let files = Files { dir, exe };
        files.policy(true);
        files
    }

    /// The policy, with the Windows package or without.
    fn policy(&self, windows: bool) {
        let entry = json!({"min": "1.9.0", "latest": "1.9.0", "source": "client-1.9.0",
                           "package": {"file": EXE, "size": self.exe.len(),
                                       "sha256": hex(&sha256(&self.exe)), "build": "1.9.0.1"}});
        let platforms = if windows {
            json!({ "windows": entry })
        } else {
            json!({ "android": {"min": "1.9.0", "latest": "1.9.0"} })
        };
        let text = json!({"v": 1, "epoch": 1, "seq": if windows { 5 } else { 6 },
                          "issued": "2026-10-20T03:00:00Z", "server": "rd.example.com",
                          "platforms": platforms});
        let envelope = json!({"v": 1, "k": 1, "p": text.to_string(), "s": "AAAA"});
        let new = self.dir.join("version-policy.json.new");
        std::fs::write(&new, format!("{envelope}\n")).unwrap();
        std::fs::rename(new, self.dir.join("version-policy.json")).unwrap();
    }

    fn vars(&self) -> Vec<(&'static str, String)> {
        vec![
            (
                "HJV_API_VERSION_POLICY",
                self.dir.join("version-policy.json").display().to_string(),
            ),
            (
                "HJV_API_PACKAGE_DIR",
                self.dir.join("pkg").display().to_string(),
            ),
        ]
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A browser's request of a support link's path.
async fn browse(h: &Harness, method: Method, path: &str, ip: &str) -> Answer {
    h.request(method, path, &[("x-real-ip", ip)], None).await
}

fn no_link() -> Value {
    json!({"error": "링크가 맞지 않거나 사용 기한이 지났습니다."})
}

/// The `update` records: result, account tag, address tag.
async fn update_records(h: &Harness) -> Vec<(String, String, String)> {
    h.state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT result, COALESCE(account_tag, ''), COALESCE(ip, '') FROM events
                 WHERE kind = 'update' ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
        .await
        .unwrap()
}

/// A support link while its code is open: 307 to the page; the page in Korean and English with
/// the code, HJViewer-1.9.0.exe and its facts, door 2's headers, no script; the package three
/// times (HEAD free) and then 404, the notice files uncounted, no installer's name; each download
/// an `update` record with the maker's tag; other values 404 without counting against the
/// address; without a Windows package SL8 and the code.
#[tokio::test]
async fn the_support_link_and_its_three_downloads() {
    let files = Files::new();
    let h = Harness::with(Options {
        vars: files.vars(),
        ..Default::default()
    })
    .await;
    let token = h.login_ok(HONG, None).await;
    let (code, link, _) = made_all(&h, &token).await;
    let ip = "10.8.11.1";
    let a = browse(&h, Method::GET, &link, ip).await;
    assert_eq!(a.status, 307, "{}", a.text);
    assert_eq!(a.headers["location"], format!("{link}/"));
    let page = format!("{link}/");
    let a = browse(&h, Method::GET, &page, ip).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.headers["content-type"], "text/html; charset=utf-8");
    assert_eq!(
        a.headers["content-security-policy"],
        "default-src 'none'; style-src 'unsafe-inline'"
    );
    assert_eq!(a.headers["cache-control"], "private, no-store");
    assert_eq!(a.headers["referrer-policy"], "no-referrer");
    assert!(a.headers["x-robots-tag"]
        .to_str()
        .unwrap()
        .contains("noindex"));
    for words in [
        code.as_str(),
        "<h1>HJViewer 원격 지원</h1>",
        "<h1>HJViewer remote support</h1>",
        "href=\"HJViewer-1.9.0.exe\"",
        "HJViewer-1.9.0.exe 받기",
        "Download HJViewer-1.9.0.exe",
        "연세대 교직원(로그인 확인)",
        "Yonsei faculty or staff (verified login)",
        &hex(&sha256(&files.exe)),
        "href=\"HJViewer-1.9.0-LICENSE.txt\"",
        "href=\"HJViewer-1.9.0-SOURCE.txt\"",
    ] {
        assert!(a.text.contains(words), "{words}: {}", a.text);
    }
    // no pilot notice since 1.9.1: the service is an official one
    for absent in [
        "<script",
        "-install.exe",
        "-qs-",
        "관리자:",
        "http://",
        "https://",
        "시범",
        "pilot",
    ] {
        assert!(!a.text.contains(absent), "{absent}");
    }
    assert!(a.text.find("원격 지원").unwrap() < a.text.find("remote support").unwrap());
    // a browser's Origin is taken here
    let a = h
        .request(
            Method::GET,
            &page,
            &[("origin", "https://example.org"), ("x-real-ip", ip)],
            None,
        )
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    // the package: HEAD is free, three GETs, then 404 (HEAD too); the notices still go out
    let exe = format!("{link}/HJViewer-1.9.0.exe");
    for _ in 0..2 {
        let a = browse(&h, Method::HEAD, &exe, ip).await;
        assert_eq!(a.status, 200, "{}", a.text);
        assert_eq!(
            a.headers["content-length"],
            files.exe.len().to_string().as_str()
        );
    }
    for _ in 0..support::LINK_DOWNLOADS {
        let a = h
            .request(Method::GET, &exe, &[("x-real-ip", ip)], None)
            .await;
        assert_eq!(a.status, 200);
        assert_eq!(
            a.headers["content-disposition"],
            "attachment; filename=\"HJViewer-1.9.0.exe\""
        );
        assert_eq!(a.text.len(), files.exe.len());
    }
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(
        status.contains("support links with downloads left: 0"),
        "{status}"
    );
    for method in [Method::GET, Method::HEAD] {
        let a = browse(&h, method, &exe, ip).await;
        assert_eq!(a.status, 404, "{}", a.text);
    }
    let a = browse(
        &h,
        Method::GET,
        &format!("{link}/HJViewer-1.9.0-LICENSE.txt"),
        ip,
    )
    .await;
    assert_eq!((a.status, a.text.as_str()), (200, "AGPL-3.0"));
    assert_eq!(browse(&h, Method::GET, &page, ip).await.status, 200);
    let tag = h.account_tag(HONG.0);
    assert_eq!(
        update_records(&h).await,
        vec![("support-windows".to_owned(), tag, h.state.tags.ip(ip)); 3]
    );
    // a fresh code's link: no installer's name, no other file, no deeper path
    let (_, fresh, _) = made_all(&h, &token).await;
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(
        status.contains("support links with downloads left: 1"),
        "{status}"
    );
    for name in [
        "HJViewer-1.9.0-install.exe",
        "HJViewer-1.9.0-windows-x86_64.exe",
        "HJViewer-1.9.0-NOTICE.txt",
    ] {
        let a = browse(&h, Method::GET, &format!("{fresh}/{name}"), ip).await;
        assert_eq!((a.status, &a.json), (404, &no_link()), "{name}");
    }
    // a deeper path is none of hjv-api's
    let a = browse(
        &h,
        Method::GET,
        &format!("{fresh}/x/HJViewer-1.9.0.exe"),
        ip,
    )
    .await;
    assert_eq!(a.status, 404, "{}", a.text);
    // any other value: the same 404, never counted against the address
    let other = "10.8.11.2";
    let unknown = format!(
        "{}{}",
        support::LINK_PATH,
        hjv_api::util::b64url(&random_bytes::<16>())
    );
    for path in [
        unknown.clone(),
        format!("{unknown}/"),
        format!("{unknown}/HJViewer-1.9.0.exe"),
        format!("{}AAAA/", support::LINK_PATH),
        format!("{}{}/", support::LINK_PATH, "%".repeat(22)),
        format!("{}{}/", support::LINK_PATH, code),
    ] {
        let a = browse(&h, Method::GET, &path, other).await;
        assert_eq!((a.status, &a.json), (404, &no_link()), "{path}");
        assert_eq!(a.headers["cache-control"], "private, no-store");
    }
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(other))
            .await,
        0
    );
    let a = h.request(Method::POST, &page, &[], None).await;
    assert_eq!(a.status, 405, "{}", a.text);
    // no Windows package in the policy: SL8 in SL3's place, the code still there
    files.policy(false);
    let (code, link, _) = made_all(&h, &token).await;
    let a = browse(&h, Method::GET, &format!("{link}/"), ip).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(a.text.contains(&code));
    assert!(
        a.text.contains("HJViewer를 준비하고 있습니다."),
        "{}",
        a.text
    );
    assert!(a.text.contains("HJViewer is being prepared."), "{}", a.text);
    assert!(!a.text.contains(".exe"), "{}", a.text);
}

/// A support link ends with its code: traded, over, its maker blocked, its maker's key logged out.
#[tokio::test]
async fn the_link_dies_with_its_code() {
    let files = Files::new();
    let h = Harness::with(Options {
        vars: files.vars(),
        ..Default::default()
    })
    .await;
    let ip = "10.8.12.1";
    let alive = |a: &Answer| a.status == 200;
    let paths = |link: &str| [format!("{link}/"), format!("{link}/HJViewer-1.9.0.exe")];
    let token = h.login_ok(HONG, None).await;
    // traded
    let (code, link, _) = made_all(&h, &token).await;
    for path in paths(&link) {
        assert!(alive(&browse(&h, Method::HEAD, &path, ip).await), "{path}");
    }
    let pc = TestDevice::windows("300000103");
    assert_eq!(
        trade(&h, &code_body(&h, &code, &pc), "10.8.12.2")
            .await
            .status,
        200
    );
    for path in paths(&link) {
        let a = browse(&h, Method::GET, &path, ip).await;
        assert_eq!((a.status, &a.json), (404, &no_link()), "{path}");
    }
    // ten minutes over
    let (_, link, _) = made_all(&h, &token).await;
    h.state.support.age_by(support::CODE_SECS);
    for path in paths(&link) {
        assert_eq!(
            browse(&h, Method::GET, &path, ip).await.status,
            404,
            "{path}"
        );
    }
    // the maker blocked (and unblocked: still over)
    let kim = h.login_ok(KIM, None).await;
    let (_, link, _) = made_all(&h, &kim).await;
    let kim_tag = format!("tag:{}", h.account_tag(KIM.0));
    h.admin(&["block", &kim_tag], false, None).await.unwrap();
    assert_eq!(
        browse(&h, Method::GET, &format!("{link}/"), ip)
            .await
            .status,
        404
    );
    h.admin(&["unblock", &kim_tag], false, None).await.unwrap();
    assert_eq!(
        browse(&h, Method::GET, &format!("{link}/"), ip)
            .await
            .status,
        404
    );
    // the maker's key logged out
    let (_, link, _) = made_all(&h, &token).await;
    assert!(alive(
        &browse(&h, Method::GET, &format!("{link}/"), ip).await
    ));
    let out = h.post("/api/logout", &token, Some(json!({}))).await;
    assert_eq!(out.status, 200, "{}", out.text);
    assert_eq!(
        browse(&h, Method::GET, &format!("{link}/"), ip)
            .await
            .status,
        404
    );
    assert!(update_records(&h).await.is_empty());
}

fn wait_path(rev: Option<u64>) -> String {
    match rev {
        Some(rev) => format!("{}?rev={rev}", support::WAIT_PATH),
        None => support::WAIT_PATH.to_owned(),
    }
}

/// The waiting room: at once without `rev` or with another; with its own, held until the PC
/// trades the code (then `used` with the PC's ID, key and the ticket's time left), until the
/// first open code ends (`over`), or for WAIT_SECS; only the codes of the request's account key;
/// two holds per key; the key's calls a minute; the App Review account's 403.
#[tokio::test]
async fn the_waiting_room() {
    let h = std::sync::Arc::new(Harness::start().await);
    // a key with no code: held the full WAIT_SECS, twice at once; a third at once
    let idle = h.login_ok(KIM, None).await;
    let first = h.get(&wait_path(None), &idle).await;
    assert_eq!(
        (first.status, &first.json),
        (200, &json!({"rev": 0, "codes": []})),
        "{}",
        first.text
    );
    let holds: Vec<_> = (0..support::HOLDS_PER_KEY)
        .map(|_| {
            let (h, idle) = (h.clone(), idle.clone());
            tokio::spawn(async move { h.get(&wait_path(Some(0)), &idle).await })
        })
        .collect();
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(h.state.support.held_waits(), support::HOLDS_PER_KEY);
    let third = h.get(&wait_path(Some(0)), &idle).await;
    assert_eq!(third.status, 200, "{}", third.text);
    assert!(
        third.elapsed < Duration::from_secs(2),
        "{:?}",
        third.elapsed
    );
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(
        status.contains("support codes' waiting room: 2 held"),
        "{status}"
    );

    let started = Instant::now();
    let token = h.login_ok(HONG, None).await;
    let a = h.get(&wait_path(None), &token).await;
    assert_eq!(a.json, json!({"rev": 0, "codes": []}), "{}", a.text);
    let (code, _, wait) = made_all(&h, &token).await;
    let a = h.get(&wait_path(Some(0)), &token).await;
    assert!(a.elapsed < Duration::from_secs(2));
    assert_eq!(a.json["rev"], 1, "{}", a.text);
    let codes = a.json["codes"].as_array().unwrap();
    assert_eq!(codes.len(), 1, "{}", a.text);
    assert_eq!(
        (&codes[0]["wait"], &codes[0]["state"]),
        (&json!(wait), &json!("open"))
    );
    assert!((598..=600).contains(&codes[0]["expires_in"].as_i64().unwrap()));
    // the account's other key and another account see none of it
    let other_key = h.login_ok(HONG, None).await;
    let lee = h.login_ok(LEE, None).await;
    for t in [&other_key, &lee] {
        let a = h.get(&wait_path(None), t).await;
        assert_eq!(a.json, json!({"rev": 0, "codes": []}), "{}", a.text);
    }
    // held with its rev until the PC trades the code
    let pc = TestDevice::windows("300000104");
    let held = {
        let (h, token) = (h.clone(), token.clone());
        tokio::spawn(async move { h.get(&wait_path(Some(1)), &token).await })
    };
    tokio::time::sleep(Duration::from_millis(700)).await;
    let traded_at = Instant::now();
    let a = trade(&h, &code_body(&h, &code, &pc), "10.8.13.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let a = held.await.unwrap();
    assert!(a.elapsed >= Duration::from_millis(600), "{:?}", a.elapsed);
    assert!(traded_at.elapsed() < Duration::from_secs(2));
    assert_eq!(a.json["rev"], 2, "{}", a.text);
    let used = &a.json["codes"][0];
    assert_eq!(
        (&used["wait"], &used["state"], &used["pc"], &used["pk"]),
        (
            &json!(wait),
            &json!("used"),
            &json!(pc.id),
            &json!(b64(&pc.public_key()))
        ),
        "{}",
        a.text
    );
    assert!(
        (7190..=7200).contains(&used["until_in"].as_i64().unwrap()),
        "{used}"
    );
    assert_eq!(used.as_object().unwrap().len(), 5);
    // held no longer than the first open code: its end wakes it, `over`
    let (_, _, second) = made_all(&h, &token).await;
    h.state.support.age_by(support::CODE_SECS - 2);
    let rev = h.get(&wait_path(None), &token).await.json["rev"]
        .as_u64()
        .unwrap();
    let a = h.get(&wait_path(Some(rev)), &token).await;
    assert!(
        a.elapsed >= Duration::from_millis(900) && a.elapsed < Duration::from_secs(5),
        "{:?}",
        a.elapsed
    );
    assert!(a.json["rev"].as_u64().unwrap() > rev, "{}", a.text);
    let over = a.json["codes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["wait"] == json!(second))
        .unwrap();
    assert_eq!(over, &json!({"wait": second, "state": "over"}));
    // a rev that is not the key's: at once
    let a = h.get(&wait_path(Some(999)), &token).await;
    assert!(a.elapsed < Duration::from_secs(2));
    let a = h
        .get(&format!("{}?rev=x", support::WAIT_PATH), &token)
        .await;
    assert!(a.elapsed < Duration::from_secs(2));
    // only GET; a token without its proof, 401
    let a = h.post(support::WAIT_PATH, &token, Some(json!({}))).await;
    assert_eq!(a.status, 405, "{}", a.text);
    assert_eq!(a.headers["allow"], "GET");
    let auth = format!("Bearer {token}");
    let a = h
        .request(
            Method::GET,
            support::WAIT_PATH,
            &[("authorization", auth.as_str())],
            None,
        )
        .await;
    assert_eq!(a.status, 401, "{}", a.text);
    // the key's calls a minute, with its other calls (the holds and the codes made counted)
    let mut answers = Vec::new();
    for _ in 0..40 {
        answers.push(h.get(&wait_path(None), &other_key).await);
    }
    let taken = answers.iter().filter(|a| a.status == 200).count();
    assert!(taken <= 30 && taken > 20, "{taken}");
    let last = answers.last().unwrap();
    assert_eq!(
        (last.status, &last.json["hjv"]),
        (429, &json!("rate_limited"))
    );
    // the first two holds: the full WAIT_SECS, the same rev, no code
    for hold in holds {
        let a = hold.await.unwrap();
        assert_eq!(
            (a.status, &a.json),
            (200, &json!({"rev": 0, "codes": []})),
            "{}",
            a.text
        );
        assert!(
            a.elapsed >= Duration::from_secs(support::WAIT_SECS) - Duration::from_millis(300),
            "{:?}",
            a.elapsed
        );
    }
    assert!(started.elapsed() < Duration::from_secs(support::WAIT_SECS + 5));
    assert_eq!(h.state.support.held_waits(), 0);
}

/// The App Review account cannot wait for codes it cannot make.
/// A held waiting-room request is answered when the service stops, not after its WAIT_SECS
/// (server.rs `serve` calls `SupportCodes::stop`).
#[tokio::test]
async fn held_waits_do_not_keep_the_service_from_stopping() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let stop = async {
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(h.state.support.held_waits(), 1);
        h.stop();
    };
    let (started, path) = (Instant::now(), wait_path(Some(0)));
    let (a, ()) = tokio::join!(h.get(&path, &token), stop);
    assert_eq!(
        (a.status, &a.json),
        (200, &json!({"rev": 0, "codes": []})),
        "{}",
        a.text
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn the_review_account_has_no_waiting_room() {
    const HASH: &str =
        "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";
    let dir = temp_dir("support-review");
    let file = dir.join("review.conf");
    std::fs::write(
        &file,
        format!(
            "ID=APPREVIEW-TEST\nHASH={HASH}\nUNTIL={}\nPC=100200300\n",
            datetime(now() + 2 * DAY)
        ),
    )
    .unwrap();
    let h = Harness::with(Options {
        vars: vec![("HJV_API_REVIEW_FILE", file.display().to_string())],
        ..Default::default()
    })
    .await;
    let a = h
        .login(
            &h.login_body("APPREVIEW-TEST", "correct horse battery staple", None),
            "10.8.14.1",
        )
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    let review = a.token();
    let a = h.get(&wait_path(Some(0)), &review).await;
    assert_eq!(
        (a.status, &a.json),
        (
            403,
            &json!({"error": support::TEXT_NO_CODE, "hjv": "no_code"})
        ),
        "{}",
        a.text
    );
    assert!(a.elapsed < Duration::from_secs(2));
    let _ = std::fs::remove_dir_all(&dir);
}
