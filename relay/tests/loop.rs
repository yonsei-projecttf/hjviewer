// SPDX-License-Identifier: AGPL-3.0-only

//! The relay loop against a test server speaking relay/PROTOCOL.md over real TLS: enrollment,
//! long-poll, signed commands and every refusal, results, clock correction, errors and back-off,
//! revocation, interface reports and the executable watch. Packets are recorded, not sent
//! (tests/netns.rs sends real ones inside network namespaces).

mod common;

use common::*;
use hjv_relay::{
    identity::Identity,
    relay::{Exit, Relay, State},
};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    net::Ipv4Addr,
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};

/// A scripted server: answers hello with `hello_state` and each poll with the next queued reply
/// (state active and nothing to do when the queue is empty).
type PollReply = Box<dyn FnOnce(&Seen, i64) -> Reply + Send>;

struct Script {
    hello_state: &'static str,
    polls: VecDeque<PollReply>,
}

fn scripted(server: &TestServer, hello_state: &'static str) -> Arc<Mutex<Script>> {
    let script = Arc::new(Mutex::new(Script {
        hello_state,
        polls: VecDeque::new(),
    }));
    let s = script.clone();
    server.set_handler(move |seen, now| {
        let mut s = s.lock().unwrap();
        if seen.op == "hello" {
            let code = Identity::from_seed(&RELAY_SEED).code;
            return Reply::ok(json!({"server_time": now, "state": s.hello_state, "code": code}));
        }
        match s.polls.pop_front() {
            Some(f) => f(seen, now),
            None => Reply::ok(json!({"server_time": now, "state": "active", "commands": []})),
        }
    });
    script
}

fn queue(script: &Arc<Mutex<Script>>, f: impl FnOnce(&Seen, i64) -> Reply + Send + 'static) {
    script.lock().unwrap().polls.push_back(Box::new(f));
}

fn relay_id() -> String {
    Identity::from_seed(&RELAY_SEED).id
}

fn results_of(seen: &Seen) -> Vec<Value> {
    seen.payload["results"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn enrollment_then_a_wake_command() {
    let log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "pending"})));
    let script = scripted(&server, "pending");
    let env = FakeEnv::new();
    let sent = env.sent.clone();
    let mut relay = Relay::new(relay_config(&server), env);

    // hello: the server keeps the relay pending; the code is in the log
    assert_eq!(relay.step(), fast_timing().pending_poll);
    assert_eq!(relay.state(), State::Pending);
    let code = Identity::from_seed(&RELAY_SEED).code;
    assert!(log
        .lock()
        .unwrap()
        .iter()
        .any(|l| l.contains(&format!("등록 코드: {code}"))));
    let hello = &server.seen()[0];
    assert_eq!(hello.op, "hello");
    assert_eq!(hello.id, relay_id());
    assert_eq!(
        hello.user_agent,
        format!("hjv-relay/{}", hjv_relay::config::VERSION)
    );
    assert_eq!(hello.host, format!("localhost:{}", server.port));
    let p = &hello.payload;
    assert_eq!(p["version"], hjv_relay::config::VERSION);
    assert_eq!(p["arch"], std::env::consts::ARCH);
    assert_eq!(p["os"], "DSM 7.2.1-69057 Update 5");
    assert_eq!(p["kernel"], "5.10.55+");
    // docker0 is not reported
    let names: Vec<&str> = p["ifaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["eth0", "eth1"]);
    assert_eq!(p["ifaces"][0]["ipv4"], json!(["10.1.5.20/22"]));
    assert_eq!(p["ifaces"][0]["mac"], "02:00:00:00:00:01");
    assert_eq!(p["ifaces"][1]["gateway"], "192.168.50.1");
    assert_eq!(p["ifaces"][1]["gateway_mac"], "02:00:00:00:00:fe");
    assert_eq!(p["ifaces_hash"].as_str().unwrap().len(), 64);

    // still pending
    queue(&script, |_, now| {
        Reply::ok(json!({"server_time": now, "state": "pending"}))
    });
    assert_eq!(relay.step(), fast_timing().pending_poll);
    let poll = server.seen().pop().unwrap();
    assert_eq!(poll.op, "poll");
    assert_eq!(poll.payload["wait"], 1);
    assert!(
        poll.payload.get("ifaces").is_none(),
        "unchanged interfaces are not sent again"
    );
    assert_eq!(poll.payload["ifaces_hash"], p["ifaces_hash"]);

    // approved, with a command
    let id = relay_id();
    let c1 = wake_command(
        "0123456789abcdef0123456789abcdef",
        &id,
        unix_now(),
        "10.1.4.0/22",
    );
    let entry = signed(&c1, &command_signing_key(), 1);
    let c1_text = entry["c"].as_str().unwrap().to_owned();
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [entry]}))
    });
    assert_eq!(relay.step(), Duration::ZERO, "results go back at once");
    assert_eq!(relay.state(), State::Active);
    {
        let sent = sent.lock().unwrap();
        assert_eq!(sent.len(), 1);
        let plan = &sent[0];
        assert_eq!(plan.iface, "eth0");
        assert_eq!(plan.source, Ipv4Addr::new(10, 1, 5, 20));
        assert_eq!(
            plan.dests,
            [Ipv4Addr::BROADCAST, Ipv4Addr::new(10, 1, 7, 255)]
        );
        assert_eq!(plan.macs, [[0x00, 0x00, 0x5e, 0x00, 0x53, 0x01]]);
        assert_eq!(plan.ports, [9, 7]);
        assert_eq!((plan.repeat, plan.gap), (3, Duration::from_millis(200)));
        assert_eq!(plan.packets(), 12);
    }
    assert_eq!(relay.pending_results(), 1);

    // the next poll carries the result
    relay.step();
    let poll = server.seen().pop().unwrap();
    let results = results_of(&poll);
    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert_eq!(r["cmd"], command_hash(&c1_text));
    assert_eq!(r["job"], "0123456789abcdef0123456789abcdef");
    assert_eq!(r["iface"], "eth0");
    assert_eq!(r["packets"], 12);
    assert_eq!(r["error"], Value::Null);
    assert!((r["sent_at"].as_i64().unwrap() - unix_now()).abs() <= 2);
    assert_eq!(relay.pending_results(), 0);
    assert!(log.lock().unwrap().iter().any(|l| l.contains("켜기 01234567 (**:**:**:**:53:01, 망 10.1.4.0/22 exact): eth0 10.1.5.20/22 -> 255.255.255.255, 10.1.7.255 포트 9,7 ×3: 12/12개 보냄")));
    // no request was refused by the server's checks
    assert!(server.refused().is_empty(), "{:?}", server.refused());
}

#[test]
fn every_refusal_is_reported_and_nothing_is_sent() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let env = FakeEnv::new();
    let sent = env.sent.clone();
    let mut relay = Relay::new(relay_config(&server), env);
    assert_eq!(relay.step(), Duration::ZERO);
    assert_eq!(relay.state(), State::Active);

    let id = relay_id();
    let key = command_signing_key();
    let other_key = ed25519_dalek::SigningKey::from_bytes(&[43; 32]);
    let now = unix_now();
    let job = |i: u32| format!("{i:032x}");
    let good = |i: u32| wake_command(&job(i), &id, now, "10.1.4.0/22");
    let with = |i: u32, field: &str, v: Value| {
        let mut c = good(i);
        c[field] = v;
        c
    };
    let mut tampered = signed(&good(4), &key, 1);
    tampered["c"] = Value::from(
        tampered["c"]
            .as_str()
            .unwrap()
            .replace("\"repeat\":3", "\"repeat\":5"),
    );
    let entries = vec![
        (signed(&good(1), &key, 7), "unknown_key"),
        (signed(&good(2), &other_key, 1), "bad_signature"),
        (tampered, "bad_signature"),
        (json!({"c": 5, "s": "x", "k": 1}), "bad_command"),
        (signed(&json!("not an object"), &key, 1), "bad_command"),
        (signed(&with(5, "v", json!(2)), &key, 1), "bad_version"),
        (
            signed(&with(6, "relay", json!("r-aaaaaaaaaaaaaaaa")), &key, 1),
            "wrong_relay",
        ),
        (signed(&with(7, "exp", json!(now - 1)), &key, 1), "expired"),
        (
            signed(
                &{
                    let mut c = with(8, "iat", json!(now + 40));
                    c["exp"] = json!(now + 100);
                    c
                },
                &key,
                1,
            ),
            "not_yet_valid",
        ),
        (
            signed(&with(9, "exp", json!(now + 500)), &key, 1),
            "bad_lifetime",
        ),
        (
            signed(&with(10, "macs", json!(["ff:ff:ff:ff:ff:ff"])), &key, 1),
            "bad_macs",
        ),
        (
            signed(
                &with(
                    11,
                    "macs",
                    json!([
                        "02:00:00:00:00:01",
                        "02:00:00:00:00:02",
                        "02:00:00:00:00:03",
                        "02:00:00:00:00:04",
                        "02:00:00:00:00:05"
                    ]),
                ),
                &key,
                1,
            ),
            "bad_macs",
        ),
        (
            signed(&with(12, "ports", json!([80])), &key, 1),
            "bad_params",
        ),
        (signed(&with(13, "repeat", json!(6)), &key, 1), "bad_params"),
        (
            signed(&with(14, "net", json!("10.99.0.0/16")), &key, 1),
            "no_iface",
        ),
        // a container bridge on the network does not count
        (
            signed(&with(15, "net", json!("172.17.0.0/16")), &key, 1),
            "no_iface",
        ),
    ];
    let expected: Vec<&str> = entries.iter().map(|(_, e)| *e).collect();
    let commands: Vec<Value> = entries.into_iter().map(|(e, _)| e).collect();
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": commands}))
    });
    assert_eq!(relay.step(), Duration::ZERO);
    assert!(sent.lock().unwrap().is_empty());
    relay.step();
    let results = results_of(&server.seen().pop().unwrap());
    let errors: Vec<&str> = results
        .iter()
        .map(|r| r["error"].as_str().unwrap())
        .collect();
    assert_eq!(errors, expected);
    assert!(results
        .iter()
        .all(|r| r["packets"] == 0 && r["sent_at"].is_null()));
    // commands that passed the signature check name their job; all name their command
    assert_eq!(results[0]["cmd"].as_str().unwrap().len(), 64);
    assert_eq!(results[3]["cmd"], Value::Null);
    assert_eq!(results[6]["job"], job(6));
    assert_eq!(results[0]["job"], Value::Null);

    // a command runs once: the same signed command again is refused
    let again = signed(&good(20), &key, 1);
    let (a, b) = (again.clone(), again.clone());
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [a]}))
    });
    relay.step();
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [b]}))
    });
    relay.step();
    relay.step();
    let results: Vec<Value> = server.seen().iter().flat_map(results_of).collect();
    let tail: Vec<&str> = results
        .iter()
        .rev()
        .take(2)
        .map(|r| r["error"].as_str().unwrap_or("ok"))
        .collect();
    assert_eq!(tail, ["replayed", "ok"]);
    assert_eq!(sent.lock().unwrap().len(), 1);

    // rate limit: 10 commands a minute (one already went out)
    let commands: Vec<Value> = (30..40).map(|i| signed(&good(i), &key, 1)).collect();
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": commands}))
    });
    relay.step();
    relay.step();
    let results = results_of(&server.seen().pop().unwrap());
    let errors: Vec<&str> = results
        .iter()
        .map(|r| r["error"].as_str().unwrap_or("ok"))
        .collect();
    assert_eq!(errors, [vec!["ok"; 9], vec!["rate_limited"]].concat());
    assert_eq!(sent.lock().unwrap().len(), 10);
    assert!(server.refused().is_empty());
}

#[test]
fn commands_are_checked_against_the_servers_clock() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    // the server runs an hour ahead of this machine
    server.shared.lock().unwrap().clock_offset = 3600;
    let script = scripted(&server, "active");
    let mut relay = Relay::new(relay_config(&server), FakeEnv::new());
    // the first hello is refused for its time, and re-signed with the server's
    relay.step();
    assert_eq!(server.refused(), [(400, "clock".to_owned())]);
    assert_eq!(relay.state(), State::Active);
    let hello = &server.seen()[0];
    assert!((hello.t - (unix_now() + 3600)).abs() <= 2);
    // a command made by the server's clock is valid; by this machine's it is expired
    let id = relay_id();
    let key = command_signing_key();
    let server_now = unix_now() + 3600;
    let fresh = signed(
        &wake_command(&format!("{:032x}", 1), &id, server_now, "10.1.4.0/22"),
        &key,
        1,
    );
    let local = signed(
        &wake_command(&format!("{:032x}", 2), &id, unix_now(), "10.1.4.0/22"),
        &key,
        1,
    );
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [fresh, local]}))
    });
    relay.step();
    relay.step();
    let results = results_of(&server.seen().pop().unwrap());
    let errors: Vec<&str> = results
        .iter()
        .map(|r| r["error"].as_str().unwrap_or("ok"))
        .collect();
    assert_eq!(errors, ["ok", "expired"]);
}

#[test]
fn unknown_relays_say_hello_again_and_revoked_ones_take_no_commands() {
    let log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let env = FakeEnv::new();
    let sent = env.sent.clone();
    let mut relay = Relay::new(relay_config(&server), env);
    relay.step();
    queue(&script, |_, now| {
        Reply::status(404, json!({"error": "unknown_relay", "server_time": now}))
    });
    let delay = relay.step();
    assert_eq!(relay.state(), State::Unknown);
    assert!(delay <= Duration::from_millis(10), "{delay:?}");
    relay.step();
    assert_eq!(server.seen().pop().unwrap().op, "hello");
    assert_eq!(relay.state(), State::Active);

    let id = relay_id();
    let cmd = signed(
        &wake_command(&format!("{:032x}", 1), &id, unix_now(), "10.1.4.0/22"),
        &command_signing_key(),
        1,
    );
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "revoked", "commands": [cmd]}))
    });
    // nothing was sent, so the not_active result waits for the next regular poll
    assert_eq!(relay.step(), fast_timing().revoked_poll);
    assert_eq!(relay.state(), State::Revoked);
    assert!(log.lock().unwrap().iter().any(|l| l.contains("폐기")));
    queue(&script, |_, now| {
        Reply::ok(json!({"server_time": now, "state": "revoked"}))
    });
    assert_eq!(relay.step(), fast_timing().revoked_poll);
    let results: Vec<Value> = server.seen().iter().flat_map(results_of).collect();
    assert_eq!(results.last().unwrap()["error"], "not_active");
    assert!(sent.lock().unwrap().is_empty());
    // the server decides how long to wait
    queue(&script, |_, now| {
        Reply::ok(json!({"server_time": now, "state": "revoked", "poll_after": 7}))
    });
    assert_eq!(relay.step(), Duration::from_secs(7));
    queue(&script, |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "poll_after": 999999}))
    });
    assert_eq!(relay.step(), Duration::from_secs(3600));
}

#[test]
fn errors_back_off_and_are_logged_once() {
    let log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let mut relay = Relay::new(relay_config(&server), FakeEnv::new());
    relay.step();
    for _ in 0..4 {
        queue(&script, |_, _| {
            Reply::status(503, json!({"error": "unavailable"}))
        });
    }
    let delays: Vec<Duration> = (0..4).map(|_| relay.step()).collect();
    // 5, 10, 20, 40 ms (fast timing), ±20%
    for (d, base) in delays.iter().zip([5u64, 10, 20, 40]) {
        let ms = d.as_secs_f64() * 1000.0;
        assert!(
            ms >= base as f64 * 0.8 - 0.01 && ms <= base as f64 * 1.2 + 0.01,
            "{delays:?}"
        );
    }
    queue(&script, |_, now| {
        Reply::status(
            429,
            json!({"error": "rate_limited", "retry_after": 9, "server_time": now}),
        )
    });
    assert_eq!(relay.step(), Duration::from_secs(9));
    // not JSON, not HTTP 200
    queue(&script, |_, _| Reply::status(502, json!("bad gateway")));
    relay.step();
    // a 200 that is not a poll response
    queue(&script, |_, _| Reply::ok(json!({"hello": "world"})));
    relay.step();
    relay.step();
    let lines = log.lock().unwrap().clone();
    let failures = lines
        .iter()
        .filter(|l| l.contains("통신하지 못했습니다"))
        .count();
    let recoveries = lines
        .iter()
        .filter(|l| l.contains("다시 통신합니다 (연속 실패 7번 뒤)"))
        .count();
    assert_eq!((failures, recoveries), (1, 1), "{lines:#?}");
    assert_eq!(relay.state(), State::Active);
}

#[test]
fn long_polls_and_timeouts() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let mut cfg = relay_config(&server);
    cfg.timing.request_timeout = Duration::from_millis(800);
    let mut relay = Relay::new(cfg, FakeEnv::new());
    relay.step();
    // held 300 ms: fine
    queue(&script, |_, now| Reply {
        status: 200,
        body: json!({"server_time": now, "state": "active"}),
        delay: Duration::from_millis(300),
    });
    let started = Instant::now();
    assert_eq!(relay.step(), Duration::ZERO);
    assert!(started.elapsed() >= Duration::from_millis(300));
    // held past the request timeout: a failure, retried after a back-off
    queue(&script, |_, now| Reply {
        status: 200,
        body: json!({"server_time": now, "state": "active"}),
        delay: Duration::from_millis(1500),
    });
    let started = Instant::now();
    let delay = relay.step();
    assert!(started.elapsed() < Duration::from_millis(1400));
    assert!(delay > Duration::ZERO);
}

#[test]
fn interfaces_are_reported_when_they_change_or_the_server_asks() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let env = FakeEnv::new();
    let ifaces = env.ifaces.clone();
    let mut relay = Relay::new(relay_config(&server), env);
    relay.step();
    relay.step();
    assert!(server.seen().pop().unwrap().payload.get("ifaces").is_none());
    ifaces.lock().unwrap()[0] = iface("eth0", &["10.1.5.21/22"]);
    relay.step();
    let poll = server.seen().pop().unwrap();
    assert_eq!(poll.payload["ifaces"][0]["ipv4"], json!(["10.1.5.21/22"]));
    relay.step();
    assert!(server.seen().pop().unwrap().payload.get("ifaces").is_none());
    queue(&script, |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "send_ifaces": true}))
    });
    relay.step();
    relay.step();
    assert!(server.seen().pop().unwrap().payload.get("ifaces").is_some());
    // a failed poll does not count as delivered
    ifaces.lock().unwrap()[0] = iface("eth0", &["10.1.5.22/22"]);
    queue(&script, |_, _| Reply::status(500, json!({})));
    relay.step();
    relay.step();
    let polls = server.seen();
    let last_two: Vec<bool> = polls
        .iter()
        .rev()
        .take(2)
        .map(|p| p.payload.get("ifaces").is_some())
        .collect();
    assert_eq!(last_two, [true, true]);
}

#[test]
fn results_survive_failed_polls() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let mut relay = Relay::new(relay_config(&server), FakeEnv::new());
    relay.step();
    let cmd = signed(
        &wake_command(
            &format!("{:032x}", 1),
            &relay_id(),
            unix_now(),
            "10.1.4.0/22",
        ),
        &command_signing_key(),
        1,
    );
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [cmd]}))
    });
    relay.step();
    queue(&script, |_, _| Reply::status(500, json!({})));
    relay.step();
    assert_eq!(relay.pending_results(), 1);
    relay.step();
    assert_eq!(relay.pending_results(), 0);
    let polls = server.seen();
    let with_results = polls.iter().filter(|p| !results_of(p).is_empty()).count();
    assert_eq!(with_results, 2, "sent again after the failed poll");
}

#[test]
fn a_wrong_certificate_authority_is_refused() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let mut cfg = relay_config(&server);
    cfg.client =
        hjv_relay::https::Client::new(server.server(), hjv_relay::https::builtin_roots()).unwrap();
    let mut relay = Relay::new(cfg, FakeEnv::new());
    let delay = relay.step();
    assert!(delay > Duration::ZERO);
    assert_eq!(relay.state(), State::Unknown);
    assert!(server.seen().is_empty());
}

#[test]
fn run_ends_when_the_executable_changes() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "pending"})));
    let env = FakeEnv::new();
    let flag = env.exe_changed.clone();
    let mut relay = Relay::new(relay_config(&server), env);
    let t = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag.store(true, Ordering::SeqCst);
    });
    assert_eq!(relay.run(), Exit::ExeChanged("test".to_owned()));
    t.join().unwrap();
    // it kept polling while pending, with waits in between
    assert!(server.seen().len() >= 2);
    assert!(relay
        .env()
        .sleeps
        .lock()
        .unwrap()
        .iter()
        .all(|d| *d <= Duration::from_millis(30)));
    let statuses = relay.env().statuses.lock().unwrap().clone();
    let last: Value = serde_json::from_str(&statuses.last().unwrap().0).unwrap();
    assert_eq!(last["state"], "pending");
    assert_eq!(last["id"], relay_id());
}

#[test]
fn results_are_delivered_before_a_restart() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let env = FakeEnv::new();
    let flag = env.exe_changed.clone();
    let sent = env.sent.clone();
    let mut relay = Relay::new(relay_config(&server), env);
    let job = format!("{:032x}", 7);
    let cmd = signed(
        &wake_command(&job, &relay_id(), unix_now(), "10.1.4.0/22"),
        &command_signing_key(),
        1,
    );
    queue(&script, move |_, now| {
        // the executable is replaced while this poll is out, and it brings a command
        flag.store(true, Ordering::SeqCst);
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [cmd]}))
    });
    assert_eq!(relay.run(), Exit::ExeChanged("test".to_owned()));
    assert_eq!(sent.lock().unwrap().len(), 1);
    // the result went out in one more poll, which asked the server not to hold it
    assert_eq!(relay.pending_results(), 0);
    let last = server.seen().pop().unwrap();
    assert_eq!(last.payload["wait"], 0);
    let results = results_of(&last);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["job"], job);
    assert_eq!(results[0]["packets"], 12);
}

#[test]
fn the_stop_marker_ends_the_loop_after_the_results_went_out() {
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let env = FakeEnv::new();
    let stop = env.stop.clone();
    let tidied = env.tidied.clone();
    let sent = env.sent.clone();
    let mut relay = Relay::new(relay_config(&server), env);
    let job = format!("{:032x}", 8);
    let cmd = signed(
        &wake_command(&job, &relay_id(), unix_now(), "10.1.4.0/22"),
        &command_signing_key(),
        1,
    );
    queue(&script, move |_, now| {
        // the marker appears while this poll is out, and the poll brings a command
        stop.store(true, Ordering::SeqCst);
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [cmd]}))
    });
    assert_eq!(relay.run(), Exit::Stopped);
    assert_eq!(sent.lock().unwrap().len(), 1);
    assert_eq!(relay.pending_results(), 0);
    let last = server.seen().pop().unwrap();
    assert_eq!(last.payload["wait"], 0);
    assert_eq!(results_of(&last)[0]["job"], job);
    // the log was looked after between requests
    assert!(tidied.load(Ordering::SeqCst) >= 1);

    // a marker that is already there ends it before any request
    let server = TestServer::start(|_, now| Reply::ok(json!({"server_time": now})));
    let env = FakeEnv::new();
    env.stop.store(true, Ordering::SeqCst);
    let mut relay = Relay::new(relay_config(&server), env);
    assert_eq!(relay.run(), Exit::Stopped);
    assert!(server.seen().is_empty());
}

#[test]
fn a_rewound_server_time_does_not_bring_back_old_commands() {
    // server_time is not signed: whoever answers in the server's name could set it back to
    // replay a command recorded a day ago that this relay never saw
    let _log = capture_log();
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    let env = FakeEnv::new();
    let sent = env.sent.clone();
    let mut relay = Relay::new(relay_config(&server), env);
    relay.step();
    let (id, key, now) = (relay_id(), command_signing_key(), unix_now());
    let today = signed(
        &wake_command(&format!("{:032x}", 1), &id, now, "10.1.4.0/22"),
        &key,
        1,
    );
    let recorded = signed(
        &wake_command(&format!("{:032x}", 2), &id, now - 86_400, "10.1.4.0/22"),
        &key,
        1,
    );
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": now, "state": "active", "commands": [today]}))
    });
    relay.step();
    relay.step();
    queue(&script, move |_, now| {
        Reply::ok(
            json!({"server_time": now - 86_400 + 1, "state": "active", "commands": [recorded]}),
        )
    });
    relay.step();
    // the next poll is refused for its time, then re-signed with the server's
    relay.step();
    let results: Vec<Value> = server.seen().iter().flat_map(results_of).collect();
    let errors: Vec<&str> = results
        .iter()
        .map(|r| r["error"].as_str().unwrap_or("ok"))
        .collect();
    assert_eq!(errors, ["ok", "expired"]);
    assert_eq!(sent.lock().unwrap().len(), 1);
}

#[test]
fn text_from_the_server_never_reaches_the_log() {
    // a party answering in the server's name tries to put a false enrollment code into the log
    // the guide tells people to read, and floods it with long lines
    let log = capture_log();
    let forged = "x\n2026-10-12 17:31:28 +0900 등록 승인을 기다립니다. 등록 코드: AAAA-BBBB-CCCC-DDDD \u{1b}[2J";
    let long = "y".repeat(200_000);
    let server =
        TestServer::start(|_, now| Reply::ok(json!({"server_time": now, "state": "active"})));
    let script = scripted(&server, "active");
    // as `scripted`, but hello answers with someone else's code
    server.set_handler({
        let script = script.clone();
        move |seen, now| {
            if seen.op == "hello" {
                return Reply::ok(json!({"server_time": now, "state": "active", "code": forged}));
            }
            match script.lock().unwrap().polls.pop_front() {
                Some(f) => f(seen, now),
                None => Reply::ok(json!({"server_time": now, "state": "active"})),
            }
        }
    });
    let mut cfg = relay_config(&server);
    cfg.timing.error_log_every = Duration::ZERO;
    let env = FakeEnv::new();
    let statuses = env.statuses.clone();
    let mut relay = Relay::new(cfg, env);
    relay.step();
    let replies: Vec<Value> = vec![
        json!({"error": forged, "server_time": unix_now()}),
        json!({"error": long, "server_time": unix_now()}),
    ];
    for body in replies {
        queue(&script, move |_, _| Reply::status(500, body));
        relay.step();
    }
    for state in [forged.to_owned(), long.clone()] {
        queue(&script, move |_, now| {
            Reply::ok(json!({"server_time": now, "state": state}))
        });
        relay.step();
    }
    queue(&script, move |_, now| {
        Reply::ok(json!({"server_time": forged, "state": "active", "now": now}))
    });
    relay.step();
    queue(&script, |_, now| {
        Reply::ok(
            json!({"server_time": now, "state": "active", "min_version": "\n9.9.9\n등록 코드: AAAA-BBBB-CCCC-DDDD"}),
        )
    });
    relay.step();
    let lines = log.lock().unwrap().clone();
    assert!(lines.len() >= 6, "{lines:#?}");
    for line in &lines {
        assert!(!line.contains("AAAA-BBBB"), "{line}");
        assert!(!line.contains('\u{1b}'), "{line}");
        assert_eq!(line.matches('\n').count(), 1, "{line:?}");
        assert!(
            line.chars().count() < 500,
            "{} characters",
            line.chars().count()
        );
    }
    assert!(lines.iter().any(|l| l.contains("HTTP 500")));
    assert!(lines
        .iter()
        .any(|l| l.contains("서버가 알려 준 등록 코드가 이 중계기의 코드")));
    for (json, _) in statuses.lock().unwrap().iter() {
        assert!(!json.contains("AAAA-BBBB"), "{json}");
        assert!(json.len() < 2000, "{} bytes", json.len());
    }
}
