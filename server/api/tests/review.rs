// SPDX-License-Identifier: AGPL-3.0-only

//! The App Review account (review.rs) end to end: the login of the ID that the operator's file
//! names (HJV_API_REVIEW_FILE) without the SSO, with the limits and answers of any ID; the file
//! missing, past its UNTIL or refused; and what the account cannot reach (a PC off its file's PC=
//! line, door 1, short links, the overview, an account it did not make), its tokens ending with
//! the file, and from 1.9.1 its own deletion in the app, which keeps its allowlist entry. From
//! 1.9.2 it holds the demo PCs of its PC= line: bound by its login on them, registered on a
//! `staff`/`pc` ticket that ends at UNTIL, out of its own lists, kept through its deletion in the
//! app, its revoke-all, a logout and a restart, never deleted from a demo PC's own login (also not
//! by a request of it that overlaps the iPad's deletion), and given no package by door 1. The ID
//! and the password are made up: the real ones live only in the server's file.

mod common;

use common::*;
use hjv_api::{
    account, db, devsig,
    limits::{SCOPE_ID, SCOPE_UPDATE_FAIL_IP},
    login::{FAILURE_TEXT, TEXT_ID_RULE, TEXT_SSO_DOWN},
    overview, review,
    sso::Failure,
    tfa::SecondFactor,
    update::{self, TEXT_REVIEW},
    util::{b64, datetime, now, random_bytes, sha256, DAY, HOUR},
};
use hyper::Method;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

/// A made-up App Review account.
const REVIEW: (&str, &str) = ("APPREVIEW-TEST", "correct horse battery staple");
/// HASH of its password: PBKDF2-HMAC-SHA256 with the salt 00 01 … 0f and 100 000 iterations, by
/// Python's hashlib (review.rs has the vector).
const HASH: &str =
    "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";
const PEERS: &str = "/api/peers?current=1&pageSize=100&accessible=&status=1";

/// The test's directory of the file, removed with it.
struct Dir(PathBuf);

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// hjv-api with HJV_API_REVIEW_FILE in a directory of the test's (no file there yet), and `vars`
/// besides.
async fn start(vars: Vec<(&'static str, String)>) -> (Harness, PathBuf, Dir) {
    start_with(Options {
        vars,
        ..Default::default()
    })
    .await
}

/// The same with `opts`.
async fn start_with(mut opts: Options) -> (Harness, PathBuf, Dir) {
    let dir = temp_dir("demo-account");
    let file = dir.join("review.conf");
    opts.vars
        .insert(0, ("HJV_API_REVIEW_FILE", file.display().to_string()));
    let h = Harness::with(opts).await;
    (h, file, Dir(dir))
}

/// A second factor whose code is "123456" for everyone (as tests/login.rs has it).
struct FixedCode;

impl SecondFactor for FixedCode {
    fn required(&self) -> bool {
        true
    }

    fn verify(&self, _login_key: &str, code: &str) -> bool {
        code == "123456"
    }
}

/// Puts the file in place as the runbook does: a new file moved over the old one.
fn put(file: &Path, text: &str) {
    let new = file.with_extension("new");
    std::fs::write(&new, text).unwrap();
    std::fs::rename(&new, file).unwrap();
}

/// The file of `id` with the made-up password, active until `until`.
fn review_file(id: &str, until: i64) -> String {
    format!(
        "# made up\nID={id}\nHASH={HASH}\nUNTIL={}\n",
        datetime(until)
    )
}

/// The file of `id` active until `until`, with the PC= line of these PCs (1.9.2: its demo PCs).
fn review_file_with(id: &str, until: i64, pcs: &[&TestDevice]) -> String {
    let ids: Vec<&str> = pcs.iter().map(|pc| pc.id.as_str()).collect();
    format!("{}PC={}\n", review_file(id, until), ids.join(","))
}

/// A login of the made-up account with its password, from `ip`.
async fn review_login(h: &Harness, ip: &str) -> Answer {
    h.login(&h.login_body(REVIEW.0, REVIEW.1, None), ip).await
}

/// A login of the made-up account on `device`, signed by it, from `ip`.
async fn review_login_on(h: &Harness, device: &TestDevice, ip: &str) -> Answer {
    h.login(&h.login_body(REVIEW.0, REVIEW.1, Some(device)), ip)
        .await
}

/// A demo PC bound by the account's login on it (its file names the PC on its PC= line): the
/// binding, and the token of that login.
async fn bind_demo(h: &Harness, pc: &TestDevice) -> (String, String) {
    let a = review_login_on(h, pc, "10.0.79.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let binding = a.json["hjv"]["binding"]
        .as_str()
        .unwrap_or_else(|| panic!("no binding: {}", a.text))
        .to_owned();
    assert_eq!(h.binding_of(&pc.id).await, binding);
    (binding, a.token())
}

/// Whether a demo PC's sync with its binding gets a list and its `staff`/`pc` ticket.
async fn ticketed(h: &Harness, pc: &TestDevice, binding: &str) -> bool {
    let synced = h.sync(pc, &payload(binding)).await;
    synced.list();
    synced
        .answer
        .json
        .get("access")
        .is_some_and(|access| ticket_of(access)["use"] == "pc")
}

/// The body of the app's [계정 삭제].
fn confirm() -> Option<Value> {
    Some(json!({"confirm": "delete-account"}))
}

/// The made-up account's row now.
async fn review_account(h: &Harness) -> Option<db::Account> {
    h.state
        .db
        .call(|c| db::account_by_key(c, "appreview-test"))
        .await
        .unwrap()
}

/// The answer to a request of an unknown token.
fn unknown_token() -> Value {
    json!({"error": "로그인이 필요합니다. 다시 로그인해 주세요."})
}

/// The status of the token's user object and device list, which must agree.
async fn status_of(h: &Harness, token: &str) -> u16 {
    let me = h.post("/api/currentUser", token, Some(json!({}))).await;
    let peers = h.get(PEERS, token).await;
    assert_eq!(me.status, peers.status, "{} / {}", me.text, peers.text);
    if me.status == 401 {
        assert_eq!(me.json, unknown_token());
    }
    me.status
}

/// The ID the file names logs in without the SSO and gets what the app takes from a login; its
/// account is its own and empty, put on the allowlist by its first login (open mode), and the
/// journal tells of the login by the account's tag only.
#[tokio::test]
async fn the_review_account_logs_in_without_the_sso() {
    let (h, file, _dir) = start(vec![]).await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let a = review_login(&h, "10.0.60.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json["type"], "access_token");
    assert_eq!(
        a.json["user"],
        json!({"name": "appreview-test", "display_name": "App Review", "status": 1})
    );
    let token = a.token();
    assert!(token.starts_with("hjv_") && token.len() == 47, "{token}");
    assert!(a.json["hjv"]["ks"].is_i64(), "{}", a.text);
    assert!(a.json["hjv"].get("binding").is_none(), "{}", a.text);
    // typed in another case, with blanks around: the same account
    let again = h
        .login(
            &h.login_body(" appreview-test ", REVIEW.1, None),
            "10.0.60.1",
        )
        .await;
    assert_eq!(again.status, 200, "{}", again.text);
    assert_eq!((h.sso.token_calls(), h.sso.user_calls()), (0, 0));
    let me = h.post("/api/currentUser", &token, Some(json!({}))).await;
    assert_eq!(me.json, a.json["user"]);
    assert_eq!(
        h.get(PEERS, &token).await.json,
        json!({"total": 0, "data": []})
    );
    assert_eq!(h.get("/api/hjv/keys", &token).await.json["total"], 2);
    let tag = h.account_tag(REVIEW.0);
    let listed = h.admin(&["allow", "list"], false, None).await.unwrap();
    assert!(listed.contains(&tag), "{listed}");
    assert_eq!(h.results().await, vec!["login:ok", "login:ok"]);
    let logs = hjv_api::log::captured();
    let line = format!("login: the App Review account logged in, without the SSO (acct={tag})");
    assert!(logs.contains(&line), "{logs:?}");
    for line in &logs {
        assert!(!line.to_lowercase().contains("appreview"), "{line}");
        assert!(!line.contains(REVIEW.1), "{line}");
    }
}

/// A wrong password is answered as one the SSO refused (the same status, text and delay) and
/// counts under the ID's limit, which then holds the right password too, until it is lifted as
/// any ID's.
#[tokio::test]
async fn a_wrong_password_fails_as_any_and_counts_for_the_id() {
    let (h, file, _dir) = start(vec![
        ("HJV_API_FAILURE_DELAY_MS", "600".into()),
        ("HJV_API_FAILURE_JITTER_MS", "100".into()),
        // its first logins count as those of an ID not on the allowlist (open mode)
        ("HJV_API_LIMIT_NEW_ID", "2/600".into()),
    ])
    .await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let tag = h.account_tag(REVIEW.0);
    let review = h
        .login(
            &h.login_body(REVIEW.0, "wrong horse battery staple", None),
            "10.0.61.1",
        )
        .await;
    let staff = h
        .login(&h.login_body(HONG.0, "wrong", None), "10.0.61.2")
        .await;
    for a in [&review, &staff] {
        assert_eq!(a.status, 401, "{}", a.text);
        assert_eq!(a.json, json!({ "error": FAILURE_TEXT }));
        assert!(a.elapsed >= Duration::from_millis(500), "{:?}", a.elapsed);
        assert!(a.elapsed < Duration::from_millis(1500), "{:?}", a.elapsed);
    }
    assert_eq!(review.text, staff.text);
    assert_eq!(h.counted(SCOPE_ID, tag.clone()).await, 1);
    let a = h
        .login(&h.login_body(REVIEW.0, "wrong again", None), "10.0.61.1")
        .await;
    assert_eq!(
        (a.status, &a.json),
        (401, &json!({ "error": FAILURE_TEXT }))
    );
    assert_eq!(h.counted(SCOPE_ID, tag.clone()).await, 2);
    // the limit is full: the right password fails the same way
    let a = review_login(&h, "10.0.61.1").await;
    assert_eq!(
        (a.status, &a.json),
        (401, &json!({ "error": FAILURE_TEXT }))
    );
    h.admin(&["unlock", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    assert_eq!(review_login(&h, "10.0.61.1").await.status, 200);
    // the one SSO call was the staff account's
    assert_eq!((h.sso.token_calls(), h.sso.user_calls()), (1, 0));
    assert_eq!(
        h.results().await,
        vec![
            "login:review-credentials",
            "login:credentials",
            "login:review-credentials",
            "login:limited-id",
            "login:ok"
        ]
    );
    let line = format!("login: a wrong password for the App Review account (acct={tag})");
    assert!(hjv_api::log::captured().contains(&line));
}

/// The success limits hold its logins as anyone's, and come before its hash is worked out, as
/// before an SSO call: once the account's limit is full even a wrong password is refused by it,
/// and the ID's attempt goes back.
#[tokio::test]
async fn the_success_limits_come_before_the_hash() {
    let (h, file, _dir) = start(vec![("HJV_API_LIMIT_OK_ID", "1/600".into())]).await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    assert_eq!(review_login(&h, "10.0.68.1").await.status, 200);
    let right = review_login(&h, "10.0.68.1").await;
    let wrong = h
        .login(
            &h.login_body(REVIEW.0, "wrong horse battery staple", None),
            "10.0.68.1",
        )
        .await;
    for a in [&right, &wrong] {
        assert_eq!(
            (a.status, &a.json),
            (401, &json!({ "error": FAILURE_TEXT }))
        );
    }
    assert_eq!(
        h.results().await,
        vec!["login:ok", "login:limited-ok", "login:limited-ok"]
    );
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(REVIEW.0)).await, 0);
}

/// It calls nothing of the SSO's: it logs in while SSO calls are paused, when a staff ID gets the
/// 503 of every ID.
#[tokio::test]
async fn the_review_login_goes_on_while_sso_calls_are_paused() {
    let (h, file, _dir) = start(vec![]).await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    h.state.sso_breaker.note(Some(Failure::AddressRefused));
    let staff = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.69.1")
        .await;
    assert_eq!(
        (staff.status, &staff.json),
        (503, &json!({ "error": TEXT_SSO_DOWN }))
    );
    assert_eq!(review_login(&h, "10.0.69.2").await.status, 200);
    assert_eq!(h.sso.token_calls(), 0);
}

/// Nor does it get the code dialog of a second factor, which every staff ID gets first.
#[tokio::test]
async fn the_review_login_gets_no_second_factor() {
    let (h, file, _dir) = start_with(Options {
        second_factor: Some(Arc::new(FixedCode)),
        ..Default::default()
    })
    .await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let staff = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.69.3")
        .await;
    assert_eq!(staff.json["type"], "email_check", "{}", staff.text);
    let a = review_login(&h, "10.0.69.4").await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert_eq!(a.json["type"], "access_token", "{}", a.text);
}

/// Without an active file (none, past its UNTIL, naming another ID, or refused) the ID is one
/// outside HJViewer's ID rule as any other: 403 at once, no SSO, nothing under the ID. A file
/// whose ID passes the rule is refused, so that a staff account is never shadowed: that ID's
/// login goes to the SSO as always.
#[tokio::test]
async fn without_an_active_file_the_id_is_outside_the_rule() {
    let (h, file, _dir) = start(vec![("HJV_API_LIMIT_IP", "100/600".into())]).await;
    let outside = |a: &Answer| {
        assert_eq!(a.status, 403, "{}", a.text);
        assert_eq!(a.json, json!({ "error": TEXT_ID_RULE }));
    };
    outside(&review_login(&h, "10.0.62.1").await);
    let active = review_file(REVIEW.0, now() + DAY);
    for text in [
        review_file(REVIEW.0, now() - 1),
        review_file("OTHER-DEMO", now() + DAY),
        active.replace("$100000$", "$99999$"),
        format!("{active}ID={}\n", REVIEW.0),
    ] {
        put(&file, &text);
        outside(&review_login(&h, "10.0.62.1").await);
    }
    assert_eq!((h.sso.token_calls(), h.sso.user_calls()), (0, 0));
    assert_eq!(h.attempts(SCOPE_ID, h.account_tag(REVIEW.0)).await, 0);
    put(&file, &active);
    assert_eq!(review_login(&h, "10.0.62.1").await.status, 200);
    // an ID of the rule: the file is refused, and the ID's login is the SSO's to answer
    put(&file, &review_file("ZZDEMO01", now() + DAY));
    let a = h
        .login(&h.login_body("ZZDEMO01", REVIEW.1, None), "10.0.62.1")
        .await;
    assert_eq!(
        (a.status, &a.json),
        (401, &json!({ "error": FAILURE_TEXT }))
    );
    assert_eq!(h.sso.token_calls(), 1);
    outside(&review_login(&h, "10.0.62.1").await);
    let logs = hjv_api::log::captured();
    for why in [
        "HASH's iterations are not 100000 to 10000000",
        "ID, HASH, UNTIL or PC is there more than once",
        "ID passes HJViewer's ID rule, so it could be a staff account's",
    ] {
        let line = format!("login: the App Review file (HJV_API_REVIEW_FILE) is not used: {why}");
        assert!(logs.contains(&line), "{why}");
    }
}

/// A login of the account on an installed Windows PC off its file's PC= line (none, or another
/// PC), signed with the key hbbs holds for it, puts the PC in no list (hbbs is not even asked),
/// and the account reaches no other account's PC.
#[tokio::test]
async fn the_review_account_holds_no_pc_off_its_line() {
    let (h, file, _dir) = start(vec![]).await;
    let pc = TestDevice::windows("700000001");
    let other = TestDevice::windows("700000009");
    h.hbbs.register(&pc, Some(1));
    put(&file, &review_file_with(REVIEW.0, now() + DAY, &[&other]));
    let a = review_login_on(&h, &pc, "10.0.63.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(a.json["hjv"].get("binding").is_none(), "{}", a.text);
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let a = review_login_on(&h, &pc, "10.0.63.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(a.json["hjv"].get("binding").is_none(), "{}", a.text);
    assert_eq!(h.binding_of(&pc.id).await, "");
    assert_eq!(h.hbbs.queries.load(Ordering::SeqCst), 0);
    let token = a.token();
    let this = h.get("/api/hjv/this-device", &token).await;
    assert_eq!(this.json["status"], "not_eligible", "{}", this.text);
    assert_eq!(this.json["undo"], false);
    assert_eq!(h.sync(&pc, &payload("")).await.answer.status, 404);
    // the PC is free for a staff account's login on it, whose trust list has none of the review
    // account's keys; the review account cannot touch the PC
    let hong = h.login_ok(HONG, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    assert_eq!(binding.len(), 32);
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    let key = |token: &str| b64(&devsig::public_key(&h.key_of(token).unwrap()));
    assert_eq!(
        h.sync(&pc, &payload(&binding)).await.keys(),
        vec![key(&hong)]
    );
    assert_eq!(h.get(PEERS, &token).await.json["total"], 0);
    let device = format!("/api/hjv/devices/{}", pc.id);
    assert_eq!(h.delete(&device, &token).await.status, 404);
    let wake = h
        .post(&format!("{device}/wake"), &token, Some(json!({})))
        .await;
    assert_eq!(wake.status, 404, "{}", wake.text);
    assert_eq!(h.get(&format!("{device}/wol"), &token).await.status, 404);
    let undo = h.post("/api/hjv/this-device/undo", &token, None).await;
    assert_eq!(undo.json["status"], "not_eligible", "{}", undo.text);
    assert_eq!(h.binding_of(&pc.id).await.len(), 32);
}

/// An account of the file's ID that its login did not make, one of before the ID rule with a PC
/// that lets the account's keys in without the password, is not its to go into: the right
/// password fails as a wrong one, the PC's list gets no key, the account keeps its name and keys
/// and stays off the allowlist when it is off, and the journal says why.
#[tokio::test]
async fn the_review_login_never_goes_into_an_account_it_did_not_make() {
    let (h, file, _dir) = start(vec![]).await;
    let pc = TestDevice::windows("700000002");
    h.hbbs.register(&pc, Some(1));
    let hong = h.login_ok(HONG, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    // the account under a key outside the rule, as one made before the rule (2026-10-14)
    h.state
        .db
        .call(|c| {
            c.execute_batch(
                "UPDATE allowlist SET login_key = 'appreview-test' WHERE login_key = 'zztest01';
                 UPDATE accounts SET login_key = 'appreview-test', portal_id = 'APPREVIEW-TEST'
                   WHERE login_key = 'zztest01';",
            )
        })
        .await
        .unwrap();
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let refused = json!({ "error": FAILURE_TEXT });
    let a = review_login(&h, "10.0.70.1").await;
    assert_eq!((a.status, &a.json), (401, &refused));
    assert_eq!(h.results().await.last().unwrap(), "login:review-existing");
    let key = b64(&devsig::public_key(&h.key_of(&hong).unwrap()));
    let synced = h.sync(&pc, &payload(&binding)).await;
    assert_eq!(
        (synced.list()["state"].clone(), synced.keys()),
        (json!("active"), vec![key])
    );
    // off the allowlist, the login does not put it back on
    h.state
        .db
        .call(|c| {
            c.execute(
                "DELETE FROM allowlist WHERE login_key = 'appreview-test'",
                [],
            )
        })
        .await
        .unwrap();
    let a = review_login(&h, "10.0.70.1").await;
    assert_eq!((a.status, &a.json), (401, &refused));
    assert_eq!(h.results().await.last().unwrap(), "login:review-existing");
    let (account, keys, listing) = h
        .state
        .db
        .call(|c| {
            let account = db::account_by_key(c, "appreview-test")?.unwrap();
            let keys = db::keys_of(c, account.id)?.len();
            Ok((account, keys, db::listing(c, "appreview-test")?))
        })
        .await
        .unwrap();
    assert_ne!(account.display_name, review::NAME);
    assert_eq!((keys, listing), (1, db::Listing::Absent));
    let line = format!(
        "login: the App Review file names the ID of an account that is not the App Review account's (acct={}); its logins fail until the file names another ID",
        h.account_tag(REVIEW.0)
    );
    assert!(hjv_api::log::captured().contains(&line));
}

/// Door 1 and short links refuse the account's login, for every platform and whether a package
/// is there or not.
#[tokio::test]
async fn door_1_and_short_links_refuse_the_review_account() {
    let (h, file, _dir) = start(vec![]).await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let token = review_login(&h, "10.0.64.1").await.token();
    let refused = json!({ "error": TEXT_REVIEW });
    for platform in ["windows", "android"] {
        let door1 = format!("/api/hjv/update/{platform}/1.6.1");
        let a = h.get(&door1, &token).await;
        assert_eq!((a.status, &a.json), (403, &refused), "{platform}");
        let a = h.signed(Method::HEAD, &door1, &token, None).await;
        assert_eq!(a.status, 403, "{platform}");
        let a = h
            .post(
                &format!("/api/hjv/update/{platform}/link"),
                &token,
                Some(json!({"installed": false})),
            )
            .await;
        assert_eq!((a.status, &a.json), (403, &refused), "{platform}");
    }
}

/// Not a super operator, whatever HJV_API_OPERATORS says: no role, and the overview is the 404 of
/// an unknown path.
#[tokio::test]
async fn the_review_account_is_never_a_super_operator() {
    let (h, file, _dir) = start(vec![(
        "HJV_API_OPERATORS",
        format!("{},{}", REVIEW.0, HONG.0),
    )])
    .await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let a = review_login(&h, "10.0.65.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    assert!(a.json["user"].get("hjv_role").is_none(), "{}", a.text);
    let token = a.token();
    let o = h.get(overview::PATH, &token).await;
    assert_eq!((o.status, o.json), (404, json!({"error": "not found"})));
    let keys = h.get("/api/hjv/keys", &token).await;
    assert!(keys.json.get("hjv_role").is_none(), "{}", keys.text);
    let staff = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.0.65.2")
        .await;
    assert_eq!(staff.json["user"]["hjv_role"], "operator");
}

/// In list mode the ID logs in once the operator imported it, as any ID: before, it fails as an
/// ID off the list, whatever the password, which is not even checked.
#[tokio::test]
async fn in_list_mode_the_operator_imports_the_id() {
    let (h, file, _dir) = start(vec![("HJV_API_LOGIN", "list".into())]).await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let wrong = h
        .login(&h.login_body(REVIEW.0, "wrong", None), "10.0.66.1")
        .await;
    for a in [&wrong, &review_login(&h, "10.0.66.1").await] {
        assert_eq!(
            (a.status, &a.json),
            (401, &json!({ "error": FAILURE_TEXT }))
        );
    }
    h.admin(
        &["allow", "import"],
        false,
        Some(&format!("{}\n", REVIEW.0)),
    )
    .await
    .unwrap();
    assert_eq!(review_login(&h, "10.0.66.1").await.status, 200);
    assert_eq!(
        h.results().await,
        vec!["login:not-allowed", "login:not-allowed", "login:ok"]
    );
    assert_eq!(h.sso.token_calls(), 0);
}

/// The account's tokens hold while the file names it: once the file goes, names another ID or
/// its UNTIL comes, its requests are answered as an unknown token's (the app logs out), and they
/// hold again when the file names it again. A staff account's tokens are not touched. The end, as
/// the runbook ends it: the file deleted, then the account.
#[tokio::test]
async fn the_review_session_ends_with_the_file() {
    let (h, file, _dir) = start(vec![]).await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let token = review_login(&h, "10.0.67.1").await.token();
    let staff = h.login_ok(HONG, None).await;
    assert_eq!(status_of(&h, &token).await, 200);
    std::fs::remove_file(&file).unwrap();
    assert_eq!(status_of(&h, &token).await, 401);
    assert_eq!(status_of(&h, &staff).await, 200);
    put(&file, &review_file(REVIEW.0, now() + DAY));
    assert_eq!(status_of(&h, &token).await, 200);
    put(&file, &review_file("OTHER-DEMO", now() + DAY));
    assert_eq!(status_of(&h, &token).await, 401);
    // its UNTIL comes while the file stays as it is
    let until = now() + 2;
    put(&file, &review_file(REVIEW.0, until));
    assert_eq!(status_of(&h, &token).await, 200);
    while now() < until {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(status_of(&h, &token).await, 401);
    put(&file, &review_file(REVIEW.0, now() - 60));
    assert_eq!(status_of(&h, &token).await, 401);
    let out = h.post("/api/logout", &token, Some(json!({}))).await;
    assert_eq!((out.status, out.json), (401, unknown_token()));
    assert_eq!(status_of(&h, &staff).await, 200);
    std::fs::remove_file(&file).unwrap();
    let tag = h.account_tag(REVIEW.0);
    let out = h
        .admin(&["user", "delete", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    assert!(out.starts_with("deleted the account"), "{out}");
    let gone = h
        .state
        .db
        .call(|c| db::account_by_key(c, "appreview-test"))
        .await
        .unwrap();
    assert!(gone.is_none());
}

/// The account deletes itself in the app as any other (1.9.1, account.rs), at once, but its
/// allowlist entry stays: the next review login, in open mode and in list mode alike, makes a new,
/// empty account (not the deleted one's: no key of it is left to notice). The journal says it was
/// the App Review account, by its tag (a log key of each harness's own: the line is that harness's);
/// the operator record, that its entry was kept.
#[tokio::test]
async fn the_review_account_deletes_itself_and_comes_back_empty() {
    for list in [false, true] {
        let vars = if list {
            vec![("HJV_API_LOGIN", "list".into())]
        } else {
            vec![]
        };
        let (h, file, _dir) = start_with(Options {
            vars,
            log_key: Some(random_bytes::<32>()),
            ..Default::default()
        })
        .await;
        put(&file, &review_file(REVIEW.0, now() + DAY));
        if list {
            h.admin(
                &["allow", "import"],
                false,
                Some(&format!("{}\n", REVIEW.0)),
            )
            .await
            .unwrap();
        }
        let of = || {
            h.state.db.call(|c| {
                Ok((
                    db::account_by_key(c, "appreview-test")?,
                    db::listing(c, "appreview-test")?,
                ))
            })
        };
        let token = review_login(&h, "10.0.71.1").await.token();
        let a = h
            .post(
                account::DELETE_PATH,
                &token,
                Some(json!({"confirm": "delete-account"})),
            )
            .await;
        assert_eq!(a.status, 200, "{}", a.text);
        assert_eq!(
            a.json,
            json!({"deleted": true, "pcs": 0, "devices": 1, "relays": 0})
        );
        let (gone, listing) = of().await.unwrap();
        assert!(gone.is_none());
        assert_eq!(listing, db::Listing::Registered, "list mode: {list}");
        assert_eq!(status_of(&h, &token).await, 401);
        let again = review_login(&h, "10.0.71.1").await;
        assert_eq!(again.status, 200, "{}", again.text);
        assert_eq!(again.json["user"]["display_name"], review::NAME);
        assert_eq!(h.results().await.last().unwrap(), "login:ok");
        assert!(of().await.unwrap().0.is_some());
        // a new account: the deleted one's key is nowhere, not even among the ended ones
        let keys = h.get("/api/hjv/keys", &again.token()).await;
        assert_eq!(
            (&keys.json["total"], &keys.json["ended"]),
            (&json!(1), &json!([])),
            "{}",
            keys.text
        );
        let tag = h.account_tag(REVIEW.0);
        let record = std::fs::read_to_string(&h.oplog).unwrap();
        assert!(
            record.contains(&format!(
                "cmd=\"app account-delete\" targets={tag} reveal=0"
            )),
            "{record}"
        );
        assert!(
            record.contains(&format!("cmd=\"app account-delete\" acct={tag} done=\"ok: 0 PCs, 1 account keys, 1 tokens, 0 relays, allowlist entry kept (App Review)\"")),
            "{record}"
        );
        let line = format!(
            "account: deleted at its own request (acct={tag}, the App Review account): 0 PCs, 1 account keys, 1 tokens, 0 relays, allowlist entry kept"
        );
        assert!(
            hjv_api::log::captured().contains(&line),
            "list mode: {list}"
        );
    }
}

/// At the start (main.rs) the journal says whether there is an account: the file's own line when
/// it is read, and `no App Review account` when there is none, also after the line of a file
/// whose UNTIL has come (runbook 7-17); without the setting, nothing.
#[tokio::test]
async fn the_start_says_whether_there_is_an_account() {
    let none = "login: no App Review account (HJV_API_REVIEW_FILE is not there, not used, or past its UNTIL)";
    // only this test starts, so only it writes that line; the UNTILs are its own
    let lines = |of: &str| {
        hjv_api::log::captured()
            .iter()
            .filter(|l| l.as_str() == of)
            .count()
    };
    let (h, file, _dir) = start(vec![]).await;
    let read = |until: i64| {
        format!(
            "login: the App Review file (HJV_API_REVIEW_FILE) read: until {}, 0 demo PC(s) (acct={})",
            datetime(until),
            h.account_tag(REVIEW.0)
        )
    };
    review::note_at_start(&h.state);
    assert_eq!(lines(none), 1);
    let until = now() + 3 * DAY + 7;
    put(&file, &review_file(REVIEW.0, until));
    review::note_at_start(&h.state);
    assert_eq!((lines(&read(until)), lines(none)), (1, 1));
    let past = now() - 3 * DAY - 7;
    put(&file, &review_file(REVIEW.0, past));
    review::note_at_start(&h.state);
    assert_eq!((lines(&read(past)), lines(none)), (1, 2));
    let off = Harness::start().await;
    review::note_at_start(&off.state);
    assert_eq!(lines(none), 2);
}

// ---- the demo PCs (1.9.2) ----

/// A login of the account on a demo PC of its file's PC= line binds it as a staff login binds its
/// PC: installed, of HJV_API_BIND_OS, its signature matching the key hbbs holds. Elsewhere it binds
/// nothing: a PC off the line is not eligible before hbbs is asked, the line's PC that a staff
/// account holds stays that account's. The account's own lists do not show its demo PC: My PCs is
/// empty, it takes the PC out neither there nor with [취소], and the wake paths do not know it.
/// The journal names the PC and the account by their tags.
#[tokio::test]
async fn the_review_account_binds_its_demo_pcs() {
    let (h, file, _dir) = start(vec![]).await;
    let demo = TestDevice::windows("700000011");
    let staffs = TestDevice::windows("700000012");
    let odd = TestDevice::windows("700000013");
    let off_line = TestDevice::windows("700000014");
    for pc in [&demo, &staffs, &odd, &off_line] {
        h.hbbs.register(pc, Some(1));
    }
    put(
        &file,
        &review_file_with(REVIEW.0, now() + DAY, &[&demo, &staffs, &odd]),
    );
    let asked = || h.hbbs.queries.load(Ordering::SeqCst);
    let status = |token: String| {
        let h = &h;
        async move { h.get("/api/hjv/this-device", &token).await.json }
    };
    // the demo PC, bound, as the login says it of any PC
    let a = review_login_on(&h, &demo, "10.0.72.1").await;
    assert_eq!(a.status, 200, "{}", a.text);
    let binding = a.json["hjv"]["binding"].as_str().unwrap_or("").to_owned();
    assert_eq!(binding.len(), 32, "{}", a.text);
    assert!(a.json["hjv"]["login_at"].is_i64(), "{}", a.text);
    assert_eq!(h.binding_of(&demo.id).await, binding);
    assert_eq!(asked(), 1);
    // its login's own ticket stays the account's: review/ctl for its demo PCs
    let p = ticket_of(&a.json["hjv"]["access"]);
    assert_eq!(
        (&p["who"], &p["use"], &p["to"]),
        (
            &json!("review"),
            &json!("ctl"),
            &json!([&demo.id, &staffs.id, &odd.id])
        )
    );
    let on_pc = a.token();
    let this = status(on_pc.clone()).await;
    assert_eq!(
        (
            &this["status"],
            &this["undo"],
            &this["binding"],
            &this["pwless"]
        ),
        (
            &json!("added"),
            &json!(false),
            &json!(binding),
            &json!("disabled")
        ),
        "{this}"
    );
    // [취소] takes nothing out
    let undo = h.post("/api/hjv/this-device/undo", &on_pc, None).await;
    assert_eq!(
        (undo.status, &undo.json["status"]),
        (200, &json!("added")),
        "{}",
        undo.text
    );
    assert_eq!(h.binding_of(&demo.id).await, binding);
    // the same PC again: already, the same binding
    let again = review_login_on(&h, &demo, "10.0.72.1").await;
    assert_eq!(
        again.json["hjv"]["binding"],
        json!(binding),
        "{}",
        again.text
    );
    assert_eq!(status(again.token()).await["status"], "already");
    // a PC off the line: not eligible, hbbs not asked, nothing to sync
    let a = review_login_on(&h, &off_line, "10.0.72.2").await;
    assert!(a.json["hjv"].get("binding").is_none(), "{}", a.text);
    assert_eq!(status(a.token()).await["status"], "not_eligible");
    assert_eq!(asked(), 2);
    assert_eq!(h.binding_of(&off_line.id).await, "");
    assert_eq!(h.sync(&off_line, &payload("")).await.answer.status, 404);
    // the line's PC that a staff account holds stays its own
    h.login_ok(HONG, Some(&staffs)).await;
    let theirs = h.binding_of(&staffs.id).await;
    let a = review_login_on(&h, &staffs, "10.0.72.3").await;
    assert!(a.json["hjv"].get("binding").is_none(), "{}", a.text);
    assert_eq!(status(a.token()).await["status"], "other_account");
    assert_eq!(h.binding_of(&staffs.id).await, theirs);
    // the line's PC as any: not installed, of another OS or unsigned is not eligible, before hbbs
    // is asked; another key in hbbs fails the check
    let before = asked();
    let portable = TestDevice {
        installed: false,
        ..odd.clone()
    };
    let linux = TestDevice {
        os: "linux".into(),
        ..odd.clone()
    };
    // (one at a time: a login from a device ends the token of the one before from it)
    for device in [portable, linux, odd.clone()] {
        let mut body = h.login_body(REVIEW.0, REVIEW.1, Some(&device));
        if device.installed && device.os == "windows" {
            body.as_object_mut().unwrap().remove("hjv");
        }
        let a = h.login(&body, "10.0.72.4").await;
        assert!(a.json["hjv"].get("binding").is_none(), "{}", a.text);
        assert_eq!(status(a.token()).await["status"], "not_eligible");
    }
    assert_eq!(asked(), before);
    h.hbbs
        .register_key(&odd.id, devsig::public_key(&[0x77; 32]));
    let a = review_login_on(&h, &odd, "10.0.72.4").await;
    assert_eq!(status(a.token()).await["status"], "check_failed");
    assert_eq!(h.binding_of(&odd.id).await, "");
    // the account's own lists: no PC in My PCs, none to take out, none to wake
    let ipad = review_login(&h, "10.0.72.9").await.token();
    assert_eq!(
        h.get(PEERS, &ipad).await.json,
        json!({"total": 0, "data": []})
    );
    let device = format!("/api/hjv/devices/{}", demo.id);
    let removed = h.delete(&device, &ipad).await;
    assert_eq!(removed.status, 404, "{}", removed.text);
    let wake = h
        .post(&format!("{device}/wake"), &ipad, Some(json!({})))
        .await;
    assert_eq!(wake.status, 404, "{}", wake.text);
    assert_eq!(h.get(&format!("{device}/wol"), &ipad).await.status, 404);
    assert_eq!(h.binding_of(&demo.id).await, binding);
    // the journal: the PC that joined, by its tag and the account's
    let line = format!(
        "login: a demo PC joined the App Review account (acct={}, dev={})",
        h.account_tag(REVIEW.0),
        h.state.tags.device(&demo.id)
    );
    let logs = hjv_api::log::captured();
    assert!(logs.contains(&line), "{logs:?}");
    for line in &logs {
        assert!(!line.contains(&demo.id), "{line}");
    }
}

/// A demo PC's sync carries its ticket, `staff`/`pc` for its ID and key, the registration alone,
/// for HJV_API_TICKET_HOURS and never past the file's UNTIL, under a list without keys whatever the
/// switches say. The file past its UNTIL, gone, naming another ID or without the PC on its line:
/// the list as before, and no ticket.
#[tokio::test]
async fn a_demo_pc_registers_on_a_staff_pc_ticket_until_until() {
    let (h, file, _dir) = start(vec![]).await;
    let demo = TestDevice::windows("700000021");
    let other = TestDevice::windows("700000022");
    h.hbbs.register(&demo, Some(1));
    put(
        &file,
        &review_file_with(REVIEW.0, now() + 2 * DAY, &[&demo]),
    );
    let (binding, _) = bind_demo(&h, &demo).await;
    let tag = h.account_tag(REVIEW.0);
    // password-less on for everyone and for the account: no key all the same
    h.admin(&["passwordless", "on"], false, None).await.unwrap();
    let out = h
        .admin(
            &["passwordless", "on", "--account", &format!("tag:{tag}")],
            false,
            None,
        )
        .await
        .unwrap();
    assert!(
        out.ends_with(
            "; the App Review account's PCs take no password-less connection, whatever its switch"
        ),
        "{out}"
    );
    let synced = h.sync(&demo, &payload(&binding)).await;
    assert_eq!(synced.list()["state"], "disabled");
    assert!(synced.keys().is_empty());
    let p = ticket_of(&synced.answer.json["access"]);
    assert_eq!(
        (&p["who"], &p["use"], &p["sub"], &p["id"], &p["pk"], &p["to"]),
        (
            &json!("staff"),
            &json!("pc"),
            &json!(tag),
            &json!(demo.id),
            &json!(b64(&demo.public_key())),
            &json!([])
        )
    );
    let life = |p: &Value| p["exp"].as_i64().unwrap() - p["iat"].as_i64().unwrap();
    assert_eq!(life(&p), DAY);
    // never past UNTIL
    let until = now() + 3600;
    put(&file, &review_file_with(REVIEW.0, until, &[&demo]));
    let synced = h.sync(&demo, &payload(&binding)).await;
    assert_eq!(
        ticket_of(&synced.answer.json["access"])["exp"].as_i64(),
        Some(until)
    );
    // no file that gives the PC a ticket: a list, no ticket
    for text in [
        Some(review_file_with(REVIEW.0, now() - 1, &[&demo])),
        Some(review_file_with(REVIEW.0, now() + DAY, &[&other])),
        Some(review_file(REVIEW.0, now() + DAY)),
        Some(review_file_with("OTHER-DEMO", now() + DAY, &[&demo])),
        None,
    ] {
        match &text {
            Some(text) => put(&file, text),
            None => std::fs::remove_file(&file).unwrap(),
        }
        let synced = h.sync(&demo, &payload(&binding)).await;
        assert_eq!(synced.list()["state"], "disabled", "{text:?}");
        assert!(
            synced.answer.json.get("access").is_none(),
            "{text:?}: {}",
            synced.answer.text
        );
    }
    // the file back: the ticket again; the operator counts them as sync pc
    put(&file, &review_file_with(REVIEW.0, now() + DAY, &[&demo]));
    let synced = h.sync(&demo, &payload(&binding)).await;
    assert_eq!(ticket_of(&synced.answer.json["access"])["use"], "pc");
    let status = h.admin(&["ticket", "status"], false, None).await.unwrap();
    assert!(status.contains("sync pc 3"), "{status}");
    assert!(!status.contains("sync both"), "{status}");
    assert!(
        status.contains("the App Review account's demo PCs 24 h, never past its file's UNTIL"),
        "{status}"
    );
    // HJV_API_TICKET_HOURS sets its lifetime, as the account's own ticket's
    let (short, short_file, _short_dir) = start(vec![("HJV_API_TICKET_HOURS", "6".into())]).await;
    short.hbbs.register(&demo, Some(1));
    put(
        &short_file,
        &review_file_with(REVIEW.0, now() + DAY, &[&demo]),
    );
    let (binding, _) = bind_demo(&short, &demo).await;
    let synced = short.sync(&demo, &payload(&binding)).await;
    assert_eq!(life(&ticket_of(&synced.answer.json["access"])), 6 * HOUR);
}

/// The account's own deletion in the app keeps the demo PCs it holds of its file's line: bound as
/// they were, to a new, empty account of its ID that the deletion makes, so that a reviewer who
/// deletes the account finds the demo PC online after the next login, in an account with no PC in
/// My PCs and one key. Everything else goes as in any deletion: its logins end, a PC that left the
/// line gets its tombstone, and a PC of the line that a staff account holds stays that account's.
/// In open and in list mode; the record and the journal count the PC kept.
#[tokio::test]
async fn the_review_accounts_deletion_keeps_its_demo_pcs() {
    for list in [false, true] {
        let (h, file, _dir) = start_with(Options {
            vars: if list {
                vec![("HJV_API_LOGIN", "list".into())]
            } else {
                vec![]
            },
            log_key: Some(random_bytes::<32>()),
            ..Default::default()
        })
        .await;
        if list {
            h.admin(
                &["allow", "import"],
                false,
                Some(&format!("{}\n", REVIEW.0)),
            )
            .await
            .unwrap();
        }
        let demo = TestDevice::windows("700000031");
        let left = TestDevice::windows("700000032");
        let staffs = TestDevice::windows("700000033");
        for pc in [&demo, &left, &staffs] {
            h.hbbs.register(pc, Some(1));
        }
        put(
            &file,
            &review_file_with(REVIEW.0, now() + DAY, &[&demo, &left, &staffs]),
        );
        let (demo_binding, on_demo) = bind_demo(&h, &demo).await;
        let (left_binding, _) = bind_demo(&h, &left).await;
        h.login_ok(HONG, Some(&staffs)).await;
        let theirs = h.binding_of(&staffs.id).await;
        // `left` leaves the line
        put(
            &file,
            &review_file_with(REVIEW.0, now() + DAY, &[&demo, &staffs]),
        );
        let ipad = review_login(&h, "10.0.73.1").await.token();
        let other = review_login(&h, "10.0.73.2").await.token();
        let keys = h.get("/api/hjv/keys", &ipad).await.json["total"].clone();
        assert_eq!(keys, json!(4));
        let old = review_account(&h).await.unwrap();
        let a = h.post(account::DELETE_PATH, &ipad, confirm()).await;
        assert_eq!(a.status, 200, "{}", a.text);
        assert_eq!(
            a.json,
            json!({"deleted": true, "pcs": 1, "devices": 4, "relays": 0})
        );
        // every login of it ended, the demo PC's own too
        for token in [&ipad, &other, &on_demo] {
            assert_eq!(status_of(&h, token).await, 401);
        }
        // a new account of its ID: another row, made now, of the account's values
        let new = review_account(&h).await.unwrap();
        assert!(new.id != old.id && review::is_account(&new), "{new:?}");
        assert!(new.created_at >= old.created_at);
        // the demo PC in it as it was: its binding, a list without keys and its ticket
        assert_eq!(h.binding_of(&demo.id).await, demo_binding);
        let synced = h.sync(&demo, &payload(&demo_binding)).await;
        assert_eq!(synced.list()["state"], "disabled");
        let p = ticket_of(&synced.answer.json["access"]);
        assert_eq!(
            (&p["use"], &p["sub"]),
            (&json!("pc"), &json!(h.account_tag(REVIEW.0)))
        );
        // the PC that left the line: a signed revoked, no ticket
        let revoked = h.sync(&left, &payload(&left_binding)).await;
        assert_eq!(revoked.list()["state"], "revoked");
        assert!(revoked.answer.json.get("access").is_none());
        // the staff account's PC of the line: its own, its ticket staff/both
        assert_eq!(h.binding_of(&staffs.id).await, theirs);
        let synced = h.sync(&staffs, &payload(&theirs)).await;
        assert_eq!(ticket_of(&synced.answer.json["access"])["use"], "both");
        // the record and the journal count the demo PC kept
        let tag = h.account_tag(REVIEW.0);
        let record = std::fs::read_to_string(&h.oplog).unwrap();
        let done = format!("cmd=\"app account-delete\" acct={tag} done=\"ok: 1 PCs, 4 account keys, 4 tokens, 0 relays, allowlist entry kept, 1 demo PCs kept (App Review)\"");
        assert!(record.contains(&done), "{record}");
        let line = format!(
            "account: deleted at its own request (acct={tag}, the App Review account): 1 PCs, 4 account keys, 4 tokens, 0 relays, allowlist entry kept, 1 demo PCs kept"
        );
        assert!(hjv_api::log::captured().contains(&line), "list: {list}");
        // the next login: the new account, empty to the reviewer, the demo PC still bound
        let again = review_login(&h, "10.0.73.1").await;
        assert_eq!(again.status, 200, "{}", again.text);
        assert_eq!(h.results().await.last().unwrap(), "login:ok");
        let token = again.token();
        assert_eq!(
            h.get(PEERS, &token).await.json,
            json!({"total": 0, "data": []})
        );
        let keys = h.get("/api/hjv/keys", &token).await;
        assert_eq!(
            (&keys.json["total"], &keys.json["ended"]),
            (&json!(1), &json!([])),
            "{}",
            keys.text
        );
        assert_eq!(review_account(&h).await.unwrap().id, new.id);
        assert_eq!(h.binding_of(&demo.id).await, demo_binding);
        let synced = h.sync(&demo, &payload(&demo_binding)).await;
        assert_eq!(ticket_of(&synced.answer.json["access"])["use"], "pc");
        let listing = h
            .state
            .db
            .call(|c| db::listing(c, "appreview-test"))
            .await
            .unwrap();
        assert_eq!(listing, db::Listing::Registered, "list: {list}");
    }
}

/// The demo PC's own login deletes nothing: the app there lets its PC go on `"deleted":true`. Its
/// [계정 삭제] gets 403 `demo_pc` with the text that sends it to the iPhone or iPad, before the
/// record takes a line; so does a login that only claims the PC's ID. The iPhone's or iPad's goes
/// through.
#[tokio::test]
async fn a_demo_pcs_own_login_deletes_nothing() {
    let (h, file, _dir) = start(vec![]).await;
    let demo = TestDevice::windows("700000041");
    h.hbbs.register(&demo, Some(1));
    put(&file, &review_file_with(REVIEW.0, now() + DAY, &[&demo]));
    let (binding, on_pc) = bind_demo(&h, &demo).await;
    let refused = json!({"error": account::TEXT_DEMO_PC, "hjv": "demo_pc"});
    let lines = || {
        std::fs::read_to_string(&h.oplog)
            .unwrap_or_default()
            .matches("cmd=\"app account-delete\"")
            .count()
    };
    let d = h.post(account::DELETE_PATH, &on_pc, confirm()).await;
    assert_eq!((d.status, &d.json), (403, &refused), "{}", d.text);
    assert_eq!(status_of(&h, &on_pc).await, 200);
    assert_eq!(h.binding_of(&demo.id).await, binding);
    assert_eq!(lines(), 0);
    // a login that claims the PC's ID, its key not the one hbbs holds
    h.hbbs
        .register_key(&demo.id, devsig::public_key(&[0x77; 32]));
    let claimed = review_login_on(&h, &demo, "10.0.74.2").await.token();
    let this = h.get("/api/hjv/this-device", &claimed).await;
    assert_eq!(this.json["status"], "check_failed", "{}", this.text);
    let d = h.post(account::DELETE_PATH, &claimed, confirm()).await;
    assert_eq!((d.status, &d.json), (403, &refused), "{}", d.text);
    assert_eq!(h.binding_of(&demo.id).await, binding);
    assert_eq!(lines(), 0);
    let line = format!(
        "account delete: refused from a demo PC of the App Review account (acct={}); the next ten minutes are not logged",
        h.account_tag(REVIEW.0)
    );
    assert!(hjv_api::log::captured().contains(&line));
    // the iPad's
    h.hbbs.register(&demo, Some(1));
    let ipad = review_login(&h, "10.0.74.3").await.token();
    let d = h.post(account::DELETE_PATH, &ipad, confirm()).await;
    assert_eq!(d.status, 200, "{}", d.text);
    assert_eq!(d.json["pcs"], 0);
    assert_eq!(h.binding_of(&demo.id).await, binding);
}

/// The demo PC's own [계정 삭제] that authenticated just before the iPad's deletion moved the PC
/// to the account's new row, and looks for its PC only after that, is refused all the same: the
/// check goes by the account's login key, which the new row keeps (account.rs `on_demo_pc`). By
/// the old row's id it found the PC in another account and went on, and the deletion found its
/// account gone: 200 `"deleted":true`, on which the 1.9.2 app there lets its PC go. The order is
/// made, not hoped for: the test runs on one runtime thread, which also runs the request's task.
/// The database is held while the request waits in its authentication; from the end of the hold
/// this thread runs nothing until the authentication has used its nonce and the iPad's
/// transaction, made here on a second connection as the deletion makes it, is in, so that the
/// request's next step comes after it. A try whose request had not reached its authentication in
/// time runs as any request of the PC (403) and is made again.
#[tokio::test(flavor = "current_thread")]
async fn a_demo_pcs_deletion_that_overlaps_the_ipads_deletes_nothing() {
    let (h, file, _dir) = start(vec![]).await;
    let demo = TestDevice::windows("700000071");
    h.hbbs.register(&demo, Some(1));
    put(&file, &review_file_with(REVIEW.0, now() + DAY, &[&demo]));
    let (binding, on_pc) = bind_demo(&h, &demo).await;
    let refused = json!({"error": account::TEXT_DEMO_PC, "hjv": "demo_pc"});
    let h = Arc::new(h);
    let mut other = db::open(&h.db_path()).unwrap();
    // the nonces of the PC's login key, which each signed request's authentication writes
    let hash = sha256(on_pc.as_bytes());
    let ckid: i64 = other
        .query_row(
            "SELECT ckid FROM tokens WHERE hash = ?1",
            [&hash[..]],
            |r| r.get(0),
        )
        .unwrap();
    let nonces = |c: &rusqlite::Connection| -> i64 {
        c.query_row(
            "SELECT COUNT(*) FROM req_nonces WHERE ckid = ?1",
            [ckid],
            |r| r.get(0),
        )
        .unwrap()
    };
    let old = review_account(&h).await.unwrap();
    let mut tries = 0;
    let answer = loop {
        tries += 1;
        assert!(
            tries <= 5,
            "the request did not wait in its authentication in 5 tries"
        );
        let before = nonces(&other);
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (end_tx, end_rx) = std::sync::mpsc::channel::<()>();
        let hold = {
            let state = h.state.clone();
            tokio::spawn(async move {
                state
                    .db
                    .call(move |_| {
                        held_tx.send(()).unwrap();
                        let _ = end_rx.recv_timeout(Duration::from_secs(10));
                        Ok(())
                    })
                    .await
                    .unwrap()
            })
        };
        let started = Instant::now();
        while held_rx.try_recv().is_err() {
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "the database was not held"
            );
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        let request = {
            let (h, token) = (h.clone(), on_pc.clone());
            tokio::spawn(async move { h.post(account::DELETE_PATH, &token, confirm()).await })
        };
        // the request reaches its authentication and waits there for the database
        tokio::time::sleep(Duration::from_millis(200)).await;
        // from here this thread runs no task until the iPad's transaction is in
        end_tx.send(()).unwrap();
        let started = Instant::now();
        while nonces(&other) == before && started.elapsed() < Duration::from_secs(3) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let made = nonces(&other) > before;
        if made {
            // the iPad's deletion as account.rs `delete_now` makes it: the tokens, then the
            // account, its demo PC kept in a new row
            let pcs = [demo.id.clone()];
            let kept = db::in_tx(&mut other, |tx| {
                db::revoke_tokens(tx, old.id)?;
                let keep = db::Keep {
                    listing: true,
                    demo_pcs: &pcs,
                };
                db::delete_account_by_user(tx, old.id, keep, now())
            })
            .unwrap();
            assert_eq!(kept, Some(1));
        }
        hold.await.unwrap();
        let answer = request.await.unwrap();
        if made {
            break answer;
        }
        // not the case: the request ran after the hold as any of the PC's
        assert_eq!(
            (answer.status, &answer.json),
            (403, &refused),
            "{}",
            answer.text
        );
    };
    assert_eq!(
        (answer.status, &answer.json),
        (403, &refused),
        "{}",
        answer.text
    );
    // nothing recorded or deleted by it; its login went with the iPad's deletion, and the PC is
    // the new row's, bound and ticketed as it was
    let record = std::fs::read_to_string(&h.oplog).unwrap_or_default();
    assert_eq!(record.matches("cmd=\"app account-delete\"").count(), 0);
    assert_eq!(status_of(&h, &on_pc).await, 401);
    let new = review_account(&h).await.unwrap();
    assert!(new.id != old.id && review::is_account(&new), "{new:?}");
    assert_eq!(h.binding_of(&demo.id).await, binding);
    assert!(ticketed(&h, &demo, &binding).await);
}

/// What ends the account's logins leaves its demo PC bound and registered: the operator's
/// revoke-all, a logout on the iPad, and a restart of hjv-api. The operator's `user delete`
/// releases it: a signed revoked, no ticket.
#[tokio::test]
async fn revoke_all_logout_and_a_restart_leave_the_demo_pc_online() {
    let (h, file, _dir) = start(vec![]).await;
    let demo = TestDevice::windows("700000051");
    h.hbbs.register(&demo, Some(1));
    put(&file, &review_file_with(REVIEW.0, now() + DAY, &[&demo]));
    let (binding, on_pc) = bind_demo(&h, &demo).await;
    let tag = h.account_tag(REVIEW.0);
    let ipad = review_login(&h, "10.0.75.1").await.token();
    h.admin(&["user", "revoke-all", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    for token in [&ipad, &on_pc] {
        assert_eq!(status_of(&h, token).await, 401);
    }
    assert!(ticketed(&h, &demo, &binding).await);
    // a logout on the iPad, signed
    let ipad = review_login(&h, "10.0.75.1").await.token();
    let out = h.post("/api/logout", &ipad, Some(json!({}))).await;
    assert_eq!(out.status, 200, "{}", out.text);
    assert!(ticketed(&h, &demo, &binding).await);
    assert_eq!(h.binding_of(&demo.id).await, binding);
    // hjv-api started again on its database
    h.state.db.call(|c| db::checkpoint(c)).await.unwrap();
    h.stop();
    let copy = h.dir.join("copy.sqlite3");
    std::fs::copy(h.db_path(), &copy).unwrap();
    let r = Harness::with(Options {
        vars: vec![("HJV_API_REVIEW_FILE", file.display().to_string())],
        db_file: Some(copy),
        ..Default::default()
    })
    .await;
    assert!(ticketed(&r, &demo, &binding).await);
    assert_eq!(r.binding_of(&demo.id).await, binding);
    // the operator's user delete releases it
    let out = r
        .admin(&["user", "delete", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    assert!(
        out.starts_with("deleted the account with 1 devices"),
        "{out}"
    );
    let revoked = r.sync(&demo, &payload(&binding)).await;
    assert_eq!(revoked.list()["state"], "revoked");
    assert!(revoked.answer.json.get("access").is_none());
}

/// Door 1 gives a demo PC's device key no package: 403 once its signature holds, which counts
/// against no address; a staff account's PC goes on to the package (none here: 404), as before.
#[tokio::test]
async fn door_1_refuses_a_demo_pcs_device_key() {
    let (h, file, _dir) = start(vec![]).await;
    let demo = TestDevice::windows("700000061");
    let staffs = TestDevice::windows("700000062");
    for pc in [&demo, &staffs] {
        h.hbbs.register(pc, Some(1));
    }
    put(&file, &review_file_with(REVIEW.0, now() + DAY, &[&demo]));
    bind_demo(&h, &demo).await;
    h.login_ok(HONG, Some(&staffs)).await;
    let path = "/api/hjv/update/windows/1.9.3";
    let by_device = |pc: TestDevice, ip: &'static str| {
        let h = &h;
        async move {
            let t = now().to_string();
            let sig = devsig::sign(
                &pc.seed,
                &update::device_message(&pc.id, &t, "windows", "1.9.3"),
            );
            h.request(
                Method::GET,
                path,
                &[
                    ("x-hjv-device", pc.id.as_str()),
                    ("x-hjv-time", t.as_str()),
                    ("x-hjv-sig", sig.as_str()),
                    ("x-real-ip", ip),
                ],
                None,
            )
            .await
        }
    };
    let a = by_device(demo.clone(), "10.0.76.1").await;
    assert_eq!(
        (a.status, &a.json),
        (
            403,
            &json!({"error": "이 기기는 새 버전을 받을 수 없습니다."})
        ),
        "{}",
        a.text
    );
    let a = by_device(staffs.clone(), "10.0.76.2").await;
    assert_eq!(a.status, 404, "{}", a.text);
    for ip in ["10.0.76.1", "10.0.76.2"] {
        assert_eq!(
            h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(ip)).await,
            0,
            "{ip}"
        );
    }
}
