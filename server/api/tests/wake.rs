// SPDX-License-Identifier: AGPL-3.0-only

//! Waking a PC (design v3 §8-2, §8-4, §8-10): the PC's wake report in its sync, the app's paths
//! in their order with every answer, the relay's choice by its grades, the command as the relay
//! checks it (relay/PROTOCOL.md §7, the shared key of §13), the job's states with the relay's
//! results and the confirmation from hbbs and the PC's sync, the limits, the verdicts in the
//! device list, and what goes with an account or past its time.

mod common;

use common::*;
use hjv_api::{
    db, devsig,
    util::{b64, hex, now, sha256},
    wake,
};
use serde_json::{json, Value};

const RELAY_IP: &str = "198.51.100.20";
const PC_IP: &str = "198.51.100.40";
const PC_MAC: &str = "00:00:5e:00:53:10";

fn lan() -> Value {
    json!([iface(
        "eth0",
        "192.0.2.10/24",
        Some("192.0.2.1"),
        Some("00:00:5e:00:53:01")
    )])
}

async fn sql(h: &Harness, statement: String) {
    h.state
        .db
        .call(move |c| c.execute_batch(&statement))
        .await
        .unwrap();
}

async fn count(h: &Harness, sql: String) -> i64 {
    h.state
        .db
        .call(move |c| c.query_row(&sql, [], |r| r.get(0)))
        .await
        .unwrap()
}

async fn job_row(h: &Harness, job: &str) -> db::Job {
    let job = job.to_owned();
    h.state
        .db
        .call(move |c| db::job(c, &job))
        .await
        .unwrap()
        .unwrap()
}

/// A PC of `account` bound by a login on it, with a wake report on `addr` from its sync.
async fn reported(
    h: &Harness,
    account: (&str, &str),
    id: &str,
    addr: &str,
    gateway: &str,
    ip: &str,
) -> (TestDevice, String) {
    let pc = TestDevice::windows(id);
    h.hbbs.register(&pc, Some(100));
    h.login_ok(account, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    let wol = wol_report(PC_MAC, addr, gateway, Some("00:00:5e:00:53:01"));
    let s = h
        .sync_from(&pc, &payload_with_wol(&binding, &wol, "h1"), ip)
        .await;
    assert_eq!(s.answer.status, 200, "{}", s.answer.text);
    (pc, binding)
}

/// The PC is asleep: hbbs last saw it 100 seconds ago, and it last synced as long ago.
async fn asleep(h: &Harness, pc: &TestDevice) {
    h.hbbs.register(pc, Some(100));
    sql(
        h,
        format!(
            "UPDATE devices SET last_sync_at = {} WHERE device_id = '{}'",
            now() - 100,
            pc.id
        ),
    )
    .await;
}

/// A relay approved for the account of `pc`, online (it just polled).
async fn approved(h: &Harness, seed: u8, ifaces: &Value, ip: &str, pc: &TestDevice) -> TestRelay {
    let r = TestRelay::new([seed; 32]);
    assert_eq!(h.hello(&r, ip, ifaces).await.json["state"], "pending");
    h.admin(
        &["relay", "approve", &r.code, "--owner-of", &pc.id, "--force"],
        false,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        h.poll(&r, ip, 0, &json!([]), None, ifaces).await.json["state"],
        "active"
    );
    r
}

async fn wake_pc(h: &Harness, id: &str, token: &str, body: Option<Value>) -> Answer {
    h.post(&format!("/api/hjv/devices/{id}/wake"), token, body)
        .await
}

/// The commands of a poll answer: (the command, its text, its signature, its key number).
fn commands(a: &Answer) -> Vec<(Value, String, String, i64)> {
    a.json["commands"]
        .as_array()
        .unwrap_or_else(|| panic!("no commands: {}", a.text))
        .iter()
        .map(|e| {
            let c = e["c"].as_str().unwrap().to_owned();
            (
                serde_json::from_str(&c).unwrap(),
                c,
                e["s"].as_str().unwrap().to_owned(),
                e["k"].as_i64().unwrap(),
            )
        })
        .collect()
}

/// A result as the relay sends it (relay/PROTOCOL.md §8.1).
fn result(c: &str, job: &str, sent_at: Option<i64>, packets: u32, error: Option<&str>) -> Value {
    json!({
        "cmd": hex(&sha256(c.as_bytes())), "job": job, "sent_at": sent_at,
        "iface": "eth0", "packets": packets, "error": error,
    })
}

#[tokio::test]
async fn a_pc_is_woken_through_its_relay() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000001", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    let phone = h.login_ok(HONG, None).await;
    // another account's PC, a job, are not there for it
    let kim = h.login_ok(KIM, None).await;
    assert_eq!(wake_pc(&h, &pc.id, &kim, None).await.status, 404);
    assert_eq!(
        h.get(&format!("/api/hjv/devices/{}/wol", pc.id), &kim)
            .await
            .status,
        404
    );
    // the token alone opens nothing
    let a = h
        .request(
            hyper::Method::POST,
            &format!("/api/hjv/devices/{}/wake", pc.id),
            &[("authorization", &format!("Bearer {phone}"))],
            None,
        )
        .await;
    assert_eq!(a.status, 401);
    // it just synced: on already
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["hjv"].as_str(), a.json["error"].as_str()),
        (409, Some("online"), Some("이미 켜져 있습니다."))
    );
    asleep(&h, &pc).await;
    // hbbs saw it register 10 seconds ago: on as well
    h.hbbs.register(&pc, Some(10));
    assert_eq!(
        wake_pc(&h, &pc.id, &phone, None).await.json["hjv"],
        "online"
    );
    h.hbbs.register(&pc, Some(100));
    // no relay in the account
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json.clone()),
        (
            409,
            json!({"error": "이 PC와 같은 네트워크에 켜져 있는 중계기가 없습니다.", "hjv": "no_relay",
                   "reason": "no_relays", "any_relay": false})
        )
    );
    // a relay on the PC's network, approved, never polled: offline
    let r = TestRelay::new([31; 32]);
    h.hello(&r, RELAY_IP, &lan()).await;
    h.admin(
        &["relay", "approve", &r.code, "--owner-of", &pc.id],
        false,
        None,
    )
    .await
    .unwrap();
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (
            a.status,
            a.json["hjv"].as_str(),
            a.json["last_seen"].clone()
        ),
        (503, Some("relay_offline"), Value::Null)
    );
    // it polls: the job is queued for it
    h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(a.status, 202, "{}", a.text);
    let job = a.json["job"].as_str().unwrap().to_owned();
    assert_eq!(job.len(), 32);
    assert_eq!(
        a.json,
        json!({"job": job, "state": "queued",
               "relay": {"name": hjv_api::relay::default_name(&r.id), "match": "exact"}, "hints": []})
    );
    // again: the job under way
    let again = wake_pc(&h, &pc.id, &phone, Some(json!({}))).await;
    assert_eq!(
        (again.status, again.json["job"].as_str()),
        (202, Some(job.as_str()))
    );
    // the relay's next poll gets the command, signed with the command key over its text
    let a = h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await;
    let got = commands(&a);
    assert_eq!(got.len(), 1);
    let (c, text, s, k) = got[0].clone();
    let iat = c["iat"].as_i64().unwrap();
    assert!((now() - iat).abs() <= 2);
    assert_eq!(
        c,
        json!({"v": 1, "job": job, "relay": r.id, "iat": iat, "exp": iat + 60,
               "net": "192.0.2.0/24", "macs": [PC_MAC], "ports": [9, 7], "repeat": 3, "gap_ms": 200})
    );
    assert_eq!(
        text,
        wake::command_text(&job, &r.id, iat, "192.0.2.0/24", &[PC_MAC.to_owned()])
    );
    assert_eq!(k, 1);
    let message = [&b"hjv-wake-v1\0"[..], text.as_bytes()].concat();
    assert!(devsig::verify(
        &devsig::public_key(&COMMAND_SEED),
        &message,
        &s
    ));
    // the same bytes again until a result names it
    let a = h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await;
    assert_eq!(commands(&a)[0], (c.clone(), text.clone(), s.clone(), k));
    // the result: sent, and the command leaves the queue (the command was issued 20 seconds ago,
    // its packets went 10 seconds ago)
    sql(
        &h,
        format!("UPDATE wake_jobs SET created_at = created_at - 20 WHERE job = '{job}'"),
    )
    .await;
    let sent_at = now() - 10;
    let results = json!([result(&text, &job, Some(sent_at), 12, None)]);
    let a = h.poll(&r, RELAY_IP, 0, &results, None, &lan()).await;
    assert_eq!(a.json["commands"], json!([]));
    let status = h.get(&format!("/api/hjv/wake/{job}"), &phone).await;
    assert_eq!(
        status.json,
        json!({"job": job, "state": "sent", "sent_at": sent_at, "confirmed_at": null, "packets": 12,
               "error": null, "relay": {"name": hjv_api::relay::default_name(&r.id), "match": "exact"},
               "hints": []})
    );
    // results are taken idempotently
    h.poll(&r, RELAY_IP, 0, &results, None, &lan()).await;
    assert_eq!(job_row(&h, &job).await.sent_at, Some(sent_at));
    // the PC registers with hbbs: confirmed
    assert_eq!(wake::tick(&h.state).await, wake::Ticked::default());
    h.hbbs.register(&pc, Some(1));
    assert_eq!(wake::tick(&h.state).await.confirmed, 1);
    let status = h.get(&format!("/api/hjv/wake/{job}"), &phone).await;
    assert_eq!(status.json["state"], "confirmed");
    let confirmed_at = status.json["confirmed_at"].as_i64().unwrap();
    assert!(confirmed_at > sent_at && confirmed_at <= now());
    // the record: the account's tag, the PC, the relay, the grade, the times
    let row = job_row(&h, &job).await;
    assert_eq!(
        (
            row.account_tag,
            row.device_id.as_str(),
            row.relay_id.as_str(),
            row.grade.as_str(),
            row.net.as_str()
        ),
        (
            h.account_tag(HONG.0),
            pc.id.as_str(),
            r.id.as_str(),
            "exact",
            "192.0.2.0/24"
        )
    );
    // once a minute per PC; the limit comes before the relay, here one that stopped polling
    h.hbbs.register(&pc, Some(100));
    sql(
        &h,
        format!("UPDATE relays SET last_poll_at = {}", now() - 600),
    )
    .await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["hjv"].as_str()),
        (429, Some("rate_limited"))
    );
    assert!((1..=60).contains(&a.json["retry_after"].as_i64().unwrap()));
    // another account cannot read the job; a job that is not one is not there
    assert_eq!(
        h.get(&format!("/api/hjv/wake/{job}"), &kim).await.status,
        404
    );
    for bad in ["x", &"A".repeat(32), &"0".repeat(31)] {
        assert_eq!(
            h.get(&format!("/api/hjv/wake/{bad}"), &phone).await.status,
            404
        );
    }
    // no MAC ever goes to an app
    for a in [&status, &again] {
        assert!(!a.text.contains("00:00:5e"), "{}", a.text);
    }
    // the methods
    let a = h
        .get(&format!("/api/hjv/devices/{}/wake", pc.id), &phone)
        .await;
    assert_eq!(
        (a.status, a.headers["allow"].to_str().unwrap()),
        (405, "POST")
    );
    let a = h
        .delete(&format!("/api/hjv/devices/{}/wol", pc.id), &phone)
        .await;
    assert_eq!(
        (a.status, a.headers["allow"].to_str().unwrap()),
        (405, "GET")
    );
    let a = wake_pc(&h, &pc.id, &phone, Some(json!([]))).await;
    assert_eq!(a.status, 400);
}

/// A new job of the PC: the one before moves back a minute, out of the PC's limit.
async fn next_job(h: &Harness, pc: &TestDevice, phone: &str) -> String {
    sql(
        h,
        format!(
            "UPDATE wake_jobs SET created_at = created_at - 61 WHERE device_id = '{}'",
            pc.id
        ),
    )
    .await;
    let a = wake_pc(h, &pc.id, phone, None).await;
    assert_eq!(a.status, 202, "{}", a.text);
    a.json["job"].as_str().unwrap().to_owned()
}

/// The relay's next poll: the text of the (last) command it gets.
async fn delivered(h: &Harness, r: &TestRelay) -> String {
    let a = h.poll(r, RELAY_IP, 0, &json!([]), None, &lan()).await;
    commands(&a).pop().unwrap().1
}

#[tokio::test]
async fn jobs_fail_with_the_relays_error_or_no_result() {
    let h = Harness::start().await;
    let (pc, binding) = reported(&h, HONG, "524000002", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    let r = approved(&h, 32, &lan(), RELAY_IP, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    // the relay's refusals the apps have texts for
    for code in [
        "no_iface",
        "send_failed",
        "expired",
        "not_active",
        "rate_limited",
    ] {
        let job = next_job(&h, &pc, &phone).await;
        let text = delivered(&h, &r).await;
        let a = h
            .poll(
                &r,
                RELAY_IP,
                0,
                &json!([result(&text, &job, None, 0, Some(code))]),
                None,
                &lan(),
            )
            .await;
        assert_eq!(a.json["commands"], json!([]), "{code}");
        let failed = |status: &Answer| {
            assert_eq!(
                (
                    status.json["state"].as_str(),
                    status.json["error"].as_str(),
                    status.json["packets"].as_i64(),
                    status.json["sent_at"].clone()
                ),
                (Some("failed"), Some(code), Some(0), Value::Null),
                "{code}"
            );
        };
        failed(&h.get(&format!("/api/hjv/wake/{job}"), &phone).await);
        // the relay's refusal is its word on the job: a result that comes after it and says the
        // packets went changes nothing
        h.poll(
            &r,
            RELAY_IP,
            0,
            &json!([result(&text, &job, Some(now()), 12, None)]),
            None,
            &lan(),
        )
        .await;
        failed(&h.get(&format!("/api/hjv/wake/{job}"), &phone).await);
    }
    // partial without a packet: nothing went out, so the job failed and is not waited for
    let job = next_job(&h, &pc, &phone).await;
    let text = delivered(&h, &r).await;
    let a = h
        .poll(
            &r,
            RELAY_IP,
            0,
            &json!([result(&text, &job, Some(now()), 0, Some("partial"))]),
            None,
            &lan(),
        )
        .await;
    assert_eq!(a.json["commands"], json!([]));
    let row = job_row(&h, &job).await;
    assert_eq!(
        (
            row.state.as_str(),
            row.error.as_deref(),
            row.packets,
            row.sent_at
        ),
        ("failed", Some("partial"), Some(0), None)
    );
    // partial, with packets: sent; replayed: nothing new
    let job = next_job(&h, &pc, &phone).await;
    let text = delivered(&h, &r).await;
    h.poll(
        &r,
        RELAY_IP,
        0,
        &json!([result(&text, &job, None, 0, Some("replayed"))]),
        None,
        &lan(),
    )
    .await;
    assert_eq!(job_row(&h, &job).await.state, "queued");
    // (issued 20 seconds ago, sent 5 seconds ago: what confirms it at the end comes later)
    sql(
        &h,
        format!("UPDATE wake_jobs SET created_at = created_at - 20 WHERE job = '{job}'"),
    )
    .await;
    h.poll(
        &r,
        RELAY_IP,
        0,
        &json!([result(&text, &job, Some(now() - 5), 6, Some("partial"))]),
        None,
        &lan(),
    )
    .await;
    let row = job_row(&h, &job).await;
    assert_eq!((row.state.as_str(), row.packets), ("sent", Some(6)));
    // within its minute the job that waits for the PC is the answer to another request; a minute
    // after it the packets may be sent again, as the new jobs below, and it keeps waiting
    let again = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (
            again.status,
            again.json["job"].as_str(),
            again.json["state"].as_str(),
            again.json["packets"].as_i64()
        ),
        (202, Some(job.as_str()), Some("sent"), Some(6))
    );
    let waiting = job;
    // a refusal that says the relay does not trust the key: failed, and the journal hears of it
    let job = next_job(&h, &pc, &phone).await;
    assert_ne!(job, waiting);
    assert_eq!(job_row(&h, &waiting).await.state, "sent");
    let text = delivered(&h, &r).await;
    h.poll(
        &r,
        RELAY_IP,
        0,
        &json!([result(&text, &job, None, 0, Some("unknown_key"))]),
        None,
        &lan(),
    )
    .await;
    assert_eq!(
        job_row(&h, &job).await.error.as_deref(),
        Some("unknown_key")
    );
    assert!(hjv_api::log::captured().iter().any(|l| l.contains(
        "refused a wake command (unknown_key): it does not trust the command key in use"
    )));
    // refusals of a relay's own naming fail their jobs too, and share one line an hour
    for code in ["made_up_one", "made_up_two", "made_up_three"] {
        let job = next_job(&h, &pc, &phone).await;
        let text = delivered(&h, &r).await;
        h.poll(
            &r,
            RELAY_IP,
            0,
            &json!([result(&text, &job, None, 0, Some(code))]),
            None,
            &lan(),
        )
        .await;
        assert_eq!(job_row(&h, &job).await.error.as_deref(), Some(code));
    }
    let lines = hjv_api::log::captured();
    let made_up: Vec<&String> = lines
        .iter()
        .filter(|l| l.contains("refused a wake command (made_up_"))
        .collect();
    assert_eq!(made_up.len(), 1, "{made_up:?}");
    // a result of a command hjv-api does not know changes nothing
    let a = h
        .poll(
            &r,
            RELAY_IP,
            0,
            &json!([result("{}", &"f".repeat(32), Some(now()), 12, None)]),
            None,
            &lan(),
        )
        .await;
    assert_eq!(a.status, 200);
    // no result within the command's life and the grace after: no_result; the command goes
    let job = next_job(&h, &pc, &phone).await;
    let text = delivered(&h, &r).await;
    sql(
        &h,
        format!(
            "UPDATE wake_jobs SET exp = {} WHERE job = '{job}'",
            now() - wake::RESULT_GRACE - 1
        ),
    )
    .await;
    assert_eq!(wake::tick(&h.state).await.no_result, 1);
    let row = job_row(&h, &job).await;
    assert_eq!(
        (row.state.as_str(), row.error.as_deref()),
        ("failed", Some("no_result"))
    );
    assert_eq!(
        count(
            &h,
            format!("SELECT COUNT(*) FROM wake_commands WHERE job = '{job}'")
        )
        .await,
        0
    );
    // a result that comes after still counts
    h.poll(
        &r,
        RELAY_IP,
        0,
        &json!([result(&text, &job, Some(now() - 50), 12, None)]),
        None,
        &lan(),
    )
    .await;
    let row = job_row(&h, &job).await;
    assert_eq!(
        (row.state.as_str(), row.error, row.packets),
        ("sent", None, Some(12))
    );
    // no sign of the PC for five minutes after the packets: not confirmed, with the hints (it
    // last synced before them)
    sql(
        &h,
        format!(
            "UPDATE wake_jobs SET sent_at = {} WHERE job = '{job}';
             UPDATE devices SET last_sync_at = {} WHERE device_id = '{}'",
            now() - wake::CONFIRM_FOR - 1,
            now() - 1000,
            pc.id
        ),
    )
    .await;
    assert_eq!(wake::tick(&h.state).await.not_confirmed, 1);
    let status = h.get(&format!("/api/hjv/wake/{job}"), &phone).await;
    assert_eq!(
        (status.json["state"].as_str(), status.json["error"].clone()),
        (Some("not_confirmed"), Value::Null)
    );
    // the PC's sync after the packets confirms too (issued 20 seconds ago, sent 5 seconds ago)
    let job = next_job(&h, &pc, &phone).await;
    let text = delivered(&h, &r).await;
    sql(
        &h,
        format!("UPDATE wake_jobs SET created_at = created_at - 20 WHERE job = '{job}'"),
    )
    .await;
    h.poll(
        &r,
        RELAY_IP,
        0,
        &json!([result(&text, &job, Some(now() - 5), 12, None)]),
        None,
        &lan(),
    )
    .await;
    // hbbs holds another key for it (set up again): its registration does not confirm
    h.hbbs.register_key(&pc.id, devsig::public_key(&[77; 32]));
    assert_eq!(wake::tick(&h.state).await.confirmed, 0);
    let s = h.sync_from(&pc, &payload(&binding), PC_IP).await;
    assert_eq!(s.answer.status, 200);
    // each job that waited for the PC is confirmed on its own: this one, and the one whose
    // packets went out earlier
    assert_eq!(wake::tick(&h.state).await.confirmed, 2);
    for id in [&job, &waiting] {
        let row = job_row(&h, id).await;
        assert_eq!(row.state, "confirmed");
        assert!(row.confirmed_at.unwrap() > row.sent_at.unwrap());
    }
}

#[tokio::test]
async fn the_relay_is_chosen_by_the_pcs_network() {
    let h = Harness::start().await;
    let phone = h.login_ok(HONG, None).await;
    // a private network: the same site only with the same egress address
    let (pc, _) = reported(
        &h,
        HONG,
        "524000003",
        "192.168.0.20/24",
        "192.168.0.1",
        PC_IP,
    )
    .await;
    asleep(&h, &pc).await;
    let home = json!([iface("eth0", "192.168.0.5/24", Some("192.168.0.1"), None)]);
    let r = approved(&h, 33, &home, RELAY_IP, &pc).await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (
            a.status,
            a.json["hjv"].as_str(),
            a.json["reason"].as_str(),
            a.json["any_relay"].as_bool()
        ),
        (409, Some("no_relay"), Some("different_egress"), Some(false))
    );
    // the user may still ask to try all relays
    let a = wake_pc(&h, &pc.id, &phone, Some(json!({"any_relay": true}))).await;
    assert_eq!(
        (a.status, a.json["relay"]["match"].as_str()),
        (202, Some("any"))
    );
    sql(&h, "DELETE FROM wake_jobs".to_owned()).await;
    // the relay behind the same address: exact
    h.poll(&r, PC_IP, 0, &json!([]), None, &home).await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["relay"]["match"].as_str()),
        (202, Some("exact"))
    );
    sql(&h, "DELETE FROM wake_jobs".to_owned()).await;
    // a public network with another gateway: not confirmed, worth trying all relays
    let (pc2, _) = reported(
        &h,
        HONG,
        "524000004",
        "203.0.113.67/24",
        "203.0.113.1",
        PC_IP,
    )
    .await;
    asleep(&h, &pc2).await;
    let other_gw = json!([iface("eth0", "203.0.113.5/24", Some("203.0.113.254"), None)]);
    let s = approved(&h, 34, &other_gw, RELAY_IP, &pc2).await;
    let a = wake_pc(&h, &pc2.id, &phone, None).await;
    assert_eq!(
        (
            a.json["hjv"].as_str(),
            a.json["reason"].as_str(),
            a.json["any_relay"].as_bool()
        ),
        (Some("no_relay"), Some("unconfirmed"), Some(true))
    );
    let a = wake_pc(&h, &pc2.id, &phone, Some(json!({"any_relay": true}))).await;
    assert_eq!(
        (a.status, a.json["relay"]["match"].as_str()),
        (202, Some("any"))
    );
    let job = a.json["job"].as_str().unwrap().to_owned();
    assert_eq!(job_row(&h, &job).await.relay_id, s.id);
    // the gateway's MAC is enough on a wire whose networks overlap
    let (pc3, _) = reported(
        &h,
        HONG,
        "524000005",
        "198.51.100.67/23",
        "198.51.100.1",
        PC_IP,
    )
    .await;
    asleep(&h, &pc3).await;
    let by_mac = json!([iface(
        "eth0",
        "198.51.100.5/24",
        Some("198.51.100.254"),
        Some("00:00:5e:00:53:01")
    )]);
    approved(&h, 35, &by_mac, RELAY_IP, &pc3).await;
    let a = wake_pc(&h, &pc3.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["relay"]["match"].as_str()),
        (202, Some("overlap"))
    );
    let job = a.json["job"].as_str().unwrap().to_owned();
    assert_eq!(job_row(&h, &job).await.net, "198.51.100.0/23");
    // the relay that would do last polled long ago: offline, since when
    let last_seen = now() - 600;
    sql(
        &h,
        format!(
            "UPDATE relays SET last_poll_at = {last_seen} WHERE relay_id = '{}'",
            r.id
        ),
    )
    .await;
    sql(&h, "DELETE FROM wake_jobs".to_owned()).await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (
            a.status,
            a.json["hjv"].as_str(),
            a.json["last_seen"].as_i64()
        ),
        (503, Some("relay_offline"), Some(last_seen))
    );
}

#[tokio::test]
async fn wakes_are_limited_per_account_and_relay() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000006", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    let r = approved(&h, 36, &lan(), RELAY_IP, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let insert = |n: usize, device: &'static str, relay: String| {
        let rows: Vec<String> = (0..n)
            .map(|i| {
                format!(
                    "INSERT INTO wake_jobs (job, account_id, account_tag, device_id, relay_id, grade, net, cmd, created_at, exp, state)
                     VALUES ('{:032x}{device}', 1, 't', '{device}', '{relay}', 'exact', '192.0.2.0/24', 'c{i}{device}', {}, 0, 'failed');",
                    i,
                    now() - 30
                )
            })
            .collect();
        rows.join("\n")
    };
    // ten of the relay's in the last minute
    sql(&h, insert(10, "524000099", r.id.clone())).await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["hjv"].as_str()),
        (429, Some("rate_limited"))
    );
    // twenty of the account's in the last hour
    sql(&h, insert(10, "524000098", "r-other".to_owned())).await;
    sql(
        &h,
        format!("UPDATE wake_jobs SET created_at = {}", now() - 3000),
    )
    .await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["hjv"].as_str()),
        (429, Some("rate_limited"))
    );
    let retry = a.json["retry_after"].as_i64().unwrap();
    assert!((599..=601).contains(&retry), "{retry}");
    sql(
        &h,
        format!("UPDATE wake_jobs SET created_at = {}", now() - 3601),
    )
    .await;
    assert_eq!(wake_pc(&h, &pc.id, &phone, None).await.status, 202);
}

/// A refused wake makes no job and so no limit counts it: the journal says so once per account in
/// ten minutes, not once per request (the release review's server-wake-refusal-unlimited).
#[tokio::test]
async fn refused_wakes_do_not_fill_the_journal() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000007", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    for _ in 0..5 {
        let a = wake_pc(&h, &pc.id, &phone, None).await;
        assert_eq!((a.status, a.json["hjv"].as_str()), (409, Some("no_relay")));
    }
    // (the tests of this file run at once and log under the same account tags: the PC's tag is
    // this test's own)
    let dev = h.state.tags.device(&pc.id);
    let lines = hjv_api::log::captured()
        .iter()
        .filter(|l| l.starts_with("wake: no relay") && l.contains(&format!("dev={dev}")))
        .count();
    assert_eq!(lines, 1);
}

/// Nor do they make a console request each: hbbs hears of an account's PCs LOOKUPS_PER_MINUTE
/// times a minute at most. Past that a PC that has not synced counts as off, and the answer is
/// the one for a PC that is off; another account's request still asks.
#[tokio::test]
async fn refused_wakes_ask_hbbs_a_few_times_a_minute() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000018", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let queries = || h.hbbs.queries.load(std::sync::atomic::Ordering::SeqCst);
    let asked = queries();
    for _ in 0..wake::LOOKUPS_PER_MINUTE + 5 {
        let a = wake_pc(&h, &pc.id, &phone, None).await;
        assert_eq!((a.status, a.json["hjv"].as_str()), (409, Some("no_relay")));
    }
    assert_eq!(queries(), asked + wake::LOOKUPS_PER_MINUTE);
    // hbbs has the PC on now, but the account's lookups are spent: off, without a question
    h.hbbs.register(&pc, Some(10));
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!((a.status, a.json["hjv"].as_str()), (409, Some("no_relay")));
    assert_eq!(queries(), asked + wake::LOOKUPS_PER_MINUTE);
    // a PC that synced just now is on without one
    sql(
        &h,
        format!(
            "UPDATE devices SET last_sync_at = {} WHERE device_id = '{}'",
            now(),
            pc.id
        ),
    )
    .await;
    assert_eq!(
        wake_pc(&h, &pc.id, &phone, None).await.json["hjv"],
        "online"
    );
    // another account's lookups are its own
    let (theirs, _) = reported(&h, KIM, "524000019", "192.0.2.68/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &theirs).await;
    h.hbbs.register(&theirs, Some(10));
    let kim = h.login_ok(KIM, None).await;
    let asked = queries();
    assert_eq!(
        wake_pc(&h, &theirs.id, &kim, None).await.json["hjv"],
        "online"
    );
    assert_eq!(queries(), asked + 1);
}

/// The PC's limit, and the job under way that answers a second request, are an account's own: a
/// job that another account made for the same PC ID (a PC that changed hands within the minute) is
/// neither this account's answer nor its wait.
#[tokio::test]
async fn another_accounts_job_of_the_pc_is_not_this_accounts() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000016", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    approved(&h, 47, &lan(), RELAY_IP, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    // KIM's job for this PC ID, ten seconds ago, its packets sent: under way
    let theirs = "f".repeat(32);
    sql(
        &h,
        format!(
            "INSERT INTO wake_jobs (job, account_id, account_tag, device_id, relay_id, grade, net, cmd, created_at, exp, state, sent_at, packets)
             SELECT '{theirs}', MAX(id), 't', '{}', 'r-theirs', 'exact', '192.0.2.0/24', 'c-theirs', {}, {}, 'sent', {}, 12 FROM accounts",
            pc.id,
            now() - 10,
            now() + 50,
            now() - 5
        ),
    )
    .await;
    assert_eq!(
        h.get(&format!("/api/hjv/wake/{theirs}"), &kim).await.json["state"],
        "sent"
    );
    // HONG's request gets a job of its own, queued: not theirs (whose ID it could not even ask
    // about), and no wait for theirs
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["state"].as_str()),
        (202, Some("queued")),
        "{}",
        a.text
    );
    let job = a.json["job"].as_str().unwrap().to_owned();
    assert_ne!(job, theirs);
    assert_eq!(job_row(&h, &job).await.account_tag, h.account_tag(HONG.0));
    assert_eq!(
        h.get(&format!("/api/hjv/wake/{job}"), &phone).await.status,
        200
    );
    // and then its own job is the one under way
    let again = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (again.status, again.json["job"].as_str()),
        (202, Some(job.as_str()))
    );
    assert_eq!(
        h.get(&format!("/api/hjv/wake/{theirs}"), &phone)
            .await
            .status,
        404
    );
}

/// The command names the PC's cards on the network that matched, and no other card of the PC: a
/// magic packet for a card on another network is for nobody on this one (relay/PROTOCOL.md §7.1).
#[tokio::test]
async fn the_command_names_the_cards_on_the_matched_network() {
    let h = Harness::start().await;
    let pc = TestDevice::windows("524000017");
    h.hbbs.register(&pc, Some(100));
    h.login_ok(HONG, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    // three wired cards: one on another network, listed first, and two on the relay's
    let mut wol = wol_report(
        PC_MAC,
        "192.0.2.67/24",
        "192.0.2.1",
        Some("00:00:5e:00:53:01"),
    );
    let card = |mac: &str, addr: &str, gateway: &str| {
        let mut card = wol["adapters"][0].clone();
        card["mac"] = json!(mac);
        card["ipv4"] = json!([addr]);
        card["gateway"] = json!(gateway);
        card
    };
    let mut elsewhere = card("00:00:5e:00:53:11", "203.0.113.5/24", "203.0.113.1");
    elsewhere["gateway_mac"] = json!("00:00:5e:00:53:02");
    let second = card("00:00:5e:00:53:12", "192.0.2.68/24", "192.0.2.1");
    let first = wol["adapters"][0].clone();
    wol["adapters"] = json!([elsewhere, first, second]);
    let s = h
        .sync_from(&pc, &payload_with_wol(&binding, &wol, "h1"), PC_IP)
        .await;
    assert_eq!(s.answer.status, 200, "{}", s.answer.text);
    asleep(&h, &pc).await;
    let r = approved(&h, 48, &lan(), RELAY_IP, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["relay"]["match"].as_str()),
        (202, Some("exact")),
        "{}",
        a.text
    );
    let got = commands(&h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await);
    assert_eq!(got.len(), 1);
    let c = &got[0].0;
    assert_eq!(c["net"], "192.0.2.0/24");
    assert_eq!(c["macs"], json!([PC_MAC, "00:00:5e:00:53:12"]));
}

#[tokio::test]
async fn the_device_list_and_readiness() {
    let h = Harness::start().await;
    let (pc, binding) = reported(&h, HONG, "524000007", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    let phone = h.login_ok(HONG, None).await;
    let peer = || {
        let (h, phone) = (&h, phone.clone());
        async move { h.get("/api/peers", &phone).await.json["data"][0].clone() }
    };
    let wol = || {
        let (h, phone, id) = (&h, phone.clone(), pc.id.clone());
        async move { h.get(&format!("/api/hjv/devices/{id}/wol"), &phone).await }
    };
    // no relay: nothing could wake it, and the note says nothing of it
    let p = peer().await;
    assert_eq!(
        (p["hjv"]["wol"].as_str(), p["note"].as_str()),
        (Some("no"), Some("비밀번호 없는 연결 일시 중지(관리자)"))
    );
    let a = wol().await;
    assert_eq!(a.json["ready"], "ok");
    assert_eq!(a.json["relay"], Value::Null);
    assert!(now() - a.json["reported_at"].as_i64().unwrap() <= 2);
    // a relay on its network
    let r = approved(&h, 37, &lan(), RELAY_IP, &pc).await;
    let p = peer().await;
    assert_eq!(
        (p["hjv"]["wol"].as_str(), p["note"].as_str()),
        (
            Some("ok"),
            Some("비밀번호 없는 연결 일시 중지(관리자) · 켜기 가능")
        )
    );
    let a = wol().await;
    assert_eq!(
        a.json,
        json!({"ready": "ok", "hints": [], "reported_at": a.json["reported_at"],
               "relay": {"name": hjv_api::relay::default_name(&r.id), "match": "exact", "online": true}})
    );
    assert!(
        !a.text.contains("00:00:5e") && !a.text.contains("192.0.2."),
        "{}",
        a.text
    );
    sql(
        &h,
        format!("UPDATE relays SET last_poll_at = {}", now() - 600),
    )
    .await;
    assert_eq!(wol().await.json["relay"]["online"], false);
    // a report with fast startup: may not wake from shutdown
    let mut report = wol_report(PC_MAC, "192.0.2.67/24", "192.0.2.1", None);
    report["power"]["fast_startup"] = json!(true);
    h.sync_from(&pc, &payload_with_wol(&binding, &report, "h2"), PC_IP)
        .await;
    let a = wol().await;
    assert_eq!(
        (a.json["ready"].as_str(), a.json["hints"].clone()),
        (
            Some("warn"),
            json!(["빠른 시작이 켜져 있어 '종료' 상태에서는 켜지지 않습니다(절전은 됩니다)."])
        )
    );
    assert_eq!(peer().await["hjv"]["wol"], "warn");
    // the card's magic packet wake off, or not told by its driver: hints too, so that "켜기"
    // stays on the app's menu and the hint comes with a wake that was not confirmed
    for (magic, hint) in [
        (json!("0"), "랜카드의 매직 패킷 깨우기가 꺼져 있습니다."),
        (
            Value::Null,
            "랜카드 드라이버가 매직 패킷 깨우기 설정을 알려 주지 않아",
        ),
    ] {
        let mut told = wol_report(PC_MAC, "192.0.2.67/24", "192.0.2.1", None);
        told["adapters"][0]["wake"]["magic"] = magic.clone();
        h.sync_from(&pc, &payload_with_wol(&binding, &told, "h2b"), PC_IP)
            .await;
        let a = wol().await;
        assert_eq!(a.json["ready"], "warn", "{magic}");
        assert_eq!(a.json["hints"].as_array().unwrap().len(), 1, "{}", a.text);
        assert!(
            a.json["hints"][0].as_str().unwrap().starts_with(hint),
            "{}",
            a.text
        );
        let p = peer().await;
        assert_eq!(
            (p["hjv"]["wol"].as_str(), p["note"].as_str()),
            (
                Some("warn"),
                Some("비밀번호 없는 연결 일시 중지(관리자) · 켜기 가능")
            )
        );
    }
    // no wired card: it cannot be woken
    report["adapters"] = json!([]);
    h.sync_from(&pc, &payload_with_wol(&binding, &report, "h3"), PC_IP)
        .await;
    let p = peer().await;
    assert_eq!(
        (p["hjv"]["wol"].as_str(), p["note"].as_str()),
        (
            Some("no"),
            Some("비밀번호 없는 연결 일시 중지(관리자) · 켜기 준비 안 됨")
        )
    );
    assert_eq!(wol().await.json["ready"], "no");
    // a PC that never reported: unknown
    let other = TestDevice::windows("524000008");
    h.hbbs.register(&other, Some(100));
    h.login_ok(HONG, Some(&other)).await;
    let a = h
        .get(&format!("/api/hjv/devices/{}/wol", other.id), &phone)
        .await;
    assert_eq!(
        a.json,
        json!({"ready": "unknown", "hints": [], "reported_at": null, "relay": null})
    );
    asleep(&h, &other).await;
    let a = wake_pc(&h, &other.id, &phone, None).await;
    assert_eq!(
        a.json,
        json!({"error": "이 PC가 아직 켜기 정보를 보내지 않았습니다. PC를 한 번 켜고 HJViewer가 설치되어 있는지 확인하세요.", "hjv": "no_wol_info"})
    );
}

#[tokio::test]
async fn the_sync_carries_the_wake_report() {
    let h = Harness::start().await;
    let (pc, binding) = reported(&h, HONG, "524000009", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    let info = || {
        let (h, id) = (&h, pc.id.clone());
        async move {
            h.state
                .db
                .call(move |c| db::wol_info(c, &id))
                .await
                .unwrap()
        }
    };
    let kept = info().await.unwrap();
    assert_eq!(
        (
            kept.info_hash.as_str(),
            kept.nets.as_str(),
            kept.egress_ip.as_str()
        ),
        ("h1", "192.0.2.0/24", PC_IP)
    );
    assert_eq!(kept.report.adapters[0].mac, PC_MAC);
    // the summary kept: nothing asked; the address moves
    let s = h
        .sync_from(
            &pc,
            &{
                let mut p = payload(&binding);
                p["wol_hash"] = json!("h1");
                p
            },
            "198.51.100.41",
        )
        .await;
    assert_eq!(s.answer.json["send_wol"], false);
    assert_eq!(info().await.unwrap().egress_ip, "198.51.100.41");
    // another summary: the whole report, please
    let s = h
        .sync_from(
            &pc,
            &{
                let mut p = payload(&binding);
                p["wol_hash"] = json!("h9");
                p
            },
            PC_IP,
        )
        .await;
    assert_eq!(s.answer.json["send_wol"], true);
    assert_eq!(info().await.unwrap().info_hash, "h1");
    // none (the PC's first minute): nothing asked, the report kept
    let s = h.sync_from(&pc, &payload(&binding), PC_IP).await;
    assert_eq!(s.answer.json["send_wol"], false);
    // a report that does not read is left out; the sync's list is all there
    let s = h
        .sync_from(
            &pc,
            &payload_with_wol(&binding, &json!({"v": 2}), "h8"),
            PC_IP,
        )
        .await;
    assert!(s.list.as_ref().unwrap().is_ok());
    assert_eq!(s.answer.json["send_wol"], false);
    assert_eq!(info().await.unwrap().info_hash, "h1");
    // a sync with another binding keeps nothing
    let wol = wol_report(PC_MAC, "198.51.100.5/24", "198.51.100.1", None);
    let s = h
        .sync_from(&pc, &payload_with_wol(&"ab".repeat(16), &wol, "h7"), PC_IP)
        .await;
    assert_eq!(s.answer.json["state"], "listed");
    assert_eq!(info().await.unwrap().info_hash, "h1");
    // the binding goes: the report with it
    let token = h.login_ok(HONG, None).await;
    assert_eq!(
        h.delete(&format!("/api/hjv/devices/{}", pc.id), &token)
            .await
            .status,
        200
    );
    assert!(info().await.is_none());
}

#[tokio::test]
async fn an_account_takes_its_wake_data_with_it_and_old_data_goes() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000010", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    let r = approved(&h, 38, &lan(), RELAY_IP, &pc).await;
    let ahead = TestRelay::new([39; 32]);
    h.admin(
        &["relay", "approve", &ahead.code, "--owner-of", &pc.id],
        false,
        None,
    )
    .await
    .unwrap();
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(wake_pc(&h, &pc.id, &phone, None).await.status, 202);
    for table in [
        "wol_info",
        "wake_jobs",
        "wake_commands",
        "relay_preapprovals",
    ] {
        assert_eq!(
            count(&h, format!("SELECT COUNT(*) FROM {table}")).await,
            1,
            "{table}"
        );
    }
    h.admin(&["user", "delete", HONG.0], false, None)
        .await
        .unwrap();
    for table in [
        "wol_info",
        "wake_jobs",
        "wake_commands",
        "relay_preapprovals",
    ] {
        assert_eq!(
            count(&h, format!("SELECT COUNT(*) FROM {table}")).await,
            0,
            "{table}"
        );
    }
    // its relay is revoked, keeping its key: it is never approved again unnoticed
    let row = h
        .state
        .db
        .call({
            let id = r.id.clone();
            move |c| db::relay(c, &id)
        })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (row.state, row.owner_account, row.ifaces.len()),
        (db::RelayState::Revoked, None, 0)
    );
    assert_eq!(
        h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await.json["state"],
        "revoked"
    );

    // the upkeep: jobs past the record's six months, pending entries past a day, approvals
    // ahead past a week, expired commands
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000011", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    approved(&h, 40, &lan(), RELAY_IP, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    assert_eq!(wake_pc(&h, &pc.id, &phone, None).await.status, 202);
    h.hello(&TestRelay::new([41; 32]), RELAY_IP, &lan()).await;
    h.admin(
        &[
            "relay",
            "approve",
            &TestRelay::new([42; 32]).code,
            "--owner-of",
            &pc.id,
        ],
        false,
        None,
    )
    .await
    .unwrap();
    let events = h.state.config.event_retention_secs;
    sql(
        &h,
        format!(
            "UPDATE wake_jobs SET created_at = created_at - {events};
         UPDATE wake_commands SET exp = {};
         UPDATE relays SET created_at = created_at - 86400 WHERE state = 'pending';
         UPDATE relay_preapprovals SET expires_at = {};",
            now() - wake::RESULT_GRACE - 1,
            now()
        ),
    )
    .await;
    let purged = hjv_api::jobs::purge_now(&h.state).await.unwrap();
    assert_eq!(purged.wake, 4, "{purged:?}");
    for table in ["wake_jobs", "wake_commands", "relay_preapprovals"] {
        assert_eq!(
            count(&h, format!("SELECT COUNT(*) FROM {table}")).await,
            0,
            "{table}"
        );
    }
    assert_eq!(count(&h, "SELECT COUNT(*) FROM relays".to_owned()).await, 1);
}

#[tokio::test]
async fn the_command_key_is_checked_and_needed() {
    // without one: no command is signed
    let h = Harness::with(Options {
        no_command_key: true,
        ..Default::default()
    })
    .await;
    let (pc, _) = reported(&h, HONG, "524000012", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    approved(&h, 43, &lan(), RELAY_IP, &pc).await;
    let phone = h.login_ok(HONG, None).await;
    let a = wake_pc(&h, &pc.id, &phone, None).await;
    assert_eq!(
        (a.status, a.json["hjv"].as_str()),
        (503, Some("unavailable"))
    );
    assert_eq!(
        count(&h, "SELECT COUNT(*) FROM wake_jobs".to_owned()).await,
        0
    );
    let report = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(report.contains("[FAIL] command key: no credential hjviewer-api-command-key, while HJV_API_COMMAND_PUBKEYS"), "{report}");
    // with the key the relays pin: the fingerprints to compare with the relay build's summary
    let h = Harness::start().await;
    let report = h.admin(&["selfcheck"], false, None).await;
    let report = report.unwrap_or_else(|e| e);
    let one = "1:24f6ed6acbfe1009c030d7ca567c33ca4830911498236b5561a6c82abec5de28";
    let two = hjv_api::trust::fingerprint(2, &devsig::public_key(&NEXT_COMMAND_SEED));
    assert!(
        report.contains(&format!(
            "[ OK ] command keys the relays pin (HJV_API_COMMAND_PUBKEYS), SHA-256: {one}, {two}"
        )),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "[ OK ] command key in use (HJV_API_COMMAND_KID 1): the pinned key 1, {one}"
        )),
        "{report}"
    );
    // the key in use is not the pinned one of its number
    let h = Harness::with(Options {
        vars: vec![("HJV_API_COMMAND_KID", "2".into())],
        ..Default::default()
    })
    .await;
    let report = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(
        report.contains("[FAIL] command key in use (HJV_API_COMMAND_KID 2, 2:"),
        "{report}"
    );
    assert!(
        report.contains("is not the pinned key 2: the relays refuse every command it signs"),
        "{report}"
    );
    assert!(!report.contains(&b64(&COMMAND_SEED)));
    // the commands of the next key: what a relay built with both takes after the change
    let h = Harness::with(Options {
        vars: vec![(
            "HJV_API_COMMAND_PUBKEYS",
            hjv_api::trust::entry(2, &devsig::public_key(&COMMAND_SEED)),
        )],
        ..Default::default()
    })
    .await;
    let report = h.admin(&["selfcheck"], false, None).await.unwrap_err();
    assert!(
        report.contains("no pinned key has the number 1"),
        "{report}"
    );
}

#[tokio::test]
async fn the_report_and_the_health_count_relays_and_wakes() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000013", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    asleep(&h, &pc).await;
    let r = approved(&h, 44, &lan(), RELAY_IP, &pc).await;
    h.hello(&TestRelay::new([45; 32]), RELAY_IP, &lan()).await;
    let phone = h.login_ok(HONG, None).await;
    let job = wake_pc(&h, &pc.id, &phone, None).await.json["job"]
        .as_str()
        .unwrap()
        .to_owned();
    let text = commands(&h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await)
        .pop()
        .unwrap()
        .1;
    h.poll(
        &r,
        RELAY_IP,
        0,
        &json!([result(&text, &job, None, 0, Some("no_iface"))]),
        None,
        &lan(),
    )
    .await;
    let report = h.admin(&["report"], false, None).await.unwrap();
    assert!(
        report.contains("relays: active 1, pending 1; codes approved ahead: 0"),
        "{report}"
    );
    assert!(
        report.contains(&format!("  active relay {}: last poll ", r.id)),
        "{report}"
    );
    assert!(
        report.contains("PCs that reported how to wake them: 1"),
        "{report}"
    );
    assert!(
        report
            .lines()
            .any(|l| l.trim_start().starts_with("failed no_iface") && l.ends_with(" 1 / 1")),
        "{report}"
    );
    let health: Value =
        serde_json::from_str(&h.admin(&["health"], false, None).await.unwrap()).unwrap();
    assert_eq!(health["relays"], json!({"active": 1, "silent": 0}));
}

/// relay/PROTOCOL.md §7.4: every queued, unexpired command of the relay in each poll answer,
/// oldest first, at most 16, until a result names it or it expires.
#[tokio::test]
async fn commands_wait_in_the_queue_sixteen_at_a_time() {
    let h = Harness::start().await;
    let (pc, _) = reported(&h, HONG, "524000014", "192.0.2.67/24", "192.0.2.1", PC_IP).await;
    let r = approved(&h, 46, &lan(), RELAY_IP, &pc).await;
    let at = now();
    let texts: Vec<String> = (0..18)
        .map(|i| {
            wake::command_text(
                &format!("{i:032x}"),
                &r.id,
                at - 18 + i,
                "192.0.2.0/24",
                &[PC_MAC.to_owned()],
            )
        })
        .collect();
    let (relay_id, queued) = (r.id.clone(), texts.clone());
    h.state
        .db
        .call(move |c| {
            // newest first, so that the order is the commands' and not the rows'
            for (i, text) in queued.iter().enumerate().rev() {
                let iat = at - 18 + i as i64;
                let job = db::Job {
                    job: format!("{i:032x}"),
                    account_id: 1,
                    account_tag: "t".into(),
                    device_id: format!("52400{i:04}"),
                    relay_id: relay_id.clone(),
                    grade: "exact".into(),
                    net: "192.0.2.0/24".into(),
                    cmd: wake::command_hash(text),
                    created_at: iat,
                    exp: iat + wake::COMMAND_LIFETIME,
                    state: "queued".into(),
                    sent_at: None,
                    packets: None,
                    confirmed_at: None,
                    error: None,
                    finished_at: None,
                };
                let command = db::NewCommand {
                    cmd: &job.cmd,
                    text,
                    sig: "c2ln",
                    kid: 1,
                    iat,
                };
                db::insert_job(c, &job, &command)?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let got = |a: &Answer| -> Vec<String> { commands(a).into_iter().map(|c| c.1).collect() };
    let a = h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await;
    assert_eq!(got(&a), texts[..16]);
    // results for the first four: the next sixteen are the rest, in their order
    let results: Vec<Value> = texts[..4]
        .iter()
        .enumerate()
        .map(|(i, t)| result(t, &format!("{i:032x}"), Some(at), 12, None))
        .collect();
    let a = h
        .poll(&r, RELAY_IP, 0, &Value::Array(results), None, &lan())
        .await;
    assert_eq!(got(&a), texts[4..]);
    // an expired command is no longer sent, whatever became of its result
    sql(
        &h,
        format!(
            "UPDATE wake_commands SET exp = {} WHERE cmd_sha256 = '{}'",
            now(),
            wake::command_hash(&texts[4])
        ),
    )
    .await;
    let a = h.poll(&r, RELAY_IP, 0, &json!([]), None, &lan()).await;
    assert_eq!(got(&a), texts[5..]);
}
