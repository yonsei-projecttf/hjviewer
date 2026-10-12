// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api admin` through the admin socket (design v2 §9-4): masked output, tags, the operator
//! record.

mod common;

use common::*;
use hjv_api::{limits::SCOPE_ID, oplog};

#[tokio::test]
async fn allowlist_from_a_file_prints_counts_only() {
    let h = Harness::with(Options {
        allow: vec!["seed0001"],
        ..Default::default()
    })
    .await;
    let file = "# lab\n2026000001\nY9001  # professor\ny9001\n\n";
    let out = h
        .admin(&["allow", "import"], false, Some(file))
        .await
        .unwrap();
    assert_eq!(out, "allowlist: 2 added, 0 already there");
    let out = h
        .admin(&["allow", "import"], false, Some(file))
        .await
        .unwrap();
    assert_eq!(out, "allowlist: 0 added, 2 already there");
    // one line that is not an ID changes nothing, and only its number is shown
    let err = h
        .admin(
            &["allow", "import"],
            false,
            Some("2026000002\n2026000003 홍길동\n"),
        )
        .await
        .unwrap_err();
    assert!(
        err.starts_with("nothing changed: line 2 of the file"),
        "{err}"
    );
    assert!(!err.contains("2026000003"), "{err}");
    let masked = h.admin(&["allow", "list"], false, None).await.unwrap();
    assert!(masked.starts_with("allowlist: 3 IDs"), "{masked}");
    assert!(
        masked.contains("2026******") && masked.contains("y9***"),
        "{masked}"
    );
    assert!(
        !masked.contains("2026000001") && !masked.contains("y9001"),
        "{masked}"
    );
    let shown = h.admin(&["allow", "list"], true, None).await.unwrap();
    assert!(
        shown.contains("2026000001") && shown.contains("y9001"),
        "{shown}"
    );
    assert!(h.admin(&["allow", "import"], false, None).await.is_err());
    // the IDs typed just now log in
    h.login_ok(HONG, None).await;
}

#[tokio::test]
async fn removing_ids_saved_with_a_byte_order_mark() {
    let h = Harness::start().await;
    let hong = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    // as Windows Notepad saves a file: a byte-order mark and CR LF line ends
    let file = "\u{feff}2026000001\r\n2026000002\r\n2026999999\r\n";
    let out = h
        .admin(&["allow", "remove"], false, Some(file))
        .await
        .unwrap();
    assert_eq!(out, "allowlist: 2 removed, 1 not on it, 2 tokens revoked");
    for token in [&hong, &kim] {
        assert_eq!(h.get("/api/users", token).await.status, 401);
    }
    let a = h
        .login(&h.login_body(HONG.0, HONG.1, None), "10.2.1.1")
        .await;
    assert_eq!(a.status, 401);
    assert_eq!(h.login_ok(LEE, None).await.len(), 47);
}

#[tokio::test]
async fn listings_are_masked_and_carry_tags() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("700000001");
    h.hbbs.register(&pc, Some(1));
    h.login_ok(HONG, Some(&pc)).await;
    let users = h.admin(&["users"], false, None).await.unwrap();
    let tag = h.account_tag(HONG.0);
    assert!(users.starts_with("accounts: 1"), "{users}");
    assert!(
        users.contains(&tag) && users.contains("2026******"),
        "{users}"
    );
    assert!(
        !users.contains("2026000001") && !users.contains("홍길동"),
        "{users}"
    );
    let revealed = h.admin(&["users"], true, None).await.unwrap();
    assert!(
        revealed.contains("2026000001") && revealed.contains("홍길동"),
        "{revealed}"
    );
    let devices = h.admin(&["devices"], false, None).await.unwrap();
    let device_tag = h.state.tags.device(&pc.id);
    assert!(
        devices.contains(&device_tag) && devices.contains("7000*****") && devices.contains(&tag),
        "{devices}"
    );
    assert!(
        !devices.contains("700000001") && !devices.contains("PC-700000001"),
        "{devices}"
    );
    assert!(h
        .admin(&["devices"], true, None)
        .await
        .unwrap()
        .contains("PC-700000001"));
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("accounts 1, devices 1, tokens 1"),
        "{report}"
    );
    assert!(report.contains("login ok"), "{report}");
    assert!(report.contains("seal key IDs: "), "{report}");
    assert!(!report.contains("2026000001"));
}

#[tokio::test]
async fn commands_by_id_or_tag() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("800000001");
    h.hbbs.register(&pc, Some(1));
    let token = h.login_ok(HONG, Some(&pc)).await;
    // unlock by tag, as the journal warning shows it
    for i in 0..3 {
        h.login(&h.login_body(KIM.0, "wrong", None), &format!("10.2.0.{i}"))
            .await;
    }
    let tag = h.account_tag(KIM.0);
    assert_eq!(h.counted(SCOPE_ID, tag.clone()).await, 3);
    let out = h
        .admin(&["unlock", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    assert!(out.contains("cleared 3"), "{out}");
    assert_eq!(h.counted(SCOPE_ID, tag).await, 0);
    // revoke every token of an account
    let out = h.admin(&["revoke", HONG.0], false, None).await.unwrap();
    assert_eq!(out, "revoked 1 tokens");
    assert_eq!(h.get("/api/users", &token).await.status, 401);
    // take the PC out by its tag
    let device_tag = h.state.tags.device(&pc.id);
    let out = h
        .admin(
            &["device", "remove", &format!("tag:{device_tag}")],
            false,
            None,
        )
        .await
        .unwrap();
    assert_eq!(out, "removed the device from its account");
    let out = h
        .admin(&["device", "remove", &pc.id], false, None)
        .await
        .unwrap();
    assert_eq!(out, "no such device in any account");
    // delete the account; the allowlist entry stays
    h.login_ok(HONG, Some(&pc)).await;
    let out = h
        .admin(&["user", "delete", "2026000001"], false, None)
        .await
        .unwrap();
    assert!(
        out.starts_with("deleted the account with 1 devices and 1 tokens"),
        "{out}"
    );
    assert!(h
        .admin(&["user", "delete", "2026000001"], false, None)
        .await
        .is_err());
    assert!(h.admin(&["revoke", "tag:zz"], false, None).await.is_err());
    assert!(h
        .admin(&["nonsense"], false, None)
        .await
        .unwrap_err()
        .contains("unknown command"));
    h.login_ok(HONG, None).await;
}

#[tokio::test]
async fn every_command_is_recorded_with_tags_only() {
    let h = Harness::start().await;
    h.admin(&["report"], false, None).await.unwrap();
    h.admin(&["unlock", "2026000002"], false, None)
        .await
        .unwrap();
    h.admin(&["users"], true, None).await.unwrap();
    h.admin(&["allow", "import"], false, Some("2026123123\n"))
        .await
        .unwrap();
    let _ = h.admin(&["2026000003"], false, None).await;
    let _ = h
        .admin(&["allow", "remove"], false, Some("2026123123\nnot an id\n"))
        .await;
    let text = std::fs::read_to_string(&h.oplog).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // two lines a command: before it runs, and what it did
    assert_eq!(lines.len(), 12, "{text}");
    assert!(
        lines[0].contains("uid=")
            && lines[0].contains("sudo_user=tester cmd=\"report\" targets=- reveal=0"),
        "{text}"
    );
    assert!(lines[1].contains("cmd=\"report\" done=\"ok\""), "{text}");
    assert!(
        lines[2].contains(&format!("cmd=\"unlock\" targets={}", h.account_tag(KIM.0))),
        "{text}"
    );
    assert!(
        lines[3].contains("cmd=\"unlock\" done=\"ok: cleared 0 counted login attempts\""),
        "{text}"
    );
    assert!(
        lines[4].contains("cmd=\"users\" targets=- reveal=1"),
        "{text}"
    );
    // an allowlist change names the IDs of its file by their tags, and its counts
    assert!(
        lines[6].contains(&format!(
            "cmd=\"allow import\" targets={}",
            h.account_tag("2026123123")
        )),
        "{text}"
    );
    assert!(
        lines[7].contains("done=\"ok: allowlist: 1 added, 0 already there\""),
        "{text}"
    );
    assert!(lines[8].contains("cmd=\"unknown\""), "{text}");
    assert!(lines[9].contains("done=\"failed\""), "{text}");
    assert!(
        lines[10].contains("cmd=\"allow remove\" targets=-")
            && lines[11].contains("done=\"failed\""),
        "{text}"
    );
    for id in ["2026000002", "2026123123", "2026000003"] {
        assert!(!text.contains(id), "{id} in the record:\n{text}");
    }
    assert_eq!(oplog::verify(&h.oplog).unwrap(), 12);
    // a long file is named by its count and the hash of its tags
    let file: String = (0..40).map(|n| format!("2026{n:06}\n")).collect();
    h.admin(&["allow", "import"], false, Some(&file))
        .await
        .unwrap();
    let text = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(text.contains("targets=40-ids,set:"), "{text}");
}

#[tokio::test]
async fn selfcheck_from_the_service() {
    let h = Harness::start().await;
    let out = h.admin(&["selfcheck"], false, None).await.unwrap();
    assert!(
        out.contains("[ OK ] token URL: 1 addresses, TCP ok"),
        "{out}"
    );
    assert!(
        out.contains("[ OK ] user-info URL: same host as above"),
        "{out}"
    );
    assert!(out.contains("[ OK ] hbbs console: answers hjv-pk"), "{out}");
    h.hbbs
        .unpatched
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let err = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(
        err.contains("[FAIL] hbbs console: no answer to hjv-pk"),
        "{err}"
    );
}
