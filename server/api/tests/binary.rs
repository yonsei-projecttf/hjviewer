// SPDX-License-Identifier: AGPL-3.0-only

//! The hjv-api program as systemd runs it: socket activation (systemd-socket-activate stands in
//! for PID 1), settings from an env file, secrets from a credentials directory, the admin command
//! line, keygen, selfcheck, and a clean stop on SIGTERM.

mod common;

use common::*;
use hjv_api::{fake_sso, keys, seal, util};
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixStream},
};

const BIN: &str = env!("CARGO_BIN_EXE_hjv-api");

fn find(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| Path::new(d).join(program))
        .find(|p| p.is_file())
}

/// The hjv-api a test started (systemd-socket-activate execs it in its own process). A dropped
/// Child goes on running, so a test that failed before it stopped the server would leave it
/// serving, with its database and sockets under /tmp: this kills it then.
struct Server(Child);

impl std::ops::Deref for Server {
    type Target = Child;

    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Server {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

/// One HTTP/1.1 request over a Unix socket, as nginx sends it.
async fn unix_request(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let mut stream = None;
    for _ in 0..100 {
        match UnixStream::connect(socket).await {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let mut stream = stream.expect("the socket accepts connections");
    let body = body.map(Value::to_string).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: rd.example.com\r\nX-Real-IP: 192.0.2.10\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut answer))
        .await
        .expect("an answer in time")
        .unwrap();
    let answer = String::from_utf8(answer).unwrap();
    let status: u16 = answer[9..12].parse().unwrap();
    let body = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

fn write_setup(
    dir: &Path,
    sso: &fake_sso::FakeSso,
    hbbs: &FakeHbbs,
) -> (PathBuf, PathBuf, [u8; 32]) {
    let env = dir.join("hjviewer-api.env");
    std::fs::write(
        &env,
        format!(
            "# test settings\nHJV_API_SSO_TOKEN_URL={}\nHJV_API_SSO_USERINFO_URL={}\nHJV_API_SSO_CLIENT_ID={}\nHJV_API_HBBS_CONSOLE={}\nHJV_API_HBBS_PUBKEY={}\nHJV_API_FAILURE_DELAY_MS=50\nHJV_API_FAILURE_JITTER_MS=10\nHJV_API_TRUST_PUBKEYS={}\n",
            sso.token_url(),
            sso.user_url(),
            fake_sso::CLIENT_ID,
            hbbs.addr,
            hbbs_pubkey(),
            trust_pubkeys(),
        ),
    )
    .unwrap();
    let creds = dir.join("credentials");
    std::fs::create_dir_all(&creds).unwrap();
    let seal_secret = [11u8; 32];
    for (name, value) in [
        ("hjviewer-sso-client-secret", fake_sso::BASIC.to_owned()),
        ("hjviewer-sso-api-uuid", fake_sso::API_UUID.to_owned()),
        (
            "hjviewer-api-login-key",
            format!("# current\n{}\n", util::b64(&seal_secret)),
        ),
        ("hjviewer-api-log-key", util::b64(&[5u8; 32])),
        (
            "hjviewer-api-list-key",
            format!("# list key 1\n{}\n", util::b64(&LIST_SEED)),
        ),
    ] {
        std::fs::write(creds.join(name), value).unwrap();
    }
    let seal_pk = *crypto_box::SecretKey::from_bytes(seal_secret)
        .public_key()
        .as_bytes();
    (env, creds, seal_pk)
}

#[tokio::test]
async fn runs_as_systemd_starts_it() {
    let Some(activate) = find("systemd-socket-activate") else {
        eprintln!("skipped: systemd-socket-activate is not installed");
        return;
    };
    let dir = temp_dir("binary");
    let sso = fake_sso::start(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        Duration::from_secs(3),
    );
    let hbbs = FakeHbbs::start().await;
    let (env, creds, seal_pk) = write_setup(&dir, &sso, &hbbs);
    let (socket, admin_socket, db, oplog) = (
        dir.join("api.sock"),
        dir.join("admin.sock"),
        dir.join("hjv-api.sqlite3"),
        dir.join("operator.log"),
    );
    let mut child = Server(
        Command::new(activate)
            .arg("-l")
            .arg(&socket)
            .arg(BIN)
            .arg("serve")
            .arg("--db")
            .arg(&db)
            .arg("--env-file")
            .arg(&env)
            .arg("--credentials")
            .arg(&creds)
            .arg("--admin-socket")
            .arg(&admin_socket)
            .arg("--operator-log")
            .arg(&oplog)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );

    // the first connection starts hjv-api with the socket (fd 3)
    let (status, source) = unix_request(&socket, "GET", "/api/hjv/source", None).await;
    assert_eq!(status, 200);
    assert_eq!(source["license"], "AGPL-3.0-only");
    for _ in 0..100 {
        if admin_socket.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // the allowlist from a file, through the command line
    let list = dir.join("allow.txt");
    std::fs::write(&list, "ZZTEST01\n").unwrap();
    let out = Command::new(BIN)
        .args(["admin", "--socket"])
        .arg(&admin_socket)
        .args(["allow", "import"])
        .arg(&list)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "allowlist: 1 added, 0 already there\n"
    );

    // a login through the socket, with its account key
    let nonce = util::b64(&[3u8; 16]);
    let (ck, cp) = keys::proof(&[5u8; 32], &nonce, "");
    let body = json!({
        "username": "zztest01",
        "password": seal::seal_login(&seal_pk, "hjv-test-pass-1", util::now(), &nonce, "", &ck, &cp),
        "id": "", "uuid": "", "autoLogin": true, "type": "account",
        "deviceInfo": {"os": "ios", "type": "client", "name": "iPhone15,2"},
    });
    let (status, answer) = unix_request(&socket, "POST", "/api/login", Some(&body)).await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(answer["user"]["display_name"], "홍길동");
    assert_eq!(answer["hjv"]["ks"], 1, "{answer}");

    let out = Command::new(BIN)
        .args(["admin", "--socket"])
        .arg(&admin_socket)
        .arg("report")
        .output()
        .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(
        report.contains("allowlist 1, accounts 1, devices 0, tokens 1"),
        "{report}"
    );
    assert!(report.contains("account keys: 1 active"), "{report}");
    let out = Command::new(BIN)
        .args(["admin", "--socket"])
        .arg(&admin_socket)
        .arg("bogus")
        .output()
        .unwrap();
    assert!(!out.status.success());

    // files readable by the owner only
    for f in [&db, &oplog, &admin_socket] {
        let mode = std::fs::metadata(f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", f.display());
    }

    // SIGTERM stops it cleanly
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .unwrap();
    assert!(status.success());
    let mut waited = None;
    for _ in 0..100 {
        if let Some(s) = child.try_wait().unwrap() {
            waited = Some(s);
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let exit = waited.expect("hjv-api stops on SIGTERM");
    let mut stderr = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
    assert!(exit.success(), "{exit:?}: {stderr}");
    assert!(stderr.contains("ready: seal keys "), "{stderr}");
    assert!(stderr.contains("hjv-api stopped"), "{stderr}");
    assert!(
        !stderr.contains("hjv-test-pass-1") && !stderr.to_lowercase().contains("zztest01"),
        "{stderr}"
    );

    let out = Command::new(BIN)
        .arg("oplog-verify")
        .arg(&oplog)
        .output()
        .unwrap();
    assert!(out.status.success());
    // two lines for each of the three commands
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "6 lines, chain intact\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn refuses_to_start_without_its_socket_or_secrets() {
    let dir = temp_dir("refuse");
    let sso = fake_sso::start(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        Duration::from_secs(3),
    );
    let hbbs = FakeHbbs::start().await;
    let (env, creds, _) = write_setup(&dir, &sso, &hbbs);
    let serve = |extra: &[&str]| {
        let out = Command::new(BIN)
            .arg("serve")
            .arg("--db")
            .arg(dir.join("db.sqlite3"))
            .arg("--env-file")
            .arg(&env)
            .args(extra)
            .env_remove("CREDENTIALS_DIRECTORY")
            .env_remove("LISTEN_FDS")
            .output()
            .unwrap();
        (
            out.status.success(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };
    let creds = creds.to_str().unwrap();
    let (ok, err) = serve(&["--credentials", creds]);
    assert!(!ok && err.contains("no socket"), "{err}");
    let (ok, err) = serve(&[]);
    assert!(!ok && err.contains("no credentials"), "{err}");
    let (ok, err) = serve(&["--credentials", creds, "--listen", "0.0.0.0:18799"]);
    assert!(!ok && err.contains("loopback"), "{err}");
    std::fs::remove_file(Path::new(creds).join("hjviewer-api-login-key")).unwrap();
    let (ok, err) = serve(&["--credentials", creds]);
    assert!(
        !ok && err.contains("credential hjviewer-api-login-key"),
        "{err}"
    );
    // the list key that signs the trust lists is one of the secrets (design v3 §5-3)
    let (_, creds_again, _) = write_setup(&dir, &sso, &hbbs);
    std::fs::remove_file(creds_again.join("hjviewer-api-list-key")).unwrap();
    let (ok, err) = serve(&["--credentials", creds_again.to_str().unwrap()]);
    assert!(
        !ok && err.contains("credential hjviewer-api-list-key"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn keygen_and_selfcheck() {
    let out = Command::new(BIN).args(["keygen", "seal"]).output().unwrap();
    assert!(out.status.success());
    let secret = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    let note = String::from_utf8_lossy(&out.stderr).into_owned();
    let keys = hjv_api::config::parse_seal_keys(&secret).unwrap();
    let kid = &seal::SealKeys::new(&keys).key_ids()[0];
    assert!(note.contains(&format!("seal key {kid}")), "{note}");
    assert!(note.contains("HJV_LOGIN_SEAL_PUBKEY"), "{note}");
    assert!(!note.contains(&secret));
    let out = Command::new(BIN)
        .args(["admin", "keygen", "log-key"])
        .output()
        .unwrap();
    assert_eq!(
        util::b64_decode(String::from_utf8_lossy(&out.stdout).trim())
            .unwrap()
            .len(),
        32
    );
    // a list key (design v3 §5-3): the seed to the credential, the public key as the entry of
    // HJV_TRUST_PUBKEYS and its fingerprint to the screen
    let out = Command::new(BIN)
        .args(["keygen", "list", "--kid", "2"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let seed =
        hjv_api::config::parse_list_key(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let public = hjv_api::devsig::public_key(&seed);
    let note = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(
        note.contains(&format!(
            "public key (an entry of HJV_TRUST_PUBKEYS and HJV_API_TRUST_PUBKEYS): {}",
            hjv_api::trust::entry(2, &public)
        )),
        "{note}"
    );
    assert!(
        note.contains(&format!(
            "fingerprint: {}",
            hjv_api::trust::fingerprint(2, &public)
        )),
        "{note}"
    );
    assert!(!note.contains(&util::b64(&*seed)));
    for bad in [
        &["keygen", "list"][..],
        &["keygen", "list", "--kid", "0"],
        &["keygen", "list", "--kid", "x"],
    ] {
        assert!(
            !Command::new(BIN)
                .args(bad)
                .output()
                .unwrap()
                .status
                .success(),
            "{bad:?}"
        );
    }

    let dir = temp_dir("selfcheck");
    let sso = fake_sso::start(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        Duration::from_secs(3),
    );
    let hbbs = FakeHbbs::start().await;
    let (env, _, _) = write_setup(&dir, &sso, &hbbs);
    let out = tokio::task::spawn_blocking({
        let env = env.clone();
        move || {
            Command::new(BIN)
                .arg("selfcheck")
                .arg("--env-file")
                .arg(&env)
                .output()
                .unwrap()
        }
    })
    .await
    .unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{report}");
    assert!(report.contains("[ OK ] hbbs console"), "{report}");
    assert_eq!(sso.token_calls(), 0, "selfcheck sends no request");
    // the fingerprints of the pinned list keys, to compare with the app build's summary; without
    // the credentials the key in use is not compared (design v3 §5-3)
    let one = hjv_api::trust::fingerprint(1, &hjv_api::devsig::public_key(&LIST_SEED));
    let two = hjv_api::trust::fingerprint(2, &hjv_api::devsig::public_key(&NEXT_LIST_SEED));
    assert!(
        report.contains(&format!(
            "[ OK ] list keys the apps pin (HJV_API_TRUST_PUBKEYS), SHA-256: {one}, {two}"
        )),
        "{report}"
    );
    assert!(
        report.contains("[ -- ] list key in use: not read here"),
        "{report}"
    );
    // with them, the key in use is the pinned key 1; a credential that is not catches the eye
    let (_, creds, _) = write_setup(&dir, &sso, &hbbs);
    let selfcheck = |creds: PathBuf, env: PathBuf| {
        tokio::task::spawn_blocking(move || {
            Command::new(BIN)
                .arg("selfcheck")
                .arg("--env-file")
                .arg(&env)
                .arg("--credentials")
                .arg(&creds)
                .output()
                .unwrap()
        })
    };
    let out = selfcheck(creds.clone(), env.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{report}");
    assert!(
        report.contains(&format!(
            "[ OK ] list key in use (HJV_API_LIST_KID 1): the pinned key 1, {one}"
        )),
        "{report}"
    );
    std::fs::write(
        creds.join("hjviewer-api-list-key"),
        util::b64(&NEXT_LIST_SEED),
    )
    .unwrap();
    let out = selfcheck(creds.clone(), env.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "{report}");
    assert!(
        report.contains(&format!(
            "[FAIL] list key in use (HJV_API_LIST_KID 1, {}) is not the pinned key 1",
            hjv_api::trust::fingerprint(1, &hjv_api::devsig::public_key(&NEXT_LIST_SEED))
        )),
        "{report}"
    );
    assert!(
        !report.contains(&util::b64(&NEXT_LIST_SEED)),
        "no secret in the report"
    );
    drop(hbbs);
    // --log-only never fails the unit
    let (_held, refusing) = refusing_addr();
    let out = tokio::task::spawn_blocking(move || {
        Command::new(BIN)
            .args(["selfcheck", "--log-only", "--env-file"])
            .arg(&env)
            .env("HJV_API_HBBS_CONSOLE", refusing.to_string())
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("selfcheck [FAIL] hbbs console"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_unusable_operator_log_takes_out_the_admin_commands_only() {
    let Some(activate) = find("systemd-socket-activate") else {
        eprintln!("skipped: systemd-socket-activate is not installed");
        return;
    };
    let dir = temp_dir("oplog");
    let sso = fake_sso::start(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        Duration::from_secs(3),
    );
    let hbbs = FakeHbbs::start().await;
    let (env, creds, _) = write_setup(&dir, &sso, &hbbs);
    let (socket, admin_socket) = (dir.join("api.sock"), dir.join("admin.sock"));
    let mut child = Server(
        Command::new(activate)
            .arg("-l")
            .arg(&socket)
            .arg(BIN)
            .arg("serve")
            .arg("--db")
            .arg(dir.join("hjv-api.sqlite3"))
            .arg("--env-file")
            .arg(&env)
            .arg("--credentials")
            .arg(&creds)
            .arg("--admin-socket")
            .arg(&admin_socket)
            .arg("--operator-log")
            .arg(dir.join("missing").join("operator.log"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    // logins and the account paths still work, only the admin socket is missing
    let (status, source) = unix_request(&socket, "GET", "/api/hjv/source", None).await;
    assert_eq!(status, 200, "{source}");
    assert!(!admin_socket.exists());
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status();
    let _ = child.wait();
    let mut stderr = String::new();
    std::io::Read::read_to_string(child.stderr.as_mut().unwrap(), &mut stderr).unwrap();
    assert!(stderr.contains("admin commands are off"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The command key (design v3 §5-3, relay/PROTOCOL.md §3.2): `keygen command --kid N` shows the
/// public key as exactly the line `N:<base64>` on the screen, then its fingerprint, and writes the
/// seed only where it is kept; `selfcheck` compares the key in the credentials with
/// HJV_API_COMMAND_PUBKEYS and shows the same fingerprints, and fails when they do not match or one
/// is missing.
#[tokio::test]
async fn keygen_command_and_selfcheck_of_the_command_key() {
    let out = Command::new(BIN)
        .args(["keygen", "command", "--kid", "2"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let seed =
        hjv_api::config::parse_list_key(String::from_utf8_lossy(&out.stdout).trim()).unwrap();
    let public = hjv_api::devsig::public_key(&seed);
    let note = String::from_utf8_lossy(&out.stderr).into_owned();
    assert_eq!(
        note,
        format!(
            "{}\nfingerprint: {}\n",
            hjv_api::trust::entry(2, &public),
            hjv_api::trust::fingerprint(2, &public)
        )
    );
    assert!(!note.contains(&util::b64(&*seed)));
    for bad in [
        &["keygen", "command"][..],
        &["keygen", "command", "--kid", "0"],
        &["keygen", "command", "--kid", "65536"],
    ] {
        assert!(
            !Command::new(BIN)
                .args(bad)
                .output()
                .unwrap()
                .status
                .success(),
            "{bad:?}"
        );
    }

    let dir = temp_dir("selfcheck-command");
    let sso = fake_sso::start(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        Duration::from_secs(3),
    );
    let hbbs = FakeHbbs::start().await;
    let (env, creds, _) = write_setup(&dir, &sso, &hbbs);
    let selfcheck = |env: PathBuf, creds: PathBuf| {
        tokio::task::spawn_blocking(move || {
            Command::new(BIN)
                .arg("selfcheck")
                .arg("--env-file")
                .arg(&env)
                .arg("--credentials")
                .arg(&creds)
                .output()
                .unwrap()
        })
    };
    // neither the setting nor the credential: Wake-on-LAN is off, nothing fails
    let out = selfcheck(env.clone(), creds.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{report}");
    assert!(report.contains("[ -- ] command key: none"), "{report}");
    // the key of the relays' first pin, made with keygen as the runbook makes it
    let out = Command::new(BIN)
        .args(["keygen", "command", "--kid", "1", "--out"])
        .arg(creds.join("hjviewer-api-command-key"))
        .output()
        .unwrap();
    assert!(out.status.success());
    // the screen: the pin on a line of its own, then the fingerprint to compare later
    let note = String::from_utf8_lossy(&out.stderr).into_owned();
    let mut lines = note.lines();
    let pin = lines.next().unwrap().to_owned();
    let shown = lines
        .next()
        .and_then(|l| l.strip_prefix("fingerprint: "))
        .unwrap()
        .to_owned();
    assert_eq!(lines.next(), None, "{note}");
    let other = hjv_api::trust::entry(2, &hjv_api::devsig::public_key(&NEXT_COMMAND_SEED));
    let text = std::fs::read_to_string(&env).unwrap();
    std::fs::write(
        &env,
        format!("{text}HJV_API_COMMAND_PUBKEYS={pin},{other}\n"),
    )
    .unwrap();
    let out = selfcheck(env.clone(), creds.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{report}");
    let seed = hjv_api::config::load_command_key(&creds).unwrap().unwrap();
    let mine = hjv_api::trust::fingerprint(1, &hjv_api::devsig::public_key(&seed));
    assert!(
        report.contains(&format!(
            "[ OK ] command key in use (HJV_API_COMMAND_KID 1): the pinned key 1, {mine}"
        )),
        "{report}"
    );
    // the fingerprint the key's maker saw is the one selfcheck shows
    assert_eq!(shown, mine);
    assert!(!report.contains(&util::b64(&*seed)));
    // the next key put in before the relays were built with it
    std::fs::write(
        creds.join("hjviewer-api-command-key"),
        util::b64(&[44u8; 32]),
    )
    .unwrap();
    let out = selfcheck(env.clone(), creds.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!out.status.success());
    assert!(
        report.contains("is not the pinned key 1: the relays refuse every command it signs"),
        "{report}"
    );
    // the setting without the credential
    std::fs::remove_file(creds.join("hjviewer-api-command-key")).unwrap();
    let out = selfcheck(env, creds).await.unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout)
        .contains("[FAIL] command key: no credential hjviewer-api-command-key"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The ticket key (docs/access-tickets.md 2-5): `keygen ticket --kid N` shows the public key as
/// exactly the line `N:<base64>`, the value of a KEY= line of hbbs's access.conf, then its
/// fingerprint, and writes the seed only where it is kept; `selfcheck` compares the key in the
/// credentials with HJV_API_TICKET_PUBKEYS and fails when hbbs would not take it.
#[tokio::test]
async fn keygen_ticket_and_selfcheck_of_the_ticket_key() {
    for bad in [
        &["keygen", "ticket"][..],
        &["keygen", "ticket", "--kid", "0"],
        &["keygen", "ticket", "--kid", "65536"],
    ] {
        assert!(
            !Command::new(BIN)
                .args(bad)
                .output()
                .unwrap()
                .status
                .success(),
            "{bad:?}"
        );
    }
    let dir = temp_dir("selfcheck-ticket");
    let sso = fake_sso::start(
        TcpListener::bind("127.0.0.1:0").await.unwrap(),
        Duration::from_secs(3),
    );
    let hbbs = FakeHbbs::start().await;
    let (env, creds, _) = write_setup(&dir, &sso, &hbbs);
    let selfcheck = |env: PathBuf, creds: PathBuf| {
        tokio::task::spawn_blocking(move || {
            Command::new(BIN)
                .arg("selfcheck")
                .arg("--env-file")
                .arg(&env)
                .arg("--credentials")
                .arg(&creds)
                .output()
                .unwrap()
        })
    };
    // neither the setting nor the credential: no ticket is signed, nothing fails
    let out = selfcheck(env.clone(), creds.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{report}");
    assert!(report.contains("[ -- ] ticket key: none"), "{report}");
    // key 1, made with keygen into the credential
    let out = Command::new(BIN)
        .args(["keygen", "ticket", "--kid", "1", "--out"])
        .arg(creds.join("hjviewer-api-ticket-key"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let note = String::from_utf8_lossy(&out.stderr).into_owned();
    let seed = hjv_api::config::load_ticket_key(&creds).unwrap().unwrap();
    let public = hjv_api::devsig::public_key(&seed);
    assert_eq!(
        note,
        format!(
            "{}\nfingerprint: {}\n",
            hjv_api::trust::entry(1, &public),
            hjv_api::trust::fingerprint(1, &public)
        )
    );
    assert!(!note.contains(&util::b64(&*seed)));
    // hbbs's access.conf takes it, with the next key, made as key 2 would be
    let pin = note.lines().next().unwrap().to_owned();
    let out = Command::new(BIN)
        .args(["keygen", "ticket", "--kid", "2", "--out"])
        .arg(dir.join("ticket-2.key"))
        .output()
        .unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let next = stderr.lines().next().unwrap().to_owned();
    let text = std::fs::read_to_string(&env).unwrap();
    std::fs::write(&env, format!("{text}HJV_API_TICKET_PUBKEYS={pin},{next}\n")).unwrap();
    let out = selfcheck(env.clone(), creds.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "{report}");
    assert!(
        report.contains(&format!(
            "[ OK ] ticket key in use (HJV_API_TICKET_KID 1): the key 1 hbbs takes, {}",
            hjv_api::trust::fingerprint(1, &public)
        )),
        "{report}"
    );
    // the vectors' key 2 as the next key: a public test key, which anyone can sign with
    let vector = hjv_api::trust::entry(2, &hjv_api::devsig::public_key(&NEXT_TICKET_SEED));
    std::fs::write(
        &env,
        format!("{text}HJV_API_TICKET_PUBKEYS={pin},{vector}\n"),
    )
    .unwrap();
    let out = selfcheck(env.clone(), creds.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!out.status.success(), "{report}");
    assert!(
        report.contains(
            "[FAIL] ticket key 2 of HJV_API_TICKET_PUBKEYS is a public test key, whose private key is in the repository"
        ),
        "{report}"
    );
    assert!(
        report.contains("[ OK ] ticket key in use (HJV_API_TICKET_KID 1)"),
        "{report}"
    );
    std::fs::write(&env, format!("{text}HJV_API_TICKET_PUBKEYS={pin},{next}\n")).unwrap();
    // a key access.conf does not take
    std::fs::write(
        creds.join("hjviewer-api-ticket-key"),
        util::b64(&[0x76u8; 32]),
    )
    .unwrap();
    let out = selfcheck(env.clone(), creds.clone()).await.unwrap();
    let report = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(!out.status.success());
    assert!(
        report.contains(
            "is not the key 1 of HJV_API_TICKET_PUBKEYS: hbbs refuses every ticket it signs"
        ),
        "{report}"
    );
    assert!(!report.contains(&util::b64(&[0x76u8; 32])));
    // a credential that is not one seed: not read, and the service would not start
    std::fs::write(creds.join("hjviewer-api-ticket-key"), "AAAA").unwrap();
    let out = selfcheck(env, creds).await.unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout)
        .contains("[FAIL] ticket key in use: credential hjviewer-api-ticket-key"));
    let _ = std::fs::remove_dir_all(&dir);
}
