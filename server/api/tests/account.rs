// SPDX-License-Identifier: AGPL-3.0-only

//! The account paths after login: the shapes the app's Dart models read (research notes §4), the
//! device list and its rules (design v2 §5, D9, D10), logout, token expiry and revocation; and
//! from 1.9.1 the account's own deletion (account.rs): what goes and what stays, the other
//! devices' tokens, the held waiting room and relay poll, the refusals, the retry, the operator
//! record, the limit, two requests at once, a late request whose account is gone or whose login
//! ended, a client that goes away, a database error and list mode.

mod common;

use common::*;
use hjv_api::{
    account, db, devsig, guest, oplog, proof, seal, support,
    util::{b64, b64url_decode, hex, now, random_bytes, sha256},
};
use hyper::Method;
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

#[tokio::test]
async fn current_user_users_and_logout() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let body = json!({"id": "", "uuid": "dXVpZA=="});
    // currentUser answers the user object itself, with what the account may do (support.rs)
    let a = h.post("/api/currentUser", &token, Some(body.clone())).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(
        a.json,
        json!({"name": "ZZTEST01", "display_name": "홍길동", "status": 1,
               "hjv": {"support_code": true}})
    );
    // users: {total, data}, never a bare list
    let a = h
        .get(
            "/api/users?current=1&pageSize=100&accessible=&status=1",
            &token,
        )
        .await;
    assert_eq!(
        a.json,
        json!({"total": 1, "data": [{"name": "ZZTEST01", "display_name": "홍길동", "status": 1}]})
    );
    let a = h.get("/api/users?current=2&pageSize=100", &token).await;
    assert_eq!(a.json, json!({"total": 1, "data": []}));
    // wrong or missing tokens: 401, which the app takes as logged out
    for bad in [
        "hjv_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "nonsense",
        "",
    ] {
        assert_eq!(
            h.post("/api/currentUser", bad, Some(body.clone()))
                .await
                .status,
            401
        );
        assert_eq!(h.get("/api/peers", bad).await.status, 401);
    }
    // logout revokes the token
    let a = h.post("/api/logout", &token, Some(body.clone())).await;
    assert_eq!((a.status, a.json.clone()), (200, json!({})));
    assert_eq!(
        h.post("/api/currentUser", &token, Some(body)).await.status,
        401
    );
    assert_eq!(h.get("/api/users", &token).await.status, 401);
}

#[tokio::test]
async fn an_installed_windows_pc_is_added_and_listed_on_other_devices() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(3));
    let pc_token = h.login_ok(HONG, Some(&pc)).await;
    let a = h.get("/api/hjv/this-device", &pc_token).await;
    assert_eq!(a.status, 200);
    assert_eq!(a.json["status"], "added");
    assert_eq!(a.json["undo"], true);
    assert_eq!(a.json["id"], "123456789");
    assert_eq!(a.json["message"], "이 PC를 내 PC 목록에 추가했습니다.");
    // the PC does not list itself; the phone lists it, as the app's PeerPayload reads it
    let a = h
        .get(
            "/api/peers?current=1&pageSize=100&accessible=&status=1",
            &pc_token,
        )
        .await;
    assert_eq!(a.json, json!({"total": 0, "data": []}));
    let phone = TestDevice::phone("987654321");
    let phone_token = h.login_ok(HONG, Some(&phone)).await;
    assert_eq!(
        h.get("/api/hjv/this-device", &phone_token).await.json["status"],
        "not_eligible"
    );
    let a = h.get("/api/peers", &phone_token).await;
    assert_eq!(
        a.json,
        json!({"total": 1, "data": [{
            "id": "123456789", "user_name": "ZZTEST01",
            "info": {"device_name": "PC-123456789", "os": "windows"}, "status": 1,
            "note": "관리자가 일시 중지함",
            // no NAS relay in the account: nothing could wake it (wake.rs peer_wol), so no
            // `wake` words; never synced, no `synced` (U27)
            "hjv": {
                "pk": hjv_api::util::b64(&pc.public_key()), "pwless": "disabled",
                "kinds": ["remote", "file", "camera"], "wol": "no", "wake": "",
            },
        }]})
    );
    // logging in again from the PC finds it there already
    let again = h.login_ok(HONG, Some(&pc)).await;
    let a = h.get("/api/hjv/this-device", &again).await;
    assert_eq!(
        (a.json["status"].clone(), a.json["undo"].clone()),
        (json!("already"), json!(false))
    );
    // [취소] only undoes what its own login did
    let a = h.post("/api/hjv/this-device/undo", &again, None).await;
    assert_eq!(a.json["status"], "already");
    assert_eq!(h.get("/api/peers", &phone_token).await.json["total"], 1);
    // another account's list does not show it
    let kim = h.login_ok(KIM, None).await;
    assert_eq!(h.get("/api/peers", &kim).await.json["total"], 0);
    assert!(h.results().await.contains(&"device:added".to_owned()));
}

#[tokio::test]
async fn devices_that_are_not_added() {
    let h = Harness::start().await;
    let mut portable = TestDevice::windows("200000001");
    portable.installed = false;
    let mut android = TestDevice::windows("200000002");
    android.os = "android".into();
    let unknown = TestDevice::windows("200000003");
    let liar = TestDevice::windows("200000004");
    h.hbbs
        .register_key(&liar.id, TestDevice::windows("other").public_key());
    // another program on the console's port vouches for the key of the login's own device: its
    // answer is not signed with hbbs's key, so the PC is not added
    let spoofed = TestDevice::windows("200000006");
    h.hbbs.forge(&spoofed.id, spoofed.public_key());
    for d in [&portable, &android] {
        h.hbbs.register(d, Some(1));
    }
    let cases = [
        (&portable, "not_eligible"),
        (&android, "not_eligible"),
        (&unknown, "check_failed"),
        (&liar, "check_failed"),
        (&spoofed, "check_failed"),
    ];
    for (device, status) in cases {
        let token = h.login_ok(HONG, Some(device)).await;
        let a = h.get("/api/hjv/this-device", &token).await;
        assert_eq!(a.json["status"], status, "{}", device.id);
        assert_eq!(a.json["undo"], false);
    }
    // an hbbs without patch 0004
    let pc = TestDevice::windows("200000005");
    h.hbbs.register(&pc, Some(1));
    h.hbbs
        .unpatched
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let token = h.login_ok(HONG, Some(&pc)).await;
    assert_eq!(
        h.get("/api/hjv/this-device", &token).await.json["status"],
        "check_failed"
    );
    // a login with no device ID
    let token = h.login_ok(HONG, None).await;
    assert_eq!(
        h.get("/api/hjv/this-device", &token).await.json["status"],
        "none"
    );
    let phone = h.login_ok(KIM, None).await;
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 0);
    let logs = hjv_api::log::captured().join("\n");
    assert!(logs.contains("patch 0004 missing"), "{logs}");
    assert!(
        logs.contains("not signed with HJV_API_HBBS_PUBKEY"),
        "{logs}"
    );
    let results = h.results().await;
    for r in [
        "device:unknown-to-hbbs",
        "device:bad-signature",
        "device:hbbs-error",
        "device:hbbs-bad-signature",
    ] {
        assert!(results.contains(&r.to_owned()), "{r}: {results:?}");
    }
    let devices = h.state.db.call(|c| db::all_devices(c)).await.unwrap();
    assert!(devices.is_empty(), "{devices:?}");
}

#[tokio::test]
async fn without_hbbs_s_key_no_pc_is_added() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_HBBS_PUBKEY", String::new())],
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("210000001");
    h.hbbs.register(&pc, Some(1));
    let token = h.login_ok(HONG, Some(&pc)).await;
    assert_eq!(
        h.get("/api/hjv/this-device", &token).await.json["status"],
        "check_failed"
    );
    assert!(h.results().await.contains(&"device:no-hbbs-key".to_owned()));
    assert_eq!(hjv_api::jobs::refresh_seen(&h.state).await.asked, 0);
}

#[tokio::test]
async fn a_new_login_replaces_the_token_of_the_same_device() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("220000001");
    h.hbbs.register(&pc, Some(1));
    let first = h.login_ok(HONG, Some(&pc)).await;
    let second = h.login_ok(HONG, Some(&pc)).await;
    assert_eq!(h.get("/api/users", &first).await.status, 401);
    assert_eq!(h.get("/api/users", &second).await.status, 200);
    // logins without a device ID keep each other, and another account on the same PC keeps its own
    let (a, b) = (h.login_ok(HONG, None).await, h.login_ok(HONG, None).await);
    let kim = h.login_ok(KIM, Some(&pc)).await;
    for token in [&a, &b, &second, &kim] {
        assert_eq!(h.get("/api/users", token).await.status, 200);
    }
    // the PC stays in the account
    assert_eq!(h.get("/api/peers", &a).await.json["total"], 1);
}

#[tokio::test]
async fn a_pc_in_another_account_is_refused() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("300000001");
    h.hbbs.register(&pc, Some(1));
    let hong = h.login_ok(HONG, Some(&pc)).await;
    // Kim logs in on the same PC: the login works, the PC stays Hong's
    let kim = h.login_ok(KIM, Some(&pc)).await;
    let a = h.get("/api/hjv/this-device", &kim).await;
    assert_eq!(a.json["status"], "other_account");
    assert_eq!(
        a.json["message"],
        "다른 계정에 등록된 PC입니다. 관리자에게 문의하세요."
    );
    assert_eq!(a.json["undo"], false);
    // Kim cannot undo, remove or log the PC out of Hong's list
    assert_eq!(
        h.post("/api/hjv/this-device/undo", &kim, None).await.json["status"],
        "other_account"
    );
    assert_eq!(
        h.delete("/api/hjv/devices/300000001", &kim).await.status,
        404
    );
    assert_eq!(
        h.post("/api/logout", &kim, Some(json!({}))).await.status,
        200
    );
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 1);
    assert_eq!(
        h.get("/api/hjv/this-device", &hong).await.json["status"],
        "added"
    );
    let logs = hjv_api::log::captured().join("\n");
    assert!(logs.contains("the device is in another account"), "{logs}");
    assert!(h.results().await.contains(&"device:conflict".to_owned()));
}

#[tokio::test]
async fn undo_remove_and_logout_take_the_pc_out() {
    let h = Harness::start().await;
    let (pc1, pc2, pc3) = (
        TestDevice::windows("400000001"),
        TestDevice::windows("400000002"),
        TestDevice::windows("400000003"),
    );
    for pc in [&pc1, &pc2, &pc3] {
        h.hbbs.register(pc, Some(1));
    }
    let t1 = h.login_ok(HONG, Some(&pc1)).await;
    let t2 = h.login_ok(HONG, Some(&pc2)).await;
    let t3 = h.login_ok(HONG, Some(&pc3)).await;
    let phone = h.login_ok(HONG, None).await;
    let listed = |a: &Answer| -> Vec<String> {
        a.json["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(listed(&h.get("/api/peers", &phone).await).len(), 3);
    assert_eq!(
        listed(&h.get("/api/peers", &t1).await),
        vec!["400000002", "400000003"]
    );
    // [취소] right after login
    let a = h.post("/api/hjv/this-device/undo", &t1, None).await;
    assert_eq!(
        (a.status, a.json["status"].clone()),
        (200, json!("removed"))
    );
    let a = h.get("/api/hjv/this-device", &t1).await;
    assert_eq!(
        (a.json["status"].clone(), a.json["undo"].clone()),
        (json!("removed"), json!(false))
    );
    // "내 PC에서 빼기" from another device
    let a = h.delete("/api/hjv/devices/400000002", &phone).await;
    assert_eq!(a.json, json!({"id": "400000002", "removed": true}));
    assert_eq!(
        h.delete("/api/hjv/devices/400000002", &phone).await.status,
        404
    );
    assert_eq!(
        h.get("/api/hjv/this-device", &t2).await.json["status"],
        "removed"
    );
    // logging out on the PC
    assert_eq!(
        h.post("/api/logout", &t3, Some(json!({"id": "400000003"})))
            .await
            .status,
        200
    );
    assert!(listed(&h.get("/api/peers", &phone).await).is_empty());
    // logging out on a phone removes nothing
    let pc = TestDevice::windows("400000004");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    h.post("/api/logout", &phone, Some(json!({}))).await;
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(
        listed(&h.get("/api/peers", &phone).await),
        vec!["400000004"]
    );
    let results = h.results().await;
    for r in [
        "device:removed-undo",
        "device:removed-by-user",
        "device:removed-logout",
    ] {
        assert!(results.contains(&r.to_owned()), "{r}: {results:?}");
    }
}

#[tokio::test]
async fn a_pc_stays_listed_when_its_token_expires() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("500000001");
    h.hbbs.register(&pc, Some(1));
    let token = h.login_ok(HONG, Some(&pc)).await;
    // ninety days without use
    h.state
        .db
        .call(|c| {
            c.execute(
                "UPDATE tokens SET last_used_at = last_used_at - 91 * 86400",
                [],
            )
        })
        .await
        .unwrap();
    assert_eq!(h.get("/api/hjv/this-device", &token).await.status, 401);
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 1);
    // a token older than the maximum lifetime stops too, even when used every day
    h.state
        .db
        .call(|c| {
            c.execute(
                "UPDATE tokens SET created_at = created_at - 366 * 86400",
                [],
            )
        })
        .await
        .unwrap();
    assert_eq!(h.get("/api/peers", &phone).await.status, 401);
}

#[tokio::test]
async fn removing_an_id_from_the_allowlist_revokes_its_tokens() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    // removing is blocking now (the open login of 2026-10-14): the entry stays, blocked
    let out = h
        .admin(&["allow", "remove"], false, Some("ZZTEST01\n"))
        .await
        .unwrap();
    assert_eq!(
        out,
        "allowlist: 1 blocked (0 of them not on the allowlist before), 0 blocked already, 1 tokens and 1 account keys revoked (allow remove blocks now; hjv-api admin unblock lifts a block)"
    );
    assert_eq!(h.get("/api/users", &token).await.status, 401);
    assert_eq!(h.get("/api/users", &kim).await.status, 200);
    // and the next login, its password right, gets the blocked account's answer
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.1.0.1")
        .await;
    assert_eq!(a.status, 403, "{}", a.text);
    assert_eq!(a.json["hjv"], "blocked");
}

#[tokio::test]
async fn a_device_with_an_odd_key_in_hbbs_does_not_stop_the_round() {
    // A client may register any key with hbbs, also one that is not 32 bytes, and patch 0004
    // signs and returns it as it is: hjv-api cannot read that answer. The daily round skips that
    // device and still checks the ones after it (it asks in the order of the device IDs).
    let h = Harness::start().await;
    let (odd, rekeyed, seen) = (
        TestDevice::windows("610000001"),
        TestDevice::windows("610000002"),
        TestDevice::windows("610000003"),
    );
    for pc in [&odd, &rekeyed, &seen] {
        h.hbbs.register(pc, Some(5));
        h.login_ok(HONG, Some(pc)).await;
    }
    h.hbbs.register_odd_key(&odd.id, &[7; 33]);
    h.hbbs.register_key(&rekeyed.id, [3; 32]);
    let round = hjv_api::jobs::refresh_seen(&h.state).await;
    assert_eq!(
        (round.asked, round.malformed, round.key_changed, round.seen),
        (3, 1, 1, 1)
    );
    let device = |id: &str| {
        let id = id.to_owned();
        h.state.db.call(move |c| db::device(c, &id))
    };
    assert!(device(&rekeyed.id)
        .await
        .unwrap()
        .unwrap()
        .key_changed_at
        .is_some());
    assert!(device(&odd.id)
        .await
        .unwrap()
        .unwrap()
        .key_changed_at
        .is_none());
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains("upkeep: hbbs answered hjv-pk in an unknown format for 1 devices; skipped"),
        "{logs}"
    );
}

#[tokio::test]
async fn a_pc_with_an_odd_key_in_hbbs_is_no_hbbs_failure() {
    // The five-minute check asks hbbs about the PCs that hold a list: one PC whose answer it
    // cannot read is not on, and hbbs is fine; hbbs fails when every answer is unreadable.
    let h = Harness::start().await;
    let (odd, on) = (
        TestDevice::windows("620000001"),
        TestDevice::windows("620000002"),
    );
    for pc in [&odd, &on] {
        h.hbbs.register(pc, Some(5));
        h.login_ok(HONG, Some(pc)).await;
    }
    let at = hjv_api::util::now();
    h.state
        .db
        .call(move |c| {
            c.execute(
                "UPDATE devices SET sync_state = 'present', last_sync_at = ?1",
                [at],
            )
        })
        .await
        .unwrap();
    h.hbbs.register_odd_key(&odd.id, &[7; 33]);
    h.state
        .online
        .backdate(&on.id, std::time::Duration::from_secs(3600));
    let health = hjv_api::check::health(&h.state).await.unwrap();
    assert_eq!(health.hbbs, hjv_api::check::HBBS_OK);
    assert_eq!((health.pcs_holding, health.pcs_on), (2, Some(1)));
    h.hbbs.register_odd_key(&on.id, &[7; 33]);
    let health = hjv_api::check::health(&h.state).await.unwrap();
    assert_eq!(health.hbbs, "answers in an unknown format");
    assert_eq!(health.pcs_on, None);
}

#[tokio::test]
async fn upkeep_keeps_what_is_seen_and_drops_what_is_not() {
    let h = Harness::start().await;
    let (seen, gone, rekeyed) = (
        TestDevice::windows("600000001"),
        TestDevice::windows("600000002"),
        TestDevice::windows("600000003"),
    );
    for pc in [&seen, &gone, &rekeyed] {
        h.hbbs.register(pc, Some(5));
        h.login_ok(HONG, Some(pc)).await;
    }
    // 200 days pass; hbbs saw one PC a minute ago, never saw one, and one came back with a new key
    h.state
        .db
        .call(|c| {
            c.execute(
                "UPDATE devices SET last_seen_at = last_seen_at - 200 * 86400",
                [],
            )
        })
        .await
        .unwrap();
    h.hbbs.register(&seen, Some(60));
    h.hbbs.register_key(&gone.id, gone.public_key());
    h.hbbs.forget_seen(&gone.id);
    h.hbbs.register_key(&rekeyed.id, [3; 32]);
    // and nobody used the account for more than a year
    h.state
        .db
        .call(|c| {
            c.execute(
                "UPDATE accounts SET last_active_at = last_active_at - 400 * 86400",
                [],
            )
        })
        .await
        .unwrap();
    let round = hjv_api::jobs::refresh_seen(&h.state).await;
    assert_eq!((round.asked, round.seen, round.key_changed), (3, 1, 1));
    let purged = hjv_api::jobs::purge_now(&h.state).await.unwrap();
    assert_eq!(purged.devices, 2);
    // the account stays while its PC is in use
    assert_eq!(purged.accounts, 0);
    let left = h.state.db.call(|c| db::all_devices(c)).await.unwrap();
    assert_eq!(
        left.iter()
            .map(|d| d.device_id.as_str())
            .collect::<Vec<_>>(),
        vec!["600000001"]
    );
    // records older than six months go as well
    h.state
        .db
        .call(|c| c.execute("UPDATE events SET at = at - 200 * 86400", []))
        .await
        .unwrap();
    let purged = hjv_api::jobs::purge_now(&h.state).await.unwrap();
    assert!(purged.events > 0);
}

// ---- the account's own deletion (1.9.1, account.rs) ----

const DELETE: &str = account::DELETE_PATH;
const CONFIRM_BODY: &[u8] = br#"{"confirm":"delete-account"}"#;
const RELAY_IP: &str = "198.51.100.20";
const PC_IP: &str = "198.51.100.40";

fn confirm() -> Option<Value> {
    Some(json!({"confirm": "delete-account"}))
}

fn unknown_token() -> Value {
    json!({"error": "로그인이 필요합니다. 다시 로그인해 주세요."})
}

fn not_recorded() -> Value {
    json!({"error": account::TEXT_NOT_RECORDED, "hjv": "not_recorded"})
}

fn lan() -> Value {
    json!([iface(
        "eth0",
        "192.0.2.10/24",
        Some("192.0.2.1"),
        Some("00:00:5e:00:53:01")
    )])
}

async fn count(h: &Harness, sql: &str) -> i64 {
    let sql = sql.to_owned();
    h.state
        .db
        .call(move |c| c.query_row(&sql, [], |r| r.get(0)))
        .await
        .unwrap()
}

async fn account_of(h: &Harness, id: &str) -> Option<db::Account> {
    let key = hjv_api::util::login_key(id);
    h.state
        .db
        .call(move |c| db::account_by_key(c, &key))
        .await
        .unwrap()
}

async fn listing_of(h: &Harness, id: &str) -> db::Listing {
    let key = hjv_api::util::login_key(id);
    h.state
        .db
        .call(move |c| db::listing(c, &key))
        .await
        .unwrap()
}

/// A harness with a log key of its own: its tags, and so its journal lines, are its own.
fn own_tags() -> Options {
    Options {
        log_key: Some(random_bytes::<32>()),
        ..Default::default()
    }
}

/// Whether the journal (the process's: every harness's) has this line.
fn journal_has(line: &str) -> bool {
    hjv_api::log::captured().iter().any(|l| l == line)
}

/// How many first lines of deletions in the app the record holds.
fn first_lines(oplog: &Path) -> usize {
    let first = format!("cmd=\"{}\" targets=", account::COMMAND);
    std::fs::read_to_string(oplog)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(&first))
        .count()
}

/// The signed confirmation of `token`, sent on its own and held between its checks and its
/// transaction: once it has passed its checks (its answer's place under its hash is there) and
/// written the record's first line, `meanwhile` runs on the database, and the request's
/// transaction waits for it. The request's answer, and whether its token was still there when
/// `meanwhile` began (if not, the transaction came first and the case was not made).
async fn delete_holding<F>(h: &Arc<Harness>, token: &str, meanwhile: F) -> (Answer, bool)
where
    F: FnOnce(&mut rusqlite::Connection) -> rusqlite::Result<()> + Send + 'static,
{
    let signed = h.proof_for("POST", DELETE, CONFIRM_BODY, token).unwrap();
    let hash = sha256(token.as_bytes());
    let sent = proof::request_hash(&signed, "POST", DELETE, &sha256(CONFIRM_BODY), &hash);
    let key = format!("account-delete.{}", hex(&sent));
    let before = first_lines(&h.oplog);
    let request = {
        let (h, token) = (h.clone(), token.to_owned());
        tokio::spawn(async move { delete_with(&h, &token, Some(&signed)).await })
    };
    let started = Instant::now();
    while h.state.request_retries.again(&key, sent).is_none() {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "no checks passed"
        );
        std::thread::yield_now();
    }
    let oplog = h.oplog.clone();
    let first = h
        .state
        .db
        .call(move |c| {
            let started = Instant::now();
            while first_lines(&oplog) == before {
                assert!(started.elapsed() < Duration::from_secs(5), "no first line");
                std::thread::sleep(Duration::from_millis(1));
            }
            let first = c.query_row(
                "SELECT EXISTS (SELECT 1 FROM tokens WHERE hash = ?1)",
                [&hash[..]],
                |r| r.get(0),
            )?;
            meanwhile(c)?;
            Ok(first)
        })
        .await
        .unwrap();
    (request.await.unwrap(), first)
}

/// The operator record's lines of deletions in the app.
fn deletion_lines(h: &Harness) -> Vec<String> {
    std::fs::read_to_string(&h.oplog)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(&format!("cmd=\"{}\"", account::COMMAND)))
        .map(str::to_owned)
        .collect()
}

/// The records of deletions in the app.
async fn deletions(h: &Harness) -> usize {
    h.results()
        .await
        .iter()
        .filter(|r| *r == "account:deleted-by-user")
        .count()
}

/// The confirmation with the token and this `X-HJV-Proof`, or none.
async fn delete_with(h: &Harness, token: &str, proof: Option<&str>) -> Answer {
    let auth = format!("Bearer {token}");
    let mut headers = vec![
        ("authorization", auth.as_str()),
        ("content-type", "application/json"),
    ];
    if let Some(p) = proof {
        headers.push((proof::HEADER, p));
    }
    h.request(Method::POST, DELETE, &headers, Some(CONFIRM_BODY.to_vec()))
        .await
}

/// A PC of HONG's bound by a login on it, with a wake report from its sync, asleep since (as
/// tests/wake.rs has it): the PC, its login's token and its binding.
async fn sleeping_pc(h: &Harness, id: &str) -> (TestDevice, String, String) {
    let pc = TestDevice::windows(id);
    h.hbbs.register(&pc, Some(100));
    let token = h.login_ok(HONG, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    assert_eq!(binding.len(), 32);
    let wol = wol_report(
        "00:00:5e:00:53:10",
        "192.0.2.67/24",
        "192.0.2.1",
        Some("00:00:5e:00:53:01"),
    );
    let s = h
        .sync_from(&pc, &payload_with_wol(&binding, &wol, "h1"), PC_IP)
        .await;
    assert_eq!(s.answer.status, 200, "{}", s.answer.text);
    let statement = format!(
        "UPDATE devices SET last_sync_at = {} WHERE device_id = '{id}'",
        now() - 100
    );
    h.state
        .db
        .call(move |c| c.execute_batch(&statement))
        .await
        .unwrap();
    (pc, token, binding)
}

/// A relay approved for the account of `pc`, online (as tests/wake.rs has it).
async fn approved(h: &Harness, seed: u8, pc: &TestDevice) -> TestRelay {
    let r = TestRelay::new([seed; 32]);
    assert_eq!(h.hello(&r, RELAY_IP, &lan()).await.json["state"], "pending");
    h.admin(
        &["relay", "approve", &r.code, "--owner-of", &pc.id, "--force"],
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await.json["state"],
        "active"
    );
    r
}

/// A support code the account of `token` makes: the code, its link and its waiting number.
async fn code_of(h: &Harness, token: &str) -> (String, String, String) {
    let a = h
        .post(support::PATH, token, Some(json!({"use": "pc"})))
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    let text = |name: &str| a.json[name].as_str().unwrap().to_owned();
    (text("code"), text("link"), text("wait"))
}

/// A PC that is not logged in trades a code it was given (as tests/support.rs has it).
async fn trade(h: &Harness, typed: &str, pc: &TestDevice) -> Answer {
    let field = seal::seal_code(
        &h.seal_pk,
        typed,
        now(),
        &b64(&random_bytes::<16>()),
        &pc.id,
    );
    let sealed = sha256(&b64url_decode(field.rsplit('.').next().unwrap()).unwrap());
    let pk = b64(&pc.public_key());
    let sig = devsig::sign(&pc.seed, &guest::pc_message(&pc.id, &pk, &sealed));
    let body = json!({"use": "pc", "id": pc.id, "pk": pk, "sig": sig, "code": field});
    h.request(
        Method::POST,
        guest::PATH,
        &[
            ("content-type", "application/json"),
            ("x-real-ip", "10.8.40.1"),
        ],
        Some(body.to_string().into_bytes()),
    )
    .await
}

/// A staff account with a PC, a second device, a relay with a wake job and an approval ahead,
/// and an open support code deletes itself from the phone: at once, with the counts; both
/// devices' tokens are an unknown token's (a logout and another deletion too); the PC's next
/// sync gets a signed `revoked`; the entry is off the list; the code ends in memory (`revoked`,
/// its link gone); the relay is revoked and the wake data goes; one record, the operator record's
/// two lines with the tag, the journal's line without the portal ID, the name or the PC; another
/// account is untouched, and the person's next login makes a new, empty account.
#[tokio::test]
async fn an_account_deletes_itself_at_once() {
    let h = Harness::with(own_tags()).await;
    let (pc, pc_token, binding) = sleeping_pc(&h, "800000001").await;
    let r = approved(&h, 50, &pc).await;
    let ahead = TestRelay::new([51; 32]);
    h.admin(
        &["relay", "approve", &ahead.code, "--owner-of", &pc.id],
        false,
        None,
    )
    .await
    .unwrap();
    let phone = h.login_ok(HONG, None).await;
    let wake = h
        .post(&format!("/api/hjv/devices/{}/wake", pc.id), &phone, None)
        .await;
    assert_eq!(wake.status, 202, "{}", wake.text);
    let (code, link, _) = code_of(&h, &phone).await;
    let page = format!("{link}/");
    let browse = || h.request(Method::GET, &page, &[("x-real-ip", "10.8.40.2")], None);
    assert_ne!(browse().await.status, 404);
    let kim = h.login_ok(KIM, None).await;
    let tag = h.account_tag(HONG.0);
    let id = account_of(&h, HONG.0).await.unwrap().id;
    let tables = [
        "wol_info",
        "wake_jobs",
        "wake_commands",
        "relay_preapprovals",
    ];
    for table in tables {
        let n = count(&h, &format!("SELECT COUNT(*) FROM {table}")).await;
        assert_eq!(n, 1, "{table}");
    }

    let a = h.post(DELETE, &phone, confirm()).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(
        a.json,
        json!({"deleted": true, "pcs": 1, "devices": 2, "relays": 1})
    );
    assert_eq!(a.headers["cache-control"], "no-store");
    assert!(account_of(&h, HONG.0).await.is_none());
    assert_eq!(listing_of(&h, HONG.0).await, db::Listing::Absent);
    for table in ["tokens", "ctl_keys"] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE account_id = {id}");
        assert_eq!(count(&h, &sql).await, 0, "{table}");
    }
    // both devices are logged out: every request of theirs is an unknown token's
    for token in [&phone, &pc_token] {
        for a in [
            h.post("/api/currentUser", token, Some(json!({}))).await,
            h.get("/api/peers", token).await,
            h.get("/api/hjv/keys", token).await,
            h.post("/api/hjv/ticket", token, Some(json!({}))).await,
            h.get(support::WAIT_PATH, token).await,
            h.post(DELETE, token, confirm()).await,
            h.post("/api/logout", token, Some(json!({}))).await,
        ] {
            assert_eq!((a.status, &a.json), (401, &unknown_token()), "{}", a.text);
        }
    }
    // the PC left with a tombstone: its next sync gets a signed revoked
    let reason = count(
        &h,
        "SELECT COUNT(*) FROM binding_tombstones WHERE device_id = '800000001' AND reason = 'account-delete'",
    )
    .await;
    assert_eq!(reason, 1);
    let list = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(
        (&list["state"], &list["keys"]),
        (&json!("revoked"), &json!([]))
    );
    // the code ended in memory at once: its link shows nothing, and its trade is refused as one
    // its maker's login ended
    assert_eq!(browse().await.status, 404);
    let traded = trade(&h, &code, &TestDevice::windows("800000009")).await;
    assert_eq!(traded.status, 401, "{}", traded.text);
    let codes: Vec<String> = h
        .results()
        .await
        .into_iter()
        .filter(|r| r.starts_with("code:"))
        .collect();
    assert_eq!(codes, ["code:made", "code:revoked"]);
    // the relay revoked, keeping its key; the wake data gone with the account
    let relay = h
        .state
        .db
        .call({
            let id = r.id.clone();
            move |c| db::relay(c, &id)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (relay.state, relay.owner_account),
        (db::RelayState::Revoked, None)
    );
    assert_eq!(
        h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await.json["state"],
        "revoked"
    );
    for table in tables {
        let n = count(&h, &format!("SELECT COUNT(*) FROM {table}")).await;
        assert_eq!(n, 0, "{table}");
    }
    // one record; the operator record's two lines by the tag; the journal's line by the tag
    assert_eq!(deletions(&h).await, 1);
    let lines = deletion_lines(&h);
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(
        lines[0].contains(&format!(
            " sudo_user=- cmd=\"app account-delete\" targets={tag} reveal=0 prev="
        )),
        "{lines:?}"
    );
    assert!(
        lines[1].contains(&format!(" sudo_user=- cmd=\"app account-delete\" acct={tag} done=\"ok: 1 PCs, 2 account keys, 2 tokens, 1 relays, allowlist entry removed\" prev=")),
        "{lines:?}"
    );
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert_eq!(oplog::verify(&h.oplog).unwrap(), record.lines().count());
    let line = format!(
        "account: deleted at its own request (acct={tag}): 1 PCs, 2 account keys, 2 tokens, 1 relays, allowlist entry removed"
    );
    assert!(journal_has(&line), "{line}");
    let journal = hjv_api::log::captured();
    for l in journal.iter().filter(|l| l.starts_with("account")) {
        for secret in [HONG.0, "zztest01", "홍길동", "800000001", "PC-800000001"] {
            assert!(!l.contains(secret), "{l}");
        }
    }
    for secret in [HONG.0, "zztest01", "홍길동", "800000001"] {
        assert!(!record.contains(secret), "{record}");
    }
    // another account is untouched; the person's next login makes a new, empty account
    let me = h.post("/api/currentUser", &kim, Some(json!({}))).await;
    assert_eq!(me.status, 200, "{}", me.text);
    let again = h.login_ok(HONG, None).await;
    assert_eq!(
        h.get("/api/peers", &again).await.json,
        json!({"total": 0, "data": []})
    );
    // the deleted account's keys are nowhere, not even among the ended ones
    let keys = h.get("/api/hjv/keys", &again).await;
    assert_eq!(
        (&keys.json["total"], &keys.json["ended"]),
        (&json!(1), &json!([])),
        "{}",
        keys.text
    );
}

/// What is held when the account goes hears of it at once: a waiting room request of the
/// account's code gets that code `over`, a relay's long-poll its `revoked`.
#[tokio::test]
async fn held_requests_hear_of_the_deletion() {
    let h = Arc::new(Harness::start().await);
    let (pc, pc_token, _) = sleeping_pc(&h, "800000002").await;
    let r = approved(&h, 52, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let (_, _, wait) = code_of(&h, &phone).await;
    let rev = h.get(support::WAIT_PATH, &phone).await.json["rev"]
        .as_u64()
        .unwrap();
    let waiting = {
        let (h, phone) = (h.clone(), phone.clone());
        let path = format!("{}?rev={rev}", support::WAIT_PATH);
        tokio::spawn(async move { h.get(&path, &phone).await })
    };
    let polling = {
        let (h, r) = (h.clone(), r.clone());
        tokio::spawn(async move { h.poll(&r, RELAY_IP, 20, &json!([]), None, &lan()).await })
    };
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(h.state.support.held_waits(), 1);
    assert!(!waiting.is_finished() && !polling.is_finished());
    let a = h.post(DELETE, &pc_token, confirm()).await;
    assert_eq!(
        a.json,
        json!({"deleted": true, "pcs": 1, "devices": 2, "relays": 1}),
        "{}",
        a.text
    );
    let w = waiting.await.unwrap();
    assert!(w.elapsed < Duration::from_secs(5), "{:?}", w.elapsed);
    assert_eq!(
        w.json["codes"],
        json!([{"wait": wait, "state": "over"}]),
        "{}",
        w.text
    );
    let p = polling.await.unwrap();
    assert!(p.elapsed < Duration::from_secs(5), "{:?}", p.elapsed);
    assert_eq!(p.json["state"], "revoked", "{}", p.text);
}

/// Only the confirmation deletes: no body, an object without it, another value, JSON that is no
/// object get 400 `bad_request`; another method 405 with `Allow: POST`; an `Origin` 403; a body
/// not sent as JSON (its `Content-Type`) 415. Nothing is deleted or recorded, and the journal says
/// it once.
#[tokio::test]
async fn only_the_confirmation_deletes() {
    let h = Harness::with(own_tags()).await;
    let token = h.login_ok(HONG, None).await;
    let bad = json!({"error": "요청을 이해하지 못했습니다.", "hjv": "bad_request"});
    let a = h.post(DELETE, &token, None).await;
    assert_eq!((a.status, &a.json), (400, &bad), "{}", a.text);
    for body in [
        json!({}),
        json!({"confirm": "delete"}),
        json!({"confirm": true}),
        json!({"Confirm": "delete-account"}),
        json!(["confirm", "delete-account"]),
        json!("delete-account"),
    ] {
        let a = h.post(DELETE, &token, Some(body.clone())).await;
        assert_eq!((a.status, &a.json), (400, &bad), "{body}: {}", a.text);
    }
    let a = h.get(DELETE, &token).await;
    assert_eq!(
        (a.status, a.headers["allow"].to_str().unwrap()),
        (405, "POST"),
        "{}",
        a.text
    );
    let auth = format!("Bearer {token}");
    let signed = h.proof_for("POST", DELETE, CONFIRM_BODY, &token).unwrap();
    let a = h
        .request(
            Method::POST,
            DELETE,
            &[
                ("authorization", &auth),
                ("content-type", "application/json"),
                (proof::HEADER, &signed),
                ("origin", "https://example.com"),
            ],
            Some(CONFIRM_BODY.to_vec()),
        )
        .await;
    assert_eq!(
        (a.status, &a.json),
        (403, &json!({"error": "접근이 차단되었습니다"}))
    );
    let signed = h.proof_for("POST", DELETE, CONFIRM_BODY, &token).unwrap();
    let a = h
        .request(
            Method::POST,
            DELETE,
            &[
                ("authorization", &auth),
                ("content-type", "text/plain"),
                (proof::HEADER, &signed),
            ],
            Some(CONFIRM_BODY.to_vec()),
        )
        .await;
    assert_eq!(a.status, 415, "{}", a.text);
    let me = h.post("/api/currentUser", &token, Some(json!({}))).await;
    assert_eq!(me.status, 200, "{}", me.text);
    assert!(account_of(&h, HONG.0).await.is_some());
    assert!(deletion_lines(&h).is_empty());
    assert_eq!(deletions(&h).await, 0);
    let line = format!(
        "account delete: a request not of the form (acct={}); the next ten minutes are not logged",
        h.account_tag(HONG.0)
    );
    assert!(journal_has(&line), "{line}");
}

/// The token counts only with its key's signature, in time: none, another device's key, a
/// signature over another token: the 401 of an unknown token; a time ten minutes off: 409
/// `clock`. Nothing is deleted until the request is signed as the app signs it.
#[tokio::test]
async fn a_deletion_needs_its_signature_in_time() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let other = h.login_ok(HONG, None).await;
    let (mine, theirs) = (h.key_of(&token).unwrap(), h.key_of(&other).unwrap());
    let signed = |seed: &[u8; 32], t: i64, over: &str| {
        proof::header_value(
            seed,
            "POST",
            DELETE,
            t,
            &random_bytes::<16>(),
            CONFIRM_BODY,
            over,
        )
    };
    for proof in [
        None,
        Some(signed(&theirs, now(), &token)),
        Some(signed(&mine, now(), &other)),
    ] {
        let a = delete_with(&h, &token, proof.as_deref()).await;
        assert_eq!((a.status, &a.json), (401, &unknown_token()), "{}", a.text);
    }
    let a = delete_with(&h, &token, Some(&signed(&mine, now() - 700, &token))).await;
    assert_eq!(
        (a.status, &a.json["hjv"], &a.json["error"]),
        (409, &json!("clock"), &json!(hjv_api::api::TEXT_CLOCK)),
        "{}",
        a.text
    );
    assert!(account_of(&h, HONG.0).await.is_some());
    assert_eq!(deletions(&h).await, 0);
    let a = delete_with(&h, &token, Some(&signed(&mine, now(), &token))).await;
    assert_eq!(
        a.json,
        json!({"deleted": true, "pcs": 0, "devices": 2, "relays": 0}),
        "{}",
        a.text
    );
}

/// The very same request again (the app's retry after a failed attempt) gets the first answer,
/// though the first run ended the token that such a retry is found by; it deletes nothing more.
/// The same proof with another body, and a new request of the deleted token: unknown tokens.
#[tokio::test]
async fn the_very_same_request_again_gets_the_first_answer() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let signed = h.proof_for("POST", DELETE, CONFIRM_BODY, &token).unwrap();
    let first = delete_with(&h, &token, Some(&signed)).await;
    assert_eq!(
        first.json,
        json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0}),
        "{}",
        first.text
    );
    let again = delete_with(&h, &token, Some(&signed)).await;
    assert_eq!((again.status, &again.text), (200, &first.text));
    assert_eq!(deletions(&h).await, 1);
    assert_eq!(deletion_lines(&h).len(), 2);
    let auth = format!("Bearer {token}");
    let other_body = br#"{"confirm":"delete-account","again":1}"#.to_vec();
    let a = h
        .request(
            Method::POST,
            DELETE,
            &[
                ("authorization", &auth),
                ("content-type", "application/json"),
                (proof::HEADER, &signed),
            ],
            Some(other_body),
        )
        .await;
    assert_eq!((a.status, &a.json), (401, &unknown_token()), "{}", a.text);
    let a = h.post(DELETE, &token, confirm()).await;
    assert_eq!((a.status, &a.json), (401, &unknown_token()), "{}", a.text);
}

/// A deletion that cannot be recorded is not made: without the admin socket (no operator record)
/// and when the record does not take the first line, 503 `not_recorded`, and the account stays;
/// once the record takes lines again, the deletion goes through and the chain holds.
#[tokio::test]
async fn without_the_operator_record_nothing_is_deleted() {
    let h = Harness::with(Options {
        no_admin: true,
        ..Default::default()
    })
    .await;
    let token = h.login_ok(HONG, None).await;
    let a = h.post(DELETE, &token, confirm()).await;
    assert_eq!((a.status, &a.json), (503, &not_recorded()), "{}", a.text);
    let me = h.post("/api/currentUser", &token, Some(json!({}))).await;
    assert_eq!(me.status, 200, "{}", me.text);
    assert!(hjv_api::log::captured().iter().any(|l| {
        l.starts_with(
        "account delete: no operator record (the admin socket is off): a deletion is refused (acct="
    )
    }));

    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let aside = h.dir.join("operator.log.aside");
    std::fs::rename(&h.oplog, &aside).unwrap();
    std::fs::create_dir(&h.oplog).unwrap();
    let a = h.post(DELETE, &token, confirm()).await;
    assert_eq!((a.status, &a.json), (503, &not_recorded()), "{}", a.text);
    assert!(account_of(&h, HONG.0).await.is_some());
    let me = h.post("/api/currentUser", &token, Some(json!({}))).await;
    assert_eq!(me.status, 200, "{}", me.text);
    std::fs::remove_dir(&h.oplog).unwrap();
    std::fs::rename(&aside, &h.oplog).unwrap();
    let a = h.post(DELETE, &token, confirm()).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(deletion_lines(&h).len(), 2);
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert_eq!(oplog::verify(&h.oplog).unwrap(), record.lines().count());
}

/// Thirty calls a minute per account key, counted with its other account calls and with the
/// refused ones: the next is 429 `rate_limited`, and the account stays; another device's key has
/// its own.
#[tokio::test]
async fn thirty_calls_a_minute_per_key() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let other = h.login_ok(HONG, None).await;
    for n in 0..hjv_api::keys::CALLS_PER_MINUTE {
        let a = if n % 3 == 0 {
            h.get("/api/hjv/keys", &token).await
        } else {
            h.post(DELETE, &token, Some(json!({}))).await
        };
        assert!(matches!(a.status, 200 | 400), "call {n}: {}", a.text);
    }
    let a = h.post(DELETE, &token, confirm()).await;
    assert_eq!(
        (a.status, &a.json),
        (
            429,
            &json!({"error": hjv_api::keys::TEXT_TOO_OFTEN, "hjv": "rate_limited"})
        ),
        "{}",
        a.text
    );
    assert!(account_of(&h, HONG.0).await.is_some());
    let a = h.post(DELETE, &other, confirm()).await;
    assert_eq!(
        a.json,
        json!({"deleted": true, "pcs": 0, "devices": 2, "relays": 0}),
        "{}",
        a.text
    );
}

/// Two requests of one token at once (the button pressed twice) while the database is busy: both
/// pass their checks before either deletes, the second finds the account gone and gets `deleted`
/// with counts of 0. One record, and the operator record's result line of each.
#[tokio::test]
async fn two_requests_at_once_delete_once() {
    let h = Harness::start().await;
    let mut tries = 0;
    let overlapped = loop {
        tries += 1;
        let token = h.login_ok(HONG, None).await;
        // the database held for a moment: both requests wait for it in their authentication,
        // and the first deletes only after its record line, when the second has passed it
        let hold = {
            let state = h.state.clone();
            tokio::spawn(async move {
                state
                    .db
                    .call(|_| {
                        std::thread::sleep(Duration::from_millis(800));
                        Ok(())
                    })
                    .await
                    .unwrap()
            })
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        let (a, b) = tokio::join!(
            h.post(DELETE, &token, confirm()),
            h.post(DELETE, &token, confirm())
        );
        hold.await.unwrap();
        match (a.status, b.status) {
            (200, 200) => break (a, b),
            // the second came after the deletion (an unknown token): not this case, again
            (200, 401) | (401, 200) if tries < 5 => continue,
            other => panic!("{other:?}: {} / {}", a.text, b.text),
        }
    };
    let deleted = json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0});
    let gone = json!({"deleted": true, "pcs": 0, "devices": 0, "relays": 0});
    let (a, b) = overlapped;
    assert!(
        (a.json == deleted && b.json == gone) || (a.json == gone && b.json == deleted),
        "{} / {}",
        a.text,
        b.text
    );
    assert_eq!(deletions(&h).await, tries);
    let lines = deletion_lines(&h);
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.contains(" done=\"ok: already deleted\" "))
            .count(),
        1,
        "{lines:?}"
    );
}

/// In list mode the ID leaves the list with the account, as with `user delete`: its next login
/// fails as an ID off the list until the operator imports it again.
#[tokio::test]
async fn in_list_mode_the_id_leaves_the_list_with_the_account() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_LOGIN", "list".into())],
        ..Default::default()
    })
    .await;
    let token = h.login_ok(HONG, None).await;
    let a = h.post(DELETE, &token, confirm()).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(listing_of(&h, HONG.0).await, db::Listing::Absent);
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.9.9.9")
        .await;
    assert_eq!(
        (a.status, &a.json),
        (401, &json!({ "error": hjv_api::login::FAILURE_TEXT })),
        "{}",
        a.text
    );
    assert_eq!(h.results().await.last().unwrap(), "login:not-allowed");
    h.admin(&["allow", "import"], false, Some(&format!("{}\n", HONG.0)))
        .await
        .unwrap();
    let again = h.login_ok(HONG, None).await;
    assert_eq!(h.get("/api/hjv/keys", &again).await.json["total"], 1);
}

/// A request held after its checks while its account goes (the operator's `user delete`, or
/// another request of the account) and another person's first login takes the freed ID
/// (accounts.id is reused) deletes nothing of that person's: its token went with its account, so
/// it answers as for an account deleted already, and the record's result line names the account
/// by its tag.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_late_request_leaves_the_account_that_took_its_id() {
    for tries in 1.. {
        let h = Arc::new(Harness::with(own_tags()).await);
        let token = h.login_ok(HONG, None).await;
        let hong = account_of(&h, HONG.0).await.unwrap().id;
        let kim = hjv_api::util::login_key(KIM.0);
        let (a, first) = delete_holding(&h, &token, move |c| {
            db::in_tx(c, |tx| db::delete_account(tx, hong, now()))?;
            db::upsert_account(c, &kim, KIM.0, "김연세", now())?;
            Ok(())
        })
        .await;
        if !first && tries < 5 {
            continue;
        }
        assert!(first, "the deletion came first in {tries} tries");
        assert_eq!(
            a.json,
            json!({"deleted": true, "pcs": 0, "devices": 0, "relays": 0}),
            "{}",
            a.text
        );
        let kim = account_of(&h, KIM.0).await.expect("KIM's account stays");
        assert_eq!(kim.id, hong, "the freed ID");
        assert_eq!(listing_of(&h, KIM.0).await, db::Listing::Registered);
        assert_eq!(deletions(&h).await, 0);
        let lines = deletion_lines(&h);
        let tag = h.account_tag(HONG.0);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains(&format!(" targets={tag} ")), "{lines:?}");
        assert!(
            lines[1].contains(&format!(" acct={tag} done=\"ok: already deleted\" ")),
            "{lines:?}"
        );
        break;
    }
}

/// A request whose login ended another way while it was held after its checks (here the
/// operator's revoke-all; another device's "log out other devices" and a block do the same)
/// deletes nothing: the 401 of an unknown token, the account as it was, a result line that says
/// so, and the journal's line by the tag. The next login of the account deletes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_login_that_ended_meanwhile_deletes_nothing() {
    for tries in 1.. {
        let h = Arc::new(Harness::with(own_tags()).await);
        let token = h.login_ok(HONG, None).await;
        let hong = account_of(&h, HONG.0).await.unwrap().id;
        let (a, first) = delete_holding(&h, &token, move |c| {
            db::in_tx(c, |tx| {
                db::revoke_tokens(tx, hong)?;
                db::revoke_account_keys(tx, hong, "admin", now())
            })?;
            Ok(())
        })
        .await;
        if !first && tries < 5 {
            continue;
        }
        assert!(first, "the deletion came first in {tries} tries");
        assert_eq!((a.status, &a.json), (401, &unknown_token()), "{}", a.text);
        assert!(account_of(&h, HONG.0).await.is_some());
        assert_eq!(listing_of(&h, HONG.0).await, db::Listing::Registered);
        assert_eq!(deletions(&h).await, 0);
        let tag = h.account_tag(HONG.0);
        let lines = deletion_lines(&h);
        assert!(
            lines[1].contains(&format!(" acct={tag} done=\"failed: the login ended\" ")),
            "{lines:?}"
        );
        assert!(journal_has(&format!(
            "account delete: the login ended while the request waited (acct={tag}); nothing was deleted"
        )));
        let again = h.login_ok(HONG, None).await;
        let a = h.post(DELETE, &again, confirm()).await;
        assert_eq!(
            a.json,
            json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0}),
            "{}",
            a.text
        );
        break;
    }
}

/// When the record takes the first line but not the result line, the deletion stands: its answer,
/// its record and the journal's line that the result line is missing, by the tag.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_result_line_the_record_does_not_take() {
    for tries in 1.. {
        let h = Arc::new(Harness::with(own_tags()).await);
        let token = h.login_ok(HONG, None).await;
        let (oplog, aside) = (h.oplog.clone(), h.dir.join("operator.log.aside"));
        let (a, first) = {
            let (oplog, aside) = (oplog.clone(), aside.clone());
            delete_holding(&h, &token, move |_| {
                std::fs::rename(&oplog, &aside).unwrap();
                std::fs::create_dir(&oplog).unwrap();
                Ok(())
            })
            .await
        };
        std::fs::remove_dir(&oplog).unwrap();
        std::fs::rename(&aside, &oplog).unwrap();
        if !first && tries < 5 {
            continue;
        }
        assert!(first, "the deletion came first in {tries} tries");
        assert_eq!(
            a.json,
            json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0}),
            "{}",
            a.text
        );
        assert!(account_of(&h, HONG.0).await.is_none());
        assert_eq!(deletions(&h).await, 1);
        assert_eq!(deletion_lines(&h).len(), 1);
        let record = std::fs::read_to_string(&oplog).unwrap();
        assert_eq!(oplog::verify(&oplog).unwrap(), record.lines().count());
        assert!(journal_has(&format!(
            "account delete: the operator record did not take the result line (acct={}); the account is deleted",
            h.account_tag(HONG.0)
        )));
        break;
    }
}

/// A client that goes away while its deletion waits for the database (the app's time limit, a
/// network change: nginx then closes the connection) does not cut the deletion short: it ends
/// with its result line, its record and journal line, and the app's retry of the same bytes gets
/// its first answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_deletion_runs_to_its_end_when_the_client_goes_away() {
    for tries in 1.. {
        let h = Arc::new(Harness::with(own_tags()).await);
        let token = h.login_ok(HONG, None).await;
        let tag = h.account_tag(HONG.0);
        let signed = h.proof_for("POST", DELETE, CONFIRM_BODY, &token).unwrap();
        let hash = sha256(token.as_bytes());
        let sent = proof::request_hash(&signed, "POST", DELETE, &sha256(CONFIRM_BODY), &hash);
        let key = format!("account-delete.{}", hex(&sent));
        // the app's request, on a connection of its own
        let mut stream = tokio::net::TcpStream::connect(h.api).await.unwrap();
        let head = format!(
            "POST {DELETE} HTTP/1.1\r\nHost: hjv\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nX-HJV-Proof: {signed}\r\nContent-Length: {}\r\n\r\n",
            CONFIRM_BODY.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(CONFIRM_BODY).await.unwrap();
        let started = Instant::now();
        while h.state.request_retries.again(&key, sent).is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "no checks passed"
            );
            std::thread::yield_now();
        }
        // the database busy a moment (a backup, the upkeep): the deletion waits for it, and the
        // client goes away meanwhile
        let hold = {
            let (state, oplog) = (h.state.clone(), h.oplog.clone());
            tokio::spawn(async move {
                state
                    .db
                    .call(move |c| {
                        let started = Instant::now();
                        while first_lines(&oplog) == 0 {
                            assert!(started.elapsed() < Duration::from_secs(5), "no first line");
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        let first = c.query_row(
                            "SELECT EXISTS (SELECT 1 FROM tokens WHERE hash = ?1)",
                            [&hash[..]],
                            |r| r.get::<_, bool>(0),
                        )?;
                        std::thread::sleep(Duration::from_millis(600));
                        Ok(first)
                    })
                    .await
                    .unwrap()
            })
        };
        tokio::time::sleep(Duration::from_millis(200)).await;
        drop(stream);
        let first = hold.await.unwrap();
        // the journal's line is the deletion's last step, after its result line and record
        let line = format!(
            "account: deleted at its own request (acct={tag}): 0 PCs, 1 account keys, 1 tokens, 0 relays, allowlist entry removed"
        );
        let started = Instant::now();
        while !journal_has(&line) {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the deletion did not end: {:?}",
                deletion_lines(&h)
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        if !first && tries < 5 {
            continue;
        }
        assert!(first, "the deletion came first in {tries} tries");
        assert!(account_of(&h, HONG.0).await.is_none());
        let lines = deletion_lines(&h);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[1].contains(&format!(" acct={tag} done=\"ok: 0 PCs, ")),
            "{lines:?}"
        );
        assert_eq!(deletions(&h).await, 1);
        let again = delete_with(&h, &token, Some(&signed)).await;
        assert_eq!(
            (again.status, &again.json),
            (
                200,
                &json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0})
            ),
            "{}",
            again.text
        );
        break;
    }
}

/// The very same request twice at once (the app's retry while the first still waits): one
/// deletion, and both get its answer.
#[tokio::test]
async fn the_very_same_request_twice_at_once() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let signed = h.proof_for("POST", DELETE, CONFIRM_BODY, &token).unwrap();
    // the database held a moment: both wait for it in their authentication
    let hold = {
        let state = h.state.clone();
        tokio::spawn(async move {
            state
                .db
                .call(|_| {
                    std::thread::sleep(Duration::from_millis(500));
                    Ok(())
                })
                .await
                .unwrap()
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    let (a, b) = tokio::join!(
        delete_with(&h, &token, Some(&signed)),
        delete_with(&h, &token, Some(&signed))
    );
    hold.await.unwrap();
    assert_eq!(
        (a.status, &a.json),
        (
            200,
            &json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0})
        ),
        "{}",
        a.text
    );
    assert_eq!((b.status, &b.text), (200, &a.text));
    assert_eq!(deletions(&h).await, 1);
    assert_eq!(deletion_lines(&h).len(), 2);
}

/// A database error deletes nothing: 500, the result line `failed: database` and the journal's
/// line by the tag, no record, the token still good; the very same request again gets the same
/// answer, and a new one deletes the account once the database works.
#[tokio::test]
async fn a_database_error_deletes_nothing() {
    let h = Harness::with(own_tags()).await;
    let token = h.login_ok(HONG, None).await;
    let tag = h.account_tag(HONG.0);
    h.state
        .db
        .call(|c| {
            c.execute_batch(
                "CREATE TEMP TRIGGER held BEFORE DELETE ON accounts
                 BEGIN SELECT RAISE(ABORT, 'held'); END;",
            )
        })
        .await
        .unwrap();
    let signed = h.proof_for("POST", DELETE, CONFIRM_BODY, &token).unwrap();
    let a = delete_with(&h, &token, Some(&signed)).await;
    let failed = json!({"error": "서버 오류입니다. 잠시 뒤 다시 시도해 주세요."});
    assert_eq!((a.status, &a.json), (500, &failed), "{}", a.text);
    assert!(account_of(&h, HONG.0).await.is_some());
    let me = h.post("/api/currentUser", &token, Some(json!({}))).await;
    assert_eq!(me.status, 200, "{}", me.text);
    assert_eq!(deletions(&h).await, 0);
    let lines = deletion_lines(&h);
    assert!(
        lines[1].contains(&format!(" acct={tag} done=\"failed: database\" ")),
        "{lines:?}"
    );
    let suffix = format!(" (acct={tag}); nothing was deleted");
    assert!(hjv_api::log::captured()
        .iter()
        .any(|l| l.starts_with("account delete: database: ") && l.ends_with(&suffix)));
    let again = delete_with(&h, &token, Some(&signed)).await;
    assert_eq!(
        (again.status, &again.json),
        (500, &failed),
        "{}",
        again.text
    );
    h.state
        .db
        .call(|c| c.execute_batch("DROP TRIGGER held"))
        .await
        .unwrap();
    let a = h.post(DELETE, &token, confirm()).await;
    assert_eq!(
        a.json,
        json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0}),
        "{}",
        a.text
    );
}
