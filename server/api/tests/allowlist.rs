// SPDX-License-Identifier: AGPL-3.0-only

//! The open login of 2026-10-14 (HJV_API_LOGIN, login.rs steps 7 and 12, schema 5): a portal ID
//! that the SSO accepts and the allowlist does not have is put on it at that login; an entry the
//! operator blocks keeps its account out (its tokens and account keys end at once, its PCs get
//! lists without keys, its next login is refused after the SSO) until it is unblocked, also when
//! the block comes while a login runs; `allow remove` blocks; an account deleted takes its entry
//! with it unless the entry is blocked; list mode admits the IDs on the list only, as D3 did. A
//! block holds for every form of the person's ID the SSO names, and in open mode an ID nobody
//! registered has lower limits of its own (HJV_API_LIMIT_NEW_ID, HJV_API_LIMIT_SSO_NEW), of whose
//! SSO calls one ID and one client address hold a quarter each, also at the code step of a second
//! factor.

mod common;

use common::*;
use hjv_api::{
    check::{self, Health, Probe},
    db::{self, Listing},
    limits::{SCOPE_ID, SCOPE_OK_ID},
    login::{FAILURE_TEXT, TEXT_BLOCKED},
    tfa::SecondFactor,
    util::{b64, now, random_bytes},
};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

async fn listing(h: &Harness, id: &str) -> Listing {
    let key = hjv_api::util::login_key(id);
    h.state
        .db
        .call(move |c| db::listing(c, &key))
        .await
        .unwrap()
}

async fn entries(h: &Harness) -> Vec<db::ListEntry> {
    h.state.db.call(|c| db::allowlist(c)).await.unwrap()
}

async fn has_account(h: &Harness, id: &str) -> bool {
    let key = hjv_api::util::login_key(id);
    h.state
        .db
        .call(move |c| db::account_by_key(c, &key))
        .await
        .unwrap()
        .is_some()
}

/// The fake SSO's other username of HONG's person: it names HONG's perslNo for it.
const ALIAS: (&str, &str) = ("hong-alias", "hjv-test-pass-1");

#[tokio::test]
async fn an_id_never_imported_logs_in_and_is_registered() {
    // only KIM imported: HONG was never put on the list
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        ..Default::default()
    })
    .await;
    assert_eq!(listing(&h, HONG.0).await, Listing::Absent);
    let token = h.login_ok(HONG, None).await;
    assert_eq!(h.get("/api/users", &token).await.status, 200);
    assert_eq!(h.sso.token_calls(), 1);
    let tag = h.account_tag(HONG.0);
    // on the list now, as a login's entry
    let before = entries(&h).await;
    let hong = before.iter().find(|e| e.login_key == "2026000001").unwrap();
    assert_eq!((hong.source.as_str(), hong.blocked_at), ("login", None));
    let listed = h.admin(&["allow", "list"], false, None).await.unwrap();
    assert!(
        listed.starts_with("allowlist: 2 IDs, 2 registered (1 by a login), 0 blocked; login open"),
        "{listed}"
    );
    let line = listed.lines().find(|l| l.contains(&tag)).unwrap();
    assert!(
        line.contains("2026******") && line.contains(" registered ") && line.contains("from login"),
        "{line}"
    );
    assert!(!listed.contains("2026000001"), "{listed}");
    // one journal line with the account's tag, no portal ID
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!(
            "login: a portal ID that was not on the allowlist logged in and was registered (acct={tag})"
        )),
        "{logs}"
    );
    assert!(!logs.contains("2026000001"), "{logs}");
    // the report and selfcheck say which mode runs, and the report counts the registrations
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("\nlogin: open (anyone the Yonsei SSO accepts, unless blocked; "),
        "{report}"
    );
    assert!(
        report.contains("allowlist: 2 may log in (1 registered by a login: 1 in the last day, 1 in the last week), 0 blocked"),
        "{report}"
    );
    let check = h.admin(&["selfcheck"], false, None).await.unwrap();
    assert!(
        check.contains("[ OK ] login (HJV_API_LOGIN): open, anyone the Yonsei SSO accepts"),
        "{check}"
    );
    // the next login finds it there: nothing changes
    h.login_ok(HONG, None).await;
    assert_eq!(entries(&h).await, before);
    assert_eq!(h.results().await, vec!["login:ok", "login:ok"]);
}

#[tokio::test]
async fn new_and_registered_ids_look_the_same() {
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        vars: vec![
            ("HJV_API_FAILURE_DELAY_MS", "600".into()),
            ("HJV_API_FAILURE_JITTER_MS", "100".into()),
        ],
        ..Default::default()
    })
    .await;
    let listed = h
        .login(&h.login_body(KIM.0, "wrong", None), "10.5.0.1")
        .await;
    let new = h
        .login(&h.login_body("2026999999", "wrong", None), "10.5.0.2")
        .await;
    for a in [&listed, &new] {
        assert_eq!(a.status, 401, "{}", a.text);
        assert_eq!(a.json, json!({ "error": FAILURE_TEXT }), "{}", a.text);
        assert!(a.elapsed >= Duration::from_millis(500), "{:?}", a.elapsed);
    }
    // both reached the SSO and count; a failed login puts nothing on the list
    assert_eq!(h.sso.token_calls(), 2);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(KIM.0)).await, 1);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag("2026999999")).await, 1);
    assert_eq!(
        h.results().await,
        vec!["login:credentials", "login:credentials"]
    );
    assert_eq!(entries(&h).await.len(), 1);
}

#[tokio::test]
async fn a_blocked_account_is_logged_out_and_refused_until_unblocked() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("710000001");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    let phone = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    h.admin(&["passwordless", "on"], false, None).await.unwrap();
    assert_eq!(
        h.sync(&pc, &payload(&binding)).await.list()["state"],
        "active"
    );
    let tag = h.account_tag(HONG.0);
    let serial = h.change_serial().await;
    let out = h
        .admin(&["block", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    assert_eq!(
        out,
        "blocked: 1 blocked (0 of them not on the allowlist before), 0 blocked already, 2 tokens and 2 account keys revoked; the account's PCs get lists without keys, and its next login is refused after the SSO (hjv-api admin unblock lifts it)"
    );
    assert!(h.change_serial().await > serial);
    // its devices are logged out at once, the other account's are not
    assert_eq!(h.get("/api/users", &phone).await.status, 401);
    assert_eq!(h.get("/api/users", &kim).await.status, 200);
    // its PC stays bound, with a list without keys
    let list = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(
        (list["state"].clone(), list["keys"].clone()),
        (json!("disabled"), json!([]))
    );
    // blocked again: no change
    let out = h.admin(&["block", HONG.0], false, None).await.unwrap();
    assert!(
        out.starts_with("blocked already (no change): 0 blocked"),
        "{out}"
    );
    // a wrong password gets the failure everyone gets
    let a = h
        .login(&h.login_body(HONG.0, "wrong", None), "10.6.0.1")
        .await;
    assert_eq!(
        (a.status, &a.json),
        (401, &json!({ "error": FAILURE_TEXT }))
    );
    // the right one, from the PC: the blocked account's own answer after the SSO, hbbs not asked
    let (calls, queries) = (h.sso.token_calls(), h.hbbs.queries.load(Ordering::SeqCst));
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, Some(&pc)), "10.6.0.2")
        .await;
    assert_eq!(a.status, 403, "{}", a.text);
    assert_eq!(a.json, json!({ "error": TEXT_BLOCKED, "hjv": "blocked" }));
    assert_eq!(h.sso.token_calls(), calls + 1);
    assert_eq!(h.hbbs.queries.load(Ordering::SeqCst), queries);
    // counted as a success (a known password does not keep calling the SSO); its failure is
    // taken back, the wrong password's stays
    assert_eq!(h.counted(SCOPE_OK_ID, tag.clone()).await, 3);
    assert_eq!(h.counted(SCOPE_ID, tag.clone()).await, 1);
    assert_eq!(h.results().await.last().unwrap(), "login:blocked");
    // nothing written for it: no token, no key
    let (tokens, keys): (i64, i64) = h
        .state
        .db
        .call(|c| {
            c.query_row(
                "SELECT (SELECT COUNT(*) FROM tokens t JOIN accounts a ON a.id = t.account_id
                         WHERE a.login_key = '2026000001'),
                        (SELECT COUNT(*) FROM ctl_keys k JOIN accounts a ON a.id = k.account_id
                         WHERE a.login_key = '2026000001' AND k.revoked_at IS NULL)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
        })
        .await
        .unwrap();
    assert_eq!((tokens, keys), (0, 0));
    // an import lifts no block
    let out = h
        .admin(&["allow", "import"], false, Some("2026000001\n"))
        .await
        .unwrap();
    assert_eq!(
        out,
        "allowlist: 0 added, 1 already there (1 of them blocked, which only unblock lifts)"
    );
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.6.0.3")
        .await;
    assert_eq!(a.status, 403, "{}", a.text);
    let listed = h.admin(&["allow", "list"], false, None).await.unwrap();
    let line = listed.lines().find(|l| l.contains(&tag)).unwrap();
    assert!(
        line.contains(" blocked ") && line.contains("from import") && line.contains("  blocked 20"),
        "{line}"
    );
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(report.starts_with("allowlist 2, accounts 2,"), "{report}");
    assert!(report.contains("), 1 blocked\n"), "{report}");
    // unblock: the account logs in again, and its PC's list has its keys once more
    let out = h.admin(&["unblock", HONG.0], false, None).await.unwrap();
    assert_eq!(
        out,
        "unblocked: the account may log in again (its devices log in again)"
    );
    assert_eq!(
        h.admin(&["unblock", HONG.0], false, None).await.unwrap(),
        "not blocked (no change)"
    );
    let again = h.login_ok(HONG, None).await;
    assert_eq!(h.get("/api/users", &again).await.status, 200);
    let list = h.sync(&pc, &payload(&binding)).await.list();
    assert_eq!(list["state"], "active");
    assert_eq!(list["keys"].as_array().unwrap().len(), 1);
    // the journal has the refusal with the tag; the record names the account by its tag only
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!("was refused (acct={tag})")),
        "{logs}"
    );
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(
        record.contains(&format!("cmd=\"block\" targets={tag} ")),
        "{record}"
    );
    assert!(
        record.contains("cmd=\"block\" done=\"ok: blocked: 1 blocked"),
        "{record}"
    );
    assert!(
        record.contains(&format!("cmd=\"unblock\" targets={tag} ")),
        "{record}"
    );
    assert!(!record.contains("2026000001"), "{record}");
    // what block and unblock take
    assert!(h.admin(&["block", "two words"], false, None).await.is_err());
    assert_eq!(
        h.admin(&["unblock", "tag:0000000000000000"], false, None)
            .await
            .unwrap_err(),
        "no account or allowlist entry has that tag"
    );
}

#[tokio::test]
async fn a_block_while_a_login_runs_wins() {
    // HONG was never imported: the login would put it on the list
    let h = Arc::new(
        Harness::with(Options {
            allow: vec![KIM.0],
            ..Default::default()
        })
        .await,
    );
    let pc = TestDevice::windows("720000001");
    h.hbbs.register(&pc, Some(1));
    let held = h.hbbs.hold.lock().await;
    let queries = h.hbbs.queries.load(Ordering::SeqCst);
    let login = {
        let h = h.clone();
        let body = h.login_body(HONG.0, HONG.1, Some(&pc));
        tokio::spawn(async move { h.login(&body, "10.7.0.1").await })
    };
    // the login waits for hbbs's answer about its PC, past the allowlist's first look
    let until = Instant::now() + Duration::from_secs(5);
    while h.hbbs.queries.load(Ordering::SeqCst) == queries {
        assert!(Instant::now() < until, "the login did not ask hbbs");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let out = h.admin(&["block", HONG.0], false, None).await.unwrap();
    assert!(
        out.starts_with("blocked: 1 blocked (1 of them not on the allowlist before), 0 blocked already, 0 tokens and 0 account keys revoked;"),
        "{out}"
    );
    drop(held);
    let a = login.await.unwrap();
    assert_eq!(a.status, 403, "{}", a.text);
    assert_eq!(a.json["hjv"], "blocked");
    // its transaction wrote nothing: no account, no PC, the entry as the operator left it
    assert!(h
        .state
        .db
        .call(|c| db::account_by_key(c, "2026000001"))
        .await
        .unwrap()
        .is_none());
    assert!(h.binding_of(&pc.id).await.is_empty());
    let hong = entries(&h)
        .await
        .into_iter()
        .find(|e| e.login_key == "2026000001");
    assert_eq!(
        hong.map(|e| (e.source, e.blocked_at.is_some())),
        Some(("import".to_owned(), true))
    );
}

#[tokio::test]
async fn an_account_deleted_takes_its_entry_unless_it_is_blocked() {
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        ..Default::default()
    })
    .await;
    h.login_ok(HONG, None).await;
    h.login_ok(KIM, None).await;
    let out = h
        .admin(&["user", "delete", HONG.0], false, None)
        .await
        .unwrap();
    assert_eq!(
        out,
        "deleted the account with 0 devices, 1 account keys and 1 tokens (and its allowlist entry)"
    );
    assert_eq!(listing(&h, HONG.0).await, Listing::Absent);
    h.admin(&["block", KIM.0], false, None).await.unwrap();
    let out = h
        .admin(&["user", "delete", KIM.0], false, None)
        .await
        .unwrap();
    assert_eq!(
        out,
        "deleted the account with 0 devices, 0 account keys and 0 tokens (its allowlist entry stays blocked)"
    );
    assert_eq!(listing(&h, KIM.0).await, Listing::Blocked);
    // in open mode the next login of the deleted account registers it again; the blocked one
    // is refused, and gets no account
    h.login_ok(HONG, None).await;
    assert_eq!(listing(&h, HONG.0).await, Listing::Registered);
    let a = h.login(&h.login_body(KIM.0, KIM.1, None), "10.9.0.1").await;
    assert_eq!(a.status, 403, "{}", a.text);
    assert!(h
        .state
        .db
        .call(|c| db::account_by_key(c, "2026000002"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn list_mode_admits_the_ids_on_the_list_only() {
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        vars: vec![("HJV_API_LOGIN", "list".into())],
        ..Default::default()
    })
    .await;
    // an entry that a login made before the switch to list mode
    h.state
        .db
        .call(|c| db::admit_login(c, "y9001", "y9001", true, hjv_api::util::now()))
        .await
        .unwrap();
    // an ID off the list: a wrong password's failure, no SSO call, nothing put on the list
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.8.0.1")
        .await;
    assert_eq!(
        (a.status, &a.json),
        (401, &json!({ "error": FAILURE_TEXT }))
    );
    assert_eq!(h.sso.token_calls(), 0);
    assert_eq!(listing(&h, HONG.0).await, Listing::Absent);
    // the IDs on the list log in, imported or registered by a login before
    h.login_ok(KIM, None).await;
    h.login_ok(LEE, None).await;
    // an import lets an ID in
    h.admin(&["allow", "import"], false, Some("2026000001\n"))
        .await
        .unwrap();
    h.login_ok(HONG, None).await;
    // a block holds here too, after the SSO
    h.admin(&["block", LEE.0], false, None).await.unwrap();
    let a = h.login(&h.login_body(LEE.0, LEE.1, None), "10.8.0.2").await;
    assert_eq!(a.status, 403, "{}", a.text);
    // selfcheck, report and allow list say which mode runs
    let check = h.admin(&["selfcheck"], false, None).await.unwrap();
    assert!(
        check.contains("[ OK ] login (HJV_API_LOGIN): list, only the portal IDs on the allowlist"),
        "{check}"
    );
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("\nlogin: list (only the portal IDs on the allowlist, unless blocked; "),
        "{report}"
    );
    let listed = h.admin(&["allow", "list"], false, None).await.unwrap();
    assert!(
        listed.starts_with("allowlist: 3 IDs, 2 registered (0 by a login), 1 blocked; login list"),
        "{listed}"
    );
    assert_eq!(
        h.results().await,
        vec![
            "login:not-allowed",
            "login:ok",
            "login:ok",
            "login:ok",
            "login:blocked"
        ]
    );
}

#[tokio::test]
async fn the_tag_of_an_entry_without_an_account_names_it() {
    // the harness imports HONG, KIM and LEE; LEE never logged in, so only the entry has the tag
    let h = Harness::start().await;
    let lee = h.account_tag(LEE.0);
    let out = h
        .admin(&["block", &format!("tag:{lee}")], false, None)
        .await
        .unwrap();
    assert!(
        out.starts_with("blocked: 1 blocked (0 of them not on the allowlist before), 0 blocked already, 0 tokens and 0 account keys revoked;"),
        "{out}"
    );
    assert_eq!(listing(&h, LEE.0).await, Listing::Blocked);
    let a = h
        .login(&h.login_body(LEE.0, LEE.1, None), "10.41.0.1")
        .await;
    assert_eq!(a.status, 403, "{}", a.text);
    let out = h
        .admin(&["unblock", &format!("tag:{lee}")], false, None)
        .await
        .unwrap();
    assert!(out.starts_with("unblocked: "), "{out}");
    h.login_ok(LEE, None).await;
    // an account blocked, then deleted: its entry stays blocked, and its tag still names it
    h.login_ok(KIM, None).await;
    let kim = format!("tag:{}", h.account_tag(KIM.0));
    h.admin(&["block", &kim], false, None).await.unwrap();
    h.admin(&["user", "delete", &kim], false, None)
        .await
        .unwrap();
    assert!(!has_account(&h, KIM.0).await);
    let out = h.admin(&["block", &kim], false, None).await.unwrap();
    assert!(out.starts_with("blocked already (no change): "), "{out}");
    let out = h.admin(&["unblock", &kim], false, None).await.unwrap();
    assert!(out.starts_with("unblocked: "), "{out}");
    h.login_ok(KIM, None).await;
}

/// A second form of a person's portal ID (the fake SSO's `hong-alias`, for which it names HONG's
/// perslNo; whether the real SSO has such forms is not known, design v2 §4-1): a block holds for
/// the person whichever form was blocked and whichever is typed, also after the account of one
/// form is deleted.
#[tokio::test]
async fn a_block_holds_for_each_form_of_the_persons_id() {
    // HONG was never imported
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        ..Default::default()
    })
    .await;
    // the perslNo form blocked: the other form is refused after the SSO, and gets nothing
    h.login_ok(HONG, None).await;
    h.admin(&["block", HONG.0], false, None).await.unwrap();
    let a = h
        .login(&h.login_body(ALIAS.0, ALIAS.1, None), "10.40.0.1")
        .await;
    assert_eq!(
        (a.status, &a.json),
        (403, &json!({ "error": TEXT_BLOCKED, "hjv": "blocked" })),
        "{}",
        a.text
    );
    assert!(!has_account(&h, ALIAS.0).await);
    assert_eq!(listing(&h, ALIAS.0).await, Listing::Absent);
    assert_eq!(
        h.admin(&["unblock", HONG.0], false, None).await.unwrap(),
        "unblocked: the account may log in again (its devices log in again)"
    );
    // the person logs in with both forms (two accounts, the same perslNo in both), and the
    // account of the other form is blocked by its tag: both forms are blocked, both logged out
    let alias_token = h.login_ok(ALIAS, None).await;
    let hong_token = h.login_ok(HONG, None).await;
    let alias = format!("tag:{}", h.account_tag(ALIAS.0));
    let out = h.admin(&["block", &alias], false, None).await.unwrap();
    assert!(
        out.starts_with("blocked: 2 blocked (0 of them not on the allowlist before; 1 of them another ID the SSO names for the same person), 0 blocked already, 2 tokens and 2 account keys revoked;"),
        "{out}"
    );
    for token in [&alias_token, &hong_token] {
        assert_eq!(h.get("/api/users", token).await.status, 401);
    }
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.40.0.2")
        .await;
    assert_eq!(a.status, 403, "{}", a.text);
    // the account of the other form deleted: the person stays blocked in either form
    let out = h
        .admin(&["user", "delete", &alias], false, None)
        .await
        .unwrap();
    assert!(
        out.ends_with("(its allowlist entry stays blocked)"),
        "{out}"
    );
    for (n, (id, password)) in [HONG, ALIAS].into_iter().enumerate() {
        let a = h
            .login(&h.login_body(id, password, None), &format!("10.40.1.{n}"))
            .await;
        assert_eq!(a.status, 403, "{id}: {}", a.text);
    }
    assert!(!has_account(&h, ALIAS.0).await);
    // unblock through HONG lifts what the server still knows together (HONG's account); the
    // entry of the deleted account is named on its own
    let out = h.admin(&["unblock", HONG.0], false, None).await.unwrap();
    assert!(out.starts_with("unblocked: "), "{out}");
    h.login_ok(HONG, None).await;
    let a = h
        .login(&h.login_body(ALIAS.0, ALIAS.1, None), "10.40.2.1")
        .await;
    assert_eq!(a.status, 403, "{}", a.text);
    h.admin(&["unblock", &alias], false, None).await.unwrap();
    h.login_ok(ALIAS, None).await;
    // blocking the perslNo form takes the account of the other form with it, and unblocking
    // either lifts both
    let out = h.admin(&["block", HONG.0], false, None).await.unwrap();
    assert!(
        out.starts_with("blocked: 2 blocked (0 of them not on the allowlist before; 1 of them another ID the SSO names for the same person),"),
        "{out}"
    );
    assert_eq!(
        h.admin(&["unblock", &alias], false, None).await.unwrap(),
        "unblocked: the account may log in again (its devices log in again); 2 entries, the IDs the SSO names for the same person"
    );
    h.login_ok(HONG, None).await;
}

/// In open mode, where any portal ID reaches the SSO, an ID nobody registered fails at most
/// HJV_API_LIMIT_NEW_ID times, and all such IDs together call the SSO at most HJV_API_LIMIT_SSO_NEW
/// times, so that the login is no way to try passwords on every Yonsei account or to lock it. The
/// IDs on the list keep their limits and their SSO calls, an import puts an ID among them, and the
/// five-minute check reports the window full. List mode keeps D3's limits.
#[tokio::test]
async fn ids_nobody_registered_have_lower_limits_in_open_mode() {
    let h = Harness::with(Options {
        allow: vec![KIM.0, LEE.0],
        vars: vec![
            ("HJV_API_LIMIT_NEW_ID", "2/86400".into()),
            ("HJV_API_LIMIT_SSO_NEW", "8/3600".into()),
        ],
        ..Default::default()
    })
    .await;
    // an ID nobody registered: two wrong passwords reach the SSO, the third does not
    let stranger = "2026999999";
    for n in 0..3 {
        let a = h
            .login(
                &h.login_body(stranger, "wrong", None),
                &format!("10.42.0.{n}"),
            )
            .await;
        assert_eq!(
            (a.status, &a.json),
            (401, &json!({ "error": FAILURE_TEXT }))
        );
    }
    assert_eq!(h.sso.token_calls(), 2);
    let tag = h.account_tag(stranger);
    assert_eq!(h.counted(SCOPE_ID, tag.clone()).await, 2);
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!(
            "login: ID limit reached (acct={tag}, an ID not on the allowlist)"
        )),
        "{logs}"
    );
    // the operator's unlock clears its counters and its share of the SSO calls (2 of 8)
    assert_eq!(
        h.admin(&["unlock", &format!("tag:{tag}")], false, None)
            .await
            .unwrap(),
        "cleared 2 counted login attempts and 2 SSO calls of IDs not on the allowlist"
    );
    let a = h
        .login(&h.login_body(stranger, "wrong", None), "10.42.0.9")
        .await;
    assert_eq!(a.status, 401);
    assert_eq!(h.sso.token_calls(), 3);
    // an ID on the list keeps HJV_API_LIMIT_ID (3 in ten minutes)
    for n in 0..4 {
        let a = h
            .login(&h.login_body(KIM.0, "wrong", None), &format!("10.42.1.{n}"))
            .await;
        assert_eq!(a.status, 401);
    }
    assert_eq!(h.sso.token_calls(), 6);
    // the IDs nobody registered call the SSO 8 times an hour together: 1 above, 7 here
    for n in 0..7 {
        let a = h
            .login(
                &h.login_body(&format!("2026888{n:03}"), "wrong", None),
                &format!("10.42.2.{n}"),
            )
            .await;
        assert_eq!(a.status, 401);
    }
    assert_eq!(h.sso.token_calls(), 13);
    assert_eq!(h.state.sso_calls_new.last_full(), None);
    // then a first login fails like a wrong password, without an SSO call, and the ID's
    // failures do not count
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.42.3.1")
        .await;
    assert_eq!(
        (a.status, &a.json),
        (401, &json!({ "error": FAILURE_TEXT }))
    );
    assert_eq!(h.sso.token_calls(), 13);
    assert_eq!(h.counted(SCOPE_ID, h.account_tag(HONG.0)).await, 0);
    assert_eq!(listing(&h, HONG.0).await, Listing::Absent);
    assert_eq!(h.results().await.last().unwrap(), "login:limited-sso-new");
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains("login: SSO call limit of the portal IDs not on the allowlist reached (HJV_API_LIMIT_SSO_NEW)"),
        "{logs}"
    );
    // the people on the list do not feel it
    h.login_ok(LEE, None).await;
    // the five-minute check reports it
    let health: Health =
        serde_json::from_str(&h.admin(&["health"], false, None).await.unwrap()).unwrap();
    assert!(
        health
            .sso_new_full
            .is_some_and(|ago| (0..=2).contains(&ago)),
        "{health:?}"
    );
    let found = check::evaluate(&Probe::Answered(1), &Ok(health));
    assert_eq!(
        found.iter().map(|f| f.code).collect::<Vec<_>>(),
        ["sso_new"],
        "{found:?}"
    );
    // an ID imported ahead is not held by those limits
    h.admin(&["allow", "import"], false, Some("2026000001\n"))
        .await
        .unwrap();
    h.login_ok(HONG, None).await;
    assert_eq!(h.sso.token_calls(), 15);

    // list mode: an ID off the list fails without an SSO call under HJV_API_LIMIT_ID, as in D3
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        vars: vec![
            ("HJV_API_LOGIN", "list".into()),
            ("HJV_API_LIMIT_NEW_ID", "1/86400".into()),
            ("HJV_API_LIMIT_SSO_NEW", "1/3600".into()),
        ],
        ..Default::default()
    })
    .await;
    for n in 0..4 {
        let a = h
            .login(
                &h.login_body(stranger, "wrong", None),
                &format!("10.43.0.{n}"),
            )
            .await;
        assert_eq!(a.status, 401);
    }
    assert_eq!(
        h.results().await,
        vec![
            "login:not-allowed",
            "login:not-allowed",
            "login:not-allowed",
            "login:limited-id"
        ]
    );
    assert_eq!(h.sso.token_calls(), 0);
    h.login_ok(KIM, None).await;
    h.login_ok(KIM, None).await;
    assert_eq!(h.state.sso_calls_new.last_full(), None);
}

/// The counterexample of the second review: one address sends two wrong passwords for each of
/// five IDs nobody registered, ten requests (its limit of ten minutes). Those calls once filled
/// HJV_API_LIMIT_SSO_NEW, so that every first login failed for an hour; now the address holds a
/// quarter of the window (two of ten), and the first login of someone else goes through. An IPv6
/// address counts by its /64, and unlocking the address frees its share with its counters.
#[tokio::test]
async fn one_address_cannot_shut_the_first_logins() {
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        vars: vec![
            ("HJV_API_LIMIT_NEW_ID", "2/86400".into()),
            ("HJV_API_LIMIT_SSO_NEW", "10/3600".into()),
        ],
        ..Default::default()
    })
    .await;
    let attacker = "192.0.2.7";
    for _ in 0..2 {
        for n in 0..5 {
            let a = h
                .login(
                    &h.login_body(&format!("2026888{n:03}"), "wrong", None),
                    attacker,
                )
                .await;
            assert_eq!(
                (a.status, &a.json),
                (401, &json!({ "error": FAILURE_TEXT }))
            );
        }
    }
    // two of them reached the SSO, the other eight failed alike without a call
    assert_eq!(h.sso.token_calls(), 2);
    let results = h.results().await;
    let count = |result: &str| results.iter().filter(|r| *r == result).count();
    assert_eq!(
        (count("login:credentials"), count("login:limited-sso-new")),
        (2, 8)
    );
    let ip = h.state.tags.ip(attacker);
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!(
            "login: an address holds its share of the SSO calls of the portal IDs not on the allowlist (ip={ip}); hjv-api admin unlock tag:{ip} lifts it"
        )),
        "{logs}"
    );
    // the window is not full: a first login from another address goes through
    assert_eq!(h.state.sso_calls_new.last_full(), None);
    h.login_ok_from(HONG, None, "192.0.2.20").await;
    assert_eq!(listing(&h, HONG.0).await, Listing::Registered);
    assert_eq!(h.sso.token_calls(), 3);
    // an IPv6 address counts by its /64, as for the address limit
    for (n, ip6) in ["2001:db8:5::1", "2001:db8:5::2", "2001:db8:5:0:1::3"]
        .into_iter()
        .enumerate()
    {
        let a = h
            .login(
                &h.login_body(&format!("2026888{:03}", 10 + n), "wrong", None),
                ip6,
            )
            .await;
        assert_eq!(a.status, 401);
    }
    assert_eq!(h.sso.token_calls(), 5);
    assert_eq!(h.results().await.last().unwrap(), "login:limited-sso-new");
    // unlocking the address clears its counters and frees its share
    assert_eq!(
        h.admin(&["unlock", &format!("ip:{attacker}")], false, None)
            .await
            .unwrap(),
        "cleared 10 counted login attempts and 2 SSO calls of IDs not on the allowlist"
    );
    let a = h
        .login(&h.login_body("2026888020", "wrong", None), attacker)
        .await;
    assert_eq!(a.status, 401);
    assert_eq!(h.sso.token_calls(), 6);
}

/// One ID and one address each hold a quarter of HJV_API_LIMIT_SSO_NEW (here one call of four),
/// and the journal names the one that holds it by its tag, for unlock. A first login that goes
/// through gives its call back, so that people logging in for the first time one after another
/// behind one address (a campus NAT) are not held; one the SSO accepted that went no further (a
/// bad account key) keeps it.
#[tokio::test]
async fn an_id_and_an_address_hold_a_quarter_each_and_a_first_login_gives_its_call_back() {
    // only an ID nobody uses is on the list: HONG, KIM and LEE log in for the first time
    let h = Harness::with(Options {
        allow: vec!["2026999998"],
        vars: vec![
            ("HJV_API_LIMIT_NEW_ID", "10/86400".into()),
            ("HJV_API_LIMIT_SSO_NEW", "4/3600".into()),
        ],
        ..Default::default()
    })
    .await;
    let failure = json!({ "error": FAILURE_TEXT });
    // the ID's share: its second call fails without an SSO call, from another address too, and
    // only the call made counts as its failure
    let stranger = "2026999999";
    let tag = h.account_tag(stranger);
    for ip in ["10.47.0.1", "10.47.0.2"] {
        let a = h.login(&h.login_body(stranger, "wrong", None), ip).await;
        assert_eq!((a.status, &a.json), (401, &failure));
    }
    assert_eq!(h.sso.token_calls(), 1);
    assert_eq!(
        h.results().await,
        vec!["login:credentials", "login:limited-sso-new"]
    );
    assert_eq!(h.counted(SCOPE_ID, tag.clone()).await, 1);
    // the address's share: another ID from the first address fails the same way
    let a = h
        .login(&h.login_body("2026888000", "wrong", None), "10.47.0.1")
        .await;
    assert_eq!((a.status, &a.json), (401, &failure));
    assert_eq!(h.sso.token_calls(), 1);
    assert_eq!(h.results().await.last().unwrap(), "login:limited-sso-new");
    let ip = h.state.tags.ip("10.47.0.1");
    let logs = hjv_api::log::captured().join("\n");
    for line in [
        format!("login: a portal ID not on the allowlist holds its share of their SSO calls (acct={tag}); hjv-api admin unlock tag:{tag} lifts it"),
        format!("login: an address holds its share of the SSO calls of the portal IDs not on the allowlist (ip={ip}); hjv-api admin unlock tag:{ip} lifts it"),
    ] {
        assert!(logs.contains(&line), "{line}\n{logs}");
    }
    // a first login the SSO accepted that went no further keeps its call: hong-alias with an
    // account key whose proof is wrong (400), and then KIM behind the same address is held
    let body = h.login_body_with(
        ALIAS.0,
        ALIAS.1,
        None,
        now(),
        &b64(&random_bytes::<16>()),
        KeyChoice::WrongProof,
    );
    let a = h.login(&body, "10.47.1.1").await;
    assert_eq!(a.status, 400, "{}", a.text);
    assert_eq!(h.sso.token_calls(), 2);
    let a = h
        .login(&h.login_body(KIM.0, KIM.1, None), "10.47.1.1")
        .await;
    assert_eq!((a.status, &a.json), (401, &failure));
    assert_eq!(h.sso.token_calls(), 2);
    // first logins that go through give their calls back: three people one after another
    // behind one address, of whose share is one call
    for account in [HONG, KIM, LEE] {
        h.login_ok_from(account, None, "10.47.2.1").await;
        assert_eq!(listing(&h, account.0).await, Listing::Registered);
    }
    assert_eq!(h.sso.token_calls(), 5);
    // so the window holds the stranger's and hong-alias's calls: two more fit, then it is full
    for n in 1..3 {
        let a = h
            .login(
                &h.login_body(&format!("2026888{n:03}"), "wrong", None),
                &format!("10.47.3.{n}"),
            )
            .await;
        assert_eq!(a.status, 401);
    }
    assert_eq!(h.sso.token_calls(), 7);
    assert_eq!(h.state.sso_calls_new.last_full(), None);
    let a = h
        .login(&h.login_body("2026888009", "wrong", None), "10.47.4.1")
        .await;
    assert_eq!((a.status, &a.json), (401, &failure));
    assert_eq!(h.sso.token_calls(), 7);
    assert!(h.state.sso_calls_new.last_full().is_some());
    // unlocking the ID's tag clears its counters and frees what it holds
    assert_eq!(
        h.admin(&["unlock", &format!("tag:{tag}")], false, None)
            .await
            .unwrap(),
        "cleared 1 counted login attempts and 1 SSO calls of IDs not on the allowlist"
    );
    let a = h
        .login(&h.login_body("2026888009", "wrong", None), "10.47.4.1")
        .await;
    assert_eq!(a.status, 401);
    assert_eq!(h.sso.token_calls(), 8);
}

/// A second factor for the code step (tfa.rs; off by D11 and U1): "123456" is right for everyone.
struct FixedCode;

impl SecondFactor for FixedCode {
    fn required(&self) -> bool {
        true
    }

    fn verify(&self, _login_key: &str, code: &str) -> bool {
        code == "123456"
    }
}

/// Code dialogs for `id` with a wrong password, one from each address; their secrets.
/// While the SSO calls of all accounts (HJV_API_LIMIT_SSO) are used up, a first login makes no
/// call, and the ID and its address take back the call they took of HJV_API_LIMIT_SSO_NEW: a
/// full window does not use up an address's share of the first logins (reverify:WP-OPEN:2).
#[tokio::test]
async fn a_first_login_that_the_full_window_refuses_keeps_no_share() {
    let h = Harness::with(Options {
        allow: vec![HONG.0],
        vars: vec![
            ("HJV_API_LIMIT_SSO", "1/3600".into()),
            ("HJV_API_LIMIT_NEW_ID", "10/86400".into()),
            ("HJV_API_LIMIT_SSO_NEW", "8/3600".into()),
        ],
        ..Default::default()
    })
    .await;
    // the registered HONG takes the one call of the window
    h.login_ok_from(HONG, None, "10.48.0.1").await;
    assert_eq!(h.sso.token_calls(), 1);
    // three IDs nobody registered from one address, whose share is two calls of the eight: each
    // is refused for the full window, none for the address's share
    for n in 1..=3 {
        let a = h
            .login(
                &h.login_body(&format!("2026777{n:03}"), "wrong", None),
                "10.48.1.1",
            )
            .await;
        assert_eq!(a.status, 401);
    }
    assert_eq!(h.sso.token_calls(), 1);
    let results = h.results().await;
    assert_eq!(
        results[results.len() - 3..],
        [
            "login:limited-sso",
            "login:limited-sso",
            "login:limited-sso"
        ]
    );
    // and the IDs' window holds nothing of them
    for n in 1..=3 {
        let id = format!("2026777{n:03}");
        let tag = h.account_tag(&id);
        assert!(!h
            .state
            .sso_calls_new
            .give_back(&[tag.as_str(), &h.state.tags.ip("10.48.1.1")]));
    }
}

async fn code_dialogs(h: &Harness, id: &str, ips: &[&str]) -> Vec<String> {
    let mut secrets = Vec::new();
    for ip in ips {
        let a = h.login(&h.login_body(id, "wrong", None), ip).await;
        assert_eq!(a.json["type"], "email_check", "{}", a.text);
        secrets.push(a.json["secret"].as_str().unwrap().to_owned());
    }
    secrets
}

/// The code request of a dialog, as the app sends it.
fn code_request(id: &str, secret: &str) -> serde_json::Value {
    json!({
        "verificationCode": "123456", "tfaCode": "123456", "secret": secret, "username": id,
        "id": "", "uuid": "dXVpZA==", "autoLogin": true, "type": "email_code",
        "deviceInfo": {"os": "ios", "type": "client", "name": "iPhone15,2"},
    })
}

/// With a second factor the code step of an ID nobody registered counts under
/// HJV_API_LIMIT_NEW_ID as its first step does, and an ID on the list keeps HJV_API_LIMIT_ID.
#[tokio::test]
async fn the_code_step_keeps_the_lower_limit_of_an_id_nobody_registered() {
    let h = Harness::with(Options {
        allow: vec![KIM.0],
        second_factor: Some(Arc::new(FixedCode)),
        vars: vec![("HJV_API_LIMIT_NEW_ID", "1/86400".into())],
        ..Default::default()
    })
    .await;
    let failure = json!({ "error": FAILURE_TEXT });
    // two dialogs: a dialog does not count against the ID
    let stranger = "2026999999";
    let secrets = code_dialogs(&h, stranger, &["10.48.0.1", "10.48.0.2"]).await;
    assert_eq!(h.sso.token_calls(), 0);
    // the first code step reaches the SSO with the wrong password: the one failure of its day
    let a = h
        .login(&code_request(stranger, &secrets[0]), "10.48.0.1")
        .await;
    assert_eq!((a.status, &a.json), (401, &failure));
    assert_eq!(h.sso.token_calls(), 1);
    // the second stops at that limit, without an SSO call
    let a = h
        .login(&code_request(stranger, &secrets[1]), "10.48.0.2")
        .await;
    assert_eq!((a.status, &a.json), (401, &failure));
    assert_eq!(h.sso.token_calls(), 1);
    assert_eq!(h.results().await.last().unwrap(), "login:limited-id");
    let tag = h.account_tag(stranger);
    let logs = hjv_api::log::captured().join("\n");
    assert!(
        logs.contains(&format!(
            "login: ID limit reached (acct={tag}, an ID not on the allowlist)"
        )),
        "{logs}"
    );
    // an ID on the list keeps HJV_API_LIMIT_ID (3 in ten minutes) at its code step
    let ips = ["10.48.1.1", "10.48.1.2"];
    let secrets = code_dialogs(&h, KIM.0, &ips).await;
    for (secret, ip) in secrets.iter().zip(ips) {
        let a = h.login(&code_request(KIM.0, secret), ip).await;
        assert_eq!((a.status, &a.json), (401, &failure));
    }
    assert_eq!(h.sso.token_calls(), 3);
}
