// SPDX-License-Identifier: AGPL-3.0-only

//! The open login of 2026-10-14 (HJV_API_LOGIN, login.rs steps 7 and 12, schema 5): a portal ID
//! that the SSO accepts and the allowlist does not have is put on it at that login; an entry the
//! operator blocks keeps its account out (its tokens and account keys end at once, its PCs get
//! lists without keys, its next login is refused after the SSO) until it is unblocked, also when
//! the block comes while a login runs; `allow remove` blocks; an account deleted takes its entry
//! with it unless the entry is blocked; list mode admits the IDs on the list only, as D3 did.

mod common;

use common::*;
use hjv_api::{
    db::{self, Listing},
    limits::{SCOPE_ID, SCOPE_OK_ID},
    login::{FAILURE_TEXT, TEXT_BLOCKED},
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
        .call(|c| db::admit_login(c, "y9001", true, hjv_api::util::now()))
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
