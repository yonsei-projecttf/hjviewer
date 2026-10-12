// SPDX-License-Identifier: AGPL-3.0-only

//! The account keys of design v3 (§4-1, §4-7, §5-2, §5-6): registration at login, the login's
//! answer, logout with and without the key's signature, the keys paths and the new-device notice,
//! the device list's password-less fields, the switches and the admin commands; the test vectors
//! of README.md (made with libsodium, the library behind the client's sodiumoxide) and the ones
//! shared with the client (tests/data/v3-vectors.json, WP-C0).

mod common;

use common::*;
use hjv_api::{
    api::pwless_note,
    db, jobs,
    keys::{self, KeyClaim, TEXT_TOO_MANY},
    login::FAILURE_TEXT,
    proof,
    util::{b64, hex, now, random_bytes},
};
use hyper::Method;
use serde_json::{json, Value};

fn assert_failure(a: &Answer, status: u16) {
    assert_eq!(a.status, status, "{}", a.text);
    assert_eq!(a.json, json!({ "error": FAILURE_TEXT }), "{}", a.text);
}

/// The account keys in the database, oldest first.
async fn stored_keys(h: &Harness) -> Vec<db::Key> {
    h.state.db.call(|c| db::all_keys(c)).await.unwrap()
}

async fn serial(h: &Harness) -> i64 {
    h.state.db.call(|c| db::change_serial(c)).await.unwrap()
}

/// A login of `account` with `key`; the answer.
async fn login_with(
    h: &Harness,
    account: (&str, &str),
    device: Option<&TestDevice>,
    key: KeyChoice,
    ip: &str,
) -> Answer {
    let body = h.login_body_with(
        account.0,
        account.1,
        device,
        now(),
        &b64(&random_bytes::<16>()),
        key,
    );
    h.login(&body, ip).await
}

/// A token request with only the Authorization header, as a stolen token would come.
async fn token_only(h: &Harness, method: Method, path: &str, token: &str) -> Answer {
    let auth = format!("Bearer {token}");
    h.request(method, path, &[("authorization", &auth)], None)
        .await
}

fn ckids(a: &Answer) -> Vec<i64> {
    a.json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["ckid"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn a_login_registers_its_account_key() {
    let h = Harness::start().await;
    // the first login: ks is the serial its registration made
    let a = login_with(&h, HONG, None, KeyChoice::Fresh, "10.3.0.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json["hjv"], json!({ "ks": 1 }));
    assert_eq!(serial(&h).await, 1);
    let first = a.token();
    let seed = h.key_of(&first).unwrap();
    let keys = stored_keys(&h).await;
    assert_eq!(keys.len(), 1);
    assert_eq!(
        keys[0].public_key,
        hjv_api::devsig::public_key(&seed).to_vec()
    );
    assert_eq!(keys[0].key_serial, 1);
    // the next login's key comes with a larger serial
    let a = login_with(&h, KIM, None, KeyChoice::Fresh, "10.3.0.2").await;
    assert_eq!(a.json["hjv"]["ks"], 2);
    // without ck and cp, with a cp of another key, with a key another login has (in this or
    // another account): the failure of every login, after the SSO accepted the password
    let calls = h.sso.token_calls();
    for (key, result) in [
        (KeyChoice::Omit, "key-missing"),
        (KeyChoice::WrongProof, "key-bad"),
        (KeyChoice::Seed(seed), "key-in-use"),
    ] {
        for account in [HONG, LEE] {
            let a = login_with(&h, account, None, key, "10.3.0.3").await;
            assert_failure(&a, 400);
            assert_eq!(
                h.results().await.last().unwrap(),
                &format!("login:{result}")
            );
        }
    }
    assert_eq!(h.sso.token_calls(), calls + 6);
    // nothing was written for them: no key, no token, no change
    assert_eq!(stored_keys(&h).await.len(), 2);
    assert_eq!(serial(&h).await, 2);
    // and they counted against neither the ID nor its successes
    assert_eq!(
        h.counted(hjv_api::limits::SCOPE_ID, h.account_tag(LEE.0))
            .await,
        0
    );
    assert_eq!(h.get("/api/users", &first).await.status, 200);
}

#[tokio::test]
async fn the_same_device_replaces_its_key() {
    let h = Harness::start().await;
    let phone = TestDevice::phone("987654321");
    let first = h.login_ok(HONG, Some(&phone)).await;
    let other = h.login_ok(HONG, None).await;
    let again = h.login_ok(HONG, Some(&phone)).await;
    // the phone's first token and key went, the other device's stayed
    assert_eq!(h.get("/api/users", &first).await.status, 401);
    assert_eq!(h.get("/api/users", &again).await.status, 200);
    assert_eq!(h.get("/api/users", &other).await.status, 200);
    let keys = stored_keys(&h).await;
    assert_eq!(keys.len(), 3);
    assert_eq!(keys[0].revoke_reason.as_deref(), Some("same-device"));
    assert!(keys[1].revoked_at.is_none() && keys[2].revoked_at.is_none());
    // the device tag is a tag, never the uuid
    assert_eq!(keys[0].dev_tag, keys[2].dev_tag);
    assert_eq!(keys[0].dev_tag.len(), 16);
    assert!(!keys.iter().any(|k| k.dev_tag.contains(&phone.uuid)));
    // the same uuid in another account is another account's device
    h.login_ok(KIM, Some(&phone)).await;
    assert_eq!(h.get("/api/users", &again).await.status, 200);
}

#[tokio::test]
async fn an_account_has_twenty_active_keys_at_most() {
    let h = Harness::with(Options {
        vars: vec![
            ("HJV_API_LIMIT_OK_ID", "100/3600".into()),
            ("HJV_API_LIMIT_OK_IP", "100/3600".into()),
            ("HJV_API_LIMIT_SSO", "400/60".into()),
        ],
        ..Default::default()
    })
    .await;
    let phone = TestDevice::phone("987654321");
    let phone_token = h.login_ok(HONG, Some(&phone)).await;
    for _ in 0..19 {
        h.login_ok(HONG, None).await;
    }
    // a 21st device: refused with its own text, after the SSO
    let a = login_with(&h, HONG, None, KeyChoice::Fresh, "10.3.1.1").await;
    assert_eq!(a.status, 403, "{}", a.text);
    assert_eq!(
        a.json,
        json!({ "error": TEXT_TOO_MANY, "hjv": "too-many-keys" })
    );
    assert_eq!(h.results().await.last().unwrap(), "login:key-limit");
    // a device that has a key replaces it
    let again = h.login_ok(HONG, Some(&phone)).await;
    assert_eq!(h.get("/api/users", &phone_token).await.status, 401);
    // once one is revoked, another device gets in
    let list = h.get("/api/hjv/keys?current=1&pageSize=100", &again).await;
    assert_eq!(list.json["total"], 20);
    let oldest = *ckids(&list).last().unwrap();
    let a = h.delete(&format!("/api/hjv/keys/{oldest}"), &again).await;
    assert_eq!(a.json, json!({"revoked": true}));
    h.login_ok(HONG, None).await;
    // other accounts are not affected
    h.login_ok(KIM, None).await;
}

#[tokio::test]
async fn binding_and_login_time_only_for_an_installed_windows_pc() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    let before = now();
    let a = login_with(&h, HONG, Some(&pc), KeyChoice::Fresh, "10.3.2.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let binding = a.json["hjv"]["binding"].as_str().unwrap().to_owned();
    assert_eq!(binding.len(), 32);
    assert!(binding
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    let login_at = a.json["hjv"]["login_at"].as_i64().unwrap();
    assert!(login_at >= before && login_at <= now());
    let stored = |id: &'static str| {
        h.state.db.call(move |c| {
            c.query_row(
                "SELECT binding_id, login_at FROM devices WHERE device_id = ?1",
                [id],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, i64>(1)?)),
            )
        })
    };
    let (id, at) = stored("123456789").await.unwrap();
    assert_eq!((hex(&id), at), (binding.clone(), login_at));
    // the same account again: the same binding, the login time of this login
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let a = login_with(&h, HONG, Some(&pc), KeyChoice::Fresh, "10.3.2.1").await;
    assert_eq!(a.json["hjv"]["binding"], binding.as_str());
    let again = a.json["hjv"]["login_at"].as_i64().unwrap();
    assert!(again > login_at);
    assert_eq!(stored("123456789").await.unwrap().1, again);
    // another account on the PC, a phone, a PC run without installing, one hbbs does not know:
    // a key, no binding
    let mut portable = TestDevice::windows("200000001");
    portable.installed = false;
    h.hbbs.register(&portable, Some(1));
    let unknown = TestDevice::windows("200000002");
    for (account, device) in [
        (KIM, pc.clone()),
        (HONG, TestDevice::phone("987654321")),
        (HONG, portable),
        (HONG, unknown),
    ] {
        let a = login_with(&h, account, Some(&device), KeyChoice::Fresh, "10.3.2.2").await;
        assert_eq!(a.status, 200, "{}", a.text);
        let hjv = a.json["hjv"].as_object().unwrap();
        assert!(hjv.contains_key("ks"), "{}", a.text);
        assert!(
            !hjv.contains_key("binding") && !hjv.contains_key("login_at"),
            "{}",
            a.text
        );
    }
    // Kim's login on Hong's PC changed nothing of the binding
    assert_eq!(stored("123456789").await.unwrap(), (id, again));
}

#[tokio::test]
async fn the_server_names_the_device_and_its_network() {
    let h = Harness::with(Options {
        vars: vec![(
            "HJV_API_CAMPUS_NETS",
            "10.20.0.0/16, 2001:db8:20::/48".into(),
        )],
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    let phone = h.login_ok_from(HONG, None, "10.20.3.4").await;
    h.login_ok_from(HONG, Some(&pc), "198.51.100.7").await;
    h.login_ok_from(HONG, None, "2001:db8:20:1::9").await;
    let list = h.get("/api/hjv/keys", &phone).await;
    assert_eq!(list.status, 200, "{}", list.text);
    let data = list.json["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);
    // newest first: the IPv6 login on campus, the PC off campus, the phone on campus
    assert_eq!(data[0]["net"], "on-campus");
    let pc_key = &data[1];
    assert_eq!(pc_key["name"], "Windows PC");
    assert_eq!(pc_key["app_name"], "PC-123456789");
    assert_eq!(pc_key["platform"], "windows");
    assert_eq!(pc_key["app_ver"], "1.6.0");
    assert_eq!(pc_key["net"], "off-campus");
    let phone_key = &data[2];
    assert_eq!(phone_key["name"], "iPhone 14 Pro");
    assert_eq!(phone_key["app_name"], "iPhone15,2");
    assert_eq!(phone_key["platform"], "ios");
    // a login without the hjv part says no version
    assert_eq!(phone_key["app_ver"], "");
    assert_eq!(phone_key["net"], "on-campus");
    assert_eq!(phone_key["current"], true);
    // no address is kept with the keys
    let rows: Vec<String> = h
        .state
        .db
        .call(|c| {
            let mut s = c.prepare(
                "SELECT dev_tag || ctl_id || platform || model || app_name || app_ver || login_net FROM ctl_keys",
            )?;
            let rows = s.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap();
    for row in rows {
        for ip in ["10.20.3.4", "198.51.100.7", "2001:db8:20:1::9"] {
            assert!(!row.contains(ip), "{row}");
        }
    }
    // without HJV_API_CAMPUS_NETS no network is told
    let h = Harness::start().await;
    let token = h.login_ok_from(HONG, None, "10.20.3.4").await;
    assert_eq!(
        h.get("/api/hjv/keys", &token).await.json["data"][0]["net"],
        ""
    );
}

#[tokio::test]
async fn logout_with_and_without_the_keys_signature() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    let phone = h.login_ok(HONG, None).await;
    // a stolen token without its key: the token and its key end at once (design v3 §1-8: the
    // keys of logged-in devices only), but the PC stays in the list
    let pc_token = h.login_ok(HONG, Some(&pc)).await;
    let serial_before = serial(&h).await;
    let a = token_only(&h, Method::POST, "/api/logout", &pc_token).await;
    assert_eq!((a.status, a.json.clone()), (200, json!({})));
    assert_eq!(h.get("/api/users", &pc_token).await.status, 401);
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 1);
    let active = |keys: &[db::Key]| keys.iter().filter(|k| k.revoked_at.is_none()).count();
    let keys = stored_keys(&h).await;
    assert_eq!(active(&keys), 1);
    assert_eq!(keys[1].revoke_reason.as_deref(), Some("logout-unsigned"));
    assert_eq!(serial(&h).await, serial_before + 1);
    assert_eq!(h.get("/api/hjv/keys", &phone).await.json["total"], 1);
    // nothing is left for the upkeep
    let purged = jobs::purge_now(&h.state).await.unwrap();
    assert_eq!(purged.keys_revoked, 0);
    // signed by its key: the token, the key, and the PC out of the list
    let pc_token = h.login_ok(HONG, Some(&pc)).await;
    let a = h
        .post("/api/logout", &pc_token, Some(json!({"id": "123456789"})))
        .await;
    assert_eq!((a.status, a.json.clone()), (200, json!({})));
    assert_eq!(h.get("/api/users", &pc_token).await.status, 401);
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 0);
    let keys = stored_keys(&h).await;
    assert_eq!(
        keys.last().unwrap().revoke_reason.as_deref(),
        Some("logout")
    );
    assert_eq!(active(&keys), 1);
    // signed, but by a device whose clock is off: still the device's own logout, which no one
    // can repeat (the token it is bound to ends with it)
    let pc_token = h.login_ok(HONG, Some(&pc)).await;
    let body = json!({"id": "123456789"}).to_string();
    let header = proof::header_value(
        &h.key_of(&pc_token).unwrap(),
        "POST",
        "/api/logout",
        now() - 3600,
        &random_bytes::<16>(),
        body.as_bytes(),
        &pc_token,
    );
    let auth = format!("Bearer {pc_token}");
    let a = h
        .request(
            Method::POST,
            "/api/logout",
            &[
                ("authorization", &auth),
                ("content-type", "application/json"),
                ("x-hjv-proof", &header),
            ],
            Some(body.into_bytes()),
        )
        .await;
    assert_eq!((a.status, a.json.clone()), (200, json!({})));
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 0);
    assert_eq!(
        stored_keys(&h)
            .await
            .last()
            .unwrap()
            .revoke_reason
            .as_deref(),
        Some("logout")
    );
    let results = h.results().await;
    for r in [
        "key:revoked-logout-unsigned",
        "key:revoked-logout",
        "device:removed-logout",
    ] {
        assert!(results.contains(&r.to_owned()), "{r}: {results:?}");
    }
    assert_eq!(
        results
            .iter()
            .filter(|r| *r == "device:removed-logout")
            .count(),
        2
    );
}

#[tokio::test]
async fn the_keys_paths() {
    let h = Harness::start().await;
    let a = h
        .login_ok(HONG, Some(&TestDevice::phone("900000001")))
        .await;
    let b = h
        .login_ok(HONG, Some(&TestDevice::phone("900000002")))
        .await;
    let kim = h.login_ok(KIM, None).await;
    let list = h.get("/api/hjv/keys", &a).await;
    assert_eq!(list.status, 200, "{}", list.text);
    assert_eq!(list.json["total"], 2);
    let (kb, ka) = (ckids(&list)[0], ckids(&list)[1]);
    let entry = |list: &Answer, ckid: i64| -> Value {
        list.json["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["ckid"] == ckid)
            .unwrap()
            .clone()
    };
    // B logged in after A and nobody confirmed it: new on A, not on B itself
    let on_a = entry(&list, kb);
    assert_eq!(
        (on_a["new"].clone(), on_a["current"].clone()),
        (json!(true), json!(false))
    );
    assert_eq!(entry(&list, ka)["current"], true);
    assert_eq!(entry(&list, ka)["new"], false);
    assert_eq!(on_a["use"], json!([]));
    assert!(on_a["last_used"].is_null());
    let list_b = h.get("/api/hjv/keys", &b).await;
    assert_eq!(entry(&list_b, ka)["new"], false);
    assert_eq!(entry(&list_b, kb)["new"], false);
    // "내 기기 맞음"
    let ack = h.post(&format!("/api/hjv/keys/{kb}/ack"), &a, None).await;
    assert_eq!(ack.json, json!({"ack": true}));
    assert_eq!(entry(&h.get("/api/hjv/keys", &a).await, kb)["new"], false);
    // another account's key, an unknown one: the same 404
    let kim_key = ckids(&h.get("/api/hjv/keys", &kim).await)[0];
    for path in [
        format!("/api/hjv/keys/{kim_key}"),
        "/api/hjv/keys/999999".to_owned(),
        "/api/hjv/keys/abc".to_owned(),
    ] {
        let d = h.delete(&path, &a).await;
        assert_eq!(
            (d.status, d.json.clone()),
            (404, json!({"error": "not found"}))
        );
        let ack = h.post(&format!("{path}/ack"), &a, None).await;
        assert_eq!(
            (ack.status, ack.json.clone()),
            (404, json!({"error": "not found"}))
        );
    }
    assert_eq!(h.get("/api/users", &kim).await.status, 200);
    // "로그아웃시키기": B's key and token go, and the change serial with them
    let before = serial(&h).await;
    let d = h.delete(&format!("/api/hjv/keys/{kb}"), &a).await;
    assert_eq!(d.json, json!({"revoked": true}));
    assert_eq!(serial(&h).await, before + 1);
    assert_eq!(h.get("/api/users", &b).await.status, 401);
    assert_eq!(
        h.delete(&format!("/api/hjv/keys/{kb}"), &a).await.status,
        404
    );
    assert_eq!(ckids(&h.get("/api/hjv/keys", &a).await), vec![ka]);
    // "다른 기기 모두 로그아웃"
    let c = h.login_ok(HONG, None).await;
    let d = h.login_ok(HONG, None).await;
    let r = h.post("/api/hjv/keys/revoke-others", &c, None).await;
    assert_eq!(r.json, json!({"revoked": 2}));
    for token in [&a, &d] {
        assert_eq!(h.get("/api/users", token).await.status, 401);
    }
    assert_eq!(h.get("/api/users", &c).await.status, 200);
    assert_eq!(
        h.post("/api/hjv/keys/revoke-others", &c, None).await.json,
        json!({"revoked": 0})
    );
    // a new key is new for seven days
    let e = h.login_ok(HONG, None).await;
    let ke = ckids(&h.get("/api/hjv/keys", &c).await)[0];
    assert_eq!(entry(&h.get("/api/hjv/keys", &c).await, ke)["new"], true);
    h.state
        .db
        .call(move |c| {
            c.execute(
                "UPDATE ctl_keys SET created_at = created_at - 8 * 86400 WHERE ckid = ?1",
                [ke],
            )
        })
        .await
        .unwrap();
    assert_eq!(entry(&h.get("/api/hjv/keys", &c).await, ke)["new"], false);
    // a device may revoke its own key, as a logout
    let mine = ckids(&h.get("/api/hjv/keys", &e).await)[0];
    assert_eq!(mine, ke);
    assert_eq!(
        h.delete(&format!("/api/hjv/keys/{mine}"), &e).await.json,
        json!({"revoked": true})
    );
    assert_eq!(h.get("/api/users", &e).await.status, 401);
    let results = h.results().await;
    for r in ["key:revoked-user", "key:revoked-others"] {
        assert!(results.contains(&r.to_owned()), "{r}: {results:?}");
    }
}

/// The account's active keys as a device sees them: (ckid, new), newest first.
async fn notice(h: &Harness, token: &str) -> Vec<(i64, bool)> {
    let list = h.get("/api/hjv/keys", token).await;
    assert_eq!(list.status, 200, "{}", list.text);
    list.json["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| (k["ckid"].as_i64().unwrap(), k["new"].as_bool().unwrap()))
        .collect()
}

async fn ack(h: &Harness, ckid: i64, token: &str) -> Answer {
    h.post(&format!("/api/hjv/keys/{ckid}/ack"), token, None)
        .await
}

/// The new-device notice is how the owner learns that someone else has the portal password
/// (design v3 §0, §1-1): the newcomer must not be able to silence it.
#[tokio::test]
async fn a_newcomer_cannot_silence_the_new_device_notice() {
    let h = Harness::start().await;
    let owner = h
        .login_ok(HONG, Some(&TestDevice::phone("900000001")))
        .await;
    // someone with the portal password, on a device of their own
    let (x, y) = (
        TestDevice::phone("900000011"),
        TestDevice::phone("900000012"),
    );
    let first = h.login_ok(HONG, Some(&x)).await;
    let [(k2, true), (k1, false)] = notice(&h, &owner).await[..] else {
        panic!("{:?}", notice(&h, &owner).await)
    };
    // it confirms neither itself nor an older key: the 404 of a key the account does not have
    for ckid in [k2, k1] {
        let a = ack(&h, ckid, &first).await;
        assert_eq!(
            (a.status, a.json.clone()),
            (404, json!({"error": "not found"}))
        );
    }
    assert_eq!(notice(&h, &owner).await, [(k2, true), (k1, false)]);
    // nor through a chain of its own logins: the first confirms the second, a new login of the
    // first device replaces its key, and the second confirms that one
    let second = h.login_ok(HONG, Some(&y)).await;
    let k3 = notice(&h, &owner).await[0].0;
    assert_eq!(ack(&h, k3, &first).await.json, json!({"ack": true}));
    let third = h.login_ok(HONG, Some(&x)).await;
    let k4 = notice(&h, &owner).await[0].0;
    assert_eq!(ack(&h, k4, &second).await.json, json!({"ack": true}));
    assert_eq!(h.get("/api/users", &first).await.status, 401);
    // the owner still sees both as new; the newcomer's devices, which confirmed them, do not
    assert_eq!(
        notice(&h, &owner).await,
        [(k4, true), (k3, true), (k1, false)]
    );
    assert_eq!(
        notice(&h, &second).await,
        [(k4, false), (k3, false), (k1, false)]
    );
    // a confirmation counts for the device that gave it and the ones that logged in after it
    assert_eq!(ack(&h, k3, &owner).await.json, json!({"ack": true}));
    assert_eq!(
        notice(&h, &owner).await,
        [(k4, true), (k3, false), (k1, false)]
    );
    // "아님: 로그아웃시키기"
    for ckid in [k4, k3] {
        let d = h.delete(&format!("/api/hjv/keys/{ckid}"), &owner).await;
        assert_eq!(d.json, json!({"revoked": true}));
    }
    for token in [&second, &third] {
        assert_eq!(h.get("/api/users", token).await.status, 401);
    }
    assert_eq!(notice(&h, &owner).await, [(k1, false)]);
}

#[tokio::test]
async fn the_keys_paths_take_thirty_calls_a_minute() {
    let h = Harness::start().await;
    let token = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    for _ in 0..keys::CALLS_PER_MINUTE {
        assert_eq!(h.get("/api/hjv/keys", &token).await.status, 200);
    }
    let a = h.get("/api/hjv/keys", &token).await;
    assert_eq!(a.status, 429, "{}", a.text);
    assert!(a.json["error"].is_string());
    // per account; the other paths do not count
    assert_eq!(h.get("/api/hjv/keys", &kim).await.status, 200);
    assert_eq!(h.get("/api/users", &token).await.status, 200);
}

#[tokio::test]
async fn the_device_list_carries_the_password_less_state() {
    let h = Harness::with(Options {
        vars: vec![("HJV_API_PASSWORDLESS_KINDS", "remote,file".into())],
        ..Default::default()
    })
    .await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let phone = h.login_ok(HONG, None).await;
    let peer = |a: &Answer| a.json["data"][0].clone();
    // off when installed: paused by the administrator
    let p = peer(&h.get("/api/peers", &phone).await);
    assert_eq!(p["note"], "비밀번호 없는 연결 일시 중지(관리자)");
    assert_eq!(
        p["hjv"],
        json!({
            "pk": b64(&pc.public_key()), "pwless": "disabled",
            "kinds": ["remote", "file"], "wol": "unknown",
        })
    );
    // switched on for the owner of the PC: listed until the PC confirms through its sync
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    let p = peer(&h.get("/api/peers", &phone).await);
    assert_eq!(p["hjv"]["pwless"], "listed");
    assert_eq!(p["note"], pwless_note("listed"));
    assert_eq!(p["note"], "이 PC에서 로그인 필요");
    // what the app's card shows stays as it was
    assert_eq!(p["info"]["device_name"], "PC-123456789");
}

#[tokio::test]
async fn switches_by_account_and_by_pc() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let hong = h.login_ok(HONG, None).await;
    let kim_pc = TestDevice::windows("223456789");
    h.hbbs.register(&kim_pc, Some(1));
    h.login_ok(KIM, Some(&kim_pc)).await;
    let kim = h.login_ok(KIM, None).await;
    let state = |token: String| {
        let h = &h;
        async move { h.get("/api/peers", &token).await.json["data"][0]["hjv"]["pwless"].clone() }
    };
    let out = h
        .admin(&["passwordless", "status"], false, None)
        .await
        .unwrap();
    assert!(
        out.starts_with("password-less connections: off for every account; strict off"),
        "{out}"
    );
    let start = serial(&h).await;
    // the operator's account first, named by its PC (no portal ID in the agent's hands)
    let out = h
        .admin(
            &["passwordless", "on", "--owner-of", "123456789"],
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        out,
        format!(
            "password-less connections: on for the account {}",
            h.account_tag(HONG.0)
        )
    );
    assert_eq!(state(hong.clone()).await, "listed");
    assert_eq!(state(kim.clone()).await, "disabled");
    let out = h
        .admin(&["passwordless", "status"], false, None)
        .await
        .unwrap();
    assert!(out.contains("accounts switched one by one: 1"), "{out}");
    assert!(
        out.contains(&h.account_tag(HONG.0)) && out.contains("2026******"),
        "{out}"
    );
    assert!(!out.contains("2026000001"), "{out}");
    assert!(h
        .admin(&["passwordless", "status"], true, None)
        .await
        .unwrap()
        .contains("2026000001"));
    // a PC no account has, a device ID that is none
    for bad in ["999999999", "12 34"] {
        assert!(h
            .admin(&["passwordless", "on", "--owner-of", bad], false, None)
            .await
            .is_err());
    }
    // everyone, then one account off by its portal ID
    h.admin(&["passwordless", "on"], false, None).await.unwrap();
    assert_eq!(state(kim.clone()).await, "listed");
    h.admin(&["passwordless", "off", "--account", KIM.0], false, None)
        .await
        .unwrap();
    assert_eq!(state(kim.clone()).await, "disabled");
    assert_eq!(state(hong.clone()).await, "listed");
    // off for everyone is the incident switch: the account switched on alone goes off too
    let out = h
        .admin(&["passwordless", "off"], false, None)
        .await
        .unwrap();
    assert_eq!(out, "password-less connections: off for every account");
    assert_eq!(state(hong.clone()).await, "disabled");
    let out = h
        .admin(&["passwordless", "off"], false, None)
        .await
        .unwrap();
    assert!(out.ends_with("(no change)"), "{out}");
    // strict
    assert_eq!(
        h.admin(&["strict", "on"], false, None).await.unwrap(),
        "strict: on"
    );
    assert!(h
        .admin(&["passwordless", "status"], false, None)
        .await
        .unwrap()
        .contains("strict on"));
    h.admin(&["strict", "off"], false, None).await.unwrap();
    // every change raised the serial once: on (Hong), on (all), off (Kim), off (all), strict ×2
    assert_eq!(serial(&h).await, start + 6);
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains(&format!("change serial {}", start + 6)),
        "{report}"
    );
    assert!(
        report.contains(
            "password-less: off for every account, accounts on 0, accounts off 1; strict off"
        ),
        "{report}"
    );
    // the record names the PC and the accounts by tags only
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(
        record.contains(&format!(
            "cmd=\"passwordless on\" targets={}",
            h.state.tags.device("123456789")
        )),
        "{record}"
    );
    assert!(
        record.contains(&format!("targets={}", h.account_tag(KIM.0))),
        "{record}"
    );
    for id in ["123456789", "2026000002"] {
        assert!(!record.contains(id), "{id}: {record}");
    }
}

#[tokio::test]
async fn admin_keys_and_revoking_everything() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    let pc_token = h.login_ok(HONG, Some(&pc)).await;
    let phone = h
        .login_ok(HONG, Some(&TestDevice::phone("987654321")))
        .await;
    let kim = h.login_ok(KIM, None).await;
    let out = h.admin(&["key", "list"], false, None).await.unwrap();
    assert!(
        out.starts_with("account keys: 3 active, 0 revoked"),
        "{out}"
    );
    assert!(
        out.contains("iPhone 14 Pro") && out.contains("Windows PC"),
        "{out}"
    );
    assert!(out.contains(&h.account_tag(HONG.0)), "{out}");
    // names the devices gave themselves, and device IDs, only with --reveal
    assert!(
        !out.contains("PC-123456789") && !out.contains("987654321"),
        "{out}"
    );
    let shown = h.admin(&["key", "list"], true, None).await.unwrap();
    assert!(
        shown.contains("PC-123456789") && shown.contains("987654321"),
        "{shown}"
    );
    let out = h.admin(&["key", "list", KIM.0], false, None).await.unwrap();
    assert!(out.starts_with("account keys: 1 active"), "{out}");
    // one key
    let phone_key = stored_keys(&h).await[1].ckid;
    let out = h
        .admin(&["key", "revoke", &phone_key.to_string()], false, None)
        .await
        .unwrap();
    assert_eq!(
        out,
        format!("revoked account key {phone_key} and its token")
    );
    assert_eq!(h.get("/api/users", &phone).await.status, 401);
    assert_eq!(
        h.admin(&["key", "revoke", &phone_key.to_string()], false, None)
            .await
            .unwrap(),
        "no such active account key"
    );
    assert!(h.admin(&["key", "revoke", "x"], false, None).await.is_err());
    // every key of every account; the PCs stay in their accounts
    let out = h
        .admin(&["user", "revoke-all", "--all"], false, None)
        .await
        .unwrap();
    assert_eq!(out, "revoked 2 account keys and 2 tokens of every account");
    for token in [&pc_token, &kim] {
        assert_eq!(h.get("/api/users", token).await.status, 401);
    }
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(h.get("/api/peers", &phone).await.json["total"], 1);
    let out = h
        .admin(&["user", "revoke-all", HONG.0], false, None)
        .await
        .unwrap();
    assert_eq!(out, "revoked 1 account keys and 1 tokens");
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("account keys: 0 active; revoked, kept 30 days: admin 4"),
        "{report}"
    );
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(
        record.contains(&format!("cmd=\"key revoke\" targets=ckid:{phone_key}")),
        "{record}"
    );
    assert!(
        record.contains("cmd=\"user revoke-all\" targets=all"),
        "{record}"
    );
}

#[tokio::test]
async fn leaving_the_allowlist_revokes_the_keys() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    let out = h
        .admin(&["allow", "remove"], false, Some("2026000001\n"))
        .await
        .unwrap();
    assert_eq!(
        out,
        "allowlist: 1 removed, 0 not on it, 2 tokens and 2 account keys revoked"
    );
    let keys = stored_keys(&h).await;
    let hong_keys: Vec<&db::Key> = keys
        .iter()
        .filter(|k| k.account_id == keys[0].account_id)
        .collect();
    assert_eq!(hong_keys.len(), 2);
    assert!(hong_keys
        .iter()
        .all(|k| k.revoke_reason.as_deref() == Some("allow-remove")));
    assert_eq!(h.get("/api/users", &kim).await.status, 200);
    // the PC stays bound to the account (design v3 §5-6)
    let bound = h.state.db.call(|c| db::all_devices(c)).await.unwrap();
    assert_eq!(bound.len(), 1);
}

#[tokio::test]
async fn deleting_an_account_leaves_no_row_of_it() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("123456789");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let phone = h.login_ok(HONG, None).await;
    // a signed request that changes something leaves a nonce
    assert_eq!(
        h.post("/api/hjv/this-device/undo", &phone, None)
            .await
            .status,
        200
    );
    h.login_ok(KIM, None).await;
    let account = h
        .state
        .db
        .call(|c| db::account_by_key(c, "2026000001"))
        .await
        .unwrap()
        .unwrap()
        .id;
    let rows = |table: &'static str| {
        let sql = match table {
            "req_nonces" => {
                "SELECT COUNT(*) FROM req_nonces n WHERE n.ckid IN
                             (SELECT ckid FROM ctl_keys WHERE account_id = ?1)
                             OR n.ckid NOT IN (SELECT ckid FROM ctl_keys)"
            }
            "accounts" => "SELECT COUNT(*) FROM accounts WHERE id = ?1",
            "tokens" => "SELECT COUNT(*) FROM tokens WHERE account_id = ?1",
            "devices" => "SELECT COUNT(*) FROM devices WHERE account_id = ?1",
            _ => "SELECT COUNT(*) FROM ctl_keys WHERE account_id = ?1",
        };
        h.state
            .db
            .call(move |c| c.query_row(sql, [account], |r| r.get::<_, i64>(0)))
    };
    let tables = ["accounts", "tokens", "devices", "ctl_keys", "req_nonces"];
    for table in tables {
        assert!(rows(table).await.unwrap() > 0, "{table} before");
    }
    let before = serial(&h).await;
    let out = h
        .admin(&["user", "delete", HONG.0], false, None)
        .await
        .unwrap();
    assert!(
        out.starts_with("deleted the account with 1 devices, 2 account keys and 2 tokens"),
        "{out}"
    );
    for table in tables {
        assert_eq!(rows(table).await.unwrap(), 0, "{table} after");
    }
    assert_eq!(serial(&h).await, before + 1);
    // Kim's rows are all there
    let left = h
        .state
        .db
        .call(|c| c.query_row("SELECT COUNT(*) FROM ctl_keys", [], |r| r.get::<_, i64>(0)))
        .await
        .unwrap();
    assert_eq!(left, 1);
}

// ---- the test vectors of README.md (design v3 §7-1) ----
//
// The account key is the Ed25519 key of seed [3; 32] (the controller's key of
// build/design-v3/synthesis/trust-v3), and the inputs follow the vectors of seal.rs and
// relay/PROTOCOL.md §13: t = 1760000000 and the nonce bytes 00 01 … 0f. The expected values were
// made with libsodium 1.0.18 (crypto_sign_seed_keypair, crypto_sign_detached) through its C API,
// independently of ed25519-dalek; README.md ("Formats") shows the same values.

const CTL_SEED: [u8; 32] = [3; 32];
const CK: &str = "7UkoxijRwsbq6QM4kFmVYSlZJzpcY/k2NsFGFKyHN9E=";
const CKEY_NONCE: &str = "AAECAwQFBgcICQoLDA0ODw==";
const CKEY_ID: &str = "987654321";
const CKEY_MESSAGE_HEX: &str = "686a762d636b65792d76310041414543417751464267634943516f4c4441304f44773d3d003938373635343332310037556b6f78696a527773627136514d346b466d5659536c5a4a7a7063592f6b324e734647464b79484e39453d";
const CP: &str =
    "tw4dTyxkVccRMV1aqo4jlinOOftOSRfHy4dAv9ItCeS/3P1eyIEUyYOuyjjyVfzhFUE2TplccLESFxCLtB3ICQ==";
const TOKEN: &str = "hjv_AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
const TOKEN_SHA256: &str = "h6q74v9/1g0Rq8wvBTM5yQIFPZwyCOmhw1Pff+EcZ6g=";
const T: i64 = 1_760_000_000;
const GET_TARGET: &str = "/api/peers?current=1&pageSize=100&accessible=&status=1";
const GET_MESSAGE_HEX: &str = "686a762d7265712d763100474554002f6170692f70656572733f63757272656e743d31267061676553697a653d3130302661636365737369626c653d267374617475733d3100313736303030303030300041414543417751464267634943516f4c4441304f4477003437444551706a38484253612b2f54496d572b354a4365755165526b6d354e4d704a575a473368537546553d00683671373476392f316730527138777642544d3579514946505a7779434f6d6877315066662b45635a36673d";
const GET_PROOF: &str = "v1.1760000000.AAECAwQFBgcICQoLDA0ODw.9tsrnUGYVeHhGWQC2oHD1-9Ahw62G854ZjhmxV4CclBAKgpKGR2ouiNwO4v88CKY0W1jqDqrm_jyzdGq_a2YCA";
const POST_TARGET: &str = "/api/logout";
const POST_BODY: &str = r#"{"id":"987654321","uuid":"dXVpZA=="}"#;
const POST_BODY_SHA256: &str = "cJ6RcjurhsiVfQGk9qmNH0XPkZ1ylZbMSytqCP6X6do=";
const POST_MESSAGE_HEX: &str = "686a762d7265712d763100504f5354002f6170692f6c6f676f757400313736303030303030300041414543417751464267634943516f4c4441304f447700634a3652636a7572687369566651476b39716d4e483058506b5a31796c5a624d537974714350365836646f3d00683671373476392f316730527138777642544d3579514946505a7779434f6d6877315066662b45635a36673d";
const POST_PROOF: &str = "v1.1760000000.AAECAwQFBgcICQoLDA0ODw.HgexD8dQd4Gq9iVRcGKrG_nqtOsGX5FbjSlBwJRkA4bdnFtbaQ48xnizvYV301E2G4vHaI_vvmoMsq_X2R_DDg";
/// design-trust-list/trust-check's golden value: seed [1; 32] over "hjv-trust-v1" NUL "golden"
const GOLDEN: &str =
    "HVaqNR32XoAL6O7jrfbYV2eI54M1J3Ou2/Vhka373a0x1AIGEjFHWE6u59vl6YVCO69a+Bi5rA0BNt4WktwsBQ==";

fn nonce_bytes() -> [u8; 16] {
    std::array::from_fn(|i| i as u8)
}

#[test]
fn the_account_key_proof_vector() {
    assert_eq!(b64(&hjv_api::devsig::public_key(&CTL_SEED)), CK);
    assert_eq!(b64(&nonce_bytes()), CKEY_NONCE);
    let message = keys::message(CKEY_NONCE, CKEY_ID, CK);
    assert_eq!(hex(&message), CKEY_MESSAGE_HEX);
    assert_eq!(message.len(), 91);
    assert_eq!(
        keys::proof(&CTL_SEED, CKEY_NONCE, CKEY_ID),
        (CK.to_owned(), CP.to_owned())
    );
    let claim = KeyClaim::new(Some(CK.into()), Some(CP.into()), CKEY_NONCE, CKEY_ID);
    assert_eq!(claim.verify(), Ok(hjv_api::devsig::public_key(&CTL_SEED)));
}

#[test]
fn the_request_proof_vectors() {
    use hjv_api::util::sha256;
    assert_eq!(b64(&sha256(TOKEN.as_bytes())), TOKEN_SHA256);
    assert_eq!(b64(&sha256(POST_BODY.as_bytes())), POST_BODY_SHA256);
    let pk = hjv_api::devsig::public_key(&CTL_SEED);
    for (method, target, body, message_hex, header) in [
        ("GET", GET_TARGET, "", GET_MESSAGE_HEX, GET_PROOF),
        ("POST", POST_TARGET, POST_BODY, POST_MESSAGE_HEX, POST_PROOF),
    ] {
        let parsed = proof::parse(header).unwrap();
        let message = proof::message(
            method,
            target,
            &parsed.t,
            &parsed.n,
            &sha256(body.as_bytes()),
            &sha256(TOKEN.as_bytes()),
        );
        assert_eq!(hex(&message), message_hex, "{method}");
        assert_eq!(
            proof::header_value(
                &CTL_SEED,
                method,
                target,
                T,
                &nonce_bytes(),
                body.as_bytes(),
                TOKEN
            ),
            header,
            "{method}"
        );
        assert_eq!(
            proof::check(
                &pk,
                Some(header),
                method,
                target,
                &sha256(body.as_bytes()),
                &sha256(TOKEN.as_bytes()),
                T
            ),
            proof::Check::Valid {
                nonce: nonce_bytes()
            },
            "{method}"
        );
    }
}

#[test]
fn ed25519_dalek_signs_as_libsodium() {
    use ed25519_dalek::Signer;
    let sig = ed25519_dalek::SigningKey::from_bytes(&[1; 32]).sign(b"hjv-trust-v1\0golden");
    assert_eq!(b64(&sig.to_bytes()), GOLDEN);
}

// ---- the vectors shared with the client: tests/data/v3-vectors.json ----
//
// The client renders the file from its src/hjviewer_account/vectors.rs (WP-C0) and checks the
// same values there on libsodium; its JSON_SHA256 pins the file byte for byte. Here hjv-api's own
// code must give the same bytes: the request proofs (proof.rs) and the sealed login's account
// key (keys.rs, seal.rs). The PC sync and the trust list (WP-S2) check theirs from the same file.

const SHARED: &str = include_str!("data/v3-vectors.json");
/// The client's vectors.rs JSON_SHA256. When the client's vectors change, the file is rendered
/// again (its write_json_copy) and this value follows.
const SHARED_SHA256: &str = "2379784b5fd3e8590b1e7783341ea9c057eb40340d929d074e0b7dc4fc3ef112";
/// The secret of the seal test key the shared vectors name (src/seal.rs, made with libsodium).
const SEAL_TEST_SECRET: &str = "d3iPGgzqABomMdrl0F29BiAI1bMPULnim+sqeCIokAQ=";

fn shared() -> Value {
    serde_json::from_str(SHARED).unwrap()
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key]
        .as_str()
        .unwrap_or_else(|| panic!("{key} is not a string"))
}

fn unhex<const N: usize>(text: &str) -> [u8; N] {
    let bytes: Vec<u8> = (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect();
    bytes.try_into().unwrap()
}

#[test]
fn the_shared_vectors_are_the_clients() {
    use hjv_api::util::sha256;
    assert_eq!(hex(&sha256(SHARED.as_bytes())), SHARED_SHA256);
    // the account key is the one of the vectors above and of README.md
    let account = &shared()["keys"]["account"];
    assert_eq!(unhex::<32>(text(account, "seed_hex")), CTL_SEED);
    assert_eq!(text(account, "pk"), CK);
}

#[test]
fn the_shared_request_proofs() {
    use hjv_api::util::{b64url, sha256};
    let v = shared();
    let seed = unhex::<32>(text(&v["keys"]["account"], "seed_hex"));
    let pk = hjv_api::devsig::public_key(&seed);
    let requests = v["request_proofs"].as_array().unwrap();
    assert_eq!(requests.len(), 2);
    for r in requests {
        let (method, target, body, token) = (
            text(r, "method"),
            text(r, "path_and_query"),
            text(r, "body"),
            text(r, "token"),
        );
        let t = r["t"].as_i64().unwrap();
        let nonce = unhex::<16>(text(r, "n_hex"));
        assert_eq!(b64url(&nonce), text(r, "n"), "{method}");
        let (body_hash, token_hash) = (sha256(body.as_bytes()), sha256(token.as_bytes()));
        let message = proof::message(
            method,
            target,
            &t.to_string(),
            text(r, "n"),
            &body_hash,
            &token_hash,
        );
        assert_eq!(hex(&message), text(r, "signed_hex"), "{method}");
        let header = proof::header_value(&seed, method, target, t, &nonce, body.as_bytes(), token);
        assert_eq!(header, text(r, "header"), "{method}");
        assert_eq!(
            proof::check(
                &pk,
                Some(text(r, "header")),
                method,
                target,
                &body_hash,
                &token_hash,
                t
            ),
            proof::Check::Valid { nonce },
            "{method}"
        );
    }
}

#[test]
fn the_shared_sealed_login_key() {
    use hjv_api::{
        seal::{self, SealKeys},
        util::{b64_decode, b64url},
    };
    let v = shared();
    let s = &v["seal_ckey"];
    let seed = unhex::<32>(text(&v["keys"]["account"], "seed_hex"));
    let (n, id, ck, cp) = (text(s, "n"), text(s, "id"), text(s, "ck"), text(s, "cp"));
    assert_eq!(hex(&keys::message(n, id, ck)), text(s, "signed_hex"));
    assert_eq!(keys::proof(&seed, n, id), (ck.to_owned(), cp.to_owned()));
    // the payload, sealed to the seal key the vectors name, opens and registers that key
    let secret: [u8; 32] = b64_decode(SEAL_TEST_SECRET).unwrap().try_into().unwrap();
    let public = *crypto_box::SecretKey::from_bytes(secret)
        .public_key()
        .as_bytes();
    let seal_key = &v["keys"]["seal"];
    assert_eq!(b64(&public), text(seal_key, "pk"));
    assert_eq!(seal::key_id(&public), text(seal_key, "kid"));
    let sealed = crypto_box::PublicKey::from(public)
        .seal(&mut crypto_box::aead::OsRng, text(s, "payload").as_bytes())
        .unwrap();
    let field = format!(
        "{}{}.{}",
        seal::FIELD_PREFIX,
        text(seal_key, "kid"),
        b64url(&sealed)
    );
    let opened = SealKeys::new(&[zeroize::Zeroizing::new(secret)])
        .open(&field)
        .unwrap();
    assert_eq!(
        (
            opened.nonce.as_str(),
            opened.device_id.as_str(),
            opened.time
        ),
        (n, id, 1_760_000_000)
    );
    assert_eq!(
        (opened.ck.as_deref(), opened.cp.as_deref()),
        (Some(ck), Some(cp))
    );
    let claim = KeyClaim::new(opened.ck, opened.cp, &opened.nonce, &opened.device_id);
    assert_eq!(claim.verify(), Ok(hjv_api::devsig::public_key(&seed)));
}

#[test]
fn the_readme_shows_the_vectors() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    for value in [
        CK,
        CP,
        CKEY_NONCE,
        CKEY_MESSAGE_HEX,
        TOKEN,
        TOKEN_SHA256,
        GET_TARGET,
        GET_MESSAGE_HEX,
        GET_PROOF,
        POST_BODY,
        POST_BODY_SHA256,
        POST_MESSAGE_HEX,
        POST_PROOF,
        "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=",
    ] {
        assert!(readme.contains(value), "README.md does not show {value}");
    }
}
