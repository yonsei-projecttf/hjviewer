// SPDX-License-Identifier: AGPL-3.0-only

//! The super operator's view (overview.rs): `GET /api/hjv/admin/overview` for a super operator
//! (the counts, the order and the fields of the answer), the 404 of an unknown path for every other
//! account, the refusals of every account path for a request without its proof, nobody without
//! HJV_API_OPERATORS, the role in the login's answer, the user object and the key list as the
//! setting says at each start, the address of each PC's last sync (overwritten, gone with the PC),
//! the line of each look in the operator record and the journal without the portal ID, and `admin
//! operators`. The portal IDs are the fake SSO's test accounts.

mod common;

use common::*;
use hjv_api::{
    db, log, oplog, overview, proof,
    util::{datetime, now, random_bytes, DAY},
};
use hyper::Method;
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::PathBuf};

const PATH: &str = overview::PATH;

fn with_operators(ids: &[&str]) -> Options {
    Options {
        vars: vec![("HJV_API_OPERATORS", ids.join(","))],
        ..Default::default()
    }
}

/// A PC of `account`'s, bound by a login on it.
async fn bind(h: &Harness, account: (&str, &str), pc: &TestDevice) -> String {
    h.hbbs.register(pc, Some(1));
    h.login_ok(account, Some(pc)).await;
    let binding = h.binding_of(&pc.id).await;
    assert_eq!(binding.len(), 32);
    binding
}

async fn sql(h: &Harness, statement: String) {
    h.state
        .db
        .call(move |c| c.execute_batch(&statement))
        .await
        .unwrap();
}

/// The member names of a JSON object.
fn members(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .unwrap_or_else(|| panic!("not an object: {v}"))
        .keys()
        .cloned()
        .collect()
}

fn set(names: &[&str]) -> BTreeSet<String> {
    names.iter().map(|n| n.to_string()).collect()
}

/// The lines of the operator record that are looks through the app.
fn looks(h: &Harness) -> Vec<String> {
    std::fs::read_to_string(&h.oplog)
        .unwrap_or_default()
        .lines()
        .filter(|l| l.contains(&format!("cmd=\"{}\"", overview::COMMAND)))
        .map(str::to_owned)
        .collect()
}

/// The database of a harness, stopped, with its log folded in: a copy in its directory, which the
/// next harness starts from (and copies again).
async fn database_of(h: &Harness) -> PathBuf {
    h.state.db.call(|c| db::checkpoint(c)).await.unwrap();
    h.stop();
    let copy = h.dir.join("copy.sqlite3");
    std::fs::copy(h.db_path(), &copy).unwrap();
    copy
}

#[tokio::test]
async fn an_operator_sees_who_uses_the_service_and_on_which_pcs() {
    let h = Harness::with(with_operators(&[HONG.0])).await;
    // HONG: two PCs (one named so that it sorts first only without case) and a phone; KIM: a PC;
    // LEE: a phone only, last used 40 days ago
    let hong_a = TestDevice::windows("710000001");
    let hong_b = TestDevice {
        hostname: "a-lab".into(),
        ..TestDevice::windows("710000002")
    };
    let kim_pc = TestDevice::windows("710000003");
    let binding_a = bind(&h, HONG, &hong_a).await;
    bind(&h, HONG, &hong_b).await;
    bind(&h, KIM, &kim_pc).await;
    let phone = h.login_ok(HONG, None).await;
    h.login_ok(LEE, None).await;
    h.admin(
        &["passwordless", "on", "--owner-of", &hong_a.id],
        false,
        None,
    )
    .await
    .unwrap();
    // HONG's first PC reports how to wake it, then syncs from another address; KIM's PC syncs
    // without a binding (it let it go)
    let wol = wol_report(
        "00:00:5e:00:53:10",
        "192.0.2.67/24",
        "192.0.2.1",
        Some("00:00:5e:00:53:01"),
    );
    h.sync_from(
        &hong_a,
        &payload_with_wol(&binding_a, &wol, "h1"),
        "192.0.2.40",
    )
    .await
    .list();
    h.sync_from(&hong_a, &payload(&binding_a), "198.51.100.7")
        .await
        .list();
    let kim_sync = h.sync_from(&kim_pc, &payload(""), "203.0.113.5").await;
    assert_eq!(
        kim_sync.answer.json["state"], "listed",
        "{}",
        kim_sync.answer.text
    );
    let at = now();
    sql(
        &h,
        format!(
            "UPDATE accounts SET last_active_at = {} WHERE login_key = 'zztest02';
             UPDATE devices SET last_seen_at = {} WHERE device_id = '710000003';
             UPDATE accounts SET last_active_at = {} WHERE login_key = 'yztest03';",
            at - 2 * DAY,
            at - 2 * DAY,
            at - 40 * DAY
        ),
    )
    .await;

    let a = h.get(PATH, &phone).await;
    assert_eq!(a.status, 200, "{}", a.text);
    let v = &a.json;
    assert_eq!(members(v), set(&["v", "at", "active_days", "users", "pcs"]));
    assert_eq!(
        (v["v"].clone(), v["active_days"].clone()),
        (json!(1), json!(30))
    );
    let shown = v["at"].as_str().unwrap();
    assert!(
        shown == datetime(at) || shown == datetime(at + 1) || shown == datetime(at + 2),
        "{shown}"
    );
    // the users: the active ones first, the newest use first
    let users = v["users"].as_array().unwrap();
    let summary: Vec<Value> = users
        .iter()
        .map(|u| json!([u["id"], u["name"], u["active"], u["keys"], u["pcs"]]))
        .collect();
    assert_eq!(
        summary,
        vec![
            json!(["ZZTEST01", "홍길동", true, 3, 2]),
            json!(["ZZTEST02", "김연세", true, 1, 1]),
            json!(["YZTEST03", "이교수", false, 1, 0]),
        ]
    );
    for u in users {
        assert_eq!(
            members(u),
            set(&["id", "name", "active", "last_use", "keys", "pcs"])
        );
    }
    assert_eq!(users[1]["last_use"], datetime(at - 2 * DAY));
    assert_eq!(users[2]["last_use"], datetime(at - 40 * DAY));
    // the PCs: by owner, then host name without case
    let pcs = v["pcs"].as_array().unwrap();
    let order: Vec<(&str, &str)> = pcs
        .iter()
        .map(|p| (p["owner"].as_str().unwrap(), p["pc"].as_str().unwrap()))
        .collect();
    assert_eq!(
        order,
        vec![
            ("ZZTEST01", "710000002"),
            ("ZZTEST01", "710000001"),
            ("ZZTEST02", "710000003"),
        ]
    );
    for p in pcs {
        assert_eq!(
            members(p),
            set(&[
                "pc",
                "host",
                "owner",
                "owner_name",
                "os",
                "version",
                "last_sync",
                "ip",
                "lan",
                "passwordless"
            ])
        );
    }
    let a_pc = &pcs[1];
    assert_eq!(a_pc["host"], "PC-710000001");
    assert_eq!(a_pc["owner_name"], "홍길동");
    assert_eq!(a_pc["os"], "windows");
    assert_eq!(a_pc["version"], "1.6.0");
    assert_eq!(a_pc["ip"], "198.51.100.7");
    assert_eq!(a_pc["lan"], json!(["192.0.2.67/24"]));
    assert_eq!(a_pc["passwordless"], "active");
    assert!(a_pc["last_sync"].as_str().unwrap().ends_with('Z'));
    // a PC that never synced: no version, sync, address or report
    let b_pc = &pcs[0];
    assert_eq!(b_pc["host"], "a-lab");
    for field in ["version", "last_sync", "ip"] {
        assert!(b_pc[field].is_null(), "{field}: {b_pc}");
    }
    assert_eq!(b_pc["lan"], json!([]));
    assert_eq!(b_pc["passwordless"], "listed");
    // KIM's: its address, and off as its account's switch is
    assert_eq!(pcs[2]["ip"], "203.0.113.5");
    assert_eq!(pcs[2]["owner_name"], "김연세");
    assert_eq!(pcs[2]["passwordless"], "disabled");

    // a blocked account (allow remove blocks since the open login) is no user any more; its PC
    // stays in the list
    h.admin(&["allow", "remove"], false, Some("ZZTEST02\n"))
        .await
        .unwrap();
    let v = h.get(PATH, &phone).await.json;
    let ids: Vec<&str> = v["users"]
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["ZZTEST01", "YZTEST03"]);
    assert_eq!(v["pcs"][2]["owner"], "ZZTEST02");
    assert_eq!(v["pcs"][2]["passwordless"], "disabled");
    // nothing changed by looking: the PCs, the keys and the change serial
    let serial = h.change_serial().await;
    h.get(PATH, &phone).await;
    assert_eq!(h.change_serial().await, serial);
    assert_eq!(h.get("/api/hjv/keys", &phone).await.json["total"], 3);
}

#[tokio::test]
async fn an_id_is_an_operator_whatever_its_case() {
    let h = Harness::with(with_operators(&["yztest03", " ZZTEST77 "])).await;
    // the login's answer and the key list carry the role of the ID typed in another case
    let answer = h.login(&h.login_body(LEE.0, LEE.1, None), "10.9.8.7").await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    assert_eq!(
        answer.json["user"]["hjv_role"], "operator",
        "{}",
        answer.text
    );
    let lee = answer.token();
    assert_eq!(
        h.get("/api/hjv/keys", &lee).await.json["hjv_role"],
        "operator"
    );
    assert_eq!(h.get(PATH, &lee).await.status, 200);
    let hong = h.login_ok(HONG, None).await;
    assert_eq!(h.get(PATH, &hong).await.status, 404);
}

/// HJV_API_ACTIVE_DAYS reaches the answer: with one day, an account last used two days ago is
/// not active, which it is with the default 30 days.
#[tokio::test]
async fn the_active_days_are_the_settings() {
    for (days, active) in [("1", false), ("30", true)] {
        let mut opts = with_operators(&[HONG.0]);
        opts.vars.push(("HJV_API_ACTIVE_DAYS", days.to_owned()));
        let h = Harness::with(opts).await;
        let hong = h.login_ok(HONG, None).await;
        h.login_ok(KIM, None).await;
        sql(
            &h,
            format!(
                "UPDATE accounts SET last_active_at = {} WHERE login_key = 'zztest02'",
                now() - 2 * DAY
            ),
        )
        .await;
        let v = h.get(PATH, &hong).await.json;
        assert_eq!(v["active_days"], days.parse::<i64>().unwrap());
        let kim = v["users"]
            .as_array()
            .unwrap()
            .iter()
            .find(|u| u["id"] == KIM.0)
            .unwrap_or_else(|| panic!("{v}"))
            .clone();
        assert_eq!(kim["active"], active, "{days} days: {kim}");
    }
}

/// Without the admin socket hjv-api has no operator record: a look that cannot be recorded is
/// not made (503 with the text, no data).
#[tokio::test]
async fn without_the_operator_record_nothing_is_shown() {
    let h = Harness::with(Options {
        no_admin: true,
        ..with_operators(&[HONG.0])
    })
    .await;
    let hong = h.login_ok(HONG, None).await;
    let a = h.get(PATH, &hong).await;
    assert_eq!(a.status, 503, "{}", a.text);
    assert_eq!(a.json, json!({ "error": overview::TEXT_NOT_RECORDED }));
    assert!(!a.text.contains("users") && !a.text.contains(HONG.0));
}

#[tokio::test]
async fn every_other_account_gets_the_404_of_an_unknown_path() {
    let h = Harness::with(with_operators(&[HONG.0])).await;
    let kim = h.login_ok(KIM, None).await;
    let unknown = h.get("/api/hjv/admin/nothing", &kim).await;
    assert_eq!(unknown.status, 404);
    let answers = [
        h.get(PATH, &kim).await,
        h.post(PATH, &kim, Some(json!({}))).await,
        h.delete(PATH, &kim).await,
        h.signed(Method::HEAD, PATH, &kim, None).await,
        h.get(&format!("{PATH}?all=1"), &kim).await,
    ];
    for a in &answers {
        assert_eq!(a.status, 404, "{}", a.text);
        for header in [
            "content-type",
            "x-content-type-options",
            "content-security-policy",
            "cache-control",
        ] {
            assert_eq!(
                a.headers.get(header),
                unknown.headers.get(header),
                "{header}"
            );
        }
        assert!(a.headers.get("allow").is_none());
    }
    for a in &answers[..3] {
        assert_eq!(a.text, unknown.text);
        assert_eq!(a.json, json!({"error": "not found"}));
    }
    // however often: no limit tells it apart, and the key list keeps all of its calls
    for _ in 0..35 {
        assert_eq!(h.get(PATH, &kim).await.status, 404);
    }
    assert_eq!(h.get("/api/hjv/keys", &kim).await.status, 200);
    assert!(looks(&h).is_empty());
    // the operator: other methods than GET are not allowed
    let hong = h.login_ok(HONG, None).await;
    let a = h.post(PATH, &hong, Some(json!({}))).await;
    assert_eq!(a.status, 405, "{}", a.text);
    assert_eq!(a.headers["allow"], "GET");
    assert_eq!(h.signed(Method::HEAD, PATH, &hong, None).await.status, 405);
    assert!(looks(&h).is_empty());
}

#[tokio::test]
async fn a_request_without_its_proof_is_refused_as_on_every_account_path() {
    let h = Harness::with(with_operators(&[HONG.0])).await;
    let token = h.login_ok(HONG, None).await;
    let other = h.login_ok(HONG, None).await;
    let seed = h.key_of(&token).unwrap();
    let request = |path: &'static str, token: String, proof: Option<String>| {
        let h = &h;
        async move {
            let auth = format!("Bearer {token}");
            let mut headers = vec![("authorization", auth.as_str())];
            if let Some(p) = &proof {
                headers.push((proof::HEADER, p.as_str()));
            }
            h.request(Method::GET, path, &headers, None).await
        }
    };
    let sign = |path: &str, time: i64, token: &str| {
        proof::header_value(&seed, "GET", path, time, &random_bytes::<16>(), b"", token)
    };
    for path in [PATH, "/api/hjv/keys"] {
        // no proof; a proof of another path; another token's; the key's own over another token;
        // no token at all
        let cases = [
            (token.clone(), None),
            (token.clone(), Some(sign("/api/peers", now(), &token))),
            (other.clone(), Some(sign(path, now(), &token))),
            (token.clone(), Some(sign(path, now(), &other))),
            (
                format!("hjv_{}", "A".repeat(43)),
                Some(sign(path, now(), &token)),
            ),
        ];
        for (n, (t, p)) in cases.into_iter().enumerate() {
            let a = request(path, t, p).await;
            assert_eq!(a.status, 401, "{path} case {n}: {}", a.text);
            assert_eq!(
                a.json,
                json!({"error": "로그인이 필요합니다. 다시 로그인해 주세요."})
            );
        }
        // the right key, the clock an hour off: 409 clock
        let a = request(path, token.clone(), Some(sign(path, now() - 3600, &token))).await;
        assert_eq!(a.status, 409, "{path}: {}", a.text);
        assert_eq!(a.json["hjv"], "clock");
    }
    let a = h.request(Method::GET, PATH, &[], None).await;
    assert_eq!(a.status, 401);
    assert!(looks(&h).is_empty());
    // signed, it opens
    assert_eq!(h.get(PATH, &token).await.status, 200);
    assert_eq!(looks(&h).len(), 1);
}

#[tokio::test]
async fn the_role_follows_the_setting_at_each_start() {
    // nobody without the setting: no role anywhere, the 404 for every account
    let a = Harness::start().await;
    let answer = a
        .login(&a.login_body(HONG.0, HONG.1, None), "10.9.9.9")
        .await;
    assert_eq!(answer.status, 200, "{}", answer.text);
    assert!(
        answer.json["user"].get("hjv_role").is_none(),
        "{}",
        answer.text
    );
    let hong = answer.token();
    let kim = a.login_ok(KIM, None).await;
    for token in [&hong, &kim] {
        assert_eq!(a.get(PATH, token).await.status, 404);
        assert!(a
            .get("/api/hjv/keys", token)
            .await
            .json
            .get("hjv_role")
            .is_none());
        assert!(a
            .post("/api/currentUser", token, None)
            .await
            .json
            .get("hjv_role")
            .is_none());
    }
    // the same database, HONG made a super operator: the same login sees it, no new one needed
    let b = Harness::with(Options {
        db_file: Some(database_of(&a).await),
        ..with_operators(&[HONG.0])
    })
    .await;
    b.adopt_keys(&a);
    let keys = b.get("/api/hjv/keys", &hong).await;
    assert_eq!(keys.status, 200, "{}", keys.text);
    assert_eq!(keys.json["hjv_role"], "operator");
    assert_eq!(keys.json["total"], 1);
    let user = b.post("/api/currentUser", &hong, None).await.json;
    assert_eq!(
        user,
        json!({"name": "ZZTEST01", "display_name": "홍길동", "status": 1, "hjv_role": "operator",
               "hjv": {"support_code": true}})
    );
    assert_eq!(
        b.get("/api/users", &hong).await.json["data"][0]["hjv_role"],
        "operator"
    );
    assert_eq!(b.get(PATH, &hong).await.status, 200);
    // a login's answer carries it too, and no other account has it
    let login = b
        .login(&b.login_body(HONG.0, HONG.1, None), "10.9.9.9")
        .await;
    assert_eq!(
        login.json["user"],
        json!({"name": "ZZTEST01", "display_name": "홍길동", "status": 1, "hjv_role": "operator"})
    );
    let login = b.login(&b.login_body(KIM.0, KIM.1, None), "10.9.9.9").await;
    assert_eq!(
        login.json["user"],
        json!({"name": "ZZTEST02", "display_name": "김연세", "status": 1})
    );
    assert!(b
        .get("/api/hjv/keys", &kim)
        .await
        .json
        .get("hjv_role")
        .is_none());
    assert_eq!(b.get(PATH, &kim).await.status, 404);
    // taken away again at the next start
    let c = Harness::with(Options {
        db_file: Some(database_of(&b).await),
        ..Default::default()
    })
    .await;
    c.adopt_keys(&b);
    let keys = c.get("/api/hjv/keys", &hong).await;
    assert_eq!(keys.status, 200, "{}", keys.text);
    assert!(keys.json.get("hjv_role").is_none(), "{}", keys.text);
    assert_eq!(c.get(PATH, &hong).await.status, 404);
}

/// The address of the first PC of the overview, as the operator sees it.
async fn shown_ip(h: &Harness, token: &str) -> Value {
    h.get(PATH, token).await.json["pcs"][0]["ip"].clone()
}

/// The address the PC's last sync came from, as the database keeps it.
async fn address_of(h: &Harness, id: &str) -> Option<String> {
    let id = id.to_owned();
    h.state
        .db
        .call(move |c| {
            c.query_row(
                "SELECT last_sync_ip FROM devices WHERE device_id = ?1",
                [id],
                |r| r.get(0),
            )
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn the_address_of_a_pcs_last_sync() {
    let h = Harness::with(with_operators(&[HONG.0])).await;
    let pc = TestDevice::windows("720000001");
    let binding = bind(&h, HONG, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(address_of(&h, &pc.id).await, None);
    assert!(shown_ip(&h, &phone).await.is_null());
    // each accepted sync puts its address in place of the one before, its mark whatever it is;
    // an IPv4 address in IPv6 form as IPv4, and what is not an address as none
    for (ip, kept) in [
        ("192.0.2.40", Some("192.0.2.40")),
        ("2001:db8::7", Some("2001:db8::7")),
        ("::ffff:198.51.100.9", Some("198.51.100.9")),
        ("not an address", None),
        ("203.0.113.9", Some("203.0.113.9")),
    ] {
        let s = h.sync_from(&pc, &payload(&binding), ip).await;
        assert_eq!(s.answer.status, 200, "{ip}: {}", s.answer.text);
        assert_eq!(address_of(&h, &pc.id).await.as_deref(), kept, "{ip}");
        assert_eq!(shown_ip(&h, &phone).await, json!(kept), "{ip}");
    }
    let s = h.sync_from(&pc, &payload(""), "198.51.100.20").await;
    assert_eq!(s.answer.json["state"], "listed");
    assert_eq!(
        address_of(&h, &pc.id).await.as_deref(),
        Some("198.51.100.20")
    );
    // a sync that is refused changes nothing: one its key did not sign, one sent again
    let forged = TestDevice {
        seed: [77; 32],
        ..pc.clone()
    };
    let s = h.sync_from(&forged, &payload(&binding), "192.0.2.66").await;
    assert_eq!(s.answer.status, 401, "{}", s.answer.text);
    assert_eq!(
        address_of(&h, &pc.id).await.as_deref(),
        Some("198.51.100.20")
    );
    let nonce = random_bytes::<16>();
    let body = sync_body(&pc, now(), &nonce, &payload(&binding).to_string());
    let first = h
        .request(
            Method::POST,
            hjv_api::pcsync::PATH,
            &[
                ("content-type", "application/json"),
                ("x-real-ip", "192.0.2.70"),
            ],
            Some(body.clone().into_bytes()),
        )
        .await;
    assert_eq!(first.status, 200, "{}", first.text);
    let again = h
        .request(
            Method::POST,
            hjv_api::pcsync::PATH,
            &[
                ("content-type", "application/json"),
                ("x-real-ip", "192.0.2.71"),
            ],
            Some(body.into_bytes()),
        )
        .await;
    assert_eq!(again.status, 409, "{}", again.text);
    assert_eq!(address_of(&h, &pc.id).await.as_deref(), Some("192.0.2.70"));
    // the PC leaves its account: its row and its address go; its next sync (a signed revoked)
    // keeps none
    let a = h
        .delete(&format!("/api/hjv/devices/{}", pc.id), &phone)
        .await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(h.get(PATH, &phone).await.json["pcs"], json!([]));
    let s = h.sync_from(&pc, &payload(&binding), "192.0.2.80").await;
    assert_eq!(s.list()["state"], "revoked");
    let left: i64 = h
        .state
        .db
        .call(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM devices WHERE last_sync_ip IS NOT NULL",
                [],
                |r| r.get(0),
            )
        })
        .await
        .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn each_look_is_recorded_without_the_portal_id() {
    let h = Harness::with(with_operators(&[HONG.0])).await;
    let pc = TestDevice::windows("730000001");
    bind(&h, KIM, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let tag = h.account_tag(HONG.0);
    for _ in 0..2 {
        assert_eq!(h.get(PATH, &phone).await.status, 200);
    }
    let lines = looks(&h);
    assert_eq!(lines.len(), 2, "{lines:?}");
    for line in &lines {
        assert!(
            line.contains(&format!(
                "sudo_user=- cmd=\"app overview\" acct={tag} done=\"ok: 2 users (2 active), 1 PCs\""
            )),
            "{line}"
        );
    }
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    for secret in [
        HONG.0,
        KIM.0,
        "홍길동",
        "김연세",
        "730000001",
        "PC-730000001",
    ] {
        assert!(
            !record.contains(secret),
            "{secret} in the record:\n{record}"
        );
    }
    assert_eq!(oplog::verify(&h.oplog).unwrap(), record.lines().count());
    // and in the journal: the tag and the counts
    let journal = log::captured();
    assert!(
        journal.iter().any(|l| l.starts_with("overview: shown")
            && l.contains(&format!("(acct={tag}): 2 users (2 active), 1 PCs"))),
        "{journal:?}"
    );
    for line in journal.iter().filter(|l| l.starts_with("overview:")) {
        for secret in [HONG.0, KIM.0, LEE.0, "홍길동", "730000001"] {
            assert!(!line.contains(secret), "{line}");
        }
    }
    // a look the record does not take is not shown
    let aside = h.dir.join("operator.log.aside");
    std::fs::rename(&h.oplog, &aside).unwrap();
    std::fs::create_dir(&h.oplog).unwrap();
    let a = h.get(PATH, &phone).await;
    assert_eq!(a.status, 503, "{}", a.text);
    assert_eq!(a.json, json!({"error": overview::TEXT_NOT_RECORDED}));
    std::fs::remove_dir(&h.oplog).unwrap();
    std::fs::rename(&aside, &h.oplog).unwrap();
    assert_eq!(h.get(PATH, &phone).await.status, 200);
    assert_eq!(looks(&h).len(), 3);
    assert_eq!(oplog::verify(&h.oplog).unwrap(), looks(&h).len());
}

#[tokio::test]
async fn an_operator_has_the_key_lists_calls() {
    let h = Harness::with(with_operators(&[HONG.0])).await;
    let phone = h.login_ok(HONG, None).await;
    for n in 0..30 {
        let path = if n % 3 == 0 { "/api/hjv/keys" } else { PATH };
        assert_eq!(h.get(path, &phone).await.status, 200, "call {n}");
    }
    let a = h.get(PATH, &phone).await;
    assert_eq!(a.status, 429, "{}", a.text);
    assert_eq!(h.get("/api/hjv/keys", &phone).await.status, 429);
    // another key of the account has its own
    let other = h.login_ok(HONG, None).await;
    assert_eq!(h.get(PATH, &other).await.status, 200);
}

#[tokio::test]
async fn admin_operators_shows_which_ids_have_an_account() {
    let h = Harness::with(with_operators(&[HONG.0, "ZZTEST77"])).await;
    h.login_ok(HONG, None).await;
    let out = h.admin(&["operators"], false, None).await.unwrap();
    assert!(
        out.starts_with("super operators (HJV_API_OPERATORS): 2 configured, 1 with an account\n"),
        "{out}"
    );
    let hong_line = out
        .lines()
        .find(|l| l.contains(&h.account_tag(HONG.0)))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        hong_line.contains("ZZTE****") && hong_line.contains("yes      yes"),
        "{hong_line}"
    );
    let other = out
        .lines()
        .find(|l| l.contains(&h.account_tag("ZZTEST77")))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(
        other.contains("no       no") && other.ends_with('-'),
        "{other}"
    );
    for secret in [HONG.0, "zztest01", "ZZTEST77", "zztest77", "홍길동"] {
        assert!(!out.contains(secret), "{secret}: {out}");
    }
    // revealed: the account's ID as the SSO named it, the other as configured, case-folded
    let shown = h.admin(&["operators"], true, None).await.unwrap();
    assert!(
        shown.contains("ZZTEST01") && shown.contains("홍길동") && shown.contains("zztest77"),
        "{shown}"
    );
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert_eq!(record.matches("cmd=\"operators\"").count(), 4, "{record}");
    // a blocked super operator: its allowlist entry says so (the open login's list)
    h.admin(&["block", HONG.0], false, None).await.unwrap();
    let out = h.admin(&["operators"], false, None).await.unwrap();
    let hong_line = out
        .lines()
        .find(|l| l.contains(&h.account_tag(HONG.0)))
        .unwrap_or_else(|| panic!("{out}"));
    assert!(hong_line.contains("yes      blocked"), "{hong_line}");
    let none = Harness::start().await;
    assert_eq!(
        none.admin(&["operators"], false, None).await.unwrap(),
        "super operators (HJV_API_OPERATORS): none, so nobody sees the overview\n"
    );
}
