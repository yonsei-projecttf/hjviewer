// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-relay itself against hjv-api (design v3 WP-W2's acceptance: the relay's loop against the
//! real hjv-api). The relay is relay/'s library, its loop driven step by step as its own
//! tests/loop.rs drives it against a scripted server: its key, envelope, HTTPS client (with a test
//! CA, relay/PROTOCOL.md §1), payloads, command checks, choice of interface and results, with
//! the network replaced by a recorder (relay/tests/netns.rs sends real packets). In front of
//! hjv-api a TLS terminator stands in for nginx and sets X-Real-IP. The scenario: enrollment and
//! approval, a long-poll that a wake request ends at once, the command signed with hjv-api's
//! command key and checked by the relay, twelve packets on the PC's /22 wire through the NAS's own
//! broadcast, the result, the confirmation, revocation, a relay deleted and enrolling again, and a
//! relay that does not trust the key in use.
//!
//! And the relay's reference of the server checks, relay/tests/common's `check_request`
//! (PROTOCOL.md's conformance material, which the relay's own tests hold the relay to), against
//! hjv-api: the same requests in the same order, well-formed and broken in every part, get the
//! same verdict from both.

mod common;
/// relay/tests/common: `check_request`, the server checks of PROTOCOL.md §4.3 as the relay's
/// tests apply them.
#[allow(unused_imports)]
#[path = "../../../relay/tests/common/mod.rs"]
mod reference;

use common::*;
use hjv_api::{
    db, envelope,
    relay::{HELLO, HELLO_PATH, POLL, POLL_PATH},
    util::{b64, b64_decode, now, random_bytes},
    wake,
};
use hjv_relay::{
    config::{CommandKey, Server},
    https,
    identity::Identity,
    netif::{self, Gateway, Iface},
    relay::{Env, Relay, RelayConfig, State, Timing},
    wake::{SendOutcome, SendPlan},
};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

/// The egress address of the PC and the NAS: one site behind one address.
const SITE_IP: &str = "198.51.100.40";
const PC_MAC: &str = "00:00:5e:00:53:01";

/// A CA and a "localhost" certificate it signed: the CA's PEM, the server's TLS settings.
fn tls() -> (String, Arc<rustls::ServerConfig>) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "hjv-api relay test CA");
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
    let leaf_cert = leaf.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![rustls::pki_types::CertificateDer::from(
            leaf_cert.der().to_vec(),
        )],
        rustls::pki_types::PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into()),
    )
    .unwrap();
    (ca_cert.pem(), Arc::new(config))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// nginx's part: TLS for "localhost" on 127.0.0.1, each request passed on to hjv-api with
/// X-Real-IP set to `egress`. Returns the port and the CA's PEM.
async fn front(api: SocketAddr, egress: &'static str) -> (u16, String) {
    let (ca_pem, config) = tls();
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                continue;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(sock).await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while find(&buf, b"\r\n\r\n").is_none() {
                    match tls.read(&mut chunk).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let Some(line) = find(&buf, b"\r\n") else {
                    return;
                };
                let mut head = buf[..line + 2].to_vec();
                head.extend_from_slice(format!("X-Real-IP: {egress}\r\n").as_bytes());
                head.extend_from_slice(&buf[line + 2..]);
                let Ok(mut up) = TcpStream::connect(api).await else {
                    return;
                };
                if up.write_all(&head).await.is_ok() {
                    let _ = tokio::io::copy_bidirectional(&mut tls, &mut up).await;
                }
            });
        }
    });
    (port, ca_pem)
}

/// The NAS of the design's lab: eth0 on the PC's /22 wire (its default route through 10.1.4.1),
/// eth1 on another LAN, a container bridge; packets recorded, not sent.
struct Lab {
    sent: Arc<Mutex<Vec<SendPlan>>>,
}

fn iface(name: &str, addr: &str) -> Iface {
    Iface {
        name: name.to_owned(),
        flags: netif::FLAG_UP | netif::FLAG_BROADCAST | netif::FLAG_RUNNING,
        mac: Some([0x02, 0, 0, 0, 0, 1]),
        addrs: vec![addr.parse().unwrap()],
    }
}

impl Env for Lab {
    fn interfaces(&mut self) -> std::io::Result<Vec<Iface>> {
        Ok(vec![
            iface("eth0", "10.1.5.20/22"),
            iface("eth1", "192.168.50.10/24"),
            iface("docker0", "172.17.0.1/16"),
        ])
    }

    fn gateways(&mut self) -> Vec<Gateway> {
        vec![Gateway {
            iface: "eth0".to_owned(),
            ip: Ipv4Addr::new(10, 1, 4, 1),
            mac: Some([0x02, 0, 0, 0, 0, 0xfe]),
        }]
    }

    fn send(&mut self, plan: &SendPlan) -> SendOutcome {
        self.sent.lock().unwrap().push(plan.clone());
        SendOutcome {
            planned: plan.packets(),
            sent: plan.packets(),
            ..SendOutcome::default()
        }
    }

    fn exe_changed(&mut self) -> Option<String> {
        None
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d.min(Duration::from_millis(20)));
    }

    fn write_status(&mut self, _json: &str, _important: bool) {}
}

/// hjv-relay with the relay key of `seed`, trusting `keys`, for the hjv-api behind `port`.
fn relay(
    port: u16,
    ca_pem: &str,
    seed: [u8; 32],
    keys: Vec<CommandKey>,
    wait: u32,
) -> (Relay<Lab>, Arc<Mutex<Vec<SendPlan>>>) {
    let sent: Arc<Mutex<Vec<SendPlan>>> = Arc::default();
    let client = https::Client::new(
        Server {
            host: "localhost".to_owned(),
            port,
        },
        https::roots_from_pem(ca_pem.as_bytes()).unwrap(),
    )
    .unwrap();
    let config = RelayConfig {
        identity: Identity::from_seed(&seed),
        keys,
        client,
        os: "DSM 7.2.1-69057 Update 5".to_owned(),
        kernel: "5.10.55+".to_owned(),
        timing: Timing {
            wait,
            request_timeout: Duration::from_secs(35),
            pending_poll: Duration::from_millis(30),
            revoked_poll: Duration::from_millis(30),
            min_gap: Duration::ZERO,
            backoff_first: Duration::from_millis(5),
            backoff_max: Duration::from_millis(300),
            error_log_every: Duration::from_secs(600),
            sleep_slice: Duration::from_millis(15),
        },
    };
    (Relay::new(config, Lab { sent: sent.clone() }), sent)
}

fn command_key(seed: &[u8; 32], id: u32) -> CommandKey {
    CommandKey {
        id,
        key: ed25519_dalek::SigningKey::from_bytes(seed).verifying_key(),
    }
}

/// One step of the relay's loop, on a thread of its own: it blocks on its HTTPS requests.
async fn step(relay: Relay<Lab>) -> (Relay<Lab>, Duration) {
    tokio::task::spawn_blocking(move || {
        let mut relay = relay;
        let delay = relay.step();
        (relay, delay)
    })
    .await
    .unwrap()
}

/// The PC of HONG on the /22 wire, bound, its wake report sent from the site's address, asleep.
async fn pc_on_the_wire(h: &Harness, id: &str) -> (TestDevice, String) {
    let pc = TestDevice::windows(id);
    h.hbbs.register(&pc, Some(100));
    h.login_ok(HONG, Some(&pc)).await;
    let binding = h.binding_of(&pc.id).await;
    let wol = wol_report(PC_MAC, "10.1.6.7/22", "10.1.4.1", Some("02:00:00:00:00:fe"));
    let s = h
        .sync_from(&pc, &payload_with_wol(&binding, &wol, "h1"), SITE_IP)
        .await;
    assert_eq!(s.answer.status, 200, "{}", s.answer.text);
    h.state
        .db
        .call({
            let id = pc.id.clone();
            move |c| {
                c.execute(
                    "UPDATE devices SET last_sync_at = ?1 WHERE device_id = ?2",
                    rusqlite::params![now() - 100, id],
                )
            }
        })
        .await
        .unwrap();
    (pc, binding)
}

async fn relay_state(h: &Harness, id: &str) -> Option<db::RelayState> {
    let id = id.to_owned();
    h.state
        .db
        .call(move |c| db::relay(c, &id))
        .await
        .unwrap()
        .map(|r| r.state)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hjv_relay_wakes_a_pc_through_hjv_api() {
    let h = Harness::start().await;
    let (pc, binding) = pc_on_the_wire(&h, "525000001").await;
    let phone = h.login_ok(HONG, None).await;
    let (port, ca) = front(h.api, SITE_IP).await;
    let seed = [5u8; 32];
    let id = Identity::from_seed(&seed);
    let (relay, sent) = relay(port, &ca, seed, vec![command_key(&COMMAND_SEED, 1)], 4);

    // hello: pending; hjv-api keeps what the relay reported of its interfaces
    let (relay, _) = step(relay).await;
    assert_eq!(relay.state(), State::Pending);
    assert_eq!(relay_state(&h, &id.id).await, Some(db::RelayState::Pending));
    let row = h
        .state
        .db
        .call({
            let id = id.id.clone();
            move |c| db::relay(c, &id)
        })
        .await
        .unwrap()
        .unwrap();
    let names: Vec<&str> = row.ifaces.iter().map(|i| i.name.as_str()).collect();
    assert_eq!(
        names,
        ["eth0", "eth1"],
        "the container bridge is not reported"
    );
    assert_eq!(row.ifaces[0].gateway.as_deref(), Some("10.1.4.1"));
    assert_eq!(row.egress_ip.as_deref(), Some(SITE_IP));

    // approved by its code: on the PC's network, behind the PC's address
    let out = h
        .admin(
            &["relay", "approve", &id.code, "--owner-of", &pc.id],
            false,
            None,
        )
        .await
        .unwrap();
    assert!(
        out.contains("on the PC's network: yes (the same network and gateway)"),
        "{out}"
    );
    assert!(
        out.contains("the same egress address as the PC: yes"),
        "{out}"
    );

    // its next poll is active; with nothing to do hjv-api holds it for the relay's wait
    let started = Instant::now();
    let (relay, _) = step(relay).await;
    assert_eq!(relay.state(), State::Active);
    assert!(
        started.elapsed() >= Duration::from_millis(3500),
        "{:?}",
        started.elapsed()
    );

    // a wake while the relay's poll is held: the poll ends at once with the command
    let started = Instant::now();
    let held = tokio::spawn(step(relay));
    tokio::time::sleep(Duration::from_millis(300)).await;
    let a = h
        .post(&format!("/api/hjv/devices/{}/wake", pc.id), &phone, None)
        .await;
    assert_eq!(a.status, 202, "{}", a.text);
    assert_eq!(a.json["relay"]["match"], "exact");
    let job = a.json["job"].as_str().unwrap().to_owned();
    let (relay, delay) = held.await.unwrap();
    assert!(
        started.elapsed() < Duration::from_millis(3500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(delay, Duration::ZERO, "results go back at once");
    // the relay checked the command and sent on its own card on the PC's wire, to its own
    // broadcast and 255.255.255.255, ports 9 and 7, three rounds: twelve packets
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

    // the result comes in the next poll: sent, twelve packets
    let (relay, _) = step(relay).await;
    assert_eq!(relay.pending_results(), 0);
    let status = h.get(&format!("/api/hjv/wake/{job}"), &phone).await;
    assert_eq!(
        (
            status.json["state"].as_str(),
            status.json["packets"].as_i64(),
            status.json["error"].clone()
        ),
        (Some("sent"), Some(12), Value::Null)
    );
    // the PC wakes and syncs: confirmed
    tokio::time::sleep(Duration::from_millis(1100)).await;
    h.sync_from(&pc, &payload(&binding), SITE_IP).await;
    assert_eq!(wake::tick(&h.state).await.confirmed, 1);
    let status = h.get(&format!("/api/hjv/wake/{job}"), &phone).await;
    assert_eq!(status.json["state"], "confirmed");

    // revoked: its next poll says so at once
    h.admin(&["relay", "revoke", &id.id], false, None)
        .await
        .unwrap();
    let started = Instant::now();
    let (relay, _) = step(relay).await;
    assert_eq!(relay.state(), State::Revoked);
    assert!(started.elapsed() < Duration::from_secs(2));

    // deleted altogether: unknown, it says hello again and waits for approval
    h.state
        .db
        .call({
            let id = id.id.clone();
            move |c| c.execute("DELETE FROM relays WHERE relay_id = ?1", [id])
        })
        .await
        .unwrap();
    let (relay, _) = step(relay).await;
    assert_eq!(relay.state(), State::Unknown);
    let (relay, _) = step(relay).await;
    assert_eq!(relay.state(), State::Pending);
    assert_eq!(relay_state(&h, &id.id).await, Some(db::RelayState::Pending));
    assert_eq!(sent.lock().unwrap().len(), 1, "one command, sent once");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relay_that_does_not_trust_the_key_refuses_and_hjv_api_says_so() {
    let h = Harness::start().await;
    let (pc, _) = pc_on_the_wire(&h, "525000002").await;
    let phone = h.login_ok(HONG, None).await;
    let (port, ca) = front(h.api, SITE_IP).await;
    let seed = [6u8; 32];
    let id = Identity::from_seed(&seed);
    // built with the next key only (number 2), while hjv-api signs with key 1
    let (relay, sent) = relay(port, &ca, seed, vec![command_key(&NEXT_COMMAND_SEED, 2)], 0);
    let (relay, _) = step(relay).await;
    h.admin(
        &["relay", "approve", &id.code, "--owner-of", &pc.id],
        false,
        None,
    )
    .await
    .unwrap();
    let (relay, _) = step(relay).await;
    assert_eq!(relay.state(), State::Active);
    let a = h
        .post(&format!("/api/hjv/devices/{}/wake", pc.id), &phone, None)
        .await;
    let job = a.json["job"].as_str().unwrap().to_owned();
    let (relay, _) = step(relay).await;
    assert!(sent.lock().unwrap().is_empty());
    assert_eq!(relay.pending_results(), 1);
    let (relay, _) = step(relay).await;
    assert_eq!(relay.pending_results(), 0);
    let status = h.get(&format!("/api/hjv/wake/{job}"), &phone).await;
    assert_eq!(
        (status.json["state"].as_str(), status.json["error"].as_str()),
        (Some("failed"), Some("unknown_key"))
    );
    assert!(hjv_api::log::captured()
        .iter()
        .any(|l| l.contains("(unknown_key): it does not trust the command key in use")));
    drop(relay);
}

/// A small generator with a seed, so that a failing case can be run again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// `text` with one character changed.
fn flip(text: &str, rng: &mut Rng) -> String {
    let mut chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return "x".into();
    }
    let i = rng.below(chars.len());
    chars[i] = match rng.below(4) {
        0 => 'A',
        1 => '-',
        2 => '=',
        _ if chars[i] == 'b' => 'c',
        _ => 'b',
    };
    chars.into_iter().collect()
}

/// One run of requests from two relays, right and wrong in what is signed (the time, a nonce
/// again, the payload, another key, another ID) and in the envelope as sent (a field gone, of
/// another type, a character changed, the body cut, too large), each given to the relay's
/// reference and to hjv-api: (requests, accepted ones, verdicts by code). Before them, a hello
/// whose key is no point of the curve, which the generator does not make.
async fn compare(seed: u64, cases: usize) -> (usize, usize, HashMap<String, usize>) {
    let h = Harness::start().await;
    let relays = [
        TestRelay::new([5; 32]),
        TestRelay::new([6; 32]),
        TestRelay::new([7; 32]),
    ];
    let ifaces = json!([common::iface(
        "eth0",
        "192.0.2.10/24",
        Some("192.0.2.1"),
        None
    )]);
    let mut known: HashMap<String, [u8; 32]> = HashMap::new();
    let mut nonces: HashSet<String> = HashSet::new();
    let mut used: Vec<(String, [u8; 16])> = Vec::new();
    let mut rng = Rng(seed);
    let (mut accepted, mut by_code) = (0, HashMap::<String, usize>::new());
    // A hello whose `pk` is 32 bytes that are no point of the curve (y = 2), under the relay ID
    // those bytes give: its shape passes (PROTOCOL.md §4.3 step 2), and no signature verifies
    // under such a key, which is step 4's refusal from both.
    {
        let mut no_point = [0u8; 32];
        no_point[0] = 2;
        assert!(ed25519_dalek::VerifyingKey::from_bytes(&no_point).is_err());
        let id = hjv_api::relay::relay_id_of(&no_point);
        let body = envelope::sign_body(
            &HELLO,
            &relays[0].seed,
            &id,
            now(),
            &random_bytes::<16>(),
            &hello_payload(&ifaces),
        );
        let mut value: Value = serde_json::from_str(&body).unwrap();
        value["pk"] = json!(b64(&no_point));
        let bytes = value.to_string().into_bytes();
        let expected = reference::check_request("hello", &bytes, &known, &mut nonces, now())
            .map(|_| ())
            .unwrap_err();
        let a = h.relay_raw(HELLO_PATH, bytes, SITE_IP).await;
        let got = (a.status, a.json["error"].as_str().unwrap_or("ok"));
        assert_eq!(got, (401, "bad_signature"), "hjv-api: {}", a.text);
        assert_eq!(expected, got, "the relay's reference");
    }
    for case in 0..cases {
        let r = &relays[rng.below(2)];
        let hello = rng.below(3) == 0 || !known.contains_key(&r.id);
        let (spec, path, op) = if hello {
            (&HELLO, HELLO_PATH, "hello")
        } else {
            (&POLL, POLL_PATH, "poll")
        };
        let mut p = if hello {
            hello_payload(&ifaces)
        } else {
            poll_payload(0, &json!([]), None, &ifaces)
        };
        let (mut t, mut nonce, mut key, mut id) =
            (now(), random_bytes::<16>(), r.seed, r.id.clone());
        match rng.below(9) {
            0 => t += [301, -301, 300, -300, 100_000, -100_000][rng.below(6)],
            1 => {
                if let Some((_, n)) = used.iter().find(|(i, _)| *i == id) {
                    nonce = *n;
                }
            }
            2 => p = p.replacen("\"v\":1", "\"v\":2", 1),
            3 => p = "not json".into(),
            4 => key = relays[2].seed,
            5 => id = relays[2].id.clone(),
            _ => {}
        }
        let body = envelope::sign_body(spec, &key, &id, t, &nonce, &p);
        let mut value: Value = serde_json::from_str(&body).unwrap();
        if key != r.seed && hello {
            // the relay's own key in the envelope: only the signature is another key's
            value["pk"] = json!(b64(&r.public_key()));
        }
        let fields = ["id", "t", "n", "p", "s", "pk"];
        match rng.below(6) {
            0 => {
                value.as_object_mut().unwrap().remove(fields[rng.below(6)]);
            }
            1 => {
                let other = [
                    json!(5),
                    json!(null),
                    json!([]),
                    json!("x"),
                    json!(1.5),
                    json!(-3),
                ];
                value[fields[rng.below(6)]] = other[rng.below(6)].clone();
            }
            2 => {
                let field = ["id", "n", "s", "pk", "p"][rng.below(5)];
                if let Some(text) = value[field].as_str().map(str::to_owned) {
                    value[field] = json!(flip(&text, &mut rng));
                }
            }
            3 => value["extra"] = json!({"a": 1}),
            _ => {}
        }
        let mut bytes = value.to_string().into_bytes();
        match rng.below(40) {
            0 => bytes = vec![b' '; 65_537],
            1 => bytes.truncate(bytes.len() / 2),
            2 => bytes = b"[]".to_vec(),
            _ => {}
        }
        let at = now();
        let expected = match reference::check_request(op, &bytes, &known, &mut nonces, at) {
            Ok((id, _, envelope, _)) => {
                if op == "hello" {
                    let pk = b64_decode(envelope["pk"].as_str().unwrap()).unwrap();
                    known.insert(id, pk.try_into().unwrap());
                }
                (200, "ok".to_owned())
            }
            Err((status, code)) => (status, code.to_owned()),
        };
        let a = h.relay_raw(path, bytes, SITE_IP).await;
        let got = (
            a.status,
            a.json["error"].as_str().unwrap_or("ok").to_owned(),
        );
        // a request signed 300 seconds off may fall on either side of a second passing between
        // the reference's clock and hjv-api's
        let edge =
            [300, 301].contains(&(t - at).abs()) && (got.1 == "clock" || expected.1 == "clock");
        assert!(
            got == expected || edge,
            "seed {seed} case {case} ({op}): hjv-api {got:?}, the relay's reference {expected:?}: {}",
            a.text
        );
        if got.0 == 200 {
            accepted += 1;
            used.push((id, nonce));
        }
        *by_code.entry(got.1).or_default() += 1;
    }
    (cases, accepted, by_code)
}

#[tokio::test]
async fn the_relays_reference_checks_and_hjv_api_agree() {
    let mut codes: HashMap<String, usize> = HashMap::new();
    let (mut requests, mut accepted) = (0, 0);
    // within hjv-api's own limits, which the reference has none of: 60 hellos an hour and 120
    // polls a minute per known relay, of the requests that are that relay's own and of this
    // time, whatever becomes of them then (a run takes about a third of each limit)
    for seed in [
        0x9e37_79b9_7f4a_7c15u64,
        0xc2b2_ae3d_27d4_eb4f,
        0x1656_67b1_9e37_79f9,
    ] {
        let (n, ok, by_code) = compare(seed, 300).await;
        requests += n;
        accepted += ok;
        for (code, n) in by_code {
            *codes.entry(code).or_default() += n;
        }
    }
    // every verdict of PROTOCOL.md §4.3 came up, and a good share passed
    for code in [
        "ok",
        "too_large",
        "bad_request",
        "unknown_relay",
        "bad_signature",
        "clock",
        "replay",
    ] {
        assert!(
            codes.get(code).copied().unwrap_or(0) >= 5,
            "{code}: {codes:?}"
        );
    }
    assert!(accepted * 10 >= requests, "{accepted} of {requests}");
}
