// SPDX-License-Identifier: AGPL-3.0-only

//! The PC sync and the trust lists (design v3 §3-6, §4-3 to §4-7, §5-2 to §5-6, WP-S2): the
//! checks in their order, the answers by row and binding, the lists' contents and their
//! signatures as the PC checks them, the ten-second repeat, the key use reports (U6), the device
//! list's states, the six ways a PC leaves its account and their tombstones, the key change that
//! hbbs confirms, the report and selfcheck, and the shared test vectors of the client.

mod common;

use common::*;
use hjv_api::{
    db, devsig,
    envelope::Envelope,
    jobs, pcsync,
    trust::{self, ListEntry, ListKey, State, TrustList},
    util::{b64, b64_decode, date, datetime, hex, now, random_bytes, DAY},
};
use hyper::Method;
use serde_json::{json, Value};
use std::path::Path;

/// A PC of `account`'s, bound by a login on it, with password-less connections switched on for
/// the account (as `passwordless on --owner-of`): its binding ID and the token of that login.
async fn bound_to(h: &Harness, account: (&str, &str), pc: &TestDevice) -> (String, String) {
    h.hbbs.register(pc, Some(1));
    let token = h.login_ok(account, Some(pc)).await;
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    let binding = h.binding_of(&pc.id).await;
    assert_eq!(binding.len(), 32);
    (binding, token)
}

async fn bound(h: &Harness, pc: &TestDevice) -> (String, String) {
    bound_to(h, HONG, pc).await
}

async fn device(h: &Harness, id: &str) -> Option<db::Device> {
    let id = id.to_owned();
    h.state.db.call(move |c| db::device(c, &id)).await.unwrap()
}

/// The tombstones of a PC ID: (binding in hex, reason).
async fn tombstones(h: &Harness, id: &str) -> Vec<(String, String)> {
    let id = id.to_owned();
    h.state
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT binding_id, reason FROM binding_tombstones WHERE device_id = ?1 ORDER BY rowid",
            )?;
            let rows = stmt.query_map([id], |r| Ok((hex(&r.get::<_, Vec<u8>>(0)?), r.get(1)?)))?;
            rows.collect()
        })
        .await
        .unwrap()
}

async fn sql(h: &Harness, statement: &'static str) {
    h.state
        .db
        .call(move |c| c.execute_batch(statement))
        .await
        .unwrap();
}

fn key_b64(h: &Harness, token: &str) -> String {
    b64(&devsig::public_key(&h.key_of(token).unwrap()))
}

/// What the error answers of the sync are: the code and the server's time.
fn refused(s: &Synced, status: u16, code: &str) {
    assert_eq!(s.answer.status, status, "{}", s.answer.text);
    assert_eq!(s.answer.json["error"], code, "{}", s.answer.text);
    assert!(s.answer.json["server_time"].is_i64(), "{}", s.answer.text);
    assert!(s.list.is_none());
}

#[tokio::test]
async fn the_checks_in_their_order() {
    let h = Harness::start().await;
    // an ID that no account has or had: 404 unbound, without a list
    let stranger = TestDevice::windows("500000009");
    let s = h.sync(&stranger, &payload("")).await;
    refused(&s, 404, "unbound");
    assert_eq!(s.answer.json["state"], "unbound");

    let pc = TestDevice::windows("500000001");
    let (binding, pc_token) = bound(&h, &pc).await;
    let p = payload(&binding).to_string();
    let body = |t: i64| sync_body(&pc, t, &random_bytes::<16>(), &p).into_bytes();
    let raw = |a: Answer| Synced {
        answer: a,
        n: String::new(),
        list: None,
    };
    // 1. 16 KiB at most
    let mut big = body(now());
    big.resize(pcsync::MAX_BODY + 1, b' ');
    refused(&raw(h.sync_raw(big).await), 413, "too_large");
    // 2. the shape, a JSON body
    let a = h
        .request(
            Method::POST,
            pcsync::PATH,
            &[("content-type", "text/plain")],
            Some(body(now())),
        )
        .await;
    refused(&raw(a), 400, "bad_request");
    for bad in ["{}", "[]", "not json", r#"{"id":"500000001","t":1}"#] {
        refused(
            &raw(h.sync_raw(bad.as_bytes().to_vec()).await),
            400,
            "bad_request",
        );
    }
    // 4. the signature of another key: 401, and the request changes nothing
    let impostor = TestDevice {
        seed: [66; 32],
        ..pc.clone()
    };
    refused(
        &h.sync(&impostor, &payload(&binding)).await,
        401,
        "bad_signature",
    );
    // 5. 300 seconds either way of the server's clock
    for t in [now() - 400, now() + 400] {
        let s = raw(h.sync_raw(body(t)).await);
        refused(&s, 400, "clock");
        assert!((s.answer.json["server_time"].as_i64().unwrap() - now()).abs() <= 5);
    }
    assert_eq!(device(&h, &pc.id).await.unwrap().last_sync_at, None);
    // 6. a request taken once
    let once = body(now());
    assert_eq!(h.sync_raw(once.clone()).await.status, 200);
    refused(&raw(h.sync_raw(once).await), 409, "replay");
    // 7. the payload
    for bad in [
        json!({"v": 2, "binding": binding}),
        json!({"v": 1}),
        json!({"v": 1, "binding": binding.to_uppercase()}),
        json!({"v": 1, "binding": binding, "off": "no"}),
    ] {
        refused(&h.sync(&pc, &bad).await, 400, "bad_request");
    }
    // the method, a browser's request
    let a = h.request(Method::GET, pcsync::PATH, &[], None).await;
    assert_eq!(a.status, 405);
    assert_eq!(a.headers["allow"], "POST");
    let a = h
        .request(
            Method::POST,
            pcsync::PATH,
            &[
                ("content-type", "application/json"),
                ("origin", "https://example.com"),
            ],
            Some(body(now())),
        )
        .await;
    refused(&raw(a), 403, "forbidden");

    // listed: the PC sent no binding (it let it go), or one that is not its binding
    let s = h.sync(&pc, &payload("")).await;
    assert_eq!(
        (s.answer.status, &s.answer.json["state"]),
        (200, &json!("listed"))
    );
    assert!(s.list.is_none() && s.answer.json["server_time"].is_i64());
    let this = h.get("/api/hjv/this-device", &pc_token).await.json;
    assert_eq!(
        (&this["binding"], &this["pwless"], &this["activation"]),
        (&json!(binding), &json!("listed"), &json!("absent"))
    );
    let s = h.sync(&pc, &payload(&hex(&random_bytes::<16>()))).await;
    assert_eq!(s.answer.json["state"], "listed");
    assert!(s.list.is_none());
    let this = h.get("/api/hjv/this-device", &pc_token).await.json;
    assert_eq!(this["activation"], "stale");
    // active: its binding, with the account's keys
    let s = h.sync(&pc, &payload(&binding)).await;
    let list = s.list();
    assert_eq!(
        (list["state"].clone(), s.answer.json["state"].clone()),
        (json!("active"), json!("active"))
    );
    assert_eq!(s.keys(), vec![key_b64(&h, &pc_token)]);
    assert_eq!(s.answer.json["send_wol"], false);
    assert_eq!(s.answer.json["refresh"], 120);
    let this = h.get("/api/hjv/this-device", &pc_token).await.json;
    assert_eq!(
        (&this["pwless"], &this["activation"]),
        (&json!("active"), &json!("present"))
    );
    // disabled: the account's switch is off
    h.admin(&["passwordless", "off", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    let list = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(
        (list["state"].clone(), list["keys"].clone()),
        (json!("disabled"), json!([]))
    );
    // and while the account is off the allowlist, its switch on
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    assert_eq!(
        h.sync(&pc, &payload(&binding)).await.list()["state"],
        "active"
    );
    h.admin(&["allow", "remove"], false, Some("2026000001\n"))
        .await
        .unwrap();
    let list = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(
        (list["state"].clone(), list["keys"].clone()),
        (json!("disabled"), json!([]))
    );
    // the owner and the login time stay for the PC's notice
    assert_eq!(list["owner"], "홍*동");
}

#[tokio::test]
async fn a_sync_counts_as_the_pc_seen() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("500000002");
    let (binding, _) = bound(&h, &pc).await;
    // not seen for 179 days, but it syncs: the upkeep keeps it
    sql(
        &h,
        "UPDATE devices SET last_seen_at = last_seen_at - 179 * 86400 WHERE device_id = '500000002'",
    )
    .await;
    assert_eq!(h.sync(&pc, &payload(&binding)).await.answer.status, 200);
    let d = device(&h, &pc.id).await.unwrap();
    assert!(now() - d.last_seen_at <= 5);
    assert_eq!(d.last_sync_at, Some(d.last_seen_at));
    assert_eq!(d.app_ver.as_deref(), Some("1.6.0"));
    let purged = jobs::purge_now(&h.state).await.unwrap();
    assert_eq!(purged.devices, 0);
    assert!(device(&h, &pc.id).await.is_some());
}

#[tokio::test]
async fn a_repeat_within_ten_seconds_is_signed_again() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("510000001");
    let (binding, _) = bound(&h, &pc).await;
    let first = h.sync(&pc, &payload(&binding)).await.list();
    // the owner's name changes without the change serial: a repeat does not read the list again
    sql(
        &h,
        "UPDATE accounts SET display_name = '김철수' WHERE login_key = '2026000001'",
    )
    .await;
    let again = h.sync(&pc, &payload(&binding)).await;
    let second = again.list();
    assert_eq!(second["req"], again.n.as_str());
    assert_ne!(second["req"], first["req"]);
    for field in ["serial", "iat", "exp", "owner", "login_at", "keys", "state"] {
        assert_eq!(second[field], first[field], "{field}");
    }
    assert_eq!(second["owner"], "홍*동");
    // no 429: a PC may ask as often as it likes
    for _ in 0..30 {
        let s = h.sync(&pc, &payload(&binding)).await;
        assert_eq!(s.answer.status, 200, "{}", s.answer.text);
        assert!(matches!(s.list, Some(Ok(_))));
    }
    // a new key moves the change serial: a new list, with it
    let phone = h.login_ok(HONG, None).await;
    let third = h.sync(&pc, &payload(&binding)).await;
    let list = third.list();
    assert!(list["serial"].as_i64() > first["serial"].as_i64());
    assert!(third.keys().contains(&key_b64(&h, &phone)));
    assert_eq!(third.keys().len(), 2);
}

#[tokio::test]
async fn the_list_says_what_the_design_says() {
    let h = Harness::with(Options {
        vars: vec![
            ("HJV_API_PASSWORDLESS_KINDS", "remote,file".into()),
            ("HJV_API_TRUST_TTL", "3600".into()),
            ("HJV_API_TRUST_REFRESH", "60".into()),
        ],
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("520000001");
    let (binding, pc_token) = bound(&h, &pc).await;
    let phone_device = TestDevice::phone("987650001");
    let phone = h.login_ok(HONG, Some(&phone_device)).await;
    let other = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    let s = h.sync(&pc, &payload(&binding)).await;
    let list = s.list();
    // the fields in their order, as the design writes them
    let text = String::from_utf8(b64_decode(s.answer.json["list"]["l"].as_str().unwrap()).unwrap())
        .unwrap();
    assert!(
        text.starts_with(&format!(
            r#"{{"v":1,"typ":"hjv-trust","kid":1,"binding":"{binding}","pc_id":"520000001","pc_pk":"{}","req":"{}","serial":"#,
            b64(&pc.public_key()),
            s.n
        )),
        "{text}"
    );
    assert_eq!(s.answer.json["list"]["kid"], 1);
    assert_eq!(
        list["exp"].as_i64().unwrap() - list["iat"].as_i64().unwrap(),
        3600
    );
    assert!((list["iat"].as_i64().unwrap() - now()).abs() <= 5);
    assert_eq!(
        (list["refresh"].clone(), s.answer.json["refresh"].clone()),
        (json!(60), json!(60))
    );
    assert_eq!(list["strict"], false);
    assert_eq!(list["owner"], "홍*동");
    let d = device(&h, &pc.id).await.unwrap();
    assert_eq!(list["login_at"], d.login_at.unwrap());
    assert_eq!(list["serial"], h.change_serial().await);
    // the account's keys that are not revoked, Kim's never
    let keys = h.state.db.call(|c| db::all_keys(c)).await.unwrap();
    let hong: Vec<&db::Key> = keys
        .iter()
        .filter(|k| k.account_id == d.account_id)
        .rev()
        .collect();
    let expected: Vec<Value> = hong
        .iter()
        .map(|k| {
            json!({"pk": b64(&k.public_key), "id": k.ctl_id, "name": k.model, "platform": k.platform,
                   "kinds": ["remote", "file"], "ks": k.key_serial})
        })
        .collect();
    assert_eq!(list["keys"], json!(expected));
    assert_eq!(hong.len(), 3);
    assert!(!s.keys().contains(&key_b64(&h, &kim)));
    let phone_entry = list["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["pk"] == key_b64(&h, &phone).as_str())
        .unwrap()
        .clone();
    assert_eq!(
        (
            &phone_entry["id"],
            &phone_entry["name"],
            &phone_entry["platform"]
        ),
        (&json!("987650001"), &json!("iPhone 14 Pro"), &json!("ios"))
    );
    // a key revoked is gone from the next list, which has a higher serial
    let ckid = h.get("/api/hjv/keys", &other).await.json["data"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["current"] == true)
        .unwrap()["ckid"]
        .as_i64()
        .unwrap();
    assert_eq!(
        h.delete(&format!("/api/hjv/keys/{ckid}"), &phone)
            .await
            .status,
        200
    );
    let next = h.sync(&pc, &payload(&binding)).await;
    assert!(!next.keys().contains(&key_b64(&h, &other)));
    assert_eq!(next.keys().len(), 2);
    assert!(next.list()["serial"].as_i64() > list["serial"].as_i64());
    // strict (incidents): the PCs ask at each connection
    h.admin(&["strict", "on"], false, None).await.unwrap();
    assert_eq!(h.sync(&pc, &payload(&binding)).await.list()["strict"], true);
    // login_at follows the last login on the PC
    sql(
        &h,
        "UPDATE devices SET login_at = login_at - 100 WHERE device_id = '520000001'",
    )
    .await;
    h.admin(&["strict", "off"], false, None).await.unwrap();
    let older = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(older["login_at"], d.login_at.unwrap() - 100);
    let again = h.login_ok(HONG, Some(&pc)).await;
    let newer = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(
        newer["login_at"],
        device(&h, &pc.id).await.unwrap().login_at.unwrap()
    );
    assert!(newer["login_at"].as_i64() >= d.login_at);
    assert_eq!(
        h.binding_of(&pc.id).await,
        binding,
        "the same account keeps the binding"
    );
    assert!(newer["keys"]
        .as_array()
        .unwrap()
        .iter()
        .any(|k| k["pk"] == key_b64(&h, &again).as_str()));
    assert!(
        !newer["keys"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["pk"] == key_b64(&h, &pc_token).as_str()),
        "the PC's older key of the same device is revoked"
    );
}

#[tokio::test]
async fn key_use_is_merged_by_the_larger_values_and_shown() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("530000001");
    let (binding, _) = bound(&h, &pc).await;
    let phone = h
        .login_ok(HONG, Some(&TestDevice::phone("987650011")))
        .await;
    let pk = key_b64(&h, &phone);
    let kim = h.login_ok(KIM, None).await;
    let at = now();
    let (today, yesterday) = (date(at), date(at - DAY));
    let ys = (at.div_euclid(DAY) - 1) * DAY;
    let line = |pk: &str, day: &str, last: i64, n: i64| json!({"pk": pk, "day": day, "last": last, "n": n});
    let report = |lines: Vec<Value>| {
        let mut p = payload(&binding);
        p["used"] = json!(lines);
        p
    };
    let uses = |token: String| {
        let h = &h;
        async move {
            let keys = h.get("/api/hjv/keys", &token).await.json;
            keys["data"]
                .as_array()
                .unwrap()
                .iter()
                .find(|k| k["current"] == true)
                .unwrap()
                .clone()
        }
    };
    let first = report(vec![
        line(&pk, &yesterday, ys + 300, 3),
        line(&pk, &today, at, 2),
        // Kim's key on Hong's PC: no key of the PC's account, not counted
        line(&key_b64(&h, &kim), &today, at, 9),
    ]);
    assert_eq!(h.sync(&pc, &first).await.answer.status, 200);
    let mine = uses(phone.clone()).await;
    let shown = json!([{"pc_id": "530000001", "pc": "PC-530000001", "last": at, "n30": 5}]);
    assert_eq!(mine["use"], shown);
    assert_eq!(
        (&mine["last_used"], &mine["last_used_pc"]),
        (&json!(at), &json!("PC-530000001"))
    );
    assert_eq!(uses(kim.clone()).await["use"], json!([]));
    // the same report again, an earlier time and a smaller count: nothing goes down
    assert_eq!(h.sync(&pc, &first).await.answer.status, 200);
    let lower = report(vec![
        line(&pk, &yesterday, ys + 100, 1),
        line(&pk, &today, at - 1, 1),
    ]);
    assert_eq!(h.sync(&pc, &lower).await.answer.status, 200);
    assert_eq!(uses(phone.clone()).await["use"], shown);
    // more uses later the same day go up
    let more = report(vec![line(&pk, &yesterday, ys + 400, 5)]);
    assert_eq!(h.sync(&pc, &more).await.answer.status, 200);
    assert_eq!(
        uses(phone.clone()).await["use"],
        json!([{"pc_id": "530000001", "pc": "PC-530000001", "last": at, "n30": 7}])
    );
    // a PC's report is merged whatever binding it carried, for keys of its account only
    let mut listed = payload("");
    listed["used"] = json!([line(&pk, &today, at, 3)]);
    assert_eq!(h.sync(&pc, &listed).await.answer.json["state"], "listed");
    assert_eq!(uses(phone.clone()).await["use"][0]["n30"], 8);
    // the admin's key list: per PC, the host name with --reveal only
    let tag = h.state.tags.device("530000001");
    let out = h.admin(&["key", "list"], false, None).await.unwrap();
    assert!(
        out.contains(&format!(
            "used on PC {tag}  5300*****     last {}  30 days 8",
            datetime(at)
        )),
        "{out}"
    );
    assert!(
        !out.contains("PC-530000001") && !out.contains("530000001 "),
        "{out}"
    );
    let shown = h.admin(&["key", "list"], true, None).await.unwrap();
    assert!(shown.contains("30 days 8  PC-530000001"), "{shown}");
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("password-less connections of the last 30 days: 8"),
        "{report}"
    );
    // the refusals add up, per server day
    let mut p = payload(&binding);
    p["refused"]["off"] = json!(2);
    p["refused"]["not_active"] = json!(1);
    h.sync(&pc, &p).await;
    h.sync(&pc, &p).await;
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("refused proofs of the last 30 days: not_active 2, off 4"),
        "{report}"
    );
    // the PC leaves the account: the key's trace stays, without the PC's name
    assert_eq!(
        h.delete("/api/hjv/devices/530000001", &phone).await.status,
        200
    );
    assert_eq!(
        uses(phone.clone()).await["use"],
        json!([{"pc_id": "530000001", "pc": "", "last": at, "n30": 8}])
    );
}

#[tokio::test]
async fn the_device_list_follows_the_pcs() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("540000001");
    let (binding, pc_token) = bound(&h, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let card = || {
        let h = &h;
        let phone = phone.clone();
        async move {
            let p = h.get("/api/peers", &phone).await.json["data"][0].clone();
            (
                p["hjv"]["pwless"].as_str().unwrap().to_owned(),
                p["note"].as_str().unwrap().to_owned(),
            )
        }
    };
    let state = |s: &str, note: &str| (s.to_owned(), note.to_owned());
    // until its first sync the PC has not confirmed its binding
    assert_eq!(card().await, state("listed", "이 PC에서 로그인 필요"));
    h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(card().await, state("active", "비밀번호 없이 연결"));
    // switched off on the PC
    let mut off = payload(&binding);
    off["off"] = json!(true);
    h.sync(&pc, &off).await;
    assert_eq!(card().await, state("off", "PC에서 꺼 둠"));
    h.sync(&pc, &payload(&binding)).await;
    assert_eq!(card().await.0, "active");
    // not heard from for longer than a list lives
    sql(
        &h,
        "UPDATE devices SET last_sync_at = last_sync_at - 3 * 86400 - 10",
    )
    .await;
    assert_eq!(card().await, state("expired", "마지막 확인 3일 전"));
    h.sync(&pc, &payload(&binding)).await;
    // the administrator's switch
    h.admin(&["passwordless", "off", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    assert_eq!(
        card().await,
        state("disabled", "비밀번호 없는 연결 일시 중지(관리자)")
    );
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    // it let its binding go (a logout on it that did not reach the server)
    h.sync(&pc, &payload("")).await;
    assert_eq!(card().await, state("listed", "이 PC에서 로그인 필요"));
    h.sync(&pc, &payload(&binding)).await;
    assert_eq!(card().await.0, "active");
    // this-device: the PC's own login sees its binding; another device none
    let this = h.get("/api/hjv/this-device", &pc_token).await.json;
    assert_eq!(
        (&this["binding"], &this["pwless"], &this["activation"]),
        (&json!(binding), &json!("active"), &json!("present"))
    );
    let theirs = h.get("/api/hjv/this-device", &phone).await.json;
    assert!(theirs.get("binding").is_none() && theirs.get("pwless").is_none());
    // what each way back changes in the next list (design v3 §4-7): the other keys revoked
    let two = h.login_ok(HONG, None).await;
    assert_eq!(h.sync(&pc, &payload(&binding)).await.keys().len(), 3);
    let a = h.post("/api/hjv/keys/revoke-others", &phone, None).await;
    assert_eq!(a.json["revoked"], 2, "{}", a.text);
    let s = h.sync(&pc, &payload(&binding)).await;
    assert_eq!(s.keys(), vec![key_b64(&h, &phone)]);
    assert_eq!(h.get("/api/users", &two).await.status, 401);
    // the account off the allowlist: no keys, the PC stays bound
    h.admin(&["allow", "remove"], false, Some("2026000001\n"))
        .await
        .unwrap();
    let list = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(
        (list["state"].clone(), list["keys"].clone()),
        (json!("disabled"), json!([]))
    );
    assert!(device(&h, &pc.id).await.is_some());
}

/// After one of the ways out: the PC is in no account, its binding is a tombstone with the way's
/// reason, the change serial went up, and the PC's next sync with that binding gets a signed
/// `revoked` (design v3 §4-3).
async fn released(h: &Harness, pc: &TestDevice, binding: &str, before: i64, reason: &str) {
    assert!(device(h, &pc.id).await.is_none(), "{reason}: still bound");
    let stones = tombstones(h, &pc.id).await;
    assert!(
        stones.contains(&(binding.to_owned(), reason.to_owned())),
        "{reason}: {stones:?}"
    );
    assert!(h.change_serial().await > before, "{reason}");
    let s = h.sync(pc, &payload(binding)).await;
    let list = s.list();
    assert_eq!(
        (&list["state"], &list["keys"], &list["binding"]),
        (&json!("revoked"), &json!([]), &json!(binding)),
        "{reason}"
    );
    assert_eq!(s.answer.json["state"], "revoked");
    // without the binding, or with another: unsigned 404
    refused(&h.sync(pc, &payload("")).await, 404, "unbound");
    refused(
        &h.sync(pc, &payload(&hex(&random_bytes::<16>()))).await,
        404,
        "unbound",
    );
}

#[tokio::test]
async fn every_way_out_leaves_a_tombstone() {
    let h = Harness::start().await;
    let phone = h.login_ok(HONG, None).await;
    // 1. the signed logout on the PC
    let pc = TestDevice::windows("550000001");
    let (binding, token) = bound(&h, &pc).await;
    h.sync(&pc, &payload(&binding)).await.list();
    let before = h.change_serial().await;
    let a = h
        .post("/api/logout", &token, Some(json!({"id": pc.id})))
        .await;
    assert_eq!(a.status, 200);
    released(&h, &pc, &binding, before, "logout").await;
    // 2. undo of the login that added it
    let pc = TestDevice::windows("550000002");
    let (binding, token) = bound(&h, &pc).await;
    let before = h.change_serial().await;
    let a = h.post("/api/hjv/this-device/undo", &token, None).await;
    assert_eq!(a.json["status"], "removed", "{}", a.text);
    released(&h, &pc, &binding, before, "undo").await;
    // 3. "내 목록에서 빼기" from another device
    let pc = TestDevice::windows("550000003");
    let (binding, _) = bound(&h, &pc).await;
    let before = h.change_serial().await;
    assert_eq!(
        h.delete("/api/hjv/devices/550000003", &phone).await.status,
        200
    );
    released(&h, &pc, &binding, before, "remove").await;
    // 4. the administrator: binding revoke, and its other name device remove
    for (id, words) in [
        ("550000004", ["binding", "revoke"]),
        ("550000005", ["device", "remove"]),
    ] {
        let pc = TestDevice::windows(id);
        let (binding, _) = bound(&h, &pc).await;
        let before = h.change_serial().await;
        let out = h
            .admin(&[words[0], words[1], id], false, None)
            .await
            .unwrap();
        assert!(out.starts_with("released the PC from its account"), "{out}");
        released(&h, &pc, &binding, before, "admin").await;
    }
    // 5. an account deleted: by the administrator, and by the upkeep (a year unused, off the
    // allowlist), its PCs released first
    let pc = TestDevice::windows("550000006");
    let (binding, _) = bound_to(&h, KIM, &pc).await;
    let before = h.change_serial().await;
    h.admin(&["user", "delete", KIM.0], false, None)
        .await
        .unwrap();
    released(&h, &pc, &binding, before, "account-delete").await;
    let pc = TestDevice::windows("550000007");
    let (binding, _) = bound_to(&h, LEE, &pc).await;
    h.admin(&["allow", "remove"], false, Some("Y9001\n"))
        .await
        .unwrap();
    sql(&h, "UPDATE accounts SET last_active_at = last_active_at - 400 * 86400 WHERE login_key = 'y9001'").await;
    let before = h.change_serial().await;
    let purged = jobs::purge_now(&h.state).await.unwrap();
    assert_eq!((purged.accounts, purged.devices), (1, 0));
    released(&h, &pc, &binding, before, "account-delete").await;
    // 6. not seen for 180 days
    let pc = TestDevice::windows("550000008");
    let (binding, _) = bound(&h, &pc).await;
    sql(&h, "UPDATE devices SET last_seen_at = last_seen_at - 181 * 86400 WHERE device_id = '550000008'").await;
    let before = h.change_serial().await;
    let purged = jobs::purge_now(&h.state).await.unwrap();
    assert_eq!(purged.devices, 1);
    released(&h, &pc, &binding, before, "unseen-purge").await;
    // a PC taken out and bound again: its old binding gets a signed revoked, its new one a list
    let pc = TestDevice::windows("550000001");
    let old = tombstones(&h, &pc.id).await[0].0.clone();
    let (binding, _) = bound(&h, &pc).await;
    assert_ne!(binding, old);
    let s = h.sync(&pc, &payload(&old)).await;
    assert_eq!(s.list()["state"], "revoked");
    assert_eq!(
        h.sync(&pc, &payload(&binding)).await.list()["state"],
        "active"
    );
    assert_eq!(
        device(&h, &pc.id).await.unwrap().sync_state.as_deref(),
        Some("present")
    );
    // the report counts the tombstones by the way out
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("released bindings kept 180 days: account-delete 2, admin 2, logout 1, remove 1, undo 1, unseen-purge 1"),
        "{report}"
    );
    let bindings = h.admin(&["binding", "list"], false, None).await.unwrap();
    assert!(
        bindings.contains("released bindings kept 180 days: account-delete 2, admin 2"),
        "{bindings}"
    );
    // the record of each way out
    let results = h.results().await;
    for r in [
        "device:removed-logout",
        "device:removed-undo",
        "device:removed-by-user",
    ] {
        assert!(results.contains(&r.to_owned()), "{r}: {results:?}");
    }
}

/// The SQL that deletes a binding or an account appears in one function each: no path can take a
/// PC out of an account without the tombstone (design v3 §4-3, WP-S2 test "소스 검사").
#[test]
fn only_release_binding_deletes_a_binding() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![];
    let mut dirs = vec![src];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                files.push(path);
            }
        }
    }
    let mut found: Vec<(String, String, String)> = vec![];
    for file in &files {
        // SQL may break its lines anywhere: the text with its runs of white space as one space
        let text = std::fs::read_to_string(file).unwrap();
        let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
        let upper = flat.to_uppercase();
        for table in ["DEVICES", "ACCOUNTS"] {
            let needle = format!("DELETE FROM {table}");
            for (at, _) in upper.match_indices(&needle) {
                // the delete of the whole table's rows, not of a table named devices_x
                let rest = &upper[at + needle.len()..];
                if rest.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                    continue;
                }
                let function = flat[..at]
                    .rmatch_indices("fn ")
                    .next()
                    .map(|(i, _)| {
                        flat[i + 3..]
                            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                            .next()
                            .unwrap()
                            .to_owned()
                    })
                    .unwrap_or_default();
                found.push((
                    file.file_name().unwrap().to_string_lossy().into_owned(),
                    table.to_owned(),
                    function,
                ));
            }
        }
    }
    found.sort();
    assert_eq!(
        found,
        vec![
            (
                "db.rs".to_owned(),
                "ACCOUNTS".to_owned(),
                "delete_account".to_owned()
            ),
            (
                "db.rs".to_owned(),
                "DEVICES".to_owned(),
                "release_binding".to_owned()
            ),
        ]
    );
    // and nothing else of the schema deletes devices behind its back
    let db =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/db.rs")).unwrap();
    assert_eq!(
        db.matches("REFERENCES devices(device_id) ON DELETE CASCADE")
            .count(),
        1
    );
}

#[tokio::test]
async fn a_changed_key_is_asked_of_hbbs() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("560000001");
    let (binding, _) = bound(&h, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    h.sync(&pc, &payload(&binding)).await.list();
    let card = || {
        let h = &h;
        let phone = phone.clone();
        async move { h.get("/api/peers", &phone).await.json["data"][0].clone() }
    };
    // the PC lost its settings: the same ID, a new key pair, which hbbs now holds
    let reinstalled = TestDevice {
        seed: [77; 32],
        ..pc.clone()
    };
    h.hbbs.register(&reinstalled, Some(1));
    let (serial, asked) = (
        h.change_serial().await,
        h.hbbs.queries.load(std::sync::atomic::Ordering::SeqCst),
    );
    refused(
        &h.sync(&reinstalled, &payload(&binding)).await,
        401,
        "bad_signature",
    );
    let queries = || h.hbbs.queries.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(queries(), asked + 1);
    assert!(device(&h, &pc.id).await.unwrap().key_changed_at.is_some());
    assert_eq!(h.change_serial().await, serial + 1);
    let p = card().await;
    assert_eq!(
        (&p["hjv"]["pwless"], &p["note"]),
        (
            &json!("needs-relogin"),
            &json!("이 PC에서 다시 로그인 필요")
        )
    );
    assert!(h.results().await.contains(&"device:key-changed".to_owned()));
    // within the minute hbbs is not asked again
    refused(
        &h.sync(&reinstalled, &payload(&binding)).await,
        401,
        "bad_signature",
    );
    assert_eq!(queries(), asked + 1);
    // the stored key signs again (the real PC came back): needs-relogin ends
    let s = h.sync(&pc, &payload(&binding)).await;
    assert_eq!(s.list()["state"], "active");
    assert!(device(&h, &pc.id).await.unwrap().key_changed_at.is_none());
    assert_eq!(h.change_serial().await, serial + 2);
    assert_eq!(card().await["hjv"]["pwless"], "active");
    // hbbs holds the key stored: a bad signature changes nothing
    let other = TestDevice::windows("560000002");
    let (other_binding, _) = bound(&h, &other).await;
    let serial = h.change_serial().await;
    let forged = TestDevice {
        seed: [78; 32],
        ..other.clone()
    };
    let asked = queries();
    refused(
        &h.sync(&forged, &payload(&other_binding)).await,
        401,
        "bad_signature",
    );
    assert_eq!(queries(), asked + 1);
    assert!(device(&h, &other.id)
        .await
        .unwrap()
        .key_changed_at
        .is_none());
    assert_eq!(h.change_serial().await, serial);
    // the daily round marks PCs that do not sync
    let round = jobs::refresh_seen(&h.state).await;
    assert_eq!((round.key_changed, round.marked), (1, 1));
    assert!(device(&h, &pc.id).await.unwrap().key_changed_at.is_some());
    // a login of the same account on the PC stores hbbs's key and keeps the binding
    h.login_ok(HONG, Some(&reinstalled)).await;
    let d = device(&h, &pc.id).await.unwrap();
    assert_eq!(
        (
            d.public_key.clone(),
            d.key_changed_at,
            h.binding_of(&pc.id).await
        ),
        (reinstalled.public_key().to_vec(), None, binding.clone())
    );
    let s = h.sync(&reinstalled, &payload(&binding)).await;
    assert_eq!(s.list()["state"], "active");
    assert_eq!(card().await["hjv"]["pk"], b64(&reinstalled.public_key()));
}

#[tokio::test]
async fn the_report_and_the_binding_list_count_the_pcs() {
    let h = Harness::start().await;
    let one = TestDevice::windows("570000001");
    let (b1, _) = bound(&h, &one).await;
    let two = TestDevice::windows("570000002");
    bound(&h, &two).await;
    let mut old = payload(&b1);
    old["ver"] = json!("1.6.0");
    h.sync(&one, &old).await.list();
    let mut gone = payload("");
    gone["ver"] = json!("1.6.1");
    h.sync(&two, &gone).await;
    let report = h.admin(&["report"], false, None).await.unwrap();
    for line in [
        "PCs holding their binding: 1, of them synced in the last 10 minutes: 1",
        "PCs that need a login on them (needs-relogin): 0",
        "PCs by app version: 1.6.0 1, 1.6.1 1",
        "refused proofs of the last 30 days: none",
        "released bindings kept 180 days: none",
        "password-less connections of the last 30 days: 0",
    ] {
        assert!(report.contains(line), "{line}: {report}");
    }
    // the incident switch: the PC that has not synced since may hold a list that says on
    h.admin(&["passwordless", "off"], false, None)
        .await
        .unwrap();
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains(
            "PCs holding their binding that have not synced since the last switch change ("
        ) && report.contains("): 1"),
        "{report}"
    );
    sql(&h, "UPDATE meta SET value = CAST(CAST(value AS INTEGER) - 5 AS TEXT) WHERE key = 'switched_at'").await;
    assert_eq!(
        h.sync(&one, &payload(&b1)).await.list()["state"],
        "disabled"
    );
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("since the last switch change ("),
        "{report}"
    );
    assert!(
        report.lines().any(
            |l| l.starts_with("PCs holding their binding that have not synced")
                && l.ends_with("): 0")
        ),
        "{report}"
    );
    // a list older than its lifetime is not held any more
    sql(&h, "UPDATE devices SET last_sync_at = last_sync_at - 86400").await;
    assert!(h
        .admin(&["report"], false, None)
        .await
        .unwrap()
        .contains("PCs holding their binding: 0"),);
    // the binding list: tags and masked IDs, the host names with --reveal
    let out = h.admin(&["binding", "list"], false, None).await.unwrap();
    assert!(out.starts_with("bindings: 2"), "{out}");
    let tag = h.state.tags.device("570000002");
    assert!(
        out.lines().any(|l| l.contains(&tag)
            && l.contains("5700*****")
            && l.contains("disabled")
            && l.contains("absent")
            && l.contains("1.6.1")),
        "{out}"
    );
    assert!(
        !out.contains("570000002") && !out.contains("PC-570000002"),
        "{out}"
    );
    assert!(h
        .admin(&["binding", "list"], true, None)
        .await
        .unwrap()
        .contains("PC-570000002"));
    // the record names the PC of binding revoke by its tag
    h.admin(&["binding", "revoke", "570000002"], false, None)
        .await
        .unwrap();
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(
        record.contains(&format!("cmd=\"binding revoke\" targets={tag}")),
        "{record}"
    );
    assert!(!record.contains("570000002"), "{record}");
}

#[tokio::test]
async fn selfcheck_compares_the_list_key_with_the_pinned_ones() {
    let fp = |seed: [u8; 32], kid: u32| trust::fingerprint(kid, &devsig::public_key(&seed));
    let h = Harness::start().await;
    let out = h.admin(&["selfcheck"], false, None).await.unwrap();
    assert!(
        out.contains(&format!(
            "[ OK ] list keys the apps pin (HJV_API_TRUST_PUBKEYS), SHA-256: {}, {}",
            fp(LIST_SEED, 1),
            fp(NEXT_LIST_SEED, 2)
        )),
        "{out}"
    );
    assert!(
        out.contains(&format!(
            "[ OK ] list key in use (HJV_API_LIST_KID 1): the pinned key 1, {}",
            fp(LIST_SEED, 1)
        )),
        "{out}"
    );
    // an app build pinned another key 1: every list would be refused
    let wrong = format!(
        "{},{}",
        trust::entry(1, &devsig::public_key(&[5; 32])),
        trust::entry(2, &devsig::public_key(&NEXT_LIST_SEED))
    );
    let h = Harness::with(Options {
        vars: vec![("HJV_API_TRUST_PUBKEYS", wrong)],
        ..Default::default()
    })
    .await;
    let err = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(
        err.contains(&format!(
            "[FAIL] list key in use (HJV_API_LIST_KID 1, {}) is not the pinned key 1",
            fp(LIST_SEED, 1)
        )),
        "{err}"
    );
    assert!(err.contains(&fp([5; 32], 1)), "{err}");
    // a key number the apps do not pin
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LIST_KID", "3".into())],
        ..Default::default()
    })
    .await;
    let err = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(err.contains("no pinned key has the number 3"), "{err}");
}

// ---- the shared test vectors (tests/data/v3-vectors.json, rendered by the client) ----

fn vectors() -> Value {
    serde_json::from_str(include_str!("data/v3-vectors.json")).unwrap()
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_else(|| panic!("{key}"))
}

/// The server's list of the vector's values is the client's bytes, with the client's signature:
/// what the client's sodiumoxide check takes (its tests verify these bytes), hjv-api makes.
#[test]
fn the_trust_list_vector() {
    let v = vectors();
    let t = &v["trust_list"];
    let given: Value = serde_json::from_str(text(t, "list")).unwrap();
    let pc_pk: [u8; 32] = b64_decode(text(t, "pc_pk")).unwrap().try_into().unwrap();
    let int = |key: &str| given[key].as_i64().unwrap();
    let mut list = TrustList::new(
        1,
        State::Active,
        text(t, "binding"),
        text(t, "pc_id"),
        &pc_pk,
        text(t, "req"),
        int("serial"),
        int("iat"),
        int("exp") - int("iat"),
        int("refresh"),
        false,
    );
    list.owner = trust::mask_name("홍길동");
    list.login_at = int("login_at");
    list.keys = given["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| ListEntry {
            pk: text(k, "pk").into(),
            id: text(k, "id").into(),
            name: text(k, "name").into(),
            platform: text(k, "platform").into(),
            kinds: serde_json::from_value(k["kinds"].clone()).unwrap(),
            ks: k["ks"].as_i64().unwrap(),
        })
        .collect();
    let bytes = list.bytes().unwrap();
    assert_eq!(String::from_utf8(bytes.clone()).unwrap(), text(t, "list"));
    assert_eq!(b64(&bytes), text(t, "l"));
    assert_eq!(hex(&trust::message(&bytes)), text(t, "signed_hex"));
    let key = ListKey::new(1, &LIST_SEED);
    assert_eq!(b64(&key.sign(&bytes)), text(t, "s"));
    assert_eq!(
        trust::entry(1, &key.public_key()),
        text(&v["keys"], "HJV_TRUST_PUBKEYS")
            .split(',')
            .next()
            .unwrap()
    );
    assert_eq!(
        trust::parse_pubkeys(text(&v["keys"], "HJV_TRUST_PUBKEYS")).unwrap(),
        trust::parse_pubkeys(&trust_pubkeys()).unwrap()
    );
}

/// The client's sync request reads, checks and parses here as the client wrote it.
#[test]
fn the_sync_vector() {
    let v = vectors();
    let p = &v["pc_sync"];
    let env = Envelope::parse(text(p, "body").as_bytes(), &pcsync::SPEC).unwrap();
    assert_eq!(hex(&env.signed_bytes(&pcsync::SPEC)), text(p, "signed_hex"));
    let device = devsig::public_key(&[2; 32]);
    assert_eq!(b64(&device), text(&v["keys"]["pc_device"], "pk"));
    assert!(env.verify(&pcsync::SPEC, &device));
    let t = p["t"].as_i64().unwrap();
    assert!(env.fresh(t) && !env.fresh(t + 301));
    let report = pcsync::parse_report(&env.p, t).unwrap();
    assert_eq!(
        report.binding.map(|b| hex(&b)).as_deref(),
        Some(text(&v["trust_list"], "binding"))
    );
    assert_eq!((report.ver.as_str(), report.off), ("1.6.0", false));
    assert!(report.refused.is_empty());
    let used: Vec<Value> = report
        .used
        .iter()
        .map(|l| json!({"pk": b64(&l.pk), "day": l.day, "last": l.last, "n": l.n}))
        .collect();
    assert_eq!(json!(used), p["used"]);
}

/// README.md's Formats show the same values.
#[test]
fn the_readme_shows_the_list_and_sync_vectors() {
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")).unwrap();
    let v = vectors();
    for value in [
        text(&v["trust_list"], "list"),
        text(&v["trust_list"], "s"),
        text(&v["pc_sync"], "s"),
        text(&v["pc_sync"], "p_sha256"),
    ] {
        assert!(readme.contains(value), "README.md lacks {value}");
    }
}
