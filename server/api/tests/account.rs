// SPDX-License-Identifier: AGPL-3.0-only

//! The account paths after login: the shapes the app's Dart models read (research notes §4), the
//! device list and its rules (design v2 §5, D9, D10), logout, token expiry and revocation.

mod common;

use common::*;
use hjv_api::db;
use serde_json::json;

#[tokio::test]
async fn current_user_users_and_logout() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let body = json!({"id": "", "uuid": "dXVpZA=="});
    // currentUser answers the user object itself
    let a = h.post("/api/currentUser", &token, Some(body.clone())).await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(
        a.json,
        json!({"name": "2026000001", "display_name": "홍길동", "status": 1})
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
        json!({"total": 1, "data": [{"name": "2026000001", "display_name": "홍길동", "status": 1}]})
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
    assert_eq!(a.json["message"], "이 PC를 내 기기 목록에 추가했습니다.");
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
            "id": "123456789", "user_name": "2026000001",
            "info": {"device_name": "PC-123456789", "os": "windows"}, "status": 1,
            "note": "비밀번호 없는 연결 일시 중지(관리자)",
            // no NAS relay in the account: nothing could wake it (wake.rs peer_wol)
            "hjv": {
                "pk": hjv_api::util::b64(&pc.public_key()), "pwless": "disabled",
                "kinds": ["remote", "file", "camera"], "wol": "no",
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
    // "내 목록에서 빼기" from another device
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
        .admin(&["allow", "remove"], false, Some("2026000001\n"))
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
