// SPDX-License-Identifier: AGPL-3.0-only

//! Test support: an HTTPS server standing in for hjv-api (with a test CA for "localhost"), which
//! checks every request the way relay/PROTOCOL.md ("Server checks") tells the server to, written
//! from the document rather than from the relay's code; and a signer for wake commands.

#![allow(dead_code)]

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use hjv_relay::{
    config::{CommandKey, Server},
    https,
    identity::Identity,
    netif::{self, Gateway, Iface, Ipv4Net},
    relay::{Env, RelayConfig, Timing},
    wake::{SendOutcome, SendPlan},
};
use rustls::{
    pki_types::{CertificateDer, PrivateKeyDer},
    ServerConfig, ServerConnection, StreamOwned,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const COMMAND_KEY_SEED: [u8; 32] = [42; 32];
pub const RELAY_SEED: [u8; 32] = [5; 32];
pub const PC_MAC: &str = "00:00:5e:00:53:01";

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// A CA and a "localhost" certificate it signed; returns the CA's PEM and the server config.
pub fn tls() -> (String, Arc<ServerConfig>) {
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.distinguished_name
        .push(DnType::CommonName, "hjv-relay test CA");
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let leaf_key = KeyPair::generate().unwrap();
    let leaf = CertificateParams::new(vec!["localhost".to_owned()]).unwrap();
    let leaf_cert = leaf.signed_by(&leaf_key, &ca_cert, &ca_key).unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(leaf_cert.der().to_vec())],
            PrivateKeyDer::Pkcs8(leaf_key.serialize_der().into()),
        )
        .unwrap();
    (ca_cert.pem(), Arc::new(config))
}

/// A request that passed the server checks.
#[derive(Clone, Debug)]
pub struct Seen {
    pub op: &'static str,
    pub id: String,
    pub t: i64,
    pub envelope: Value,
    pub payload: Value,
    pub user_agent: String,
    pub host: String,
}

pub struct Reply {
    pub status: u16,
    pub body: Value,
    /// Held this long before answering (long-poll).
    pub delay: Duration,
}

impl Reply {
    pub fn ok(body: Value) -> Reply {
        Reply {
            status: 200,
            body,
            delay: Duration::ZERO,
        }
    }

    pub fn status(status: u16, body: Value) -> Reply {
        Reply {
            status,
            body,
            delay: Duration::ZERO,
        }
    }
}

type Handler = Box<dyn FnMut(&Seen, i64) -> Reply + Send>;

pub struct Shared {
    /// Requests that passed the checks.
    pub seen: Vec<Seen>,
    /// Requests the checks refused: (status, error).
    pub refused: Vec<(u16, String)>,
    /// Relay keys from verified hellos.
    pub known: HashMap<String, [u8; 32]>,
    pub nonces: HashSet<String>,
    /// Seconds added to this machine's clock to make the server's.
    pub clock_offset: i64,
    pub handler: Handler,
}

pub struct TestServer {
    pub port: u16,
    pub ca_pem: String,
    pub shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl TestServer {
    /// Starts on 127.0.0.1 of the calling thread's network namespace.
    pub fn start(handler: impl FnMut(&Seen, i64) -> Reply + Send + 'static) -> TestServer {
        let (ca_pem, config) = tls();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let shared = Arc::new(Mutex::new(Shared {
            seen: Vec::new(),
            refused: Vec::new(),
            known: HashMap::new(),
            nonces: HashSet::new(),
            clock_offset: 0,
            handler: Box::new(handler),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let (shared, stop) = (shared.clone(), stop.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((sock, _)) => {
                            let _ = serve(sock, &config, &shared);
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5))
                        }
                        Err(_) => break,
                    }
                }
            })
        };
        TestServer {
            port,
            ca_pem,
            shared,
            stop,
            thread: Some(thread),
        }
    }

    pub fn server(&self) -> Server {
        Server {
            host: "localhost".to_owned(),
            port: self.port,
        }
    }

    pub fn client(&self) -> https::Client {
        https::Client::new(
            self.server(),
            https::roots_from_pem(self.ca_pem.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    pub fn seen(&self) -> Vec<Seen> {
        self.shared.lock().unwrap().seen.clone()
    }

    pub fn refused(&self) -> Vec<(u16, String)> {
        self.shared.lock().unwrap().refused.clone()
    }

    pub fn set_handler(&self, handler: impl FnMut(&Seen, i64) -> Reply + Send + 'static) {
        self.shared.lock().unwrap().handler = Box::new(handler);
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn serve(
    sock: TcpStream,
    config: &Arc<ServerConfig>,
    shared: &Arc<Mutex<Shared>>,
) -> io::Result<()> {
    sock.set_nonblocking(false)?;
    sock.set_read_timeout(Some(Duration::from_secs(10)))?;
    let conn = ServerConnection::new(config.clone()).map_err(io::Error::other)?;
    let mut tls = StreamOwned::new(conn, sock);
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let mut chunk = [0u8; 4096];
        let n = tls.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|l| l.parse().ok())
        .unwrap_or(0);
    let mut body = buf[head_end..].to_vec();
    while body.len() < length {
        let mut chunk = [0u8; 4096];
        let n = tls.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let op = match request_line.split(' ').nth(1) {
        Some("/api/hjv/relay/hello") if request_line.starts_with("POST ") => "hello",
        Some("/api/hjv/relay/poll") if request_line.starts_with("POST ") => "poll",
        _ => "",
    };
    let reply = {
        let mut s = shared.lock().unwrap();
        let now = unix_now() + s.clock_offset;
        if op.is_empty() {
            Reply::status(404, json!({"error": "not_found"}))
        } else {
            let Shared { known, nonces, .. } = &mut *s;
            match check_request(op, &body, known, nonces, now) {
                Err((status, error)) => {
                    s.refused.push((status, error.to_owned()));
                    Reply::status(status, json!({"error": error, "server_time": now}))
                }
                Ok((id, t, envelope, payload)) => {
                    if op == "hello" {
                        let pk = decode32(envelope["pk"].as_str().unwrap_or_default()).unwrap();
                        s.known.insert(id.clone(), pk);
                    }
                    let seen = Seen {
                        op,
                        id,
                        t,
                        envelope,
                        payload,
                        user_agent: headers.get("user-agent").cloned().unwrap_or_default(),
                        host: headers.get("host").cloned().unwrap_or_default(),
                    };
                    s.seen.push(seen.clone());
                    (s.handler)(&seen, now)
                }
            }
        }
    };
    thread::sleep(reply.delay);
    let text = reply.body.to_string();
    let response = format!(
        "HTTP/1.1 {} X\r\nContent-Type: application/json; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        reply.status,
        text.len()
    );
    tls.write_all(response.as_bytes())?;
    tls.conn.send_close_notify();
    tls.flush()
}

fn decode32(s: &str) -> Option<[u8; 32]> {
    STANDARD.decode(s).ok()?.try_into().ok()
}

fn relay_id_of(pk: &[u8; 32]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let digest = Sha256::digest(pk);
    let mut bits = 0u128;
    for b in &digest[..10] {
        bits = (bits << 8) | u128::from(*b);
    }
    let chars: String = (0..16)
        .map(|i| char::from(ALPHABET[((bits >> (75 - 5 * i)) & 31) as usize]))
        .collect();
    format!("r-{chars}")
}

/// The server checks of PROTOCOL.md, in order: size, envelope shape, the relay's key, the
/// signature, the time window, the nonce, the payload.
pub fn check_request(
    op: &str,
    body: &[u8],
    known: &HashMap<String, [u8; 32]>,
    nonces: &mut HashSet<String>,
    now: i64,
) -> Result<(String, i64, Value, Value), (u16, &'static str)> {
    let bad = (400, "bad_request");
    if body.len() > 65536 {
        return Err((413, "too_large"));
    }
    let envelope: Value = serde_json::from_slice(body).map_err(|_| bad)?;
    let id = envelope["id"].as_str().ok_or(bad)?.to_owned();
    let t = envelope["t"].as_i64().ok_or(bad)?;
    let n = envelope["n"].as_str().ok_or(bad)?.to_owned();
    let p = envelope["p"].as_str().ok_or(bad)?.to_owned();
    let s = envelope["s"].as_str().ok_or(bad)?;
    let id_ok = id.len() == 18
        && id.starts_with("r-")
        && id[2..]
            .bytes()
            .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b));
    let n_ok = n.len() == 22 && URL_SAFE_NO_PAD.decode(&n).is_ok_and(|b| b.len() == 16);
    let signature: [u8; 64] = STANDARD
        .decode(s)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(bad)?;
    if !id_ok || !n_ok || t <= 0 {
        return Err(bad);
    }
    let pk = if op == "hello" {
        let pk = decode32(envelope["pk"].as_str().ok_or(bad)?).ok_or(bad)?;
        if relay_id_of(&pk) != id {
            return Err(bad);
        }
        pk
    } else {
        *known.get(&id).ok_or((404, "unknown_relay"))?
    };
    let digest = STANDARD.encode(Sha256::digest(p.as_bytes()));
    let msg = format!("hjv-relay-v1\0{op}\0{id}\0{t}\0{n}\0{digest}");
    let key = VerifyingKey::from_bytes(&pk).map_err(|_| bad)?;
    key.verify_strict(msg.as_bytes(), &Signature::from_bytes(&signature))
        .map_err(|_| (401, "bad_signature"))?;
    if (t - now).abs() > 300 {
        return Err((400, "clock"));
    }
    if !nonces.insert(format!("{id}/{n}")) {
        return Err((409, "replay"));
    }
    let payload: Value = serde_json::from_str(&p).map_err(|_| bad)?;
    if payload["v"] != 1 {
        return Err(bad);
    }
    Ok((id, t, envelope, payload))
}

pub fn command_signing_key() -> SigningKey {
    SigningKey::from_bytes(&COMMAND_KEY_SEED)
}

pub fn command_keys() -> Vec<CommandKey> {
    vec![CommandKey {
        id: 1,
        key: command_signing_key().verifying_key(),
    }]
}

/// `{"c", "s", "k"}` for a command, signed with `key`.
pub fn signed(command: &Value, key: &SigningKey, k: u64) -> Value {
    let c = command.to_string();
    let mut msg = b"hjv-wake-v1\0".to_vec();
    msg.extend_from_slice(c.as_bytes());
    json!({"c": c, "s": STANDARD.encode(key.sign(&msg).to_bytes()), "k": k})
}

pub fn wake_command(job: &str, relay: &str, now: i64, net: &str) -> Value {
    json!({
        "v": 1, "job": job, "relay": relay, "iat": now, "exp": now + 60, "net": net,
        "macs": [PC_MAC], "ports": [9, 7], "repeat": 3, "gap_ms": 200
    })
}

pub fn command_hash(c: &str) -> String {
    Sha256::digest(c.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn fast_timing() -> Timing {
    Timing {
        wait: 1,
        request_timeout: Duration::from_secs(10),
        pending_poll: Duration::from_millis(30),
        revoked_poll: Duration::from_millis(3600),
        min_gap: Duration::ZERO,
        backoff_first: Duration::from_millis(5),
        backoff_max: Duration::from_millis(300),
        error_log_every: Duration::from_secs(600),
        sleep_slice: Duration::from_millis(15),
    }
}

pub fn relay_config(server: &TestServer) -> RelayConfig {
    RelayConfig {
        identity: Identity::from_seed(&RELAY_SEED),
        keys: command_keys(),
        client: server.client(),
        os: "DSM 7.2.1-69057 Update 5".to_owned(),
        kernel: "5.10.55+".to_owned(),
        timing: fast_timing(),
    }
}

pub fn iface(name: &str, addrs: &[&str]) -> Iface {
    Iface {
        name: name.to_owned(),
        flags: netif::FLAG_UP | netif::FLAG_BROADCAST | netif::FLAG_RUNNING,
        mac: Some([0x02, 0x00, 0x00, 0, 0, 1]),
        addrs: addrs
            .iter()
            .map(|a| a.parse::<Ipv4Net>().unwrap())
            .collect(),
    }
}

/// Interfaces of the lab's NAS, a recorder instead of the network, and an executable that
/// "changes" and a stop marker that appears when told to.
pub struct FakeEnv {
    pub ifaces: Arc<Mutex<Vec<Iface>>>,
    pub sent: Arc<Mutex<Vec<SendPlan>>>,
    pub exe_changed: Arc<AtomicBool>,
    pub stop: Arc<AtomicBool>,
    pub tidied: Arc<AtomicUsize>,
    pub statuses: Arc<Mutex<Vec<(String, bool)>>>,
    pub sleeps: Arc<Mutex<Vec<Duration>>>,
}

impl FakeEnv {
    pub fn new() -> FakeEnv {
        FakeEnv {
            ifaces: Arc::new(Mutex::new(vec![
                iface("eth0", &["10.1.5.20/22"]),
                iface("eth1", &["192.168.50.10/24"]),
                iface("docker0", &["172.17.0.1/16"]),
            ])),
            sent: Arc::default(),
            exe_changed: Arc::default(),
            stop: Arc::default(),
            tidied: Arc::default(),
            statuses: Arc::default(),
            sleeps: Arc::default(),
        }
    }
}

impl Env for FakeEnv {
    fn interfaces(&mut self) -> io::Result<Vec<Iface>> {
        Ok(self.ifaces.lock().unwrap().clone())
    }

    fn gateways(&mut self) -> Vec<Gateway> {
        vec![Gateway {
            iface: "eth1".to_owned(),
            ip: "192.168.50.1".parse().unwrap(),
            mac: Some([0x02, 0x00, 0x00, 0, 0, 0xfe]),
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
        self.exe_changed
            .load(Ordering::SeqCst)
            .then(|| "test".to_owned())
    }

    fn stop_requested(&mut self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn tidy(&mut self) {
        self.tidied.fetch_add(1, Ordering::SeqCst);
    }

    fn sleep(&mut self, d: Duration) {
        self.sleeps.lock().unwrap().push(d);
        thread::sleep(d.min(Duration::from_millis(20)));
    }

    fn write_status(&mut self, json: &str, important: bool) {
        self.statuses
            .lock()
            .unwrap()
            .push((json.to_owned(), important));
    }
}

/// Collects this thread's log lines.
pub fn capture_log() -> Arc<Mutex<Vec<String>>> {
    let lines: Arc<Mutex<Vec<String>>> = Arc::default();
    let sink = lines.clone();
    hjv_relay::log::capture(Some(Box::new(move |line: &str| {
        eprint!("{line}");
        sink.lock().unwrap().push(line.to_owned());
    })));
    lines
}

/// hjv-relay built the way a release is, but for a test server on localhost and the test command
/// key: `run` works against [`TestServer`] with `--server localhost:<port> --ca <its CA>`. Built
/// once into target/netns-configured (the namespace and boot-up loop tests share it).
pub fn configured_relay() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = manifest.join("target").join("netns-configured");
    let key = STANDARD.encode(command_signing_key().verifying_key().to_bytes());
    let status = std::process::Command::new(env!("CARGO"))
        .current_dir(manifest)
        .args(["build", "--offline", "--locked", "--bin", "hjv-relay"])
        .env("CARGO_TARGET_DIR", &target)
        .env("HJV_SERVER", "localhost")
        .env("HJV_COMMAND_PUBKEY", format!("1:{key}"))
        .env_remove("HJV_BUILD_COMMIT")
        .status()
        .unwrap();
    assert!(
        status.success(),
        "building the test-configured hjv-relay failed"
    );
    target.join("debug").join("hjv-relay")
}

pub fn wait_until(what: &str, timeout: Duration, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + timeout;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}
