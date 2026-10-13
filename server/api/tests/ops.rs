// SPDX-License-Identifier: AGPL-3.0-only

//! Monitoring and backups (design v3 §5-9, WP-O): `hjv-api admin backup` and the rows of its
//! file, the places it refuses, the backups it deletes, a backup while requests go on, the restore
//! procedure on a restored database, the five-minute check (a fake report with a stopped socket
//! and no syncs, then the service's own counts), the alert with a stand-in for Telegram, and the
//! units under `systemd-analyze verify`.

mod common;

use common::*;
use hjv_api::{
    backup, check, db,
    server::{self, Listener},
    sso::Failure,
    util::{b64, date, now},
};
use http_body_util::{BodyExt, Full};
use hyper::{body::Bytes, server::conn::http1, service::service_fn, Response};
use hyper_util::rt::TokioIo;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use std::{
    collections::BTreeMap,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{
        atomic::{AtomicU16, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, UnixListener},
    sync::watch,
};

const BIN: &str = env!("CARGO_BIN_EXE_hjv-api");

/// The program with these arguments (and environment), off the runtime that serves hjv-api.
async fn run(args: Vec<String>, env: Vec<(&'static str, String)>) -> Output {
    tokio::task::spawn_blocking(move || {
        let mut command = Command::new(BIN);
        command.args(&args).env_remove("JOURNAL_STREAM");
        for (k, v) in env {
            command.env(k, v);
        }
        command.output().unwrap()
    })
    .await
    .unwrap()
}

fn args(v: &[&dyn AsRef<std::ffi::OsStr>]) -> Vec<String> {
    v.iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect()
}

fn text(out: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn private_dir(name: &str) -> PathBuf {
    let dir = temp_dir(name);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    dir
}

/// The rows of each table of a database file, opened read-only.
fn rows_of(path: &Path) -> BTreeMap<String, i64> {
    let c = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    rows(&c)
}

fn rows(c: &Connection) -> BTreeMap<String, i64> {
    let mut stmt = c
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
        )
        .unwrap();
    let tables: Vec<String> = stmt
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    tables
        .into_iter()
        .map(|t| {
            let n = c
                .query_row(&format!("SELECT COUNT(*) FROM \"{t}\""), [], |r| r.get(0))
                .unwrap();
            (t, n)
        })
        .collect()
}

/// The rows the service has now.
async fn live_rows(h: &Harness) -> BTreeMap<String, i64> {
    h.state.db.call(|c| Ok(rows(c))).await.unwrap()
}

/// A PC of `account`'s, bound by a login on it (hbbs sees it on), with password-less connections
/// on for the account: its binding and the login's token.
async fn bind_pc(h: &Harness, account: (&str, &str), pc: &TestDevice) -> (String, String) {
    h.hbbs.register(pc, Some(5));
    let token = h.login_ok(account, Some(pc)).await;
    h.admin(&["passwordless", "on", "--owner-of", &pc.id], false, None)
        .await
        .unwrap();
    (h.binding_of(&pc.id).await, token)
}

/// Data in every table that a day of use fills: accounts, keys, tokens, PCs bound and syncing
/// with key use and refusals, a released binding, records and limit counters.
struct Used {
    pc: TestDevice,
    pc_binding: String,
    kim_pc: TestDevice,
    kim_binding: String,
    phone: String,
}

async fn a_day_of_use(h: &Harness) -> Used {
    let pc = TestDevice::windows("611000001");
    let (pc_binding, _) = bind_pc(h, HONG, &pc).await;
    let kim_pc = TestDevice::windows("611000002");
    let (kim_binding, _) = bind_pc(h, KIM, &kim_pc).await;
    let phone = h.login_ok(HONG, None).await;
    h.login_ok(LEE, None).await;
    // a PC that leaves its account again (a tombstone)
    let gone = TestDevice::windows("611000003");
    let (_, gone_token) = bind_pc(h, HONG, &gone).await;
    let removed = h
        .delete(&format!("/api/hjv/devices/{}", gone.id), &gone_token)
        .await;
    assert_eq!(removed.status, 200, "{}", removed.text);
    // a failed login: a record and a counted attempt
    let failed = h
        .login(&h.login_body(HONG.0, "wrong-password", None), "192.0.2.77")
        .await;
    assert_eq!(failed.status, 401);
    // the PCs' syncs, one with the phone key's use and a refused proof
    let mut p = payload(&pc_binding);
    p["used"] = json!([{"pk": b64(&hjv_api::devsig::public_key(&h.key_of(&phone).unwrap())),
                        "day": date(now()), "last": now(), "n": 2}]);
    p["refused"]["unknown_key"] = json!(1);
    let synced = h.sync(&pc, &p).await;
    assert_eq!(synced.list()["state"], "active");
    h.sync(&kim_pc, &payload(&kim_binding)).await.list();
    Used {
        pc,
        pc_binding,
        kim_pc,
        kim_binding,
        phone,
    }
}

#[tokio::test]
async fn a_backup_has_the_rows_of_the_database() {
    let h = Harness::start().await;
    a_day_of_use(&h).await;
    let dir = private_dir("backup");
    let file = dir.join("daily.sqlite3");
    let out = run(
        args(&[&"admin", &"--socket", &h.admin_socket, &"backup", &file]),
        vec![("SUDO_USER", "operator".into())],
    )
    .await;
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");

    // every table, row for row as many as the service has
    let live = live_rows(&h).await;
    let copy = rows_of(&file);
    assert_eq!(copy, live);
    for table in [
        "accounts",
        "allowlist",
        "attempts",
        "binding_tombstones",
        "ctl_keys",
        "devices",
        "events",
        "key_use",
        "meta",
        "pc_nonces",
        "pc_refusals",
        "seal_nonces",
        "tokens",
    ] {
        assert!(copy[table] > 0, "{table} is empty: {copy:?}");
    }
    let c = Connection::open_with_flags(&file, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    let pragma = |p: &str| -> i64 {
        c.query_row(&format!("PRAGMA {p}"), [], |r| r.get(0))
            .unwrap()
    };
    assert_eq!(pragma("user_version"), db::MIGRATIONS.len() as i64);
    assert_eq!(pragma("application_id"), i64::from(backup::APPLICATION_ID));
    let mode: String = c
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "delete");
    drop(c);
    // root's file in root's directory (here the tester's), and nothing else left there or beside
    // the service's database
    assert_eq!(std::fs::metadata(&file).unwrap().mode() & 0o777, 0o600);
    let names = |d: &Path| -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(d)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    };
    assert_eq!(names(&dir), ["daily.sqlite3"]);
    assert!(
        !names(&h.dir).iter().any(|n| n.contains("backup")),
        "{:?}",
        names(&h.dir)
    );
    // a copy left by a backup that broke off with the service goes with the hourly upkeep, an
    // hour on
    let left = h.dir.join(".hjv-api-backup-1-0123456789ab.sqlite3");
    let fresh = h.dir.join(".hjv-api-backup-1-ba9876543210.sqlite3");
    std::fs::copy(&file, &left).unwrap();
    std::fs::copy(&file, &fresh).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&left)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(3700))
        .unwrap();
    hjv_api::jobs::purge_now(&h.state).await.unwrap();
    assert!(!left.exists() && fresh.exists() && h.db_path().exists());
    // what it says: counts, never an ID, a name or a host
    assert!(
        stdout.starts_with(&format!("{}\n", file.display())),
        "{stdout}"
    );
    assert!(stdout.contains("checked (quick_check, schema and rows as the service counted)"));
    assert!(
        stdout.contains(&format!("accounts {}", live["accounts"])),
        "{stdout}"
    );
    for secret in [HONG.0, KIM.0, "홍길동", "PC-611000001", "611000001"] {
        assert!(
            !stdout.contains(secret) && !stderr.contains(secret),
            "{secret}"
        );
    }
    // the operator record: who took it, the file's name, what was copied
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    let lines: Vec<&str> = record
        .lines()
        .filter(|l| l.contains("cmd=\"backup\""))
        .collect();
    assert_eq!(lines.len(), 2, "{record}");
    assert!(lines[0].contains("sudo_user=operator") && lines[0].contains("targets=daily.sqlite3"));
    assert!(
        lines[1].contains(&format!(
            "done=\"ok: backup: schema {}, ",
            db::MIGRATIONS.len()
        )),
        "{}",
        lines[1]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backups_go_to_closed_places_only_and_age_out() {
    let h = Harness::start().await;
    let backup = |a: Vec<String>| {
        let mut all = args(&[&"admin", &"--socket", &h.admin_socket, &"backup"]);
        all.extend(a);
        run(all, vec![])
    };
    let one = |p: &Path| vec![p.to_string_lossy().into_owned()];

    // a directory others may enter: refused, and nothing written
    let open = temp_dir("open");
    std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = backup(one(&open.join("b.sqlite3"))).await;
    assert!(!out.status.success());
    assert!(
        text(&out).1.contains("is open to others (mode 755)"),
        "{}",
        text(&out).1
    );
    assert_eq!(std::fs::read_dir(&open).unwrap().count(), 0);
    // a name that is not a backup's, a file that is there already
    let dir = private_dir("closed");
    let out = backup(one(&dir.join("b.db"))).await;
    assert!(
        text(&out).1.contains("ends in .sqlite3"),
        "{}",
        text(&out).1
    );
    std::fs::write(dir.join("b.sqlite3"), "keep me").unwrap();
    let out = backup(one(&dir.join("b.sqlite3"))).await;
    assert!(text(&out).1.contains("exists already"), "{}", text(&out).1);
    assert_eq!(
        std::fs::read_to_string(dir.join("b.sqlite3")).unwrap(),
        "keep me"
    );
    // a missing directory whose parent is there: made, closed
    let made = dir.join("backup");
    let out = backup(vec![format!("{}/", made.display())]).await;
    assert!(out.status.success(), "{}", text(&out).1);
    assert_eq!(std::fs::metadata(&made).unwrap().mode() & 0o777, 0o700);
    // a directory gets a name of the time; two in one second do not collide
    let named: Vec<String> = std::fs::read_dir(&made)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(named.len(), 1);
    assert!(
        named[0].starts_with("hjv-api-") && named[0].ends_with("Z.sqlite3"),
        "{named:?}"
    );
    let out = backup(one(&made)).await;
    assert!(out.status.success(), "{}", text(&out).1);
    assert_eq!(std::fs::read_dir(&made).unwrap().count(), 2);

    // --keep-days: old backups go, other files stay, whatever their age
    let old = made.join("before-1.6.0.sqlite3");
    std::fs::copy(made.join(&named[0]), &old).unwrap();
    let foreign = made.join("other.sqlite3");
    Connection::open(&foreign)
        .unwrap()
        .execute_batch("CREATE TABLE t (x)")
        .unwrap();
    let notes = made.join("notes.txt");
    std::fs::write(&notes, "x").unwrap();
    let past = SystemTime::now() - Duration::from_secs(20 * 86_400);
    for p in [&old, &foreign, &notes] {
        std::fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(past)
            .unwrap();
    }
    let out = backup(vec![
        "--keep-days".into(),
        "14".into(),
        made.to_string_lossy().into_owned(),
    ])
    .await;
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stderr}");
    assert!(
        stdout.contains("removed 1 backups older than 14 days"),
        "{stdout}"
    );
    assert!(!old.exists() && foreign.exists() && notes.exists());
    assert_eq!(
        std::fs::read_dir(&made)
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("hjv-api-"))
            .count(),
        3
    );
    let _ = std::fs::remove_dir_all(&open);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_backup_while_requests_go_on() {
    let h = Arc::new(Harness::start().await);
    a_day_of_use(&h).await;
    let dir = private_dir("busy");
    // logins and syncs while three backups are taken: each copy is whole and checks out
    let busy = {
        let h = h.clone();
        tokio::spawn(async move {
            // within the successful logins an account may have in an hour
            for i in 0..8 {
                h.login_ok(if i % 2 == 0 { LEE } else { KIM }, None).await;
            }
        })
    };
    for i in 0..3 {
        let file = dir.join(format!("b{i}.sqlite3"));
        let out = run(
            args(&[&"admin", &"--socket", &h.admin_socket, &"backup", &file]),
            vec![],
        )
        .await;
        assert!(out.status.success(), "{}", text(&out).1);
        assert!(rows_of(&file)["tokens"] >= 4);
    }
    busy.await.unwrap();
    let last = dir.join("last.sqlite3");
    let out = run(
        args(&[&"admin", &"--socket", &h.admin_socket, &"backup", &last]),
        vec![],
    )
    .await;
    assert!(out.status.success());
    assert_eq!(rows_of(&last), live_rows(&h).await);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The restore of design v3 §5-9 (README.md, Backup and restore) on the database level:
/// `passwordless off`, the backup in place of the database, `user revoke-all --all`, the
/// allowlist and the switches looked at, `passwordless on`; then the apps log in again and the
/// PCs, still bound, get their lists.
#[tokio::test]
async fn the_restore_procedure() {
    let h = Harness::start().await;
    let used = a_day_of_use(&h).await;
    h.admin(&["passwordless", "on"], false, None).await.unwrap();
    let dir = private_dir("restore");
    let file = dir.join("hjv-api-before.sqlite3");
    let out = run(
        args(&[&"admin", &"--socket", &h.admin_socket, &"backup", &file]),
        vec![],
    )
    .await;
    assert!(out.status.success(), "{}", text(&out).1);
    let backed_up = rows_of(&file);

    // after the backup, what the restore takes back: a new login, KIM's PC leaving its account
    // (it gets its signed revoked and clears its mark), the switch off
    let later = h.login_ok(LEE, None).await;
    let kim_token = h.login_ok(KIM, None).await;
    assert_eq!(
        h.delete(&format!("/api/hjv/devices/{}", used.kim_pc.id), &kim_token)
            .await
            .status,
        200
    );
    let revoked = h.sync(&used.kim_pc, &payload(&used.kim_binding)).await;
    assert_eq!(revoked.list()["state"], "revoked");

    // the restore: hjv-api started again on the backup
    let r = Harness::with(Options {
        db_file: Some(file.clone()),
        ..Default::default()
    })
    .await;
    r.adopt_keys(&h);
    assert_eq!(live_rows(&r).await, backed_up);
    assert_eq!(
        r.admin(&["passwordless", "off"], false, None)
            .await
            .unwrap(),
        "password-less connections: off for every account"
    );
    let out = r
        .admin(&["user", "revoke-all", "--all"], false, None)
        .await
        .unwrap();
    let keys: i64 = Connection::open_with_flags(&file, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM ctl_keys WHERE revoked_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(keys >= 4);
    assert_eq!(
        out,
        format!(
            "revoked {keys} account keys and {} tokens of every account",
            backed_up["tokens"]
        )
    );
    let status = r
        .admin(&["passwordless", "status"], false, None)
        .await
        .unwrap();
    assert!(
        status.starts_with("password-less connections: off for every account"),
        "{status}"
    );
    assert!(r
        .admin(&["allow", "list"], false, None)
        .await
        .unwrap()
        .starts_with("allowlist: 3 IDs"));
    // every token from before is gone, also the one the backup never saw
    for token in [&used.phone, &later, &kim_token] {
        assert_eq!(r.post("/api/currentUser", token, None).await.status, 401);
    }
    // the bound PCs stay bound: switched off they get a signed disabled list
    r.hbbs.register(&used.pc, Some(5));
    let off = r.sync(&used.pc, &payload(&used.pc_binding)).await;
    assert_eq!(off.list()["state"], "disabled");
    // KIM's PC is bound on the server again, but cleared its mark: no list for it (listed)
    r.hbbs.register(&used.kim_pc, Some(5));
    let listed = r.sync(&used.kim_pc, &payload("")).await;
    assert_eq!(
        listed.answer.json["state"], "listed",
        "{}",
        listed.answer.text
    );
    assert!(listed.list.is_none());
    // the switch on again; the phone logs in again, and the PC's list has its new key only
    r.admin(&["passwordless", "on"], false, None).await.unwrap();
    let phone = r.login_ok(HONG, None).await;
    let active = r.sync(&used.pc, &payload(&used.pc_binding)).await;
    assert_eq!(active.list()["state"], "active");
    assert!(active.keys().contains(&b64(&hjv_api::devsig::public_key(
        &r.key_of(&phone).unwrap()
    ))));
    assert!(!active.keys().contains(&b64(&hjv_api::devsig::public_key(
        &r.key_of(&used.phone).unwrap()
    ))));
    let _ = std::fs::remove_dir_all(&dir);
}

/// A stand-in for the admin socket that answers every request with this `health`.
async fn fake_admin(path: &Path, health: check::Health) {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let health = health.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                let _ = BufReader::new(read).read_line(&mut line).await;
                let answer = json!({"ok": true, "output": serde_json::to_string(&health).unwrap()});
                let _ = write.write_all(format!("{answer}\n").as_bytes()).await;
            });
        }
    });
}

/// A stand-in for hjv-api's socket that answers /api/hjv/source.
async fn fake_api(path: &Path) {
    let listener = UnixListener::bind(path).unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            tokio::spawn(async move {
                let service = service_fn(|_req| async {
                    Ok::<_, std::convert::Infallible>(Response::new(Full::new(Bytes::from(
                        r#"{"license":"AGPL-3.0-only"}"#,
                    ))))
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
}

#[tokio::test]
async fn the_check_catches_a_stopped_socket_and_no_syncs() {
    // a fake report: three PCs on, none of them synced; and nothing on the API socket
    let dir = temp_dir("check-fake");
    let (api, admin, findings) = (
        dir.join("api.sock"),
        dir.join("admin.sock"),
        dir.join("findings"),
    );
    let no_syncs = check::Health {
        v: 1,
        time: now(),
        pcs_holding: 5,
        pcs_synced: 0,
        pcs_on: Some(3),
        pcs_on_synced: Some(0),
        hbbs: "ok".into(),
        sso: None,
        relays: None,
    };
    fake_admin(&admin, no_syncs.clone()).await;
    let check_args = || {
        args(&[
            &"check",
            &"--api-socket",
            &api,
            &"--admin-socket",
            &admin,
            &"--findings",
            &findings,
        ])
    };
    let out = run(check_args(), vec![]).await;
    let (stdout, stderr) = text(&out);
    assert_eq!(out.status.code(), Some(1), "{stdout}{stderr}");
    assert!(stdout.is_empty(), "{stdout}");
    assert!(
        stderr.contains("check: socket: hjv-api does not answer on its socket (connect: "),
        "{stderr}"
    );
    assert!(
        stderr.contains("check: sync: none of the 3 PCs that are on (by hbbs) synced in the last 10 minutes (0 of 5"),
        "{stderr}"
    );
    assert!(
        stderr.contains("2 problems found: socket, sync"),
        "{stderr}"
    );
    let written = check::read_findings(&std::fs::read_to_string(&findings).unwrap());
    assert_eq!(
        written.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
        ["socket", "sync"]
    );
    // the socket back and the syncs too: one line, and the findings go
    fake_api(&api).await;
    let healthy = dir.join("admin2.sock");
    fake_admin(
        &healthy,
        check::Health {
            pcs_synced: 4,
            pcs_on_synced: Some(3),
            ..no_syncs
        },
    )
    .await;
    let out = run(
        args(&[
            &"check",
            &"--api-socket",
            &api,
            &"--admin-socket",
            &healthy,
            &"--findings",
            &findings,
        ]),
        vec![],
    )
    .await;
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.starts_with("check: ok; socket "), "{stdout}");
    assert!(
        stdout.contains("PCs holding their binding 5, synced in the last 10 minutes 4, on 3, of them synced 3; hbbs ok"),
        "{stdout}"
    );
    assert!(!findings.exists());
    // no admin socket at all
    let out = run(
        args(&[
            &"check",
            &"--api-socket",
            &api,
            &"--admin-socket",
            &dir.join("none.sock"),
        ]),
        vec![],
    )
    .await;
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out)
        .1
        .contains("check: admin: hjv-api gave no counts on its admin socket"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_check_with_the_service() {
    let h = Harness::start().await;
    // hjv-api's own socket, as nginx reaches it
    let api = h.dir.join("api.sock");
    let (stop, stop_rx) = watch::channel(false);
    let mut rx = stop_rx.clone();
    let served = tokio::spawn(server::serve(
        h.state.clone(),
        Listener::Unix(UnixListener::bind(&api).unwrap()),
        async move {
            let _ = rx.wait_for(|s| *s).await;
        },
    ));
    let findings = h.dir.join("findings");
    let check = || {
        run(
            args(&[
                &"check",
                &"--api-socket",
                &api,
                &"--admin-socket",
                &h.admin_socket,
                &"--findings",
                &findings,
            ]),
            vec![],
        )
    };
    let out = check().await;
    assert!(out.status.success(), "{}", text(&out).1);
    assert!(
        text(&out).0.contains("PCs holding their binding 0"),
        "{}",
        text(&out).0
    );

    let one = TestDevice::windows("622000001");
    let two = TestDevice::windows("622000002");
    let (b1, _) = bind_pc(&h, HONG, &one).await;
    let (b2, _) = bind_pc(&h, KIM, &two).await;
    h.sync(&one, &payload(&b1)).await.list();
    h.sync(&two, &payload(&b2)).await.list();
    // on by hbbs, but just now: not counted yet (a PC that has just woken syncs first)
    let out = check().await;
    assert!(out.status.success(), "{}", text(&out).1);
    assert!(
        text(&out)
            .0
            .contains("holding their binding 2, synced in the last 10 minutes 2, on 0"),
        "{}",
        text(&out).0
    );
    // on for a while and synced
    for pc in [&one, &two] {
        h.state.online.backdate(&pc.id, Duration::from_secs(600));
    }
    let out = check().await;
    assert!(out.status.success(), "{}", text(&out).1);
    assert!(
        text(&out).0.contains("on 2, of them synced 2; hbbs ok"),
        "{}",
        text(&out).0
    );
    // their syncs stop
    let stale = now() - 1200;
    h.state
        .db
        .call(move |c| c.execute("UPDATE devices SET last_sync_at = ?1", [stale]))
        .await
        .unwrap();
    let out = check().await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out)
            .1
            .contains("check: sync: none of the 2 PCs that are on"),
        "{}",
        text(&out).1
    );
    assert!(std::fs::read_to_string(&findings)
        .unwrap()
        .starts_with("sync: "));
    // one syncs again: all well
    h.sync(&one, &payload(&b1)).await.list();
    let out = check().await;
    assert!(out.status.success(), "{}", text(&out).1);
    assert!(!findings.exists());
    // a PC off by hbbs is no alarm, however old its last sync
    h.hbbs.register(&two, Some(4000));
    h.state
        .db
        .call(move |c| c.execute("UPDATE devices SET last_sync_at = ?1", [stale]))
        .await
        .unwrap();
    h.state.online.backdate(&one.id, Duration::from_secs(600));
    let out = check().await;
    assert_eq!(out.status.code(), Some(1), "PC one is still on");
    h.hbbs.register(&one, Some(4000));
    let out = check().await;
    assert!(out.status.success(), "{}{}", text(&out).0, text(&out).1);
    assert!(text(&out).0.contains("on 0"), "{}", text(&out).0);
    // the SSO refuses the client credentials: one login pauses the calls
    h.state.sso_breaker.note(Some(Failure::ClientRefused));
    let out = check().await;
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).1.contains("check: sso_client: the SSO refuses this server's client credentials (invalid_client), SSO calls paused now"), "{}", text(&out).1);
    h.state.sso_breaker.reset();
    // hbbs without hjv-pk: nobody knows which PCs are on
    h.hbbs
        .unpatched
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let out = check().await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out)
            .1
            .contains("check: hbbs: hbbs's console: no hjv-pk (patch 0004 missing)"),
        "{}",
        text(&out).1
    );
    h.hbbs
        .unpatched
        .store(false, std::sync::atomic::Ordering::SeqCst);
    // a database the service cannot read is a finding, not a pass
    h.state
        .db
        .call(|c| c.execute_batch("ALTER TABLE devices RENAME TO devices_away"))
        .await
        .unwrap();
    let out = check().await;
    assert_eq!(out.status.code(), Some(1));
    assert!(
        text(&out).1.contains("check: admin: hjv-api gave no counts on its admin socket (database: no such table: devices)"),
        "{}",
        text(&out).1
    );
    h.state
        .db
        .call(|c| c.execute_batch("ALTER TABLE devices_away RENAME TO devices"))
        .await
        .unwrap();
    // the checks are not in the operator record; the socket stopped is a finding
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert!(!record.contains("health"), "{record}");
    let _ = stop.send(true);
    served.await.unwrap();
    let _ = std::fs::remove_file(&api);
    let out = check().await;
    assert_eq!(out.status.code(), Some(1));
    assert!(text(&out).1.contains("check: socket: "), "{}", text(&out).1);
}

/// A stand-in for Telegram's Bot API: what it was sent, and the status it answers with.
struct FakeTelegram {
    url: String,
    got: Arc<Mutex<Vec<(String, String)>>>,
    status: Arc<AtomicU16>,
}

async fn fake_telegram() -> FakeTelegram {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let got: Arc<Mutex<Vec<(String, String)>>> = Default::default();
    let status = Arc::new(AtomicU16::new(200));
    let (g, s) = (got.clone(), status.clone());
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let (g, s) = (g.clone(), s.clone());
            tokio::spawn(async move {
                let service = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let (g, s) = (g.clone(), s.clone());
                    async move {
                        let path = req.uri().path().to_owned();
                        let body = req.into_body().collect().await.unwrap().to_bytes();
                        g.lock()
                            .unwrap()
                            .push((path, String::from_utf8_lossy(&body).into_owned()));
                        let code = s.load(Ordering::SeqCst);
                        let answer = if code == 200 {
                            r#"{"ok":true,"result":{}}"#.to_owned()
                        } else {
                            format!(r#"{{"ok":false,"description":"Unauthorized for {TOKEN}"}}"#)
                        };
                        Ok::<_, std::convert::Infallible>(
                            Response::builder()
                                .status(code)
                                .body(Full::new(Bytes::from(answer)))
                                .unwrap(),
                        )
                    }
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service)
                    .await;
            });
        }
    });
    FakeTelegram { url, got, status }
}

const TOKEN: &str = "123456:TEST-not-a-real-bot-token_x";

/// The text of a sent message.
fn sent_text(body: &str) -> String {
    form_urlencoded::parse(body.as_bytes())
        .find(|(k, _)| k == "text")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

#[tokio::test]
async fn alerts_go_to_the_journal_and_to_telegram_once_in_six_hours() {
    let telegram = fake_telegram().await;
    let dir = private_dir("alert");
    let (creds, state, findings) = (dir.join("creds"), dir.join("state"), dir.join("findings"));
    std::fs::create_dir_all(&creds).unwrap();
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(
        creds.join("hjviewer-api-telegram"),
        format!("TELEGRAM_BOT_TOKEN={TOKEN}\nTELEGRAM_CHAT_ID=-1001234\n"),
    )
    .unwrap();
    std::fs::write(
        &findings,
        "sync: none of the 2 PCs that are on (by hbbs) synced in the last 10 minutes (0 of 3 PCs holding their binding did): …\nsso_client: the SSO refuses this server's client credentials (invalid_client), SSO calls paused now: …\n",
    )
    .unwrap();
    let alert = |unit: &str| {
        let a = args(&[
            &"alert",
            &unit,
            &"--findings",
            &findings,
            &"--state-dir",
            &state,
            &"--telegram-api",
            &telegram.url,
        ]);
        run(
            a,
            vec![(
                "CREDENTIALS_DIRECTORY",
                creds.to_string_lossy().into_owned(),
            )],
        )
    };
    let sent = || telegram.got.lock().unwrap().clone();

    let out = alert("hjviewer-api-check.service").await;
    let (stdout, stderr) = text(&out);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(
        stderr.contains("error: alert: hjviewer-api-check.service failed (sync, sso_client)"),
        "{stderr}"
    );
    assert!(stderr.contains("alert: sent to Telegram"), "{stderr}");
    let got = sent();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, format!("/bot{TOKEN}/sendMessage"));
    assert!(got[0].1.contains("chat_id=-1001234"));
    let message = sent_text(&got[0].1);
    assert!(
        message.starts_with("[HJViewer 경보] hjviewer-api-check.service 실패\n"),
        "{message}"
    );
    assert!(
        message.contains("켜진 PC들이 10분 동안 동기화하지 못했습니다"),
        "{message}"
    );
    assert!(message.contains("invalid_client"), "{message}");
    assert!(message.contains("journalctl --namespace=hjviewer -u hjviewer-api-check -n 50"));
    for out in [&stdout, &stderr] {
        assert!(!out.contains("TEST-not-a-real"), "the token: {out}");
    }

    // the same alert again within six hours: the journal only
    let out = alert("hjviewer-api-check.service").await;
    assert!(out.status.success());
    assert!(
        text(&out)
            .1
            .contains("alert: sent to Telegram 0 minutes ago; again after 6 hours"),
        "{}",
        text(&out).1
    );
    assert_eq!(sent().len(), 1);
    // other findings go at once
    std::fs::write(
        &findings,
        "socket: hjv-api does not answer on its socket (connect: …)\n",
    )
    .unwrap();
    let out = alert("hjviewer-api-check.service").await;
    assert!(out.status.success());
    assert_eq!(sent().len(), 2);
    assert!(sent_text(&sent()[1].1).contains("hjv-api가 소켓에서 답하지 않습니다"));
    // an hour on, still not; six hours on, the same alert goes again
    let sent_ago = |secs: i64| {
        for stamp in std::fs::read_dir(&state).unwrap() {
            let path = stamp.unwrap().path();
            std::fs::write(&path, (now() - secs).to_string()).unwrap();
        }
    };
    sent_ago(5 * 3600 + 1800);
    let out = alert("hjviewer-api-check.service").await;
    assert!(out.status.success());
    assert!(
        text(&out)
            .1
            .contains("alert: sent to Telegram 330 minutes ago"),
        "{}",
        text(&out).1
    );
    assert_eq!(sent().len(), 2);
    sent_ago(6 * 3600 + 1);
    let out = alert("hjviewer-api-check.service").await;
    assert!(out.status.success());
    assert_eq!(sent().len(), 3);
    // the service's own failure, without findings (the check's are not its), with how it ended
    // as systemd tells the OnFailure= unit
    let ended = |result: &str, code: &str, status: &str| {
        let a = args(&[
            &"alert",
            &"hjviewer-api.service",
            &"--state-dir",
            &state,
            &"--telegram-api",
            &telegram.url,
        ]);
        run(
            a,
            vec![
                (
                    "CREDENTIALS_DIRECTORY",
                    creds.to_string_lossy().into_owned(),
                ),
                ("MONITOR_SERVICE_RESULT", result.to_owned()),
                ("MONITOR_EXIT_CODE", code.to_owned()),
                ("MONITOR_EXIT_STATUS", status.to_owned()),
            ],
        )
    };
    let out = ended("signal", "killed", "KILL").await;
    assert!(out.status.success());
    assert!(
        text(&out)
            .1
            .contains("alert: hjviewer-api.service failed [signal, killed KILL]"),
        "{}",
        text(&out).1
    );
    let message = sent_text(&sent()[3].1);
    assert!(message.contains("hjv-api 서비스가 멈췄습니다"), "{message}");
    assert!(message.contains("종료: signal, killed KILL"), "{message}");
    assert!(!message.contains("socket"), "{message}");
    // the same crash again: once in six hours; a start that fails is another alert
    assert!(ended("signal", "killed", "KILL").await.status.success());
    assert_eq!(sent().len(), 4);
    assert!(ended("exit-code", "exited", "1").await.status.success());
    assert_eq!(sent().len(), 5);
    assert!(sent_text(&sent()[4].1).contains("종료: exit-code, exited 1"));
    // stale findings are not the check's of now
    let past = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&findings)
        .unwrap()
        .set_modified(past)
        .unwrap();
    for stamp in std::fs::read_dir(&state).unwrap() {
        std::fs::remove_file(stamp.unwrap().path()).unwrap();
    }
    let out = alert("hjviewer-api-check.service").await;
    assert!(out.status.success());
    assert!(sent_text(&sent()[5].1).contains("점검 자체가 실패했습니다"));
    // the test message goes whatever was sent
    let out = alert("test").await;
    assert!(out.status.success());
    assert_eq!(sent().len(), 7);
    assert!(sent_text(&sent()[6].1).starts_with("[HJViewer 경보 시험]"));
    // Telegram refuses: the unit fails, and the token appears nowhere
    telegram.status.store(401, Ordering::SeqCst);
    let out = alert("test").await;
    let (stdout, stderr) = text(&out);
    assert!(!out.status.success());
    assert!(
        stderr.contains("alert: Telegram: HTTP 401 Unauthorized for <token>"),
        "{stderr}"
    );
    assert!(!stdout.contains("TEST-not-a-real") && !stderr.contains("TEST-not-a-real"));
    // without the credential: the journal line is the alert
    let before = sent().len();
    let out = run(
        args(&[
            &"alert",
            &"hjviewer-api-backup.service",
            &"--telegram-api",
            &telegram.url,
        ]),
        vec![(
            "CREDENTIALS_DIRECTORY",
            dir.join("none").to_string_lossy().into_owned(),
        )],
    )
    .await;
    assert!(out.status.success());
    assert!(text(&out)
        .1
        .contains("error: alert: hjviewer-api-backup.service failed"));
    assert!(text(&out)
        .1
        .contains("no Telegram (credential hjviewer-api-telegram)"));
    assert_eq!(sent().len(), before);
    // a credential in another form is an error that shows no value
    std::fs::write(
        creds.join("hjviewer-api-telegram"),
        format!("TOKEN={TOKEN}\n"),
    )
    .unwrap();
    let out = alert("test").await;
    assert!(!out.status.success());
    assert!(!text(&out).1.contains("TEST-not-a-real"));
    // only a local stand-in may replace Telegram
    let out = run(
        args(&[&"alert", &"test", &"--telegram-api", &"https://example.com"]),
        vec![],
    )
    .await;
    assert!(!out.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_units_verify() {
    let Ok(out) = Command::new("systemd-analyze").arg("--version").output() else {
        eprintln!("skipped: systemd-analyze is not installed");
        return;
    };
    assert!(out.status.success());
    // the shipped units, with the binary of this build in place of /opt/hjviewer/bin/hjv-api
    let deploy = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/systemd");
    let dir = temp_dir("units");
    let mut units = Vec::new();
    for entry in std::fs::read_dir(&deploy).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !name.starts_with("hjviewer-api") || name.ends_with(".example") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap()
            .replace("/opt/hjviewer/bin/hjv-api", BIN);
        std::fs::write(dir.join(&name), text).unwrap();
        units.push(name.replace("@.", "@ci."));
    }
    units.sort();
    assert_eq!(
        units,
        [
            "hjviewer-api-alert@ci.service",
            "hjviewer-api-backup.service",
            "hjviewer-api-backup.timer",
            "hjviewer-api-check.service",
            "hjviewer-api-check.timer",
            "hjviewer-api.service",
            "hjviewer-api.socket"
        ]
    );
    let out = Command::new("systemd-analyze")
        .args(["verify", "--man=no"])
        .args(&units)
        .current_dir(&dir)
        .env("SYSTEMD_UNIT_PATH", format!("{}:", dir.display()))
        .output()
        .unwrap();
    let (stdout, stderr) = text(&out);
    assert!(
        out.status.success() && stderr.is_empty(),
        "{stdout}{stderr}"
    );
    // the watch: every unit of hjv-api that fails raises the alert, never the alert itself
    let unit = |name: &str| std::fs::read_to_string(deploy.join(name)).unwrap();
    for name in [
        "hjviewer-api.service",
        "hjviewer-api-check.service",
        "hjviewer-api-backup.service",
    ] {
        assert!(
            unit(name).contains("\nOnFailure=hjviewer-api-alert@%n.service\n"),
            "{name}"
        );
    }
    assert!(!unit("hjviewer-api-alert@.service")
        .lines()
        .any(|l| l.starts_with("OnFailure=")));
    assert!(unit("hjviewer-api-backup.service").contains(
        "\nExecStart=/opt/hjviewer/bin/hjv-api admin backup --keep-days 14 /var/lib/hjviewer-api/backup\n"
    ));
    let journal = std::fs::read_to_string(deploy.join("../journald@hjviewer.conf")).unwrap();
    assert!(journal.contains("\nMaxRetentionSec=30day\n"));
    let _ = std::fs::remove_dir_all(&dir);
}
