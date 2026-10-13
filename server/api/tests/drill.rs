// SPDX-License-Identifier: AGPL-3.0-only

//! The apps and PCs of the operations drill (design v3 §5-9, WP-O; tools/e2e/hjv-api-drill.sh,
//! docs/hjv-api-drill.md): the drill script runs these phases inside its test server, a container
//! with systemd, as root, against the installed hjv-api on /run/hjviewer-api.sock and its admin
//! socket, with the fake SSO (`hjv-fake-sso`) and the test keys of tests/common (seal key, list
//! keys 1 and 2, hbbs's key pair). Each phase is one ignored test, so `cargo test` passes them
//! by; what one phase leaves for the next (tokens with their account keys, bindings) is in
//! /var/tmp/hjv-drill/state.json.
//!
//! - `drill_values`: the test keys and the fake SSO's values, for the script to install.
//! - `drill_fake_hbbs`: hbbs's console on 127.0.0.1:21115, answering `hjv-pk` for the test PCs
//!   (their keys follow from their IDs) as on, unless /var/tmp/hjv-drill/off names them; runs
//!   until stopped.
//! - `drill_populate`: a day of use: PCs bound by logins on them and syncing, phones, a released
//!   PC, a failed login.
//! - `drill_sync`: the PCs that are bound sync, and their lists are checked as the PCs check them
//!   (HJV_DRILL_EXPECT: the state the lists must have; `disabled` right after the restore's
//!   start, before the operator's next command).
//! - `drill_invalid_client`: a login the SSO answers with invalid_client.
//! - `drill_after_backup`: what a restore takes back: a new login, a PC released by its owner,
//!   an account's keys revoked (a lost phone).
//! - `drill_after_restore`: after the restore: every token from before refused, the PCs still
//!   bound (switched off), the released PC listed.
//! - `drill_after_on`: the phone logs in again, and the PC's list holds the new key only.
//! - `drill_relay`: Wake-on-LAN (design v3 §8, WP-W2): HONG's PC reports how to wake it, a NAS
//!   relay says hello and is approved by its code for the account of that PC, and the phone wakes
//!   the PC through it: the command, checked with the command keys the relays pin
//!   (HJV_API_COMMAND_PUBKEYS of the server's settings), its result, the confirmation.
//! - `drill_rebuilt`: on a rebuilt host (seal key from its offline copy, list key 2, command key
//!   2): the phone logs in with the same app, the PC's list is signed with key 2 and passes the
//!   PC's checks, and a wake through the relay of the backup is signed with command key 2.

mod common;

use common::*;
use hjv_api::{
    admin::{self, AdminRequest},
    devsig, hbbs, keys, proof,
    relay::{HELLO, HELLO_PATH, POLL, POLL_PATH},
    seal,
    util::{b64, b64url_decode, hex, now, random_bytes, sha256},
};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, UnixStream},
};

const API: &str = "/run/hjviewer-api.sock";
const ADMIN: &str = "/run/hjviewer-api-admin/admin.sock";
const DIR: &str = "/var/tmp/hjv-drill";

/// The PCs: HONG's, KIM's, and one of HONG's that leaves its account at once.
const PC_HONG: &str = "700000001";
const PC_KIM: &str = "700000002";
const PC_GONE: &str = "700000003";

fn state_path() -> String {
    format!("{DIR}/state.json")
}

fn load() -> Value {
    std::fs::read_to_string(state_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_else(|| json!({"tokens": {}, "bindings": {}}))
}

fn save(state: &Value) {
    std::fs::create_dir_all(DIR).unwrap();
    std::fs::write(state_path(), state.to_string()).unwrap();
}

/// One HTTP/1.1 request on hjv-api's socket, as nginx sends it.
async fn request(
    method: &str,
    target: &str,
    headers: &[(&str, String)],
    body: &[u8],
) -> (u16, Value) {
    let mut stream = UnixStream::connect(API).await.expect("hjv-api's socket");
    let mut head = format!(
        "{method} {target} HTTP/1.1\r\nHost: rd.example.com\r\nX-Real-IP: 192.0.2.10\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut answer))
        .await
        .expect("an answer in time")
        .unwrap();
    let answer = String::from_utf8_lossy(&answer).into_owned();
    let status = answer[9..12].parse().unwrap();
    let body = answer.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    (status, serde_json::from_str(body).unwrap_or(Value::Null))
}

async fn admin(argv: &[&str]) -> String {
    admin::send(
        Path::new(ADMIN),
        &AdminRequest {
            argv: argv.iter().map(|a| a.to_string()).collect(),
            reveal: false,
            sudo_user: "drill".into(),
            input: None,
        },
    )
    .await
    .unwrap()
}

fn seal_pk() -> [u8; 32] {
    *crypto_box::SecretKey::from_bytes(SEAL_SECRET)
        .public_key()
        .as_bytes()
}

/// A login as the app sends it, from `device` when given; returns the status, the answer and
/// the account key's seed.
async fn login(user: &str, password: &str, device: Option<&TestDevice>) -> (u16, Value, [u8; 32]) {
    let id = device.map_or("", |d| d.id.as_str());
    let (time, nonce) = (now(), b64(&random_bytes::<16>()));
    let seed = random_bytes::<32>();
    let (ck, cp) = keys::proof(&seed, &nonce, id);
    let field = seal::seal_login(&seal_pk(), password, time, &nonce, id, &ck, &cp);
    let mut body = json!({
        "username": user, "password": field, "id": id,
        "uuid": device.map_or_else(|| b64(&random_bytes::<12>()), |d| d.uuid.clone()),
        "autoLogin": true, "type": "account",
        "deviceInfo": {"os": device.map_or("ios", |d| d.os.as_str()), "type": "client",
                       "name": device.map_or("iPhone15,2", |d| d.hostname.as_str())},
    });
    if let Some(d) = device {
        let sealed = b64url_decode(field.rsplit_once('.').unwrap().1).unwrap();
        let message =
            devsig::login_message(&d.id, time, &nonce, &sha256(&sealed), d.installed, &d.os);
        body["hjv"] = json!({"v": 1, "sig": devsig::sign(&d.seed, &message),
                             "installed": d.installed, "ver": "1.6.0"});
    }
    let (status, answer) = request("POST", "/api/login", &[], body.to_string().as_bytes()).await;
    (status, answer, seed)
}

/// A test account's login, kept in the state under `name`.
async fn login_as(
    state: &mut Value,
    name: &str,
    account: (&str, &str),
    device: Option<&TestDevice>,
) {
    let (status, answer, seed) = login(account.0, account.1, device).await;
    assert_eq!(status, 200, "{answer}");
    state["tokens"][name] = json!({"token": answer["access_token"], "seed": hex(&seed)});
}

fn token_of(state: &Value, name: &str) -> (String, [u8; 32]) {
    let t = &state["tokens"][name];
    let seed: Vec<u8> = (0..32)
        .map(|i| u8::from_str_radix(&t["seed"].as_str().unwrap()[i * 2..i * 2 + 2], 16).unwrap())
        .collect();
    (
        t["token"].as_str().unwrap().to_owned(),
        seed.try_into().unwrap(),
    )
}

/// A request signed by the token's account key, as the app's hook sends it.
async fn signed(
    state: &Value,
    name: &str,
    method: &str,
    target: &str,
    body: &[u8],
) -> (u16, Value) {
    let (token, seed) = token_of(state, name);
    let proof = proof::header_value(
        &seed,
        method,
        target,
        now(),
        &random_bytes::<16>(),
        body,
        &token,
    );
    request(
        method,
        target,
        &[
            ("Authorization", format!("Bearer {token}")),
            (proof::HEADER, proof),
        ],
        body,
    )
    .await
}

/// A PC's sync, the answer's list as the PC checks it.
async fn sync(pc: &TestDevice, binding: &str) -> (u16, Value, Option<Result<Value, String>>) {
    let nonce = random_bytes::<16>();
    let body = sync_body(pc, now(), &nonce, &payload(binding).to_string());
    let (status, answer) = request("POST", hjv_api::pcsync::PATH, &[], body.as_bytes()).await;
    let list = check_list(&answer, pc, binding, &hjv_api::util::b64url(&nonce));
    (status, answer, list)
}

fn pc(id: &str) -> TestDevice {
    TestDevice::windows(id)
}

/// The values the drill puts into the test server: the test keys of tests/common (public test
/// values, never for anything else) as `NAME=value` lines for the script.
#[test]
#[ignore = "the drill's test server only"]
fn drill_values() {
    // after the harness's "test drill_values ... "
    println!();
    println!("SEAL_KEY={}", b64(&SEAL_SECRET));
    println!("LOG_KEY={}", b64(&LOG_KEY));
    println!("LIST_KEY_1={}", b64(&LIST_SEED));
    println!("LIST_KEY_2={}", b64(&NEXT_LIST_SEED));
    println!("HBBS_PUBKEY={}", hbbs_pubkey());
    println!("TRUST_PUBKEYS={}", trust_pubkeys());
    println!("SSO_BASIC={}", hjv_api::fake_sso::BASIC);
    println!("SSO_API_UUID={}", hjv_api::fake_sso::API_UUID);
    println!("SSO_CLIENT_ID={}", hjv_api::fake_sso::CLIENT_ID);
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_fake_hbbs() {
    let listener = TcpListener::bind("127.0.0.1:21115").await.unwrap();
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        tokio::spawn(async move {
            let mut buf = [0u8; 1024];
            let Ok(n) = stream.read(&mut buf).await else {
                return;
            };
            let cmd = String::from_utf8_lossy(&buf[..n]).trim().to_owned();
            let reply = match cmd.split_once(' ') {
                Some(("hjv-pk", id)) if id.starts_with("7000000") => {
                    let off = std::fs::read_to_string(format!("{DIR}/off"))
                        .unwrap_or_default()
                        .lines()
                        .any(|l| l.trim() == id);
                    let key = pc(id).public_key();
                    let signed = hbbs::sign_id_pk(&HBBS_SEED, id, &key);
                    hbbs::format_reply(id, Some(&signed), Some(if off { 4000 } else { 5 }))
                }
                Some(("hjv-pk", id)) => hbbs::format_reply(id, None, None),
                _ => String::new(),
            };
            let _ = stream.write_all(reply.as_bytes()).await;
        });
    }
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_populate() {
    let mut state = load();
    for (name, account, id) in [
        ("pc_hong", HONG, PC_HONG),
        ("pc_kim", KIM, PC_KIM),
        ("pc_gone", HONG, PC_GONE),
    ] {
        login_as(&mut state, name, account, Some(&pc(id))).await;
        admin(&["passwordless", "on", "--owner-of", id]).await;
    }
    login_as(&mut state, "phone_hong", HONG, None).await;
    login_as(&mut state, "phone_lee", LEE, None).await;
    // PC_GONE leaves its account at once: a tombstone
    let (status, answer) = signed(
        &state,
        "pc_gone",
        "DELETE",
        &format!("/api/hjv/devices/{PC_GONE}"),
        b"",
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    // a failed login
    let (status, _, _) = login(HONG.0, "not-the-password", None).await;
    assert_eq!(status, 401);
    // the bindings, from this-device of each PC's login
    for (name, id) in [("pc_hong", PC_HONG), ("pc_kim", PC_KIM)] {
        let (status, answer) = signed(&state, name, "GET", "/api/hjv/this-device", b"").await;
        assert_eq!(status, 200, "{answer}");
        state["bindings"][id] = answer["binding"].clone();
    }
    save(&state);
    sync_all(&state, "active").await;
    println!("populated: 3 PCs bound (1 released), 2 phones, 1 failed login");
}

async fn sync_all(state: &Value, expect: &str) {
    for id in [PC_HONG, PC_KIM] {
        // a PC that cleared its mark (PC_KIM after drill_after_backup) has no list to check
        let Some(binding) = state["bindings"][id].as_str().filter(|b| !b.is_empty()) else {
            continue;
        };
        let (status, answer, list) = sync(&pc(id), binding).await;
        match list {
            Some(Ok(list)) => {
                assert_eq!(list["state"], expect, "{id}: {list}");
                println!(
                    "PC {id}: {status}, list {} with {} keys, signed by key {}",
                    list["state"],
                    list["keys"].as_array().map_or(0, Vec::len),
                    list["kid"]
                );
            }
            other => panic!("{id}: {status} {answer} {other:?}"),
        }
    }
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_sync() {
    let expect = std::env::var("HJV_DRILL_EXPECT").unwrap_or_else(|_| "active".into());
    sync_all(&load(), &expect).await;
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_invalid_client() {
    // the fake SSO refuses this server's client credentials for this name: that login fails like
    // any other, and the SSO calls pause, so that the next login gets the 503 of SSO trouble
    let (status, answer, _) = login("sso-invalid-client", "x", None).await;
    assert_eq!(status, 401, "{answer}");
    let (status, answer, _) = login(KIM.0, KIM.1, None).await;
    assert_eq!(status, 503, "{answer}");
    println!("invalid_client: the login 401, the next one 503 (SSO calls paused)");
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_after_backup() {
    let mut state = load();
    login_as(&mut state, "later_lee", LEE, None).await;
    login_as(&mut state, "phone_kim", KIM, None).await;
    let binding = state["bindings"][PC_KIM].as_str().unwrap().to_owned();
    let (status, answer) = signed(
        &state,
        "phone_kim",
        "DELETE",
        &format!("/api/hjv/devices/{PC_KIM}"),
        b"",
    )
    .await;
    assert_eq!(status, 200, "{answer}");
    // the PC gets its signed revoked and clears its mark
    let (_, _, list) = sync(&pc(PC_KIM), &binding).await;
    assert_eq!(list.unwrap().unwrap()["state"], "revoked");
    state["bindings"][PC_KIM] = json!("");
    save(&state);
    // HONG's phone lost: the account's keys and tokens revoked, so that its PC's list holds none
    println!("{}", admin(&["user", "revoke-all", HONG.0]).await);
    let hong = state["bindings"][PC_HONG].as_str().unwrap();
    let (_, _, list) = sync(&pc(PC_HONG), hong).await;
    let list = list.unwrap().unwrap();
    assert_eq!(list["state"], "active", "{list}");
    assert_eq!(list["keys"].as_array().map_or(0, Vec::len), 0, "{list}");
    println!(
        "after the backup: 2 new logins, PC {PC_KIM} released (signed revoked, mark cleared), HONG's keys revoked (PC {PC_HONG}: active, 0 keys)"
    );
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_after_restore() {
    let state = load();
    let names: Vec<String> = state["tokens"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    for name in &names {
        let (status, _) = signed(&state, name, "POST", "/api/currentUser", b"{}").await;
        assert_eq!(
            status, 401,
            "{name}: a token from before the restore still works"
        );
    }
    println!("{} tokens from before: all 401", names.len());
    // still bound, switched off
    let binding = state["bindings"][PC_HONG].as_str().unwrap();
    let (_, _, list) = sync(&pc(PC_HONG), binding).await;
    let list = list.unwrap().unwrap();
    assert_eq!(list["state"], "disabled", "{list}");
    println!("PC {PC_HONG}: still bound, list {}", list["state"]);
    // bound again on the server, but its mark was cleared: listed, no list
    let (status, answer, list) = sync(&pc(PC_KIM), "").await;
    assert_eq!(answer["state"], "listed", "{status} {answer}");
    assert!(list.is_none());
    println!("PC {PC_KIM}: {status} listed (its mark was cleared before the restore)");
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_after_on() {
    let mut state = load();
    let (_, old_seed) = token_of(&state, "phone_hong");
    login_as(&mut state, "phone_hong_again", HONG, None).await;
    let (_, new_seed) = token_of(&state, "phone_hong_again");
    save(&state);
    let binding = state["bindings"][PC_HONG].as_str().unwrap();
    let (_, _, list) = sync(&pc(PC_HONG), binding).await;
    let list = list.unwrap().unwrap();
    assert_eq!(list["state"], "active", "{list}");
    let keys: Vec<&str> = list["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|k| k["pk"].as_str().unwrap())
        .collect();
    assert!(keys.contains(&b64(&devsig::public_key(&new_seed)).as_str()));
    assert!(!keys.contains(&b64(&devsig::public_key(&old_seed)).as_str()));
    println!(
        "PC {PC_HONG}: active with the new key of the phone's login, not the old one ({} keys)",
        keys.len()
    );
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_rebuilt() {
    let mut state = load();
    // the same app (the seal key's public key built in) logs in on the rebuilt host
    login_as(&mut state, "phone_hong_rebuilt", HONG, None).await;
    save(&state);
    let binding = state["bindings"][PC_HONG].as_str().unwrap();
    let (_, answer, list) = sync(&pc(PC_HONG), binding).await;
    let list = list.unwrap_or_else(|| panic!("no list: {answer}")).unwrap();
    assert_eq!(list["kid"], 2, "{list}");
    assert_eq!(list["state"], "active", "{list}");
    println!("rebuilt host: the phone logged in; PC {PC_HONG}'s list is signed with list key 2 and passes its checks");
    // the relay of the backup polls this host; a wake is signed with command key 2, which the
    // relays pin already
    let (status, answer) = relay_poll(&json!([])).await;
    assert_eq!(
        (status, answer["state"].as_str()),
        (200, Some("active")),
        "{answer}"
    );
    let k = wake_through_relay(&state, "phone_hong_rebuilt", binding).await;
    assert_eq!(k, 2);
    println!("rebuilt host: the relay of the backup took a wake command signed with command key 2");
}

/// The drill's NAS relay: hjv-relay's key of the seed [5; 32], its card on the PCs' network.
fn relay() -> TestRelay {
    TestRelay::new([5; 32])
}

fn relay_lan() -> Value {
    json!([iface(
        "eth0",
        "192.0.2.10/24",
        Some("192.0.2.1"),
        Some("00:00:5e:00:53:01")
    )])
}

async fn relay_poll(results: &Value) -> (u16, Value) {
    let p = poll_payload(0, results, None, &relay_lan());
    let body = relay().body(&POLL, now(), &p);
    request("POST", POLL_PATH, &[], body.as_bytes()).await
}

/// The command keys a relay of this server is built with: HJV_API_COMMAND_PUBKEYS of its
/// settings (the relay build's HJV_COMMAND_PUBKEY).
fn pinned_command_keys() -> Vec<(u32, [u8; 32])> {
    let env = hjv_api::config::read_env_file(Path::new("/etc/hjviewer/hjviewer-api.env")).unwrap();
    hjv_api::trust::parse_pubkeys(
        env.get("HJV_API_COMMAND_PUBKEYS")
            .map_or("", String::as_str),
    )
    .unwrap()
}

/// The PC is off for hbbs's console (drill_fake_hbbs) while `off` is true.
fn hbbs_off(off: bool) {
    let path = format!("{DIR}/off");
    if off {
        std::fs::write(path, format!("{PC_HONG}\n")).unwrap();
    } else {
        let _ = std::fs::remove_file(path);
    }
}

/// HONG's PC asleep (off for hbbs, its last sync half a minute ago) woken by the phone `name`
/// through the relay: the command as the relay checks it, twelve packets, the PC's sync after
/// them, the confirmation. Returns the number of the command key that signed it.
async fn wake_through_relay(state: &Value, name: &str, binding: &str) -> i64 {
    hbbs_off(true);
    tokio::time::sleep(Duration::from_secs(31)).await;
    let (status, answer) = signed(
        state,
        name,
        "POST",
        &format!("/api/hjv/devices/{PC_HONG}/wake"),
        b"",
    )
    .await;
    assert_eq!(status, 202, "{answer}");
    let job = answer["job"].as_str().unwrap().to_owned();
    println!(
        "wake: 202 {}, relay \"{}\" ({})",
        answer["state"].as_str().unwrap_or("?"),
        answer["relay"]["name"].as_str().unwrap_or("?"),
        answer["relay"]["match"].as_str().unwrap_or("?")
    );
    let (_, answer) = relay_poll(&json!([])).await;
    let entry = &answer["commands"][0];
    let (c, s, k) = (
        entry["c"]
            .as_str()
            .unwrap_or_else(|| panic!("no command: {answer}")),
        entry["s"].as_str().unwrap(),
        entry["k"].as_i64().unwrap(),
    );
    let key = pinned_command_keys()
        .into_iter()
        .find(|(kid, _)| i64::from(*kid) == k)
        .map(|(_, key)| key)
        .expect("a key the relays pin");
    let message = [&b"hjv-wake-v1\0"[..], c.as_bytes()].concat();
    assert!(devsig::verify(&key, &message, s), "the command's signature");
    let command: Value = serde_json::from_str(c).unwrap();
    assert_eq!(
        (
            command["job"].as_str(),
            command["relay"].as_str(),
            command["net"].as_str()
        ),
        (
            Some(job.as_str()),
            Some(relay().id.as_str()),
            Some("192.0.2.0/24")
        )
    );
    let result = json!([{"cmd": hex(&sha256(c.as_bytes())), "job": job, "sent_at": now(),
                         "iface": "eth0", "packets": 12, "error": null}]);
    let (status, _) = relay_poll(&result).await;
    assert_eq!(status, 200);
    // the PC wakes and syncs; hjv-api's ticker confirms within seconds
    hbbs_off(false);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (status, _, _) = sync(&pc(PC_HONG), binding).await;
    assert_eq!(status, 200);
    let mut last = Value::Null;
    for _ in 0..30 {
        let (_, answer) = signed(state, name, "GET", &format!("/api/hjv/wake/{job}"), b"").await;
        if answer["state"] == "confirmed" {
            println!("wake: confirmed, {} packets", answer["packets"]);
            return k;
        }
        last = answer;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    panic!("not confirmed: {last}");
}

#[tokio::test]
#[ignore = "the drill's test server only"]
async fn drill_relay() {
    let state = load();
    let binding = state["bindings"][PC_HONG].as_str().unwrap().to_owned();
    // the PC reports how to wake it, in a sync of its binding
    let wol = wol_report(
        "00:00:5e:00:53:10",
        "192.0.2.67/24",
        "192.0.2.1",
        Some("00:00:5e:00:53:01"),
    );
    let p = payload_with_wol(&binding, &wol, "drill");
    let body = sync_body(&pc(PC_HONG), now(), &random_bytes::<16>(), &p.to_string());
    let (status, answer) = request("POST", hjv_api::pcsync::PATH, &[], body.as_bytes()).await;
    assert_eq!(status, 200, "{answer}");
    // the NAS's relay says hello, and is approved by its code for the account of the PC
    let r = relay();
    let body = r.body(&HELLO, now(), &hello_payload(&relay_lan()));
    let (status, answer) = request("POST", HELLO_PATH, &[], body.as_bytes()).await;
    assert_eq!(
        (status, answer["state"].as_str()),
        (200, Some("pending")),
        "{answer}"
    );
    println!(
        "relay hello: pending, code {}",
        answer["code"].as_str().unwrap_or("?")
    );
    let out = admin(&["relay", "approve", &r.code, "--owner-of", PC_HONG]).await;
    print!("{out}");
    assert!(out.contains("on the PC's network: yes"), "{out}");
    assert!(
        !out.contains("192.0.2."),
        "no address in the approval tool's output"
    );
    let (status, answer) = relay_poll(&json!([])).await;
    assert_eq!(
        (status, answer["state"].as_str()),
        (200, Some("active")),
        "{answer}"
    );
    let k = wake_through_relay(&state, "phone_hong", &binding).await;
    assert_eq!(k, 1);
    print!("{}", admin(&["relay", "list"]).await);
}
