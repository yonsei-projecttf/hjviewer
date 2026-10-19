// SPDX-License-Identifier: AGPL-3.0-only

//! The access password (guest.rs, docs/access-tickets.md 3-5) end to end: a controller's and a
//! PC's ticket for one PC; the same 401 for a wrong password and a closed file, both after the
//! failure delay; the seal that must be the access password's for this request; the limits before
//! the hash; hbbs's key of a PC; the records with the address's tag only. The password and its
//! hash are made up: the real one lives only in the server's file.

mod common;

use common::*;
use hjv_api::{
    devsig, guest, seal, ticket,
    util::{b64, b64url_decode, datetime, now, random_bytes, sha256, HOUR},
};
use hyper::Method;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const PASSWORD: &str = "correct horse battery staple";
/// PBKDF2-HMAC-SHA256 of PASSWORD with the salt 00 01 … 0f and 100 000 iterations (review.rs has
/// the vector).
const HASH: &str =
    "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";
const TARGET: &str = "123456789";

/// The test's directory of the file, removed with it.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// hjv-api with HJV_API_GUEST_FILE in a directory of the test's, the file there and active for a
/// day, and `opts` besides.
async fn start_with(mut opts: Options) -> (Harness, PathBuf, Dir) {
    let dir = temp_dir("guest");
    let file = dir.join("guest.conf");
    put(&file, &guest_file(now() + 24 * HOUR));
    opts.vars
        .insert(0, ("HJV_API_GUEST_FILE", file.display().to_string()));
    let h = Harness::with(opts).await;
    (h, file, Dir(dir))
}

async fn start() -> (Harness, PathBuf, Dir) {
    start_with(Options::default()).await
}

/// Puts the file in place as the runbook does: a new file moved over the old one.
fn put(file: &Path, text: &str) {
    let new = file.with_extension("new");
    std::fs::write(&new, text).unwrap();
    std::fs::rename(&new, file).unwrap();
}

fn guest_file(until: i64) -> String {
    format!("# made up\nHASH={HASH}\nUNTIL={}\n", datetime(until))
}

fn nonce() -> String {
    b64(&random_bytes::<16>())
}

/// A controller's request for the PC `to`, sealed at `t` as the app seals it.
fn ctl_body(h: &Harness, password: &str, to: &str, t: i64) -> Value {
    let field = seal::seal_guest(&h.seal_pk, password, t, &nonce(), "ctl", to);
    json!({"use": "ctl", "to": to, "password": field})
}

/// The sealed bytes of a sealed field, which a PC's signature covers.
fn sealed_hash(field: &str) -> [u8; 32] {
    sha256(&b64url_decode(field.rsplit('.').next().unwrap()).unwrap())
}

/// A PC's request for itself, its device key's signature over the seal.
fn pc_body(h: &Harness, password: &str, pc: &TestDevice, t: i64) -> Value {
    let field = seal::seal_guest(&h.seal_pk, password, t, &nonce(), "pc", &pc.id);
    let pk = b64(&pc.public_key());
    let sig = devsig::sign(
        &pc.seed,
        &guest::pc_message(&pc.id, &pk, &sealed_hash(&field)),
    );
    json!({"use": "pc", "id": pc.id, "pk": pk, "sig": sig, "password": field})
}

async fn ask(h: &Harness, body: &Value, ip: &str) -> Answer {
    ask_raw(h, body.to_string().into_bytes(), ip).await
}

async fn ask_raw(h: &Harness, body: Vec<u8>, ip: &str) -> Answer {
    h.request(
        Method::POST,
        guest::PATH,
        &[("content-type", "application/json"), ("x-real-ip", ip)],
        Some(body),
    )
    .await
}

/// The payload of a ticket object's ticket, once the harness's ticket key's signature holds; the
/// object checked as the app takes it.
fn payload_of(object: &Value) -> Value {
    assert_eq!(object.as_object().unwrap().len(), 4, "{object}");
    let ticket_text = object["ticket"].as_str().unwrap();
    let parts: Vec<&str> = ticket_text.split('.').collect();
    assert_eq!(
        (parts.len(), parts[0], parts[1]),
        (4, "t1", "1"),
        "{ticket_text}"
    );
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

fn denied() -> Value {
    json!({"error": guest::TEXT_DENIED, "hjv": "denied"})
}

/// The records of the access password's tries: result and the address as kept.
async fn guest_records(h: &Harness) -> Vec<(String, String)> {
    h.state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT result, COALESCE(ip, '') FROM events WHERE kind = 'guest' ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })
        .await
        .unwrap()
}

/// A controller gets a `guest`/`ctl` ticket for the one PC it named, a PC a `guest`/`pc` one for
/// its ID and key, whether hbbs knows it yet or not, each for 12 hours; the records keep the
/// address's tag only.
#[tokio::test]
async fn a_controller_and_a_pc_get_their_tickets() {
    let (h, _file, _dir) = start().await;
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.0.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let p = payload_of(&a.json);
    assert_eq!(
        (&p["who"], &p["use"], &p["sub"], &p["id"], &p["pk"], &p["to"]),
        (
            &json!("guest"),
            &json!("ctl"),
            &json!(""),
            &json!(""),
            &json!(""),
            &json!([TARGET])
        )
    );
    assert_eq!(
        p["exp"].as_i64().unwrap() - p["iat"].as_i64().unwrap(),
        12 * HOUR
    );
    // a PC hbbs knows by its key, and one it does not know yet
    let pc = TestDevice::windows("300000071");
    h.hbbs.register(&pc, Some(1));
    let fresh = TestDevice::windows("Office-PC_7");
    for device in [&pc, &fresh] {
        let a = ask(&h, &pc_body(&h, PASSWORD, device, now()), "10.7.0.2").await;
        assert_eq!(a.status, 200, "{}", a.text);
        let p = payload_of(&a.json);
        assert_eq!(
            (&p["who"], &p["use"], &p["id"], &p["pk"], &p["to"]),
            (
                &json!("guest"),
                &json!("pc"),
                &json!(device.id),
                &json!(b64(&device.public_key())),
                &json!([])
            )
        );
    }
    let records = guest_records(&h).await;
    assert_eq!(
        records,
        vec![
            ("ok-ctl".to_owned(), h.state.tags.ip("10.7.0.1")),
            ("ok-pc".to_owned(), h.state.tags.ip("10.7.0.2")),
            ("ok-pc".to_owned(), h.state.tags.ip("10.7.0.2")),
        ]
    );
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(status.contains("guest ctl 1, guest pc 2"), "{status}");
    assert!(status.contains("access password: on until "), "{status}");
    assert!(
        status.contains("access password, the last day's tries: ok-ctl 1, ok-pc 2"),
        "{status}"
    );
}

/// A wrong password and a closed file (none, past its UNTIL, refused) get the same 401, no
/// sooner than the failure delay; each is the address's failure.
#[tokio::test]
async fn a_wrong_password_and_a_closed_file_get_the_same_answer() {
    let (h, file, _dir) = start_with(Options {
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
    let wrong = ask(&h, &ctl_body(&h, "wrong horse", TARGET, now()), "10.7.1.1").await;
    expect_denied(&wrong);
    put(&file, &guest_file(now() - 1));
    expect_denied(&ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.1.2").await);
    put(&file, "HASH=nothing\nUNTIL=later\n");
    expect_denied(&ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.1.3").await);
    std::fs::remove_file(&file).unwrap();
    expect_denied(&ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.1.4").await);
    // back again: the file is read again, no restart
    put(&file, &guest_file(now() + HOUR));
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.1.5").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let results: Vec<String> = guest_records(&h)
        .await
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    assert_eq!(results, ["wrong", "closed", "closed", "closed", "ok-ctl"]);
    // each failure counted against its address
    assert_eq!(h.counted("guest-ip", h.state.tags.ip("10.7.1.1")).await, 1);
    assert_eq!(h.counted("guest-ip", h.state.tags.ip("10.7.1.5")).await, 0);
}

/// The seal must be the access password's, for this request's use and PC, fresh and used once;
/// a login's seal is refused here and the access password's at the login, and a PC's signature
/// must cover its seal.
#[tokio::test]
async fn the_seal_is_the_access_passwords_for_this_request() {
    let (h, _file, _dir) = start().await;
    let bad = json!({"error": "요청을 이해하지 못했습니다.", "hjv": "bad_request"});
    let t = now();
    // a login's seal
    let login_seal = seal::seal_password(&h.seal_pk, PASSWORD, t, &nonce(), TARGET);
    let a = ask(
        &h,
        &json!({"use": "ctl", "to": TARGET, "password": login_seal}),
        "10.7.2.1",
    )
    .await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    // sealed for another PC, or for the other use
    let other = seal::seal_guest(&h.seal_pk, PASSWORD, t, &nonce(), "ctl", "222333444");
    let a = ask(
        &h,
        &json!({"use": "ctl", "to": TARGET, "password": other}),
        "10.7.2.2",
    )
    .await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    let pc_use = seal::seal_guest(&h.seal_pk, PASSWORD, t, &nonce(), "pc", TARGET);
    let a = ask(
        &h,
        &json!({"use": "ctl", "to": TARGET, "password": pc_use}),
        "10.7.2.3",
    )
    .await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    // not sealed at all
    let a = ask(
        &h,
        &json!({"use": "ctl", "to": TARGET, "password": PASSWORD}),
        "10.7.2.4",
    )
    .await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    // a time more than ten minutes off: 409 with the server's time
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, t - 601), "10.7.2.5").await;
    assert_eq!(a.status, 409, "{}", a.text);
    assert_eq!(a.json["hjv"], "clock");
    assert!((a.json["server_time"].as_i64().unwrap() - now()).abs() <= 5);
    // the very same body again: the first answer, the same ticket; another body with its nonce:
    // refused
    let body = ctl_body(&h, PASSWORD, TARGET, now());
    let first = ask(&h, &body, "10.7.2.6").await;
    assert_eq!(first.status, 200, "{}", first.text);
    let again = ask(&h, &body, "10.7.2.6").await;
    assert_eq!((again.status, &again.json), (200, &first.json));
    // the same members with a blank more: another body, the same nonce
    let reordered = format!(" {body}");
    let a = ask_raw(&h, reordered.into_bytes(), "10.7.2.6").await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    // a PC whose signature is of another seal, or by another key
    let pc = TestDevice::windows("300000072");
    let mut swapped = pc_body(&h, PASSWORD, &pc, now());
    swapped["password"] = json!(seal::seal_guest(
        &h.seal_pk,
        PASSWORD,
        now(),
        &nonce(),
        "pc",
        &pc.id
    ));
    let a = ask(&h, &swapped, "10.7.2.7").await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    let mut other_key = pc_body(&h, PASSWORD, &pc, now());
    other_key["pk"] = json!(b64(&TestDevice::windows("300000073").public_key()));
    let a = ask(&h, &other_key, "10.7.2.8").await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    // and the access password's seal at the login
    let field = seal::seal_guest(&h.seal_pk, PASSWORD, now(), &nonce(), "ctl", TARGET);
    let mut login = h.login_body(HONG.0, HONG.1, None);
    login["password"] = json!(field);
    login["id"] = json!(TARGET);
    let a = h.login(&login, "10.7.2.9").await;
    assert_eq!(a.status, 400, "{}", a.text);
    let results: Vec<String> = guest_records(&h)
        .await
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    assert_eq!(
        results,
        [
            "seal-purpose",
            "seal-request",
            "seal-request",
            "seal-missing",
            "seal-time",
            "ok-ctl",
            "seal-nonce",
            "pc-signature",
            "pc-signature"
        ]
    );
    assert!(h.results().await.contains(&"login:seal-purpose".to_owned()));
}

/// Bodies that are no request get 400 and count nowhere; the other forms of the path are a
/// login's.
#[tokio::test]
async fn the_form_of_the_request() {
    let (h, _file, _dir) = start_with(Options {
        vars: vec![("HJV_API_LIMIT_GUEST_IP", "1/600".into())],
        ..Default::default()
    })
    .await;
    let ip = "10.7.3.1";
    for body in [
        b"not json".to_vec(),
        b"[]".to_vec(),
        json!({}).to_string().into_bytes(),
        json!({"use": "ctl", "to": TARGET}).to_string().into_bytes(),
        json!({"use": "both", "to": TARGET, "password": "x"})
            .to_string()
            .into_bytes(),
        json!({"use": "ctl", "to": "no spaces", "password": "x"})
            .to_string()
            .into_bytes(),
    ] {
        let a = ask_raw(&h, body.clone(), ip).await;
        assert_eq!(
            a.status,
            400,
            "{}: {}",
            String::from_utf8_lossy(&body),
            a.text
        );
        assert_eq!(a.json["hjv"], "bad_request");
    }
    let a = h
        .request(Method::GET, guest::PATH, &[("x-real-ip", ip)], None)
        .await;
    assert_eq!(a.status, 405);
    assert_eq!(a.headers["allow"], "POST");
    let a = h
        .request(
            Method::POST,
            guest::PATH,
            &[
                ("content-type", "application/json"),
                ("origin", "https://example.org"),
            ],
            Some(
                ctl_body(&h, PASSWORD, TARGET, now())
                    .to_string()
                    .into_bytes(),
            ),
        )
        .await;
    assert_eq!(a.status, 403);
    let a = h
        .request(
            Method::POST,
            guest::PATH,
            &[("content-type", "text/plain")],
            Some(b"{}".to_vec()),
        )
        .await;
    assert_eq!(a.status, 415);
    // none of these counted: the address's one try is still there
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), ip).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(guest_records(&h).await.len() == 1);
}

/// The limits come before the hash: the address's failures, its tickets, and the tries of all
/// addresses of which one holds a quarter; `admin unlock` clears an address's.
#[tokio::test]
async fn the_limits_come_before_the_hash() {
    let (h, _file, _dir) = start_with(Options {
        vars: vec![
            ("HJV_API_LIMIT_GUEST_IP", "2/600".into()),
            ("HJV_API_LIMIT_GUEST_TICKETS", "2/86400".into()),
            ("HJV_API_LIMIT_GUEST_ALL", "8/3600".into()),
        ],
        ..Default::default()
    })
    .await;
    // 429 with the seconds until the limit's window has room again: about the whole window here
    let too_often = |a: &Answer, window: u64| {
        assert_eq!(a.status, 429, "{}", a.text);
        assert_eq!(a.json["error"], guest::TEXT_TOO_OFTEN, "{}", a.text);
        assert_eq!(a.json["hjv"], "rate_limited", "{}", a.text);
        let wait = a.json["retry_after"].as_u64().unwrap_or(0);
        assert!(window - 60 < wait && wait <= window, "{}", a.text);
    };
    // two failures, then even the right password waits
    for _ in 0..2 {
        let a = ask(&h, &ctl_body(&h, "wrong horse", TARGET, now()), "10.7.4.1").await;
        assert_eq!(a.status, 401, "{}", a.text);
    }
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.4.1").await;
    too_often(&a, 600);
    let tag = format!("tag:{}", h.state.tags.ip("10.7.4.1"));
    let unlocked = h
        .admin(&["unlock", tag.as_str()], false, None)
        .await
        .unwrap();
    assert!(
        unlocked.starts_with("cleared 2 counted login attempts"),
        "{unlocked}"
    );
    assert!(
        unlocked.ends_with(", and 2 tries of the access password and support codes"),
        "{unlocked}"
    );
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.4.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    // two tickets a day from one address; a third is refused before its hash
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.4.2").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let a = ask(&h, &ctl_body(&h, PASSWORD, "222333444", now()), "10.7.4.2").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.4.2").await;
    too_often(&a, 86400);
    // eight tries of everyone an hour, two of them one address's: 10.7.4.2 holds its two
    let a = ask(&h, &ctl_body(&h, "wrong horse", TARGET, now()), "10.7.4.3").await;
    assert_eq!(a.status, 401, "{}", a.text);
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.4.3").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.4.3").await;
    too_often(&a, 3600);
    for ip in ["10.7.4.4", "10.7.4.5", "10.7.4.6"] {
        let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), ip).await;
        assert_eq!(a.status, 200, "{}", a.text);
    }
    // 1 (10.7.4.1's after the unlock forgot its two) + 2 + 2 + 3 = 8: the window is full
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.4.7").await;
    too_often(&a, 3600);
    let results: Vec<String> = guest_records(&h)
        .await
        .into_iter()
        .map(|(r, _)| r)
        .collect();
    assert!(results.contains(&"limited-ip".to_owned()), "{results:?}");
    assert!(
        results.contains(&"limited-tickets".to_owned()),
        "{results:?}"
    );
    assert!(results.contains(&"limited-all".to_owned()), "{results:?}");
}

/// A PC that hbbs knows by another key gets no ticket; hbbs is asked about a PC once a minute.
#[tokio::test]
async fn a_pc_hbbs_knows_by_another_key() {
    let (h, _file, _dir) = start().await;
    let pc = TestDevice::windows("300000074");
    h.hbbs
        .register_key(&pc.id, TestDevice::windows("300000075").public_key());
    let wrong_pc = json!({"error": guest::TEXT_WRONG_PC, "hjv": "wrong_pc"});
    let before = h.hbbs.queries.load(std::sync::atomic::Ordering::SeqCst);
    let a = ask(&h, &pc_body(&h, PASSWORD, &pc, now()), "10.7.5.1").await;
    assert_eq!((a.status, &a.json), (403, &wrong_pc), "{}", a.text);
    // the PC's own key now: within the minute the first answer still serves
    h.hbbs.register(&pc, Some(1));
    let a = ask(&h, &pc_body(&h, PASSWORD, &pc, now()), "10.7.5.1").await;
    assert_eq!((a.status, &a.json), (403, &wrong_pc), "{}", a.text);
    assert_eq!(
        h.hbbs.queries.load(std::sync::atomic::Ordering::SeqCst) - before,
        1
    );
    // a right password whose PC hbbs knows by another key is no failure of the address
    assert_eq!(h.counted("guest-ip", h.state.tags.ip("10.7.5.1")).await, 0);
    // a console that does not answer hjv-pk: 503
    let other = TestDevice::windows("300000076");
    h.hbbs
        .unpatched
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let a = ask(&h, &pc_body(&h, PASSWORD, &other, now()), "10.7.5.2").await;
    assert_eq!(a.status, 503, "{}", a.text);
    assert_eq!(a.json["hjv"], "unavailable");
}

/// Without the ticket key no ticket is signed: 503 at once, nothing counted.
#[tokio::test]
async fn without_a_ticket_key_none() {
    let (h, _file, _dir) = start_with(Options {
        no_ticket_key: true,
        ..Default::default()
    })
    .await;
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.6.1").await;
    assert_eq!(a.status, 503, "{}", a.text);
    assert_eq!(a.json["hjv"], "unavailable");
    assert!(guest_records(&h).await.is_empty());
}

/// Neither the password nor a ticket nor an address goes to the journal or the records.
#[tokio::test]
async fn nothing_of_the_password_is_logged_or_recorded() {
    let (h, _file, _dir) = start().await;
    let a = ask(&h, &ctl_body(&h, PASSWORD, TARGET, now()), "10.7.7.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let ticket_payload = a.json["ticket"]
        .as_str()
        .unwrap()
        .split('.')
        .nth(2)
        .unwrap()
        .to_owned();
    assert_eq!(
        ask(&h, &ctl_body(&h, "wrong horse", TARGET, now()), "10.7.7.1")
            .await
            .status,
        401
    );
    let lines = hjv_api::log::captured();
    let records = guest_records(&h).await;
    for line in lines
        .iter()
        .chain(records.iter().map(|(result, _)| result))
        .chain(records.iter().map(|(_, ip)| ip))
    {
        for secret in [PASSWORD, "wrong horse", "10.7.7.1", &ticket_payload, TARGET] {
            assert!(!line.contains(secret), "{line}");
        }
    }
}

/// README.md, The access password: a PC's request signature with the vectors' PC key and the
/// login seal vector's sealed hash, as OpenSSL signs it too.
#[test]
fn the_readme_shows_the_pc_vector() {
    let seed = [2u8; 32];
    let pk = b64(&devsig::public_key(&seed));
    let sealed_hash: [u8; 32] =
        hjv_api::util::b64_decode("qIDSw48Gkb5KSslffq/QEWRaClx5ihVLTE5+jqOHQ6Q=")
            .unwrap()
            .try_into()
            .unwrap();
    let message = guest::pc_message("123456789", &pk, &sealed_hash);
    let sig = devsig::sign(&seed, &message);
    assert_eq!(
        sig,
        "LCgw71tbHtZvAgBNOoElzNffv/sjmraHOBM/jvDp1fiGwvACfrVddPgq/FbR25j+KhE5fvd5rf2nT1KFbi0UDg=="
    );
    assert!(devsig::verify(&devsig::public_key(&seed), &message, &sig));
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    for value in [
        pk,
        hjv_api::util::hex(&message),
        sig,
        format!("signed ({})", message.len()),
    ] {
        assert!(readme.contains(&value), "README.md does not show {value}");
    }
}
