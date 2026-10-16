// SPDX-License-Identifier: AGPL-3.0-only

//! Access tickets (docs/access-tickets.md, ticket.rs): the shared vectors of
//! tests/vectors/access-tickets.json, which hbbs's tests read too, made again byte for byte by
//! hjv-api's signer; its rules held against every payload of the vectors; and the places hjv-api
//! hands tickets out: `POST /api/hjv/ticket`, the login's answer and a bound PC's sync, with what
//! each refuses, `admin ticket status` and selfcheck. The keys are the vectors' public test keys.

mod common;

use common::*;
use hjv_api::{
    config::Config,
    devsig,
    log::Tagger,
    ticket::{self, Claims, TicketKey, Use, Who},
    trust,
    util::{b64, b64_decode, b64url_decode, datetime, hex, now, sha256, DAY},
};
use hyper::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path};

const JSON: &str = include_str!("vectors/access-tickets.json");
/// The file as access-tickets.py renders it: a new rendering is a change of the contract that
/// hbbs's tests and the app's see too.
const JSON_SHA256: &str = "ff78a8788f411ad705e43ac501ec284b1ec2d2b067bfab358a877953a85e2377";

fn vectors() -> Value {
    serde_json::from_str(JSON).unwrap()
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string in {v}"))
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn seed(key: &Value) -> [u8; 32] {
    unhex(s(key, "seed_hex")).try_into().unwrap()
}

/// A payload's members as hbbs reads them (docs/access-tickets.md 2-3): all nine, nothing else,
/// whole numbers of zero or more.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Members {
    v: u64,
    who: String,
    #[serde(rename = "use")]
    use_: String,
    sub: String,
    id: String,
    pk: String,
    to: Vec<String>,
    iat: u64,
    exp: u64,
}

/// The claims of a payload's members, when they name a kind and a key at all.
fn claims_from(m: Members) -> Option<Claims> {
    if m.v != 1 {
        return None;
    }
    let pk = match m.pk.as_str() {
        "" => None,
        text => Some(
            b64_decode(text)
                .and_then(|k| <[u8; 32]>::try_from(k).ok())
                .filter(|k| b64(k) == text)?,
        ),
    };
    Some(Claims {
        who: Who::parse(&m.who)?,
        use_: Use::parse(&m.use_)?,
        sub: m.sub,
        id: m.id,
        pk,
        to: m.to,
        iat: i64::try_from(m.iat).ok()?,
        exp: i64::try_from(m.exp).ok()?,
    })
}

/// A ticket's parts as 2-1 reads them, strictly: the kid, the payload bytes, the signature.
fn wire(ticket: &str) -> Option<(u32, Vec<u8>, [u8; 64])> {
    let mut parts = ticket.split('.');
    let (Some("t1"), Some(kid), Some(p), Some(sig), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    let digits = !kid.is_empty()
        && kid.len() <= 5
        && kid.bytes().all(|b| b.is_ascii_digit())
        && !kid.starts_with('0');
    let kid = digits
        .then(|| kid.parse::<u32>().ok())
        .flatten()
        .filter(|k| *k <= 65_535)?;
    let payload = b64url_decode(p).filter(|b| !b.is_empty())?;
    if sig.len() != 86 {
        return None;
    }
    let sig = <[u8; 64]>::try_from(b64url_decode(sig)?).ok()?;
    Some((kid, payload, sig))
}

/// A ticket from hjv-api as hbbs checks it with the harness's keys: its payload, once the
/// signature of the key its kid names holds.
fn read(ticket: &str) -> Value {
    let (kid, payload, sig) = wire(ticket).unwrap_or_else(|| panic!("not a ticket: {ticket}"));
    let seed = match kid {
        1 => TICKET_SEED,
        2 => NEXT_TICKET_SEED,
        _ => panic!("kid {kid}"),
    };
    assert!(
        devsig::verify(
            &devsig::public_key(&seed),
            &ticket::message(&payload),
            &b64(&sig)
        ),
        "the signature of {ticket}"
    );
    assert!(ticket.len() <= ticket::MAX_BYTES);
    serde_json::from_slice(&payload).unwrap()
}

/// The payload of the ticket object `access` of an answer, checked as the app takes the object.
fn access(object: &Value) -> Value {
    let p = read(s(object, "ticket"));
    assert_eq!(object["use"], p["use"], "{object}");
    assert_eq!(object["exp"], p["exp"], "{object}");
    let server_time = object["server_time"].as_i64().unwrap();
    assert!((server_time - now()).abs() <= 5, "{object}");
    assert_eq!(object.as_object().unwrap().len(), 4, "{object}");
    p
}

fn lifetime(p: &Value) -> i64 {
    p["exp"].as_i64().unwrap() - p["iat"].as_i64().unwrap()
}

/// The body of a Windows app's request for a `both` ticket: its device key's signature of its
/// ID, its key and the token's hash.
fn pc_body(device: &TestDevice, token: &str) -> Value {
    let pk = b64(&device.public_key());
    let sig = devsig::sign(
        &device.seed,
        &ticket::pc_message(&device.id, &pk, &sha256(token.as_bytes())),
    );
    json!({"id": device.id, "pk": pk, "sig": sig})
}

/// `POST /api/hjv/ticket` as the app sends it: signed, with a JSON body or none.
async fn ask(h: &Harness, token: &str, body: Option<Value>) -> Answer {
    match body {
        Some(body) => h.post(ticket::PATH, token, Some(body)).await,
        None => h.signed(Method::POST, ticket::PATH, token, None).await,
    }
}

// ---- the shared vectors ----

#[test]
fn the_file_is_the_one_both_sides_read() {
    assert_eq!(hex(&sha256(JSON.as_bytes())), JSON_SHA256);
    let v = vectors();
    assert_eq!(v["domain"], "hjv-ticket-v1");
    assert_eq!(ticket::DOMAIN, b"hjv-ticket-v1");
    let limits = &v["limits"];
    assert_eq!(limits["ticket_bytes"], ticket::MAX_BYTES as u64);
    assert_eq!(limits["max_lifetime_seconds"], ticket::MAX_LIFETIME);
    assert_eq!(limits["max_to"], ticket::MAX_TO as u64);
    assert_eq!(limits["kid"], json!([1, trust::MAX_KID]));
}

#[test]
fn the_keys_the_tags_and_the_lifetimes() {
    let v = vectors();
    let keys = &v["keys"];
    for name in ["ticket_1", "ticket_2", "ticket_3", "pc", "other_pc"] {
        assert_eq!(
            b64(&devsig::public_key(&seed(&keys[name]))),
            s(&keys[name], "pk"),
            "{name}"
        );
    }
    // the harness signs with key 1 and has hbbs take keys 1 and 2, as the vectors' rule does
    assert_eq!(seed(&keys["ticket_1"]), TICKET_SEED);
    assert_eq!(seed(&keys["ticket_2"]), NEXT_TICKET_SEED);
    let rule = &v["rule"]["keys"];
    assert_eq!(
        ticket_pubkeys(),
        format!("1:{},2:{}", s(rule, "1"), s(rule, "2"))
    );
    // sub is the account's tag of log.rs, made with the login key
    let tags = Tagger::new(&unhex(s(&v["accounts"], "log_key_hex")));
    for who in ["staff", "review"] {
        let account = &v["accounts"][who];
        assert_eq!(
            tags.account(s(account, "login_key")),
            s(account, "sub"),
            "{who}"
        );
    }
    // the vectors' lifetimes are hjv-api's defaults
    let vars: HashMap<String, String> = [
        ("HJV_API_SSO_TOKEN_URL", "https://sso.example.ac.kr/token"),
        ("HJV_API_SSO_USERINFO_URL", "https://sso.example.ac.kr/user"),
        ("HJV_API_SSO_CLIENT_ID", "client"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    let config = Config::from_vars(&vars).unwrap();
    let lifetimes = &v["lifetimes"];
    assert_eq!(lifetimes["staff"], config.ticket_secs);
    assert_eq!(lifetimes["review"], config.ticket_secs);
    assert_eq!(lifetimes["bound_pc"], config.pc_ticket_secs);
}

/// docs/access-tickets.md 8: the payload made from each valid ticket's `fields` and signed with
/// its key is the ticket, byte for byte.
#[test]
fn every_valid_ticket_is_made_again() {
    let v = vectors();
    let mut made = 0;
    for t in v["tickets"].as_array().unwrap() {
        let Some(fields) = t.get("fields") else {
            continue;
        };
        let name = s(t, "name");
        let members: Members = serde_json::from_value(fields.clone()).unwrap();
        let claims = claims_from(members).unwrap_or_else(|| panic!("{name}"));
        let payload = ticket::payload(&claims).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            std::str::from_utf8(&payload).unwrap(),
            s(t, "payload"),
            "{name}"
        );
        assert_eq!(
            hex(&ticket::message(&payload)),
            s(t, "signed_hex"),
            "{name}"
        );
        let key = &v["keys"][s(t, "signed_by")];
        let kid = u32::try_from(key["kid"].as_u64().unwrap()).unwrap();
        assert_eq!(t["kid"], kid, "{name}");
        let signed = TicketKey::new(kid, &seed(key)).sign(&claims).unwrap();
        assert_eq!(signed, s(t, "ticket"), "{name}");
        assert_eq!(t["bytes"], signed.len() as u64, "{name}");
        made += 1;
    }
    assert_eq!(made, 14);
}

/// The kinds hjv-api hands out, by the constructors it uses, are the vectors' kinds.
#[test]
fn the_kinds_hjv_api_makes_are_the_vectors() {
    let v = vectors();
    let fields = |name: &str| {
        let t = v["tickets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"] == name)
            .unwrap_or_else(|| panic!("{name}"));
        claims_from(serde_json::from_value(t["fields"].clone()).unwrap()).unwrap()
    };
    let (sub, review_sub) = (
        s(&v["accounts"]["staff"], "sub"),
        s(&v["accounts"]["review"], "sub"),
    );
    let pc = <[u8; 32]>::try_from(b64_decode(s(&v["keys"]["pc"], "pk")).unwrap()).unwrap();
    let at = 1_760_000_000;
    let lifetimes = &v["lifetimes"];
    let secs = |kind: &str| lifetimes[kind].as_i64().unwrap();
    assert_eq!(
        Claims::staff(sub, None, at, secs("staff")),
        fields("staff-ctl")
    );
    assert_eq!(
        Claims::staff(sub, Some(("123456789", pc)), at, secs("staff")),
        fields("staff-both")
    );
    assert_eq!(
        Claims::staff(sub, Some(("123456789", pc)), at, secs("bound_pc")),
        fields("bound-pc")
    );
    assert_eq!(
        Claims::guest_ctl("123456789", at, secs("guest")),
        fields("guest-ctl")
    );
    assert_eq!(
        Claims::guest_pc("123456789", pc, at, secs("guest")),
        fields("guest-pc")
    );
    assert_eq!(
        Claims::review(
            review_sub,
            vec!["100200300".into(), "100200301".into()],
            at,
            at + secs("review")
        ),
        fields("review-ctl")
    );
}

/// hjv-api never signs what hbbs refuses as malformed, and signs every payload hbbs takes: for
/// each ticket of the vectors whose payload hbbs judges (its form read, its key known, its
/// signature good), the signer makes the same bytes exactly when hbbs's reason is not
/// `malformed`.
#[test]
fn the_signer_keeps_the_rules_hbbs_reads_by() {
    let v = vectors();
    let (mut signed, mut refused) = (0, 0);
    for t in v["tickets"].as_array().unwrap() {
        let (name, ticket) = (s(t, "name"), s(t, "ticket"));
        let expects: Vec<&str> = t["checks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| s(c, "expect"))
            .collect();
        // the size, the key and the signature come before the payload (4-1)
        let before = ["missing", "oversize", "unknown_kid", "bad_signature"];
        if ticket.len() > ticket::MAX_BYTES || expects.iter().any(|e| before.contains(e)) {
            continue;
        }
        let Some((_, payload, _)) = wire(ticket) else {
            assert!(
                expects.iter().all(|e| *e == "malformed"),
                "{name}: hbbs reads its form, this test does not"
            );
            continue;
        };
        let made = serde_json::from_slice::<Members>(&payload)
            .ok()
            .and_then(claims_from)
            .and_then(|c| ticket::payload(&c).ok());
        let signable = made.as_deref() == Some(payload.as_slice());
        assert_eq!(
            signable,
            !expects.contains(&"malformed"),
            "{name}: {}",
            String::from_utf8_lossy(&payload)
        );
        if signable {
            signed += 1;
        } else {
            refused += 1;
        }
    }
    // the 14 valid tickets and the expired, early and revoked ones; every payload rule of 2-3
    // and 2-4 that the vectors break
    assert_eq!((signed, refused), (17, 41));
}

/// README.md's Formats show the vector of a Windows app's request for a `both` ticket; the
/// signature is OpenSSL's of the same seed and bytes (Ed25519 signs the same way everywhere).
#[test]
fn the_readme_shows_the_request_vector() {
    let token = "hjv_AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    let seed = [2u8; 32];
    let pk = b64(&devsig::public_key(&seed));
    let message = ticket::pc_message("123456789", &pk, &sha256(token.as_bytes()));
    let sig = devsig::sign(&seed, &message);
    assert_eq!(
        sig,
        "nG4V0N+qAT9bPvurXKhwQG3b4vvYwN+nqee9l9iA6qoRUIRsICcBdsYnaj3FWb3L4p/NC1GGMV6awpRVZazoAg=="
    );
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    for value in [
        pk.clone(),
        hex(&message),
        sig,
        format!("signed ({})", message.len()),
        b64(&sha256(token.as_bytes())),
    ] {
        assert!(readme.contains(&value), "README.md does not show {value}");
    }
}

// ---- POST /api/hjv/ticket ----

/// A phone asks with `{}` or nothing and gets `staff`/`ctl`; a Windows app proves its device key
/// and gets `staff`/`both` for its own ID and key; each for a day, its account's tag in `sub`.
#[tokio::test]
async fn a_phone_gets_ctl_and_a_windows_app_both() {
    let h = Harness::start().await;
    let phone = h.login_ok(HONG, None).await;
    for body in [None, Some(json!({}))] {
        let a = ask(&h, &phone, body).await;
        assert_eq!(a.status, 200, "{}", a.text);
        let p = access(&a.json);
        assert_eq!(
            (&p["v"], &p["who"], &p["use"]),
            (&json!(1), &json!("staff"), &json!("ctl"))
        );
        assert_eq!(p["sub"], h.account_tag(HONG.0));
        assert_eq!(
            (&p["id"], &p["pk"], &p["to"]),
            (&json!(""), &json!(""), &json!([]))
        );
        assert_eq!(lifetime(&p), DAY);
        assert!((p["iat"].as_i64().unwrap() - now()).abs() <= 5);
    }
    // a Windows app that runs without installing: its login gave it no `both`, its request does
    let mut portable = TestDevice::windows("300000001");
    portable.installed = false;
    let token = h.login_ok(KIM, Some(&portable)).await;
    let a = ask(&h, &token, Some(pc_body(&portable, &token))).await;
    assert_eq!(a.status, 200, "{}", a.text);
    let p = access(&a.json);
    assert_eq!((&p["who"], &p["use"]), (&json!("staff"), &json!("both")));
    assert_eq!(p["id"], portable.id);
    assert_eq!(p["pk"], b64(&portable.public_key()));
    assert_eq!(p["sub"], h.account_tag(KIM.0));
    assert_eq!(p["to"], json!([]));
    // an ID the app took since its login is its own as well: the device key is what counts
    let renamed = TestDevice {
        id: "Lab-PC_2".into(),
        ..portable.clone()
    };
    let a = ask(&h, &token, Some(pc_body(&renamed, &token))).await;
    assert_eq!(access(&a.json)["id"], "Lab-PC_2");
    // HJV_API_TICKET_HOURS
    let h = Harness::with(Options {
        vars: vec![("HJV_API_TICKET_HOURS", "6".into())],
        ..Default::default()
    })
    .await;
    let phone = h.login_ok(HONG, None).await;
    let a = ask(&h, &phone, None).await;
    assert_eq!(lifetime(&access(&a.json)), 6 * 3600);
}

#[tokio::test]
async fn the_ticket_path_refuses() {
    let h = Harness::start().await;
    let mut pc = TestDevice::windows("300000002");
    pc.installed = false;
    let token = h.login_ok(HONG, Some(&pc)).await;
    let good = pc_body(&pc, &token);
    let bad_request = json!({"error": "요청을 이해하지 못했습니다.", "hjv": "bad_request"});
    // the PC part, whole and signed for this token, or nothing
    let other_token = h.login_ok(KIM, None).await;
    let mut bodies = vec![
        json!({"id": pc.id}),
        json!({"id": pc.id, "pk": good["pk"]}),
        json!({"pk": good["pk"], "sig": good["sig"]}),
        json!({"id": "300000003", "pk": good["pk"], "sig": good["sig"]}),
        json!({"id": "a b", "pk": good["pk"], "sig": good["sig"]}),
        json!({"id": pc.id, "pk": b64(&[1u8; 31]), "sig": good["sig"]}),
        json!({"id": pc.id, "pk": good["pk"], "sig": "AAAA"}),
        json!([]),
        json!("text"),
        // what serde alone would take for no PC part or for one
        json!({"x": 1}),
        json!({"id": null, "pk": null, "sig": null}),
        json!([null, null, null]),
        json!([pc.id, good["pk"], good["sig"]]),
        json!({"id": pc.id, "pk": good["pk"], "sig": good["sig"], "x": "1"}),
    ];
    // another token's request signed by the PC
    bodies.push(pc_body(&pc, &other_token));
    for body in bodies {
        let a = ask(&h, &token, Some(body.clone())).await;
        assert_eq!((a.status, &a.json), (400, &bad_request), "{body}");
    }
    // the token alone, without its key's proof: the 401 of a wrong token
    let auth = format!("Bearer {token}");
    let a = h
        .request(
            Method::POST,
            ticket::PATH,
            &[
                ("authorization", auth.as_str()),
                ("content-type", "application/json"),
            ],
            Some(b"{}".to_vec()),
        )
        .await;
    assert_eq!(a.status, 401, "{}", a.text);
    // POST only
    let a = h.get(ticket::PATH, &token).await;
    assert_eq!(a.status, 405, "{}", a.text);
    assert_eq!(a.headers["allow"], "POST");
    // the very same request again within the retry window: the first answer, not a second ticket
    let body = b"{}".to_vec();
    let proof = h
        .proof_for("POST", ticket::PATH, &body, &other_token)
        .unwrap();
    let auth = format!("Bearer {other_token}");
    let headers = [
        ("authorization", auth.as_str()),
        ("content-type", "application/json"),
        ("x-hjv-proof", proof.as_str()),
    ];
    let first = h
        .request(Method::POST, ticket::PATH, &headers, Some(body.clone()))
        .await;
    let again = h
        .request(Method::POST, ticket::PATH, &headers, Some(body))
        .await;
    assert_eq!(first.status, 200, "{}", first.text);
    assert_eq!(again.json["ticket"], first.json["ticket"]);
    // a blocked account gets nothing: its tokens end
    h.admin(
        &["block", &format!("tag:{}", h.account_tag(HONG.0))],
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(ask(&h, &token, None).await.status, 401);
    // 30 calls a minute of the account key, with its other account calls (keys.rs)
    let token = h.login_ok(LEE, None).await;
    for _ in 0..29 {
        assert_eq!(ask(&h, &token, None).await.status, 200);
    }
    assert_eq!(h.get("/api/hjv/keys", &token).await.status, 200);
    let a = ask(&h, &token, None).await;
    assert_eq!(a.status, 429, "{}", a.text);
    assert_eq!(a.json["hjv"], "rate_limited");
}

/// Without the credential hjviewer-api-ticket-key there are no tickets: the path answers 503, the
/// login's and the sync's answers carry none, and the operator's commands say so.
#[tokio::test]
async fn without_a_ticket_key_no_tickets() {
    let h = Harness::with(Options {
        no_ticket_key: true,
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("300000011");
    h.hbbs.register(&pc, Some(1));
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, Some(&pc)), "10.9.9.9")
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(a.json["hjv"].get("access").is_none(), "{}", a.text);
    let binding = s(&a.json["hjv"], "binding").to_owned();
    let a = ask(&h, &a.token(), None).await;
    assert_eq!(
        (a.status, &a.json["hjv"]),
        (503, &json!("unavailable")),
        "{}",
        a.text
    );
    let synced = h.sync(&pc, &payload(&binding)).await;
    synced.list();
    assert!(synced.answer.json.get("access").is_none());
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(
        status.starts_with("tickets: off (no credential hjviewer-api-ticket-key)"),
        "{status}"
    );
    // hbbs takes keys (HJV_API_TICKET_PUBKEYS) that nothing signs with
    let report = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(
        report.contains("[FAIL] ticket key: no credential hjviewer-api-ticket-key, while HJV_API_TICKET_PUBKEYS names hbbs's keys"),
        "{report}"
    );
    // neither the key nor the setting: tickets are off, and nothing fails
    let h = Harness::with(Options {
        no_ticket_key: true,
        vars: vec![("HJV_API_TICKET_PUBKEYS", String::new())],
        ..Default::default()
    })
    .await;
    let report = h.admin(&["selfcheck"], false, None).await.unwrap();
    assert!(report.contains("[ -- ] ticket key: none"), "{report}");
}

/// The App Review account gets a ticket only for the demo PCs of its file's PC= line, at its login
/// and on request: `review`/`ctl` for them in the file's order, until the earlier of a day and the
/// file's UNTIL; without the line, none.
#[tokio::test]
async fn the_review_account_gets_its_demo_pcs() {
    const HASH: &str =
        "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";
    let dir = temp_dir("ticket-review");
    let file = dir.join("review.conf");
    let put = |until: i64, pcs: &str| {
        let new = file.with_extension("new");
        std::fs::write(
            &new,
            format!(
                "ID=APPREVIEW-TEST\nHASH={HASH}\nUNTIL={}\n{pcs}",
                datetime(until)
            ),
        )
        .unwrap();
        std::fs::rename(&new, &file).unwrap();
    };
    put(now() + 2 * DAY, "");
    let h = Harness::with(Options {
        vars: vec![("HJV_API_REVIEW_FILE", file.display().to_string())],
        ..Default::default()
    })
    .await;
    let login = |ip: &'static str| {
        let h = &h;
        async move {
            let a = h
                .login(
                    &h.login_body("APPREVIEW-TEST", "correct horse battery staple", None),
                    ip,
                )
                .await;
            assert_eq!(a.status, 200, "{}", a.text);
            a
        }
    };
    // without the PC= line: no ticket, at the login or on request
    let a = login("10.0.61.1").await;
    assert!(a.json["hjv"].get("access").is_none(), "{}", a.text);
    let a = ask(&h, &a.token(), None).await;
    assert_eq!(a.status, 403, "{}", a.text);
    assert_eq!(
        a.json,
        json!({"error": "이 계정으로는 출입증을 받을 수 없습니다.", "hjv": "no_ticket"})
    );
    // the demo PCs in the file's order, a day
    put(now() + 2 * DAY, "PC=100200301,100200300\n");
    let a = login("10.0.61.2").await;
    let p = access(&a.json["hjv"]["access"]);
    assert_eq!(
        (&p["who"], &p["use"], &p["to"], &p["id"], &p["pk"]),
        (
            &json!("review"),
            &json!("ctl"),
            &json!(["100200301", "100200300"]),
            &json!(""),
            &json!("")
        )
    );
    assert_eq!(lifetime(&p), DAY);
    let asked = ask(&h, &a.token(), None).await;
    assert_eq!(asked.status, 200, "{}", asked.text);
    let q = access(&asked.json);
    assert_eq!((&q["who"], &q["to"]), (&p["who"], &p["to"]));
    // never past UNTIL
    let until = now() + 3600;
    put(until, "PC=100200300\n");
    let a = login("10.0.61.3").await;
    let p = access(&a.json["hjv"]["access"]);
    assert_eq!(
        (&p["to"], p["exp"].as_i64()),
        (&json!(["100200300"]), Some(until))
    );
    // a PC= line it cannot take: the whole file goes, and with it the account
    put(now() + 2 * DAY, "PC=100200300,100200300\n");
    let a = h
        .login(
            &h.login_body("APPREVIEW-TEST", "correct horse battery staple", None),
            "10.0.61.4",
        )
        .await;
    assert_ne!(a.status, 200, "{}", a.text);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- the login's answer ----

/// The login's answer carries the app's ticket in `hjv.access`: `both` for an installed PC whose
/// signature matched the key hbbs holds, `ctl` for a phone, a Windows app that runs without
/// installing, and a PC whose key hbbs does not confirm.
#[tokio::test]
async fn the_login_answer_carries_the_ticket() {
    let h = Harness::start().await;
    let login = |device: Option<TestDevice>| {
        let h = &h;
        async move {
            let a = h
                .login(&h.login_body(HONG.0, HONG.1, device.as_ref()), "10.9.9.10")
                .await;
            assert_eq!(a.status, 200, "{}", a.text);
            a
        }
    };
    let a = login(None).await;
    let p = access(&a.json["hjv"]["access"]);
    assert_eq!((&p["use"], &p["id"]), (&json!("ctl"), &json!("")));
    assert_eq!(p["sub"], h.account_tag(HONG.0));
    assert_eq!(lifetime(&p), DAY);
    // the installed PC that the login binds
    let pc = TestDevice::windows("300000021");
    h.hbbs.register(&pc, Some(1));
    let a = login(Some(pc.clone())).await;
    assert!(a.json["hjv"]["binding"].is_string(), "{}", a.text);
    let p = access(&a.json["hjv"]["access"]);
    assert_eq!((&p["who"], &p["use"]), (&json!("staff"), &json!("both")));
    assert_eq!(
        (&p["id"], &p["pk"]),
        (&json!(pc.id), &json!(b64(&pc.public_key())))
    );
    // not installed, or hbbs holds another key for the ID: ctl
    let mut portable = TestDevice::windows("300000022");
    portable.installed = false;
    h.hbbs.register(&portable, Some(1));
    let other = TestDevice::windows("300000023");
    h.hbbs
        .register_key(&other.id, devsig::public_key(&[0x77; 32]));
    for device in [portable, other] {
        let a = login(Some(device)).await;
        assert_eq!(access(&a.json["hjv"]["access"])["use"], "ctl", "{}", a.text);
    }
}

// ---- the PC sync ----

/// A bound PC's sync with its binding carries the PC's ticket, `staff`/`both` for its ID and key
/// for three days, each time anew, while its account may use HJViewer, whatever its password-less
/// switch; the other answers carry none.
#[tokio::test]
async fn a_bound_pc_gets_its_ticket_with_each_sync() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("300000031");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    // the switch is off when installed: a disabled list, and the ticket all the same
    let synced = h.sync(&pc, &payload(&binding)).await;
    assert_eq!(synced.list()["state"], "disabled");
    let p = access(&synced.answer.json["access"]);
    assert_eq!((&p["who"], &p["use"]), (&json!("staff"), &json!("both")));
    assert_eq!(
        (&p["id"], &p["pk"], &p["to"]),
        (&json!(pc.id), &json!(b64(&pc.public_key())), &json!([]))
    );
    assert_eq!(p["sub"], h.account_tag(HONG.0));
    assert_eq!(lifetime(&p), 3 * DAY);
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    let synced = h.sync(&pc, &payload(&binding)).await;
    assert_eq!(synced.list()["state"], "active");
    access(&synced.answer.json["access"]);
    // the repeat within ten seconds gets the last list again, and a ticket
    let again = h.sync(&pc, &payload(&binding)).await;
    again.list();
    access(&again.answer.json["access"]);
    // a sync without its binding is listed: no list, no ticket
    let listed = h.sync(&pc, &payload("")).await;
    assert_eq!(listed.answer.json["state"], "listed");
    assert!(listed.answer.json.get("access").is_none());
    // HJV_API_PC_TICKET_HOURS
    let short = Harness::with(Options {
        vars: vec![("HJV_API_PC_TICKET_HOURS", "2".into())],
        ..Default::default()
    })
    .await;
    short.hbbs.register(&pc, Some(1));
    short.login_ok(HONG, Some(&pc)).await;
    let synced = short
        .sync(&pc, &payload(&short.binding_of(&pc.id).await))
        .await;
    assert_eq!(lifetime(&access(&synced.answer.json["access"])), 2 * 3600);
    // a blocked account's PC: a disabled list without a ticket
    h.admin(
        &["block", &format!("tag:{}", h.account_tag(HONG.0))],
        false,
        None,
    )
    .await
    .unwrap();
    let synced = h.sync(&pc, &payload(&binding)).await;
    assert_eq!(synced.list()["state"], "disabled");
    assert!(synced.answer.json.get("access").is_none());
    h.admin(
        &["unblock", &format!("tag:{}", h.account_tag(HONG.0))],
        false,
        None,
    )
    .await
    .unwrap();
    access(&h.sync(&pc, &payload(&binding)).await.answer.json["access"]);
    // released: a signed revoked, without a ticket
    let token = h.login_ok(HONG, None).await;
    assert_eq!(
        h.delete(&format!("/api/hjv/devices/{}", pc.id), &token)
            .await
            .status,
        200
    );
    let revoked = h.sync(&pc, &payload(&binding)).await;
    assert_eq!(revoked.list()["state"], "revoked");
    assert!(revoked.answer.json.get("access").is_none());
}

// ---- what the operator sees ----

#[tokio::test]
async fn ticket_status_and_selfcheck() {
    let h = Harness::start().await;
    let fp = |seed: [u8; 32], kid: u32| trust::fingerprint(kid, &devsig::public_key(&seed));
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(ask(&h, &phone, None).await.status, 200);
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(
        status.starts_with(&format!(
            "tickets: on, key 1 ({}), the key 1 of HJV_API_TICKET_PUBKEYS\n",
            fp(TICKET_SEED, 1)
        )),
        "{status}"
    );
    assert!(
        status.contains(&format!(
            "hbbs's access.conf takes it as KEY={}\n",
            trust::entry(1, &devsig::public_key(&TICKET_SEED))
        )),
        "{status}"
    );
    assert!(
        status.contains("lifetimes: logged-in apps 24 h (HJV_API_TICKET_HOURS), bound PCs 72 h"),
        "{status}"
    );
    assert!(status.contains("login ctl 1, request ctl 1"), "{status}");
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(record.contains(r#"cmd="ticket status""#), "{record}");
    let report = h.admin(&["selfcheck"], false, None).await.unwrap();
    assert!(
        report.contains(&format!(
            "[ OK ] ticket keys hbbs takes (HJV_API_TICKET_PUBKEYS, its access.conf's KEY= lines), SHA-256: {}, {}",
            fp(TICKET_SEED, 1),
            fp(NEXT_TICKET_SEED, 2)
        )),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "[ OK ] ticket key in use (HJV_API_TICKET_KID 1): the key 1 hbbs takes, {}",
            fp(TICKET_SEED, 1)
        )),
        "{report}"
    );
    // the key in use under another number than hbbs has it: every ticket would be refused
    let h = Harness::with(Options {
        vars: vec![("HJV_API_TICKET_KID", "2".into())],
        ..Default::default()
    })
    .await;
    let report = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(
        report.contains(&format!(
            "[FAIL] ticket key in use (HJV_API_TICKET_KID 2, {}) is not the key 2 of HJV_API_TICKET_PUBKEYS",
            fp(TICKET_SEED, 2)
        )),
        "{report}"
    );
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(
        status.contains("NOT the key 2 of HJV_API_TICKET_PUBKEYS"),
        "{status}"
    );
    assert!(!report.contains(&b64(&TICKET_SEED)) && !status.contains(&b64(&TICKET_SEED)));
    // outside the harness the vectors' keys are public test keys, which hbbs must never take
    let h = Harness::with(Options {
        refuse_test_keys: true,
        ..Default::default()
    })
    .await;
    let report = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    for kid in [1, 2] {
        assert!(
            report.contains(&format!(
                "[FAIL] ticket key {kid} of HJV_API_TICKET_PUBKEYS is a public test key, whose private key is in the repository: with it in hbbs's access.conf anyone can sign tickets"
            )),
            "{report}"
        );
    }
}

/// No ticket goes to the journal or the records: they are bearer values for their lifetime.
#[tokio::test]
async fn no_ticket_is_logged_or_recorded() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("300000041");
    h.hbbs.register(&pc, Some(1));
    let login = h
        .login(&h.login_body(HONG.0, HONG.1, Some(&pc)), "10.9.9.11")
        .await;
    let token = login.token();
    let binding = s(&login.json["hjv"], "binding").to_owned();
    let mut tickets = vec![s(&login.json["hjv"]["access"], "ticket").to_owned()];
    tickets.push(
        s(
            &ask(&h, &token, Some(pc_body(&pc, &token))).await.json,
            "ticket",
        )
        .to_owned(),
    );
    let synced = h.sync(&pc, &payload(&binding)).await;
    tickets.push(s(&synced.answer.json["access"], "ticket").to_owned());
    // a refused request is journalled by its reason only
    assert_eq!(
        ask(&h, &token, Some(json!({"id": pc.id}))).await.status,
        400
    );
    let lines = hjv_api::log::captured();
    let records = h.results().await;
    for t in &tickets {
        let payload_part = t.split('.').nth(2).unwrap();
        for line in lines.iter().chain(records.iter()) {
            assert!(!line.contains(payload_part), "{line}");
        }
    }
    assert!(
        lines.iter().any(|l| l
            .starts_with("ticket: a request not of the form (id, pk and sig go together; acct=")),
        "{lines:?}"
    );
}
