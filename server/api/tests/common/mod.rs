// SPDX-License-Identifier: AGPL-3.0-only

//! The test harness: hjv-api in-process on 127.0.0.1, with the fake SSO (fake_sso.rs), a fake
//! hbbs console that answers `hjv-pk` like patch 0004, an admin socket and a client that seals
//! and signs logins as client patch 0030 will: every login makes a new account key and seals it
//! with its proof (keys.rs), and `get`, `post` and `delete` sign with the key of the token's login
//! (proof.rs), as the app's request hook does. `sync` is a PC's sync (pcsync.rs), signed with its
//! device key, and checks the trust list of the answer as the PC does (design v3 §3-6). The list
//! keys are the shared test vectors' (tests/data/v3-vectors.json): key 1 of seed [1; 32] signs,
//! key 2 of seed [9; 32] is the other one the apps pin.

#![allow(dead_code)]

use hjv_api::{
    admin::{self, AdminRequest},
    config::{Config, Secrets},
    db, devsig, envelope,
    fake_sso::{self, FakeSso},
    hbbs, keys,
    oplog::OpLog,
    pcsync, proof, relay, seal,
    server::{self, Listener},
    state::AppState,
    tfa::SecondFactor,
    trust,
    util::{b64, b64_decode, b64url, now, random_bytes},
};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::Bytes,
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, Request,
};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::watch,
};
use zeroize::Zeroizing;

pub const SEAL_SECRET: [u8; 32] = [42; 32];
pub const LOG_KEY: [u8; 32] = [7; 32];
/// hbbs's key pair (the seed of its id_ed25519), which signs the fake console's answers
pub const HBBS_SEED: [u8; 32] = [33; 32];

/// HJV_API_HBBS_PUBKEY for the fake console.
pub fn hbbs_pubkey() -> String {
    b64(&devsig::public_key(&HBBS_SEED))
}

/// The list key hjv-api signs with (number 1), and the next one the apps pin (number 2): the
/// shared vectors' keys.
pub const LIST_SEED: [u8; 32] = [1; 32];
pub const NEXT_LIST_SEED: [u8; 32] = [9; 32];

/// The command key hjv-api signs wake commands with (number 1: relay/PROTOCOL.md §13's key, the
/// seed 20 21 … 3f), and the next one the relays pin (number 2).
pub const COMMAND_SEED: [u8; 32] = {
    let mut seed = [0u8; 32];
    let mut i = 0;
    while i < 32 {
        seed[i] = i as u8 + 32;
        i += 1;
    }
    seed
};
pub const NEXT_COMMAND_SEED: [u8; 32] = [43; 32];

/// HJV_API_COMMAND_PUBKEYS, as the test relays pin them (HJV_COMMAND_PUBKEY).
pub fn command_pubkeys() -> String {
    format!(
        "{},{}",
        trust::entry(1, &devsig::public_key(&COMMAND_SEED)),
        trust::entry(2, &devsig::public_key(&NEXT_COMMAND_SEED))
    )
}

/// HJV_API_TRUST_PUBKEYS, as the test apps pin them (HJV_TRUST_PUBKEYS).
pub fn trust_pubkeys() -> String {
    format!(
        "{},{}",
        trust::entry(1, &devsig::public_key(&LIST_SEED)),
        trust::entry(2, &devsig::public_key(&NEXT_LIST_SEED))
    )
}

/// The test accounts of the fake SSO.
pub const HONG: (&str, &str) = ("2026000001", "hjv-test-pass-1");
pub const KIM: (&str, &str) = ("2026000002", "hjv-test-pass-2");
pub const LEE: (&str, &str) = ("Y9001", "hjv-test-pass-3");

/// A device as the app runs it: an ID, the Ed25519 key pair it registered with hbbs, and its
/// uuid (the same for every login from it).
#[derive(Clone)]
pub struct TestDevice {
    pub id: String,
    pub seed: [u8; 32],
    pub os: String,
    pub installed: bool,
    pub hostname: String,
    pub uuid: String,
}

impl TestDevice {
    pub fn windows(id: &str) -> TestDevice {
        let mut seed = [0u8; 32];
        seed[..id.len().min(32)].copy_from_slice(&id.as_bytes()[..id.len().min(32)]);
        seed[31] = 0x5a;
        TestDevice {
            id: id.to_owned(),
            seed,
            os: "windows".into(),
            installed: true,
            hostname: format!("PC-{id}"),
            uuid: b64(format!("uuid-{id}").as_bytes()),
        }
    }

    pub fn phone(id: &str) -> TestDevice {
        TestDevice {
            os: "ios".into(),
            installed: false,
            hostname: "iPhone15,2".into(),
            ..TestDevice::windows(id)
        }
    }

    pub fn public_key(&self) -> [u8; 32] {
        devsig::public_key(&self.seed)
    }
}

/// What the fake hbbs knows of a device.
#[derive(Clone)]
struct Entry {
    public_key: [u8; 32],
    /// seconds since it registered
    age: Option<u64>,
    /// the key pair that signs the answer: hbbs's, or another program's that took the port
    signer: [u8; 32],
}

type Registry = Arc<Mutex<HashMap<String, Entry>>>;

/// The hbbs loopback console, answering `hjv-pk <id>` as patch 0004 does.
pub struct FakeHbbs {
    pub addr: SocketAddr,
    keys: Registry,
    pub unpatched: Arc<AtomicBool>,
    pub queries: Arc<AtomicUsize>,
}

impl FakeHbbs {
    pub async fn start() -> FakeHbbs {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let keys: Registry = Default::default();
        let unpatched = Arc::new(AtomicBool::new(false));
        let queries = Arc::new(AtomicUsize::new(0));
        let (k, u, q) = (keys.clone(), unpatched.clone(), queries.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    continue;
                };
                let (k, u, q) = (k.clone(), u.clone(), q.clone());
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let Ok(n) = stream.read(&mut buf).await else {
                        return;
                    };
                    q.fetch_add(1, Ordering::SeqCst);
                    let cmd = String::from_utf8_lossy(&buf[..n]).trim().to_owned();
                    let mut parts = cmd.split(' ');
                    let reply = match (parts.next(), parts.next()) {
                        (Some("hjv-pk"), Some(id)) if !u.load(Ordering::SeqCst) => {
                            let entry = k.lock().unwrap().get(id).cloned();
                            let signed = entry
                                .as_ref()
                                .map(|e| hbbs::sign_id_pk(&e.signer, id, &e.public_key));
                            hbbs::format_reply(id, signed.as_deref(), entry.and_then(|e| e.age))
                        }
                        _ => String::new(),
                    };
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        FakeHbbs {
            addr,
            keys,
            unpatched,
            queries,
        }
    }

    fn insert(&self, id: &str, public_key: [u8; 32], age: Option<u64>, signer: [u8; 32]) {
        self.keys.lock().unwrap().insert(
            id.to_owned(),
            Entry {
                public_key,
                age,
                signer,
            },
        );
    }

    /// The device registered with hbbs, seen `age` seconds ago.
    pub fn register(&self, device: &TestDevice, age: Option<u64>) {
        self.insert(&device.id, device.public_key(), age, HBBS_SEED);
    }

    pub fn register_key(&self, id: &str, key: [u8; 32]) {
        self.insert(id, key, Some(1), HBBS_SEED);
    }

    /// Another program on the console's port answers for the device with a key of its choice,
    /// signed with a key pair of its own.
    pub fn forge(&self, id: &str, key: [u8; 32]) {
        self.insert(id, key, Some(1), [99; 32]);
    }

    /// hbbs knows the key but not when the device last registered (as after a restart).
    pub fn forget_seen(&self, id: &str) {
        if let Some(entry) = self.keys.lock().unwrap().get_mut(id) {
            entry.age = None;
        }
    }
}

/// How a test login carries its account key (keys.rs).
#[derive(Clone, Copy, Debug)]
pub enum KeyChoice {
    /// a new key, as the app makes for every login
    Fresh,
    /// the key of this seed
    Seed([u8; 32]),
    /// no `ck` and `cp`
    Omit,
    /// `ck` of a new key, `cp` signed by another
    WrongProof,
}

#[derive(Default)]
pub struct Options {
    /// settings over the harness's defaults
    pub vars: Vec<(&'static str, String)>,
    pub second_factor: Option<Arc<dyn SecondFactor>>,
    /// portal IDs on the allowlist (the three test accounts when empty)
    pub allow: Vec<&'static str>,
    pub sso_slow: Option<Duration>,
    /// start from a copy of this database (a restored backup), its allowlist as it is
    pub db_file: Option<PathBuf>,
    /// no command key (credential hjviewer-api-command-key): no wake command is signed
    pub no_command_key: bool,
}

pub struct Harness {
    pub api: SocketAddr,
    pub state: Arc<AppState>,
    pub sso: FakeSso,
    pub hbbs: FakeHbbs,
    pub dir: PathBuf,
    pub admin_socket: PathBuf,
    pub oplog: PathBuf,
    pub seal_pk: [u8; 32],
    client: Client<HttpConnector, Full<Bytes>>,
    stop: watch::Sender<bool>,
    /// the account key of each login body (by its sealed password field) and of each token
    keys: Mutex<HashMap<String, [u8; 32]>>,
}

/// An answer, read.
#[derive(Debug)]
pub struct Answer {
    pub status: u16,
    pub headers: HeaderMap,
    pub text: String,
    pub json: Value,
    pub elapsed: Duration,
}

impl Answer {
    pub fn token(&self) -> String {
        self.json["access_token"]
            .as_str()
            .unwrap_or_else(|| panic!("no token in {} {}", self.status, self.text))
            .to_owned()
    }
}

/// A local address that refuses connections at once, for as long as the returned socket lives:
/// bound and never listening, so that no other test is handed its port meanwhile.
pub fn refusing_addr() -> (tokio::net::TcpSocket, SocketAddr) {
    let socket = tokio::net::TcpSocket::new_v4().unwrap();
    socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let addr = socket.local_addr().unwrap();
    (socket, addr)
}

/// A new directory for one test's files and Unix sockets. It lives under a short base, not the
/// target directory: a socket path has to fit in 108 bytes (SUN_LEN), however deep the checkout.
pub fn temp_dir(name: &str) -> PathBuf {
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() <= 40 {
        base
    } else {
        PathBuf::from("/tmp")
    };
    let dir = base.join(format!(
        "hjv-{name}-{}-{}",
        std::process::id(),
        hjv_api::util::hex(&random_bytes::<4>())
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

impl Harness {
    pub async fn start() -> Harness {
        Harness::with(Options::default()).await
    }

    pub async fn with(opts: Options) -> Harness {
        hjv_api::log::capture();
        let sso_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sso = fake_sso::start(
            sso_listener,
            opts.sso_slow.unwrap_or(Duration::from_secs(3)),
        );
        let hbbs = FakeHbbs::start().await;
        let mut vars: HashMap<String, String> = [
            ("HJV_API_SSO_TOKEN_URL", sso.token_url()),
            ("HJV_API_SSO_USERINFO_URL", sso.user_url()),
            ("HJV_API_SSO_CLIENT_ID", fake_sso::CLIENT_ID.to_owned()),
            ("HJV_API_HBBS_CONSOLE", hbbs.addr.to_string()),
            ("HJV_API_HBBS_PUBKEY", hbbs_pubkey()),
            ("HJV_API_FAILURE_DELAY_MS", "60".to_owned()),
            ("HJV_API_FAILURE_JITTER_MS", "20".to_owned()),
            ("HJV_API_SSO_TIMEOUT_MS", "1500".to_owned()),
            // tests log one account in several times a minute: more than a quarter of 20 calls
            ("HJV_API_LIMIT_SSO", "100/60".to_owned()),
            ("HJV_API_TRUST_PUBKEYS", trust_pubkeys()),
            ("HJV_API_COMMAND_PUBKEYS", command_pubkeys()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        for (k, v) in opts.vars {
            vars.insert(k.to_owned(), v);
        }
        let config = Config::from_vars(&vars).unwrap();
        let secrets = Secrets {
            sso_basic: Zeroizing::new(fake_sso::BASIC.to_owned()),
            sso_api_uuid: Zeroizing::new(fake_sso::API_UUID.to_owned()),
            seal_keys: vec![Zeroizing::new(SEAL_SECRET)],
            log_key: Zeroizing::new(LOG_KEY.to_vec()),
            list_key: Zeroizing::new(LIST_SEED),
            command_key: if opts.no_command_key {
                None
            } else {
                Some(Zeroizing::new(COMMAND_SEED))
            },
        };
        let dir = temp_dir("harness");
        let db_path = dir.join("hjv-api.sqlite3");
        if let Some(file) = &opts.db_file {
            std::fs::copy(file, &db_path).unwrap();
        }
        let mut conn = db::open(&db_path).unwrap();
        if opts.db_file.is_none() {
            let allow: Vec<String> = if opts.allow.is_empty() {
                vec![HONG.0, KIM.0, LEE.0]
            } else {
                opts.allow
            }
            .iter()
            .map(|id| hjv_api::util::login_key(id))
            .collect();
            db::allow_add(&mut conn, &allow, now()).unwrap();
        }
        let state = AppState::new(config, secrets, conn, opts.second_factor).unwrap();

        let (stop, stop_rx) = watch::channel(false);
        let stopped = move || {
            let mut rx = stop_rx.clone();
            async move {
                let _ = rx.wait_for(|s| *s).await;
            }
        };
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = listener.local_addr().unwrap();
        tokio::spawn(server::serve(
            state.clone(),
            Listener::Tcp(listener),
            stopped(),
        ));
        let admin_socket = dir.join("admin.sock");
        let oplog = dir.join("operator.log");
        let admin_listener = admin::bind(&admin_socket).unwrap();
        tokio::spawn(admin::serve(
            state.clone(),
            admin_listener,
            admin_socket.clone(),
            Arc::new(OpLog::open(&oplog).unwrap()),
            stopped(),
        ));
        let seal_pk = *crypto_box::SecretKey::from_bytes(SEAL_SECRET)
            .public_key()
            .as_bytes();
        Harness {
            api,
            state,
            sso,
            hbbs,
            dir,
            admin_socket,
            oplog,
            seal_pk,
            client: Client::builder(TokioExecutor::new()).build_http(),
            stop,
            keys: Mutex::new(HashMap::new()),
        }
    }

    /// A login body as the app with patch 0008 sends it; `device` signs when given.
    pub fn login_body(&self, username: &str, password: &str, device: Option<&TestDevice>) -> Value {
        self.login_body_at(
            username,
            password,
            device,
            now(),
            &b64(&random_bytes::<16>()),
        )
    }

    pub fn login_body_at(
        &self,
        username: &str,
        password: &str,
        device: Option<&TestDevice>,
        time: i64,
        nonce: &str,
    ) -> Value {
        self.login_body_with(username, password, device, time, nonce, KeyChoice::Fresh)
    }

    /// A login body as the app sends it, the account key as `key` says. A device logs in with its
    /// own uuid; a login without one gets a uuid of its own, as another phone would.
    pub fn login_body_with(
        &self,
        username: &str,
        password: &str,
        device: Option<&TestDevice>,
        time: i64,
        nonce: &str,
        key: KeyChoice,
    ) -> Value {
        let id = device.map_or("", |d| d.id.as_str());
        let seed = match key {
            KeyChoice::Seed(seed) => seed,
            _ => random_bytes::<32>(),
        };
        let (ck, cp) = keys::proof(&seed, nonce, id);
        let sealed = match key {
            KeyChoice::Omit => seal::seal_password(&self.seal_pk, password, time, nonce, id),
            KeyChoice::WrongProof => {
                let (_, other) = keys::proof(&random_bytes::<32>(), nonce, id);
                seal::seal_login(&self.seal_pk, password, time, nonce, id, &ck, &other)
            }
            _ => seal::seal_login(&self.seal_pk, password, time, nonce, id, &ck, &cp),
        };
        self.keys.lock().unwrap().insert(sealed.clone(), seed);
        let uuid = device.map_or_else(|| b64(&random_bytes::<12>()), |d| d.uuid.clone());
        let mut body = json!({
            "username": username,
            "password": sealed,
            "id": id,
            "uuid": uuid,
            "autoLogin": true,
            "type": "account",
            "deviceInfo": {
                "os": device.map_or("ios", |d| d.os.as_str()),
                "type": "client",
                "name": device.map_or("iPhone15,2", |d| d.hostname.as_str()),
            },
        });
        if let Some(d) = device {
            let field = body["password"].as_str().unwrap().to_owned();
            let opened = self.state.seal.open(&field).unwrap();
            let message =
                devsig::login_message(&d.id, time, nonce, &opened.sealed_hash, d.installed, &d.os);
            body["hjv"] = json!({
                "v": 1,
                "sig": devsig::sign(&d.seed, &message),
                "installed": d.installed,
                "ver": "1.6.0",
            });
        }
        body
    }

    /// Stops the service, as SIGTERM does: no new requests, the open ones finish.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// The account key's seed of a token's login, when this harness made the login.
    pub fn key_of(&self, token: &str) -> Option<[u8; 32]> {
        self.keys.lock().unwrap().get(token).copied()
    }

    /// Takes the account keys of `other`'s logins, so that their tokens sign here as their apps
    /// would (a restored database, the same apps).
    pub fn adopt_keys(&self, other: &Harness) {
        let theirs = other.keys.lock().unwrap().clone();
        self.keys.lock().unwrap().extend(theirs);
    }

    /// The database file this harness serves.
    pub fn db_path(&self) -> PathBuf {
        self.dir.join("hjv-api.sqlite3")
    }

    /// The `X-HJV-Proof` the app would send with a request (with the token's key, now).
    pub fn proof_for(
        &self,
        method: &str,
        target: &str,
        body: &[u8],
        token: &str,
    ) -> Option<String> {
        let seed = self.key_of(token)?;
        Some(proof::header_value(
            &seed,
            method,
            target,
            now(),
            &random_bytes::<16>(),
            body,
            token,
        ))
    }

    pub async fn request(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: Option<Vec<u8>>,
    ) -> Answer {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("http://{}{path}", self.api));
        for (k, v) in headers {
            builder = builder.header(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        let request = builder
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .unwrap();
        let started = Instant::now();
        let response = self.client.request(request).await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let text = String::from_utf8(bytes.to_vec()).unwrap();
        Answer {
            status,
            headers,
            json: serde_json::from_str(&text).unwrap_or(Value::Null),
            text,
            elapsed: started.elapsed(),
        }
    }

    /// POST /api/login from `ip`; a token in the answer is filed with the body's account key.
    pub async fn login(&self, body: &Value, ip: &str) -> Answer {
        let answer = self
            .request(
                Method::POST,
                "/api/login",
                &[("content-type", "application/json"), ("x-real-ip", ip)],
                Some(body.to_string().into_bytes()),
            )
            .await;
        if let (Some(token), Some(field)) = (
            answer.json["access_token"].as_str(),
            body["password"].as_str(),
        ) {
            let mut keys = self.keys.lock().unwrap();
            if let Some(seed) = keys.get(field).copied() {
                keys.insert(token.to_owned(), seed);
            }
        }
        answer
    }

    /// Logs in a test account from `device`; returns the token.
    pub async fn login_ok(&self, account: (&str, &str), device: Option<&TestDevice>) -> String {
        self.login_ok_from(account, device, "10.9.9.9").await
    }

    /// [`Harness::login_ok`] from the client address `ip`.
    pub async fn login_ok_from(
        &self,
        account: (&str, &str),
        device: Option<&TestDevice>,
        ip: &str,
    ) -> String {
        let answer = self
            .login(&self.login_body(account.0, account.1, device), ip)
            .await;
        assert_eq!(answer.status, 200, "{}", answer.text);
        answer.token()
    }

    /// A token request as the app sends it: the token and, when the harness knows the token's
    /// key, its `X-HJV-Proof`.
    pub async fn signed(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Vec<u8>>,
    ) -> Answer {
        let auth = format!("Bearer {token}");
        let proof = self.proof_for(
            method.as_str(),
            path,
            body.as_deref().unwrap_or_default(),
            token,
        );
        let mut headers = vec![("authorization", auth.as_str())];
        if body.is_some() {
            headers.push(("content-type", "application/json"));
        }
        if let Some(p) = &proof {
            headers.push((proof::HEADER, p.as_str()));
        }
        self.request(method, path, &headers, body).await
    }

    pub async fn get(&self, path: &str, token: &str) -> Answer {
        self.signed(Method::GET, path, token, None).await
    }

    /// POST with a JSON body, as the app's currentUser and logout.
    pub async fn post(&self, path: &str, token: &str, body: Option<Value>) -> Answer {
        self.signed(
            Method::POST,
            path,
            token,
            body.map(|b| b.to_string().into_bytes()),
        )
        .await
    }

    pub async fn delete(&self, path: &str, token: &str) -> Answer {
        self.signed(Method::DELETE, path, token, None).await
    }

    pub async fn admin(
        &self,
        argv: &[&str],
        reveal: bool,
        input: Option<&str>,
    ) -> Result<String, String> {
        admin::send(
            &self.admin_socket,
            &AdminRequest {
                argv: argv.iter().map(|s| s.to_string()).collect(),
                reveal,
                sudo_user: "tester".into(),
                input: input.map(str::to_owned),
            },
        )
        .await
    }

    /// Attempts counted under a scope and key now (failures, or successes in the ok-* scopes).
    pub async fn counted(&self, scope: &'static str, key: String) -> i64 {
        self.state
            .db
            .call(move |c| {
                c.query_row(
                    "SELECT COUNT(*) FROM attempts WHERE scope = ?1 AND key = ?2 AND counts = 1",
                    [scope, key.as_str()],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    /// Attempts under a scope and key, counted or still running.
    pub async fn attempts(&self, scope: &'static str, key: String) -> i64 {
        self.state
            .db
            .call(move |c| {
                c.query_row(
                    "SELECT COUNT(*) FROM attempts WHERE scope = ?1 AND key = ?2",
                    [scope, key.as_str()],
                    |r| r.get(0),
                )
            })
            .await
            .unwrap()
    }

    /// The login records' results, oldest first.
    pub async fn results(&self) -> Vec<String> {
        self.state
            .db
            .call(|c| {
                let mut stmt = c.prepare("SELECT kind || ':' || result FROM events ORDER BY id")?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                rows.collect()
            })
            .await
            .unwrap()
    }

    pub fn account_tag(&self, id: &str) -> String {
        self.state.tags.account(&hjv_api::util::login_key(id))
    }

    /// The binding ID hjv-api holds for a PC, in hex ("" when no account has it).
    pub async fn binding_of(&self, id: &str) -> String {
        let id = id.to_owned();
        self.state
            .db
            .call(move |c| db::device(c, &id))
            .await
            .unwrap()
            .and_then(|d| d.binding_id)
            .map_or_else(String::new, |b| hjv_api::util::hex(&b))
    }

    /// The change serial now.
    pub async fn change_serial(&self) -> i64 {
        self.state.db.call(|c| db::change_serial(c)).await.unwrap()
    }

    /// POST /api/hjv/pc/sync with this body, as the PC's service sends it.
    pub async fn sync_raw(&self, body: Vec<u8>) -> Answer {
        self.request(
            Method::POST,
            pcsync::PATH,
            &[
                ("content-type", "application/json"),
                ("x-real-ip", "192.0.2.40"),
            ],
            Some(body),
        )
        .await
    }

    /// A PC's sync now, with a new nonce, and its answer read as the PC reads it.
    pub async fn sync(&self, pc: &TestDevice, p: &Value) -> Synced {
        self.sync_at(pc, p, now()).await
    }

    /// A PC's sync signed with the time `t`.
    pub async fn sync_at(&self, pc: &TestDevice, p: &Value, t: i64) -> Synced {
        self.sync_signed(pc, p, t, "192.0.2.40").await
    }

    /// A PC's sync now from the address `ip` (its egress address, X-Real-IP).
    pub async fn sync_from(&self, pc: &TestDevice, p: &Value, ip: &str) -> Synced {
        self.sync_signed(pc, p, now(), ip).await
    }

    async fn sync_signed(&self, pc: &TestDevice, p: &Value, t: i64, ip: &str) -> Synced {
        let nonce = random_bytes::<16>();
        let body = sync_body(pc, t, &nonce, &p.to_string());
        let answer = self
            .request(
                Method::POST,
                pcsync::PATH,
                &[("content-type", "application/json"), ("x-real-ip", ip)],
                Some(body.into_bytes()),
            )
            .await;
        let binding = p["binding"].as_str().unwrap_or("").to_owned();
        let n = b64url(&nonce);
        let list = check_list(&answer.json, pc, &binding, &n);
        Synced { answer, n, list }
    }

    /// POST to a relay path (relay/PROTOCOL.md §5) with this body, from `ip` (X-Real-IP).
    pub async fn relay_raw(&self, path: &str, body: Vec<u8>, ip: &str) -> Answer {
        self.request(
            Method::POST,
            path,
            &[("content-type", "application/json"), ("x-real-ip", ip)],
            Some(body),
        )
        .await
    }

    /// A relay's hello now, reporting `ifaces`.
    pub async fn hello(&self, relay: &TestRelay, ip: &str, ifaces: &Value) -> Answer {
        let body = relay.body(&relay::HELLO, now(), &hello_payload(ifaces));
        self.relay_raw(relay::HELLO_PATH, body.into_bytes(), ip)
            .await
    }

    /// A relay's poll now: `wait` seconds, its `results`, and its interfaces when `ifaces` is
    /// given (their summary always, of `ifaces` or else of `reported`).
    pub async fn poll(
        &self,
        relay: &TestRelay,
        ip: &str,
        wait: u64,
        results: &Value,
        ifaces: Option<&Value>,
        reported: &Value,
    ) -> Answer {
        let body = relay.body(
            &relay::POLL,
            now(),
            &poll_payload(wait, results, ifaces, reported),
        );
        self.relay_raw(relay::POLL_PATH, body.into_bytes(), ip)
            .await
    }
}

/// A NAS relay as hjv-relay runs it: its key pair, ID and enrollment code (relay/PROTOCOL.md
/// §3.1).
#[derive(Clone)]
pub struct TestRelay {
    pub seed: [u8; 32],
    pub id: String,
    pub code: String,
}

impl TestRelay {
    pub fn new(seed: [u8; 32]) -> TestRelay {
        let id = relay::relay_id_of(&devsig::public_key(&seed));
        TestRelay {
            seed,
            code: relay::code_of(&id),
            id,
        }
    }

    /// relay/PROTOCOL.md §13's relay: the seed 00 01 … 1f, `r-kzdvvj2umnduyauf`.
    pub fn of_protocol_md() -> TestRelay {
        let mut seed = [0u8; 32];
        for (i, b) in seed.iter_mut().enumerate() {
            *b = i as u8;
        }
        TestRelay::new(seed)
    }

    pub fn public_key(&self) -> [u8; 32] {
        devsig::public_key(&self.seed)
    }

    /// The body of a request: the envelope signed with the relay's key, a new nonce.
    pub fn body(&self, spec: &envelope::Spec, t: i64, p: &str) -> String {
        envelope::sign_body(spec, &self.seed, &self.id, t, &random_bytes::<16>(), p)
    }
}

/// A relay's interface as it reports it (relay/PROTOCOL.md §9.1).
pub fn iface(name: &str, addr: &str, gateway: Option<&str>, gateway_mac: Option<&str>) -> Value {
    json!({
        "name": name, "mac": "02:00:00:00:00:01", "ipv4": [addr], "up": true, "running": true,
        "gateway": gateway, "gateway_mac": gateway_mac,
    })
}

/// hex SHA-256 of the interfaces' JSON text, as the relay sends `ifaces_hash`.
pub fn ifaces_hash(ifaces: &Value) -> String {
    hjv_api::util::hex(&hjv_api::util::sha256(ifaces.to_string().as_bytes()))
}

/// A hello's payload (relay/PROTOCOL.md §5.1).
pub fn hello_payload(ifaces: &Value) -> String {
    json!({
        "v": 1, "version": "1.6.0", "arch": "x86_64", "os": "DSM 7.2.1-69057 Update 5",
        "kernel": "5.10.55+", "ifaces_hash": ifaces_hash(ifaces), "ifaces": ifaces,
    })
    .to_string()
}

/// A poll's payload (relay/PROTOCOL.md §5.2); `reported` names the interfaces whose summary it
/// carries when `ifaces` is not sent.
pub fn poll_payload(
    wait: u64,
    results: &Value,
    ifaces: Option<&Value>,
    reported: &Value,
) -> String {
    let mut p = json!({
        "v": 1, "version": "1.6.0", "arch": "x86_64", "os": "DSM 7.2.1-69057 Update 5",
        "kernel": "5.10.55+", "wait": wait, "ifaces_hash": ifaces_hash(ifaces.unwrap_or(reported)),
        "results": results,
    });
    if let Some(ifaces) = ifaces {
        p["ifaces"] = ifaces.clone();
    }
    p.to_string()
}

/// A PC's wake report (design v3 §8-2) of one wired card, documentation values.
pub fn wol_report(mac: &str, addr: &str, gateway: &str, gateway_mac: Option<&str>) -> Value {
    json!({"v": 1,
        "adapters": [{"mac": mac, "ipv4": [addr], "gateway": gateway, "gateway_mac": gateway_mac,
            "desc": "Example PCIe GbE Controller", "bus": "pci", "link_mbps": 1000,
            "driver": {"provider": "Example", "version": "1.0.0.0", "date": "1-1-2024"},
            "wake": {"magic": "1", "pattern": "1", "ms_magic": null, "s5": "1",
                     "shutdown_speed": "0", "eee": "0", "green": "0"},
            "armed": true}],
        "power": {"s3": true, "s4": true, "s5": true, "hiberfile": true, "aoac": false,
                  "fast_startup": false, "on_ac": true},
        "uptime_s": 3600})
}

/// A sync's payload that carries a wake report with its summary.
pub fn payload_with_wol(binding: &str, wol: &Value, hash: &str) -> Value {
    let mut p = payload(binding);
    p["wol"] = wol.clone();
    p["wol_hash"] = json!(hash);
    p
}

/// A sync's payload as the PC's service writes it (design v3 §3-6), with nothing to report.
pub fn payload(binding: &str) -> Value {
    json!({
        "v": 1, "binding": binding, "ver": "1.6.0", "off": false, "used": [],
        "refused": {"unknown_key": 0, "kind": 0, "temporary_only": 0, "not_secured": 0,
                    "bad_sig": 0, "expired": 0, "off": 0},
        "wol_hash": "",
    })
}

/// The body of a PC's sync: the envelope signed with its device key.
pub fn sync_body(pc: &TestDevice, t: i64, nonce: &[u8; 16], p: &str) -> String {
    envelope::sign_body(&pcsync::SPEC, &pc.seed, &pc.id, t, nonce, p)
}

/// What a PC got for its sync.
#[derive(Debug)]
pub struct Synced {
    pub answer: Answer,
    /// the request's `n`
    pub n: String,
    /// the answer's list, as the PC checks it: `Ok(list)` when it passes, `Err(why)` when it does
    /// not, `None` when there is none
    pub list: Option<Result<Value, String>>,
}

impl Synced {
    /// The list, which must have come and passed the PC's checks.
    pub fn list(&self) -> Value {
        match &self.list {
            Some(Ok(list)) => list.clone(),
            other => panic!(
                "no good list ({other:?}): {} {}",
                self.answer.status, self.answer.text
            ),
        }
    }

    /// The account keys of the list, base64.
    pub fn keys(&self) -> Vec<String> {
        self.list()["keys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k["pk"].as_str().unwrap().to_owned())
            .collect()
    }
}

/// The PC's checks of a sync answer's list (design v3 §3-6 steps 1-8, as the client's
/// verify_list; the reference with libsodium is in the client): a pinned key number, the
/// signature over the bytes as received, then the bytes: this PC, its key, the binding it sent,
/// this request, a lifetime of at most 72 hours, a known state.
pub fn check_list(
    answer: &Value,
    pc: &TestDevice,
    binding: &str,
    n: &str,
) -> Option<Result<Value, String>> {
    let signed = answer.get("list")?;
    Some((|| {
        let kid = signed["kid"].as_u64().ok_or("kid")?;
        let seed = match kid {
            1 => LIST_SEED,
            2 => NEXT_LIST_SEED,
            _ => return Err(format!("kid {kid} is not pinned")),
        };
        let bytes = b64_decode(signed["l"].as_str().ok_or("l")?).ok_or("l is not base64")?;
        let s = signed["s"].as_str().ok_or("s")?;
        let message = [&b"hjv-trust-v1\0"[..], &bytes].concat();
        if !devsig::verify(&devsig::public_key(&seed), &message, s) {
            return Err("the signature".into());
        }
        let list: Value = serde_json::from_slice(&bytes).map_err(|_| "not JSON")?;
        let checks = [
            ("v", list["v"] == 1),
            ("typ", list["typ"] == "hjv-trust"),
            ("kid", list["kid"] == kid),
            ("pc_id", list["pc_id"] == pc.id.as_str()),
            ("pc_pk", list["pc_pk"] == b64(&pc.public_key()).as_str()),
            ("binding", list["binding"] == binding),
            ("req", list["req"] == n),
            (
                "state",
                matches!(
                    list["state"].as_str(),
                    Some("active" | "disabled" | "revoked")
                ),
            ),
        ];
        if let Some((what, _)) = checks.iter().find(|(_, ok)| !ok) {
            return Err(format!("{what}: {list}"));
        }
        let (iat, exp) = (
            list["iat"].as_i64().ok_or("iat")?,
            list["exp"].as_i64().ok_or("exp")?,
        );
        if exp <= iat || exp - iat > 72 * 3600 {
            return Err(format!("lifetime {}", exp - iat));
        }
        Ok(list)
    })())
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
