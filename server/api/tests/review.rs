// SPDX-License-Identifier: AGPL-3.0-only

//! The App Review account (review.rs) end to end: the login of the ID that the operator's file
//! names (HJV_API_REVIEW_FILE) without the SSO, with the limits and answers of any ID; the file
//! missing, past its UNTIL or refused; and what the account cannot reach (a PC, door 1, short
//! links, the overview, an account it did not make), its tokens ending with the file. The ID and
//! the password are made up: the real ones live only in the server's file.

mod common;

use common::*;
use hjv_api::{
    db, devsig,
    limits::SCOPE_ID,
    login::{FAILURE_TEXT, TEXT_ID_RULE, TEXT_SSO_DOWN},
    overview, review,
    sso::Failure,
    tfa::SecondFactor,
    update::TEXT_REVIEW,
    util::{b64, datetime, now, DAY},
};
use hyper::Method;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    time::Duration,
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

/// A login of the made-up account with its password, from `ip`.
async fn review_login(h: &Harness, ip: &str) -> Answer {
    h.login(&h.login_body(REVIEW.0, REVIEW.1, None), ip).await
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
        "ID, HASH or UNTIL is there more than once",
        "ID passes HJViewer's ID rule, so it could be a staff account's",
    ] {
        let line = format!("login: the App Review file (HJV_API_REVIEW_FILE) is not used: {why}");
        assert!(logs.contains(&line), "{why}");
    }
}

/// A login of the account on an installed Windows PC, signed with the key hbbs holds for it, puts
/// the PC in no list (hbbs is not even asked), and the account reaches no other account's PC.
#[tokio::test]
async fn the_review_account_never_holds_a_pc() {
    let (h, file, _dir) = start(vec![]).await;
    put(&file, &review_file(REVIEW.0, now() + DAY));
    let pc = TestDevice::windows("700000001");
    h.hbbs.register(&pc, Some(1));
    let a = h
        .login(&h.login_body(REVIEW.0, REVIEW.1, Some(&pc)), "10.0.63.1")
        .await;
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
            "login: the App Review file (HJV_API_REVIEW_FILE) read: until {} (acct={})",
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
