// SPDX-License-Identifier: AGPL-3.0-only

//! Support codes (support.rs, guest.rs, docs/access-tickets.md 3-7) end to end: a staff member's
//! app makes a code, a PC trades it once within ten minutes for a two-hour `guest`/`pc` ticket;
//! wrong, expired, used and blocked codes get one 401 after the failure delay; one of two trades
//! at once; the access password's limits; the caps; who may not make one; nothing of a code in
//! the journal or the records.

mod common;

use common::*;
use hjv_api::{
    devsig, guest, seal, support, ticket,
    util::{b64, b64url_decode, datetime, now, random_bytes, sha256, DAY, HOUR},
};
use hyper::Method;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;

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
    assert_eq!(object.as_object().unwrap().len(), 4, "{object}");
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
    assert_eq!(a.json.as_object().unwrap().len(), 3, "{}", a.text);
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
/// PC's ID.
#[tokio::test]
async fn nothing_of_a_code_is_logged_or_recorded() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let codes = [made(&h, &token).await, made(&h, &token).await];
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
    // used, wrong, and the caps' journal line
    trade(&h, &code_body(&h, &codes[0], &pc), "10.8.8.1").await;
    trade(&h, &code_body(&h, "ZZZZ-ZZZZ", &pc), "10.8.8.1").await;
    for _ in 0..support::OPEN_PER_ACCOUNT {
        make(&h, &token, json!({"use": "pc"})).await;
    }
    let lines = hjv_api::log::captured();
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
    let mut secrets = vec![payload, "10.8.8.1".to_owned(), pc.id.clone()];
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
