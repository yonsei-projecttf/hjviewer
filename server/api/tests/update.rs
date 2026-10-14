// SPDX-License-Identifier: AGPL-3.0-only

//! The packages of new app versions (update.rs; the design of the minimum version and forced
//! update, §4-6): door 1 by a login with its proof and by a bound PC's device key, short links,
//! door 2, the limits, the records, and what is sent with a package.

mod common;

use common::*;
use hjv_api::{
    devsig,
    limits::{
        SCOPE_UPDATE_FAIL_IP, SCOPE_UPDATE_LINK, SCOPE_UPDATE_OK_ACCOUNT, SCOPE_UPDATE_OK_DEVICE,
    },
    proof, update,
    util::{datetime, hex, now, random_bytes, sha256},
};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::Bytes,
    header::{HeaderMap, HeaderName, HeaderValue},
    Method, Request,
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde_json::{json, Value};
use std::path::PathBuf;

const EXE: &str = "HJViewer-1.6.1-windows-x86_64.exe";
const APK: &str = "HJViewer-1.6.1-android-arm64.apk";
const EXE_TYPE: &str = "application/vnd.microsoft.portable-executable";
const APK_TYPE: &str = "application/vnd.android.package-archive";
const WINDOWS: &str = "/api/hjv/update/windows/1.6.1";
const ANDROID: &str = "/api/hjv/update/android/1.6.1";
const DOOR2: &str = "5ca1ab1e0ddba11c0ffee15dec0dedaa";

/// Bytes that tell a file from another and a place in it from the next.
fn content(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u64 * 31 + u64::from(seed) * 7 + (i as u64 >> 8)) as u8)
        .collect()
}

/// The operator's files of a test: the policy, the package directory, door 2's file.
struct Area {
    dir: PathBuf,
}

impl Area {
    fn new() -> Area {
        let dir = temp_dir("update");
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        Area { dir }
    }

    /// The settings that point hjv-api at these files, and `more`.
    fn vars(&self, more: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        let path = |name: &str| self.dir.join(name).display().to_string();
        let mut vars = vec![
            ("HJV_API_VERSION_POLICY", path("version-policy.json")),
            ("HJV_API_PACKAGE_DIR", path("pkg")),
            ("HJV_API_DOOR2_FILE", path("door2.conf")),
        ];
        vars.extend(more.iter().map(|(k, v)| (*k, (*v).to_owned())));
        vars
    }

    fn pkg(&self, name: &str) -> PathBuf {
        self.dir.join("pkg").join(name)
    }

    fn put(&self, name: &str, bytes: &[u8]) {
        let path = self.pkg(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Installs a policy as the runbook does: a new file moved over the old one.
    fn policy(&self, seq: u64, platforms: Value) {
        let text = json!({"v": 1, "epoch": 1, "seq": seq, "issued": "2026-10-05T03:00:00Z",
                          "server": "rd.example.com", "platforms": platforms});
        let envelope = json!({"v": 1, "k": 1, "p": text.to_string(), "s": "AAAA"});
        let new = self.dir.join("version-policy.json.new");
        std::fs::write(&new, format!("{envelope}\n")).unwrap();
        std::fs::rename(new, self.dir.join("version-policy.json")).unwrap();
    }

    fn door2(&self, code: &str, until: i64) {
        let new = self.dir.join("door2.conf.new");
        std::fs::write(&new, format!("CODE={code}\nUNTIL={}\n", datetime(until))).unwrap();
        std::fs::rename(new, self.dir.join("door2.conf")).unwrap();
    }

    /// The usual files: 1.6.1 for Windows and Android, each with its package.
    fn usual(&self) -> (Vec<u8>, Vec<u8>) {
        let (exe, apk) = (content(1, 150_017), content(2, 70_001));
        self.put(EXE, &exe);
        self.put(APK, &apk);
        self.policy(
            3,
            json!({"windows": entry("1.6.1", EXE, &exe), "android": entry("1.6.1", APK, &apk)}),
        );
        (exe, apk)
    }
}

impl Drop for Area {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A policy's entry for `latest` with its package.
fn entry(latest: &str, file: &str, bytes: &[u8]) -> Value {
    json!({"min": "1.6.0", "latest": latest, "source": format!("client-{latest}"),
           "package": {"file": file, "size": bytes.len(), "sha256": hex(&sha256(bytes)),
                       "build": "1.6.1.41"}})
}

async fn start(area: &Area, more: &[(&'static str, &str)]) -> Harness {
    Harness::with(Options {
        vars: area.vars(more),
        ..Default::default()
    })
    .await
}

/// An answer with its body as bytes.
struct Got {
    status: u16,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Got {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn header(&self, name: &str) -> &str {
        self.headers
            .get(name)
            .map_or("", |value| value.to_str().unwrap())
    }
}

async fn send(
    h: &Harness,
    method: Method,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Got {
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build_http();
    let mut request = Request::builder()
        .method(method)
        .uri(format!("http://{}{path}", h.api));
    for (name, value) in headers {
        request = request.header(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    let request = request
        .body(Full::new(Bytes::from(
            body.unwrap_or_default().as_bytes().to_vec(),
        )))
        .unwrap();
    let response = client.request(request).await.unwrap();
    let (status, headers) = (response.status().as_u16(), response.headers().clone());
    let body = response.into_body().collect().await.unwrap().to_bytes();
    Got {
        status,
        headers,
        body: body.to_vec(),
    }
}

/// A request without any proof of who asks, from `ip`.
async fn bare(h: &Harness, method: Method, path: &str, ip: &str) -> Got {
    send(h, method, path, &[("x-real-ip", ip)], None).await
}

/// A request as a logged-in app sends it: the token and its account key's proof.
async fn by_login(
    h: &Harness,
    method: Method,
    path: &str,
    token: &str,
    ip: &str,
    body: Option<&str>,
) -> Got {
    let auth = format!("Bearer {token}");
    let proof = h
        .proof_for(
            method.as_str(),
            path,
            body.unwrap_or_default().as_bytes(),
            token,
        )
        .unwrap();
    let mut headers = vec![
        ("authorization", auth.as_str()),
        (proof::HEADER, proof.as_str()),
        ("x-real-ip", ip),
    ];
    if body.is_some() {
        headers.push(("content-type", "application/json"));
    }
    send(h, method, path, &headers, body).await
}

/// A request that door 1 refuses: what is wrong with it, its path, its headers.
type Refused<'a> = (&'a str, &'a str, Vec<(&'static str, String)>);

/// The three headers of a PC's request for `platform` and `version`, signed with `seed` at `t`.
fn device_headers(
    id: &str,
    seed: &[u8; 32],
    platform: &str,
    version: &str,
    t: i64,
) -> [(&'static str, String); 3] {
    let message = update::device_message(id, &t.to_string(), platform, version);
    [
        ("x-hjv-device", id.to_owned()),
        ("x-hjv-time", t.to_string()),
        ("x-hjv-sig", devsig::sign(seed, &message)),
    ]
}

/// A request as the service of a PC sends it: signed with its device key, now.
async fn by_device(h: &Harness, method: Method, pc: &TestDevice, path: &str, ip: &str) -> Got {
    let mut parts = path.rsplit('/');
    let (version, platform) = (parts.next().unwrap(), parts.next().unwrap());
    let signed = device_headers(&pc.id, &pc.seed, platform, version, now());
    with_device_headers(h, method, path, &signed, ip).await
}

async fn with_device_headers(
    h: &Harness,
    method: Method,
    path: &str,
    signed: &[(&'static str, String)],
    ip: &str,
) -> Got {
    let mut headers: Vec<(&str, &str)> = signed.iter().map(|(k, v)| (*k, v.as_str())).collect();
    headers.push(("x-real-ip", ip));
    send(h, method, path, &headers, None).await
}

/// The records of kind `update`, oldest first.
async fn update_results(h: &Harness) -> Vec<String> {
    let mut results = h.results().await;
    results.retain(|r| r.starts_with("update:"));
    results
}

/// A PC in `account`'s list: installed, registered with hbbs, logged in once.
async fn bound_pc(h: &Harness, id: &str, account: (&str, &str)) -> TestDevice {
    let pc = TestDevice::windows(id);
    h.hbbs.register(&pc, Some(1));
    h.login_ok(account, Some(&pc)).await;
    assert!(!h.binding_of(id).await.is_empty(), "{id} is not bound");
    pc
}

/// Asks for a short link; the answer's `url` and `file`.
async fn link(h: &Harness, token: &str, platform: &str, body: &str) -> (String, String) {
    let path = format!("/api/hjv/update/{platform}/link");
    let got = by_login(h, Method::POST, &path, token, "10.3.0.1", Some(body)).await;
    assert_eq!(got.status, 200, "{}", got.text());
    let answer = got.json();
    assert_eq!(answer["expires_in"], 600);
    (
        answer["url"].as_str().unwrap().to_owned(),
        answer["file"].as_str().unwrap().to_owned(),
    )
}

/// `^/api/hjv/u/[0-9A-HJKMNP-TV-Z]{4}-[0-9A-HJKMNP-TV-Z]{4}$`, the form the apps take a link in.
fn is_short_link(url: &str) -> bool {
    let Some(code) = url.strip_prefix("/api/hjv/u/") else {
        return false;
    };
    let symbol = |b: &u8| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(b);
    let code = code.as_bytes();
    code.len() == 9 && code[4] == b'-' && code[..4].iter().chain(&code[5..]).all(symbol)
}

fn assert_package_head(got: &Got, kind: &str, name: &str, size: usize) {
    assert_eq!(got.status, 200, "{}", got.text());
    for (header, value) in [
        ("content-type", kind),
        ("content-length", size.to_string().as_str()),
        (
            "content-disposition",
            format!("attachment; filename=\"{name}\"").as_str(),
        ),
        ("cache-control", "private, no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-robots-tag", "noindex, nofollow, noarchive"),
        ("referrer-policy", "no-referrer"),
        ("x-accel-buffering", "no"),
        ("x-accel-limit-rate", "3840000"),
        ("accept-ranges", "none"),
    ] {
        assert_eq!(got.header(header), value, "{header}");
    }
    assert!(got.headers.get("set-cookie").is_none());
    assert!(got.headers.get("access-control-allow-origin").is_none());
    assert!(got.headers.get("etag").is_none() && got.headers.get("last-modified").is_none());
}

/// README.md's Formats show the vector of a PC's download signature (update.rs checks it against
/// the code).
#[test]
fn the_readme_shows_the_device_signature() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let seed: [u8; 32] = std::array::from_fn(|i| 0x60 + i as u8);
    let message = update::device_message("123456789", "1793577600", "windows", "1.6.1");
    for value in [
        hjv_api::util::b64(&devsig::public_key(&seed)),
        format!("signed ({})      {}", message.len(), hex(&message)),
        format!("X-HJV-Sig        {}", devsig::sign(&seed, &message)),
        "GET /api/hjv/update/windows/1.6.1, X-HJV-Device: 123456789, X-HJV-Time: 1793577600".into(),
    ] {
        assert!(readme.contains(&value), "README.md does not show {value}");
    }
}

#[tokio::test]
async fn a_login_gets_the_latest_package() {
    let area = Area::new();
    let (exe, apk) = area.usual();
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;

    let got = by_login(&h, Method::GET, WINDOWS, &token, "10.3.0.1", None).await;
    // an app never gets a name that opens the installer
    assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.1.exe", exe.len());
    assert_eq!(got.body, exe);
    let got = by_login(&h, Method::GET, ANDROID, &token, "10.3.0.1", None).await;
    assert_package_head(&got, APK_TYPE, APK, apk.len());
    assert_eq!(got.body, apk);
    // HEAD: the same head and nothing else
    let got = by_login(&h, Method::HEAD, WINDOWS, &token, "10.3.0.1", None).await;
    assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.1.exe", exe.len());
    assert!(got.body.is_empty());

    // only the latest of the platform, by the policy
    for path in [
        "/api/hjv/update/windows/1.6.0",
        "/api/hjv/update/windows/1.6.2",
        "/api/hjv/update/android/1.5.1",
        "/api/hjv/update/windows/0.0.0",
    ] {
        let got = by_login(&h, Method::GET, path, &token, "10.3.0.1", None).await;
        assert_eq!(got.status, 404, "{path}");
        assert_eq!(
            got.json()["error"],
            "새 버전을 준비하고 있습니다. 잠시 뒤 다시 시도해 주세요."
        );
        assert_eq!(
            got.header("content-type"),
            "application/json; charset=utf-8"
        );
    }
    // what is no platform with a package, and no version
    for path in [
        "/api/hjv/update/ios/1.6.1",
        "/api/hjv/update/android_play/1.6.1",
        "/api/hjv/update/Windows/1.6.1",
        "/api/hjv/update/windows/1.6",
        "/api/hjv/update/windows/1.6.1.0",
        "/api/hjv/update/windows/latest",
        "/api/hjv/update/windows/1.06.1",
        "/api/hjv/update/windows/..",
        "/api/hjv/update/windows/HJViewer-1.6.1-windows-x86_64.exe",
    ] {
        let got = by_login(&h, Method::GET, path, &token, "10.3.0.1", None).await;
        assert_eq!(got.status, 400, "{path}");
    }
    // no other method, no deeper path
    for (method, path, status) in [
        (Method::POST, WINDOWS, 405),
        (Method::DELETE, WINDOWS, 405),
        (Method::PUT, WINDOWS, 405),
        (Method::GET, "/api/hjv/update/windows/link", 405),
        (Method::GET, "/api/hjv/update/windows/1.6.1/x", 404),
        (Method::GET, "/api/hjv/update/windows", 404),
        (Method::GET, "/api/hjv/update", 404),
    ] {
        let got = by_login(&h, method.clone(), path, &token, "10.3.0.1", None).await;
        assert_eq!(got.status, status, "{method} {path}");
    }
    let got = by_login(&h, Method::POST, WINDOWS, &token, "10.3.0.1", None).await;
    assert_eq!(got.header("allow"), "GET, HEAD");

    // each download is a record with tags: the account, and the address as its tag
    assert_eq!(
        h.results().await,
        ["login:ok", "update:token-windows", "update:token-android"]
    );
    let rows: Vec<(Option<String>, Option<String>, Option<String>)> = h
        .state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT account_tag, device_tag, ip FROM events WHERE kind = 'update' ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
        .await
        .unwrap();
    let expected = (
        Some(h.account_tag(HONG.0)),
        None,
        Some(h.state.tags.ip("10.3.0.1")),
    );
    assert_eq!(rows, [expected.clone(), expected]);
    // and counts for the account, a HEAD and the refused ones not
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_ACCOUNT, h.account_tag(HONG.0))
            .await,
        2
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("10.3.0.1"))
            .await,
        0
    );
}

#[tokio::test]
async fn the_token_alone_opens_nothing() {
    let area = Area::new();
    let (exe, _) = area.usual();
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;
    let other = h.login_ok(KIM, None).await;
    let auth = format!("Bearer {token}");
    let ip = ("x-real-ip", "10.3.1.1");
    let seed = h.key_of(&token).unwrap();
    let signed = |method: &str, path: &str, time: i64, seed: &[u8; 32]| {
        proof::header_value(seed, method, path, time, &random_bytes::<16>(), b"", &token)
    };

    // the right proof: the token's key signed this method and this path, now
    let right = signed("GET", WINDOWS, now(), &seed);
    let got = send(
        &h,
        Method::GET,
        WINDOWS,
        &[("authorization", &auth), ("x-hjv-proof", &right), ip],
        None,
    )
    .await;
    assert_eq!((got.status, &got.body), (200, &exe));

    // no proof, and every wrong one: the 401 of a wrong token
    let wrong: Vec<(&str, Option<String>)> = vec![
        ("no proof", None),
        ("not a proof", Some("v1.x.y.z".into())),
        (
            "of another path",
            Some(signed("GET", ANDROID, now(), &seed)),
        ),
        (
            "of another version",
            Some(signed("GET", "/api/hjv/update/windows/1.6.0", now(), &seed)),
        ),
        (
            "of another method",
            Some(signed("HEAD", WINDOWS, now(), &seed)),
        ),
        (
            "by another account's key",
            Some(signed("GET", WINDOWS, now(), &h.key_of(&other).unwrap())),
        ),
        (
            "by a key of no login",
            Some(signed("GET", WINDOWS, now(), &[77; 32])),
        ),
        (
            "for another token",
            Some(proof::header_value(
                &seed,
                "GET",
                WINDOWS,
                now(),
                &random_bytes::<16>(),
                b"",
                &other,
            )),
        ),
    ];
    for (what, proof) in &wrong {
        let mut headers = vec![("authorization", auth.as_str()), ip];
        if let Some(proof) = proof {
            headers.push(("x-hjv-proof", proof));
        }
        let got = send(&h, Method::GET, WINDOWS, &headers, None).await;
        assert_eq!(got.status, 401, "{what}");
        assert_eq!(
            got.json()["error"],
            "로그인이 필요합니다. 다시 로그인해 주세요."
        );
        assert_eq!(
            got.header("content-type"),
            "application/json; charset=utf-8"
        );
        assert!(got.headers.get("content-disposition").is_none(), "{what}");
    }
    // no token at all, and what is no token
    for auth in [None, Some("Bearer hjv_x"), Some("Basic abc"), Some("")] {
        let mut headers = vec![ip];
        if let Some(auth) = auth {
            headers.push(("authorization", auth));
        }
        let got = send(&h, Method::GET, WINDOWS, &headers, None).await;
        assert_eq!(got.status, 401, "{auth:?}");
    }
    // each of them counted against the address
    let ip_tag = h.state.tags.ip("10.3.1.1");
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, ip_tag.clone()).await,
        wrong.len() as i64 + 4
    );

    // the key's signature at a wrong time is the 409 of a clock that is off: no failure of the
    // address, and no package
    let late = signed("GET", WINDOWS, now() - 601, &seed);
    let got = send(
        &h,
        Method::GET,
        WINDOWS,
        &[("authorization", &auth), ("x-hjv-proof", &late), ip],
        None,
    )
    .await;
    assert_eq!(got.status, 409);
    assert_eq!(got.json()["hjv"], "clock");
    assert!(got.json()["server_time"].is_i64());
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, ip_tag).await,
        wrong.len() as i64 + 4
    );
    // the same holds for making a link
    let got = send(
        &h,
        Method::POST,
        "/api/hjv/update/windows/link",
        &[
            ("authorization", &auth),
            ("content-type", "application/json"),
            ip,
        ],
        Some("{\"installed\":true}"),
    )
    .await;
    assert_eq!(got.status, 401);
    assert_eq!(h.state.updates.links_kept(), 0);
    // one download in all
    assert_eq!(
        h.results().await,
        ["login:ok", "login:ok", "update:token-windows"]
    );
}

#[tokio::test]
async fn a_blocked_account_gets_nothing() {
    let area = Area::new();
    let (exe, _) = area.usual();
    let h = start(&area, &[]).await;
    let pc = bound_pc(&h, "810000001", HONG).await;
    let token = h.login_ok(HONG, None).await;
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    let got = by_login(&h, Method::GET, WINDOWS, &token, "10.3.2.1", None).await;
    assert_eq!((got.status, &got.body), (200, &exe));
    assert_eq!(
        by_device(&h, Method::HEAD, &pc, WINDOWS, "10.3.2.2")
            .await
            .status,
        200
    );
    assert_eq!(bare(&h, Method::HEAD, &url, "10.3.2.3").await.status, 200);

    // blocked (allow remove blocks too since the open login): db::allowed says no
    h.admin(&["block", "2026000001"], false, None)
        .await
        .unwrap();
    // the login is over (a block ends its tokens), as on every account path
    let got = by_login(&h, Method::GET, WINDOWS, &token, "10.3.2.1", None).await;
    assert_eq!(got.status, 401);
    // the PC is still in the account, and its key opens nothing
    assert!(!h.binding_of(&pc.id).await.is_empty());
    let got = by_device(&h, Method::GET, &pc, WINDOWS, "10.3.2.2").await;
    assert_eq!(got.status, 403);
    // and the link the account made is over
    let got = bare(&h, Method::GET, &url, "10.3.2.3").await;
    assert_eq!(got.status, 404);
    assert_eq!(h.state.updates.links_kept(), 0);
    for ip in ["10.3.2.1", "10.3.2.2", "10.3.2.3"] {
        assert_eq!(
            h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(ip)).await,
            1
        );
    }
    // unblocked, the PC's key works again without a login (allow import lifts no block)
    h.admin(&["unblock", "2026000001"], false, None)
        .await
        .unwrap();
    let got = by_device(&h, Method::GET, &pc, WINDOWS, "10.3.2.2").await;
    assert_eq!((got.status, &got.body), (200, &exe));
}

#[tokio::test]
async fn a_pc_of_an_account_downloads_with_its_device_key() {
    let area = Area::new();
    let (exe, _) = area.usual();
    let h = start(&area, &[]).await;
    let pc = bound_pc(&h, "820000001", HONG).await;
    let (device_tag, account_tag) = (h.state.tags.device(&pc.id), h.account_tag(HONG.0));

    let got = by_device(&h, Method::GET, &pc, WINDOWS, "192.0.2.50").await;
    // the service runs what it gets with --update: never a name that opens the installer
    assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.1.exe", exe.len());
    assert_eq!(got.body, exe);
    let got = by_device(&h, Method::HEAD, &pc, WINDOWS, "192.0.2.50").await;
    assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.1.exe", exe.len());
    assert!(got.body.is_empty());
    // within ten minutes either way (to the second: update.rs's own test)
    for off in [-570, 570] {
        let signed = device_headers(&pc.id, &pc.seed, "windows", "1.6.1", now() + off);
        let got = with_device_headers(&h, Method::HEAD, WINDOWS, &signed, "192.0.2.50").await;
        assert_eq!(got.status, 200, "{off}");
    }
    // a version that is not the latest: the PC is who it says, there is just no such package
    let got = by_device(
        &h,
        Method::GET,
        &pc,
        "/api/hjv/update/windows/1.6.0",
        "192.0.2.50",
    )
    .await;
    assert_eq!(got.status, 404);
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("192.0.2.50"))
            .await,
        0
    );

    // everything else is the same 403, whatever is wrong
    let stranger = TestDevice::windows("820000002");
    h.hbbs.register(&stranger, Some(1));
    let phone = TestDevice::phone("820000003");
    h.hbbs.register(&phone, Some(1));
    h.login_ok(KIM, Some(&phone)).await;
    assert!(h.binding_of(&phone.id).await.is_empty());
    let t = now();
    let good = device_headers(&pc.id, &pc.seed, "windows", "1.6.1", t);
    let with = |at: usize, value: &str| {
        let mut headers = good.clone();
        headers[at].1 = value.to_owned();
        headers.to_vec()
    };
    let refused: Vec<Refused> = vec![
        // the platform of the PC only: its key gets no APK
        (
            "another platform, signed",
            ANDROID,
            device_headers(&pc.id, &pc.seed, "android", "1.6.1", t).to_vec(),
        ),
        ("another platform's path", ANDROID, good.to_vec()),
        (
            "a PC in no account",
            WINDOWS,
            device_headers(&stranger.id, &stranger.seed, "windows", "1.6.1", t).to_vec(),
        ),
        (
            "a phone that is logged in",
            WINDOWS,
            device_headers(&phone.id, &phone.seed, "windows", "1.6.1", t).to_vec(),
        ),
        (
            "another key",
            WINDOWS,
            device_headers(&pc.id, &stranger.seed, "windows", "1.6.1", t).to_vec(),
        ),
        (
            "signed for another version",
            WINDOWS,
            device_headers(&pc.id, &pc.seed, "windows", "1.6.0", t).to_vec(),
        ),
        (
            "too old",
            WINDOWS,
            device_headers(&pc.id, &pc.seed, "windows", "1.6.1", t - 601).to_vec(),
        ),
        // (a minute past the ten: the clock runs on while this test works)
        (
            "from the future",
            WINDOWS,
            device_headers(&pc.id, &pc.seed, "windows", "1.6.1", t + 660).to_vec(),
        ),
        (
            "another time than signed",
            WINDOWS,
            with(1, &(t + 1).to_string()),
        ),
        ("a time that is no number", WINDOWS, with(1, "now")),
        ("a time with a sign", WINDOWS, with(1, &format!("+{t}"))),
        (
            "a time with a zero in front",
            WINDOWS,
            with(1, &format!("0{t}")),
        ),
        ("no signature", WINDOWS, with(2, "")),
        ("not base64", WINDOWS, with(2, "not a signature")),
        ("a short signature", WINDOWS, with(2, "AAAA")),
        ("an ID that is none", WINDOWS, with(0, "8200 00001")),
        ("the ID alone", WINDOWS, good[..1].to_vec()),
        ("no ID", WINDOWS, good[1..].to_vec()),
        ("no time", WINDOWS, vec![good[0].clone(), good[2].clone()]),
        // the login's own signature of the PC is not this one (another context)
        (
            "a signature of something else",
            WINDOWS,
            with(2, &devsig::sign(&pc.seed, b"hjv-login-v1\0820000001")),
        ),
    ];
    let mut answers = Vec::new();
    for (i, (what, path, headers)) in refused.iter().enumerate() {
        // an address each: thirty failures a day is the limit of one
        let ip = format!("192.0.2.{}", 100 + i);
        for method in [Method::GET, Method::HEAD] {
            let got = with_device_headers(&h, method.clone(), path, headers, &ip).await;
            assert_eq!(got.status, 403, "{what} ({method})");
            assert!(got.headers.get("content-disposition").is_none(), "{what}");
            assert_eq!(
                got.header("content-type"),
                "application/json; charset=utf-8"
            );
            if method == Method::GET {
                answers.push(got.text());
            }
        }
        assert_eq!(
            h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(&ip)).await,
            2,
            "{what}"
        );
    }
    answers.dedup();
    assert_eq!(
        answers,
        ["{\"error\":\"이 기기는 새 버전을 받을 수 없습니다.\"}"]
    );
    // a login's proof on a request with the PC's headers is the login's request
    let token = h.login_ok(KIM, None).await;
    let auth = format!("Bearer {token}");
    let proof = h.proof_for("GET", WINDOWS, b"", &token).unwrap();
    let got = send(
        &h,
        Method::GET,
        WINDOWS,
        &[
            ("authorization", auth.as_str()),
            ("x-hjv-proof", proof.as_str()),
            ("x-hjv-device", stranger.id.as_str()),
            ("x-hjv-time", "0"),
            ("x-hjv-sig", "AAAA"),
        ],
        None,
    )
    .await;
    assert_eq!(got.status, 200);

    // the records: the PC with its account, the address as its tag; and the first download from
    // an address the PC never used before is a record of its own
    assert_eq!(
        update_results(&h).await,
        [
            "update:device-windows",
            "update:device-new-address",
            "update:token-windows"
        ]
    );
    let got = by_device(&h, Method::GET, &pc, WINDOWS, "192.0.2.50").await;
    assert_eq!(got.status, 200);
    let got = by_device(&h, Method::GET, &pc, WINDOWS, "198.51.100.7").await;
    assert_eq!(got.status, 200);
    assert_eq!(
        update_results(&h).await[3..],
        [
            "update:device-windows",
            "update:device-windows",
            "update:device-new-address"
        ]
    );
    let rows: Vec<(String, String, String)> = h
        .state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT account_tag, device_tag, ip FROM events
                 WHERE kind = 'update' AND result LIKE 'device-%' ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
        .await
        .unwrap();
    assert_eq!(rows.len(), 5);
    for (i, row) in rows.iter().enumerate() {
        let ip = if i < 3 { "192.0.2.50" } else { "198.51.100.7" };
        assert_eq!(
            *row,
            (account_tag.clone(), device_tag.clone(), h.state.tags.ip(ip))
        );
    }
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_DEVICE, device_tag.clone()).await,
        3
    );
    assert_eq!(h.counted(SCOPE_UPDATE_OK_ACCOUNT, account_tag).await, 0);
    let journal = hjv_api::log::captured();
    assert!(journal.iter().any(|l| l.contains(&format!(
        "update: a PC's key downloads from an address it did not use before (acct={}, dev={device_tag}, ip={})",
        h.account_tag(HONG.0),
        h.state.tags.ip("198.51.100.7")
    ))));
    // and the operator record has a line for each address the PC's key came from first: the
    // service's own, in the record's form and chain
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    let lines: Vec<&str> = record.lines().collect();
    assert_eq!(lines.len(), 2, "{record}");
    for (line, ip) in lines.iter().zip(["192.0.2.50", "198.51.100.7"]) {
        let said = format!(
            " sudo_user=- cmd=\"update device-new-address\" targets={device_tag},{} reveal=0 prev=",
            h.state.tags.ip(ip)
        );
        assert!(line.contains(&said), "{line}");
        assert!(!line.contains(&pc.id) && !line.contains(ip), "{line}");
    }
    assert_eq!(hjv_api::oplog::verify(&h.oplog).unwrap(), 2);
}

#[tokio::test]
async fn short_links() {
    let area = Area::new();
    let (exe, apk) = area.usual();
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;

    // for a PC with HJViewer installed, the name that opens the installer
    let (url, file) = link(&h, &token, "windows", "{\"installed\":true}").await;
    assert!(is_short_link(&url), "{url}");
    assert_eq!(file, "HJViewer-1.6.1-install.exe");
    // the browser of the remote PC: no token, no header of the app
    let got = bare(&h, Method::GET, &url, "203.0.113.5").await;
    assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.1-install.exe", exe.len());
    assert_eq!(got.body, exe);
    // HEAD is free, before and after
    for _ in 0..5 {
        let got = bare(&h, Method::HEAD, &url, "203.0.113.5").await;
        assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.1-install.exe", exe.len());
        assert!(got.body.is_empty());
    }
    // typed by hand: any case, with or without the dash, spaces, O for 0 and I or L for 1
    let code = &url["/api/hjv/u/".len()..];
    let plain = code.replace('-', "");
    let got = bare(
        &h,
        Method::GET,
        &format!("/api/hjv/u/{}", plain.to_lowercase()),
        "203.0.113.5",
    )
    .await;
    assert_eq!((got.status, &got.body), (200, &exe));
    let alike = |part: &str| part.replace('0', "o").replace('1', "L");
    let spaced = format!(
        "/api/hjv/u/%20{}%20{}%20",
        alike(&plain[..4]),
        alike(&plain[4..])
    );
    let got = bare(&h, Method::GET, &spaced, "203.0.113.5").await;
    assert_eq!((got.status, &got.body), (200, &exe));
    // three downloads, then it is gone, for HEAD too
    for method in [Method::GET, Method::HEAD] {
        let got = bare(&h, method, &url, "203.0.113.5").await;
        assert_eq!(got.status, 404);
        assert_eq!(got.header("cache-control"), "private, no-store");
        assert_eq!(got.header("x-robots-tag"), "noindex, nofollow, noarchive");
    }
    assert_eq!(h.state.updates.links_kept(), 0);

    // without HJViewer installed, the name that runs it; an APK keeps its name
    let (run, file) = link(&h, &token, "windows", "{\"installed\":false}").await;
    assert_eq!(file, "HJViewer-1.6.1.exe");
    let got = bare(&h, Method::GET, &run, "203.0.113.5").await;
    assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.1.exe", exe.len());
    assert_eq!(
        link(&h, &token, "windows", "{}").await.1,
        "HJViewer-1.6.1.exe"
    );
    let (to_apk, file) = link(&h, &token, "android", "{\"installed\":true}").await;
    assert_eq!(file, APK);
    let got = bare(&h, Method::GET, &to_apk, "203.0.113.5").await;
    assert_package_head(&got, APK_TYPE, APK, apk.len());
    assert_eq!(got.body, apk);
    assert_ne!(run, to_apk);

    // what is no request for a link
    let post = "/api/hjv/update/windows/link";
    for (body, status) in [
        ("{\"installed\":\"yes\"}", 400),
        ("{\"installed\":1}", 400),
        ("[true]", 400),
        ("true", 400),
        ("{", 400),
    ] {
        let got = by_login(&h, Method::POST, post, &token, "10.3.0.1", Some(body)).await;
        assert_eq!(got.status, status, "{body}");
    }
    for platform in ["ios", "android_play", "linux"] {
        let path = format!("/api/hjv/update/{platform}/link");
        let got = by_login(&h, Method::POST, &path, &token, "10.3.0.1", Some("{}")).await;
        assert_eq!(got.status, 400, "{platform}");
    }
    let got = bare(&h, Method::POST, post, "10.3.0.9").await;
    assert_eq!(got.status, 401);
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("10.3.0.9"))
            .await,
        1
    );
    for (method, path) in [
        (Method::POST, url.as_str()),
        (Method::DELETE, url.as_str()),
        (Method::GET, post),
    ] {
        assert_eq!(bare(&h, method, path, "10.3.0.9").await.status, 405);
    }

    // a code that is none: 404, and a failure of its address
    for (i, wrong) in [
        "/api/hjv/u/0000-0000",
        "/api/hjv/u/K7M2-Q9X",
        "/api/hjv/u/K7M2-Q9XDD",
        "/api/hjv/u/K7M2-Q9XU",
        "/api/hjv/u/..",
        "/api/hjv/u/HJViewer-1.6.1-install.exe",
        "/api/hjv/u/%00%00",
    ]
    .iter()
    .enumerate()
    {
        let got = bare(&h, Method::GET, wrong, "203.0.113.9").await;
        assert_eq!(got.status, 404, "{wrong}");
        assert_eq!(
            got.text(),
            "{\"error\":\"링크가 맞지 않거나 사용 기한이 지났습니다.\"}"
        );
        assert_eq!(
            h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("203.0.113.9"))
                .await,
            i as i64 + 1
        );
    }
    // where the right codes came from, only the two requests for the link that was used up count
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("203.0.113.5"))
            .await,
        2
    );

    // ten minutes (to the second: update.rs's own test)
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    h.state.updates.age_links(540);
    assert_eq!(
        bare(&h, Method::HEAD, &url, "203.0.113.5").await.status,
        200
    );
    h.state.updates.age_links(60);
    assert_eq!(
        bare(&h, Method::HEAD, &url, "203.0.113.5").await.status,
        404
    );
    assert_eq!(bare(&h, Method::GET, &url, "203.0.113.5").await.status, 404);

    // a link is for the package it was made for, by its version and by its file: the same file
    // as a later version is another package, and so is another file as the same version
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    area.policy(4, json!({"windows": entry("1.6.2", EXE, &exe)}));
    assert_eq!(bare(&h, Method::GET, &url, "203.0.113.5").await.status, 404);
    let (url, file) = link(&h, &token, "windows", "{\"installed\":true}").await;
    assert_eq!(file, "HJViewer-1.6.2-install.exe");
    area.put("HJViewer-1.6.2-windows-x86_64-b.exe", &exe);
    area.policy(
        4,
        json!({"windows": entry("1.6.2", "HJViewer-1.6.2-windows-x86_64-b.exe", &exe)}),
    );
    assert_eq!(bare(&h, Method::GET, &url, "203.0.113.5").await.status, 404);
    // and it ends when its package is no longer the latest
    area.usual();
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    let newer = content(9, 4000);
    area.put("HJViewer-1.6.2-windows-x86_64.exe", &newer);
    area.policy(
        4,
        json!({"windows": entry("1.6.2", "HJViewer-1.6.2-windows-x86_64.exe", &newer)}),
    );
    assert_eq!(bare(&h, Method::GET, &url, "203.0.113.5").await.status, 404);
    let (url, file) = link(&h, &token, "windows", "{\"installed\":true}").await;
    assert_eq!(file, "HJViewer-1.6.2-install.exe");
    let got = bare(&h, Method::GET, &url, "203.0.113.5").await;
    assert_package_head(&got, EXE_TYPE, "HJViewer-1.6.2-install.exe", newer.len());
    assert_eq!(got.body, newer);
    // no package of the platform: no link
    let got = by_login(
        &h,
        Method::POST,
        "/api/hjv/update/android/link",
        &token,
        "10.3.0.1",
        Some("{}"),
    )
    .await;
    assert_eq!(got.status, 404);

    // the records name who made a link and where each download went
    let results = h.results().await;
    let count = |result: &str| results.iter().filter(|r| *r == result).count();
    assert_eq!(count("update:link-made-windows"), 8);
    assert_eq!(count("update:link-made-android"), 1);
    assert_eq!(count("update:link-windows"), 5);
    assert_eq!(count("update:link-android"), 1);
    assert_eq!(h.counted(SCOPE_UPDATE_LINK, h.account_tag(HONG.0)).await, 9);
    // downloads by a link are not the account's own downloads
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_ACCOUNT, h.account_tag(HONG.0))
            .await,
        0
    );
}

#[tokio::test]
async fn a_link_request_sent_again_gets_the_same_link() {
    let area = Area::new();
    area.usual();
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;
    let (path, body) = ("/api/hjv/update/windows/link", "{\"installed\":true}");
    let auth = format!("Bearer {token}");
    let proof = h.proof_for("POST", path, body.as_bytes(), &token).unwrap();
    let headers = [
        ("authorization", auth.as_str()),
        ("x-hjv-proof", proof.as_str()),
        ("content-type", "application/json"),
    ];
    // the app's HTTP client sends a request again byte for byte when its first try failed late
    let first = send(&h, Method::POST, path, &headers, Some(body)).await;
    let again = send(&h, Method::POST, path, &headers, Some(body)).await;
    assert_eq!(first.status, 200, "{}", first.text());
    assert_eq!((again.status, again.text()), (200, first.text()));
    assert_eq!(h.state.updates.links_kept(), 1);
    assert_eq!(h.counted(SCOPE_UPDATE_LINK, h.account_tag(HONG.0)).await, 1);
    // twenty links a day for an account
    for _ in 0..19 {
        link(&h, &token, "windows", body).await;
    }
    let got = by_login(&h, Method::POST, path, &token, "10.3.0.1", Some(body)).await;
    assert_eq!(got.status, 429);
    assert_eq!(got.json()["hjv"], "rate_limited");
    let wait: i64 = got.header("retry-after").parse().unwrap();
    assert!((86_300..=86_400).contains(&wait), "{wait}");
    assert_eq!(got.json()["retry_after"], wait);
    assert_eq!(h.state.updates.links_kept(), 20);
    // another account's are its own
    let other = h.login_ok(KIM, None).await;
    link(&h, &other, "windows", body).await;
}

#[tokio::test]
async fn door_2() {
    let area = Area::new();
    let (exe, apk) = area.usual();
    for (name, text) in [
        ("LICENSE", "GNU AFFERO GENERAL PUBLIC LICENSE"),
        ("NOTICE", "HJViewer 알림"),
        ("SOURCE", "client-1.6.1"),
    ] {
        area.put(&format!("notices-1.6.1/{name}.txt"), text.as_bytes());
    }
    let h = start(&area, &[]).await;
    let ip = "203.0.113.20";
    let door = format!("/api/hjv/get/{DOOR2}");
    let page = format!("{door}/");

    // no file of door 2: there is no door 2
    for path in [
        door.clone(),
        page.clone(),
        format!("{door}/HJViewer-1.6.1.exe"),
    ] {
        let got = bare(&h, Method::GET, &path, ip).await;
        assert_eq!(got.status, 404, "{path}");
        assert_eq!(
            got.text(),
            "{\"error\":\"링크가 맞지 않거나 사용 기한이 지났습니다.\"}"
        );
    }
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(ip)).await,
        3
    );

    area.door2(DOOR2, now() + 3600);
    // the page, made from the policy
    let got = bare(&h, Method::GET, &page, ip).await;
    assert_eq!(got.status, 200, "{}", got.text());
    for (header, value) in [
        ("content-type", "text/html; charset=utf-8"),
        ("content-disposition", "inline"),
        (
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'",
        ),
        ("cache-control", "private, no-store"),
        ("x-content-type-options", "nosniff"),
        ("x-robots-tag", "noindex, nofollow, noarchive"),
        ("referrer-policy", "no-referrer"),
    ] {
        assert_eq!(got.header(header), value, "{header}");
    }
    assert_eq!(got.header("content-length"), got.body.len().to_string());
    let html = got.text();
    for part in [
        "새 버전은 HJViewer 1.6.1입니다.",
        "<a href=\"HJViewer-1.6.1-install.exe\">HJViewer-1.6.1-install.exe</a>",
        "<a href=\"HJViewer-1.6.1.exe\">HJViewer-1.6.1.exe</a>",
        &format!("<a href=\"{APK}\">{APK}</a>"),
        &format!("SHA-256 <code>{}</code>", hex(&sha256(&exe))),
        &format!("SHA-256 <code>{}</code>", hex(&sha256(&apk))),
        "크기 150,017바이트",
        "(태그 client-1.6.1)",
        // the notice files that are there, and only those
        "HJViewer 1.6.1의 알림 파일: <a href=\"HJViewer-1.6.1-LICENSE.txt\">LICENSE</a> · <a href=\"HJViewer-1.6.1-NOTICE.txt\">NOTICE</a> · <a href=\"HJViewer-1.6.1-SOURCE.txt\">SOURCE</a></p>",
    ] {
        assert!(html.contains(part), "{part}\n{html}");
    }
    // the code is in the address only
    assert!(!html.contains(DOOR2));
    let head = bare(&h, Method::HEAD, &page, ip).await;
    assert_eq!((head.status, head.body.len()), (200, 0));
    assert_eq!(head.header("content-length"), got.body.len().to_string());

    // the address without its last / leads to the one with it, where the page's links work
    let got = bare(&h, Method::GET, &door, ip).await;
    assert_eq!((got.status, got.header("location")), (307, page.as_str()));
    assert_eq!(got.header("cache-control"), "private, no-store");

    // the Windows package under its two names, the APK under its own
    for name in ["HJViewer-1.6.1-install.exe", "HJViewer-1.6.1.exe"] {
        let got = bare(&h, Method::GET, &format!("{page}{name}"), ip).await;
        assert_package_head(&got, EXE_TYPE, name, exe.len());
        assert_eq!(got.body, exe);
        let got = bare(&h, Method::HEAD, &format!("{page}{name}"), ip).await;
        assert_package_head(&got, EXE_TYPE, name, exe.len());
        assert!(got.body.is_empty());
    }
    let got = bare(&h, Method::GET, &format!("{page}{APK}"), ip).await;
    assert_package_head(&got, APK_TYPE, APK, apk.len());
    assert_eq!(got.body, apk);
    // the notice files, shown in the browser
    let got = bare(
        &h,
        Method::GET,
        &format!("{page}HJViewer-1.6.1-NOTICE.txt"),
        ip,
    )
    .await;
    assert_eq!(got.status, 200);
    assert_eq!(got.text(), "HJViewer 알림");
    assert_eq!(got.header("content-type"), "text/plain; charset=utf-8");
    assert_eq!(
        got.header("content-disposition"),
        "inline; filename=\"HJViewer-1.6.1-NOTICE.txt\""
    );
    assert_eq!(got.header("x-content-type-options"), "nosniff");
    assert_eq!(got.header("cache-control"), "private, no-store");

    // nothing else: no name of the package directory, no path, no file that is not there
    let failures = h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(ip)).await;
    for name in [
        EXE,
        "HJViewer-1.6.0-install.exe",
        "HJViewer-1.6.1-install.exe.",
        "hjviewer-1.6.1.exe",
        "HJViewer-1.6.1-THIRD_PARTY_NOTICES.txt",
        "HJViewer-1.6.1-README.txt",
        "notices-1.6.1",
        "LICENSE.txt",
        "..",
        "%2e%2e",
        "version-policy.json",
        "door2.conf",
        "index.html",
    ] {
        let got = bare(&h, Method::GET, &format!("{page}{name}"), ip).await;
        assert_eq!(got.status, 404, "{name}");
    }
    for path in [
        format!("{page}notices-1.6.1/LICENSE.txt"),
        format!("{page}HJViewer-1.6.1.exe/"),
        format!("{page}/HJViewer-1.6.1.exe"),
    ] {
        assert_eq!(bare(&h, Method::GET, &path, ip).await.status, 404, "{path}");
    }
    // the code was right: those are no failures of the address
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(ip)).await,
        failures
    );
    for method in [Method::POST, Method::PUT, Method::DELETE] {
        let got = bare(&h, method, &page, ip).await;
        assert_eq!((got.status, got.header("allow")), (405, "GET, HEAD"));
    }

    // a wrong code: the same 404 as a closed door, and a failure of the address, also for a HEAD
    // (a guess at the code all the same)
    let mut near = DOOR2.to_owned();
    near.replace_range(31.., "b");
    for (i, wrong) in [
        near.as_str(),
        &DOOR2[..31],
        &DOOR2.to_uppercase(),
        &format!("{DOOR2}0"),
        "0",
        "%35ca1ab1e0ddba11c0ffee15dec0dedaa",
    ]
    .iter()
    .enumerate()
    {
        for (method, path) in [
            (Method::GET, format!("/api/hjv/get/{wrong}/")),
            (
                Method::GET,
                format!("/api/hjv/get/{wrong}/HJViewer-1.6.1.exe"),
            ),
            (
                Method::HEAD,
                format!("/api/hjv/get/{wrong}/HJViewer-1.6.1.exe"),
            ),
        ] {
            let got = bare(&h, method, &path, "203.0.113.21").await;
            assert_eq!(got.status, 404, "{path}");
            assert!(got.headers.get("content-disposition").is_none());
        }
        assert_eq!(
            h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("203.0.113.21"))
                .await,
            3 * (i as i64 + 1)
        );
    }

    // the page follows the policy, without a restart
    let newer = content(7, 9000);
    area.put("HJViewer-1.6.2-windows-x86_64.exe", &newer);
    area.policy(
        4,
        json!({"windows": entry("1.6.2", "HJViewer-1.6.2-windows-x86_64.exe", &newer),
               "android": entry("1.6.1", APK, &apk)}),
    );
    let html = bare(&h, Method::GET, &page, ip).await.text();
    assert!(html.contains("새 버전은 HJViewer 1.6.2입니다."), "{html}");
    assert!(html.contains("<a href=\"HJViewer-1.6.2-install.exe\">"));
    assert!(html.contains(&format!("<a href=\"{APK}\">")));
    assert!(!html.contains("HJViewer-1.6.1-install.exe"));
    assert!(html.contains("(태그 client-1.6.2, client-1.6.1)"));
    // the notices of 1.6.1 are the APK's now; 1.6.2 has none
    assert!(
        html.contains("HJViewer 1.6.1의 알림 파일:")
            && !html.contains("HJViewer 1.6.2의 알림 파일:")
    );
    let got = bare(&h, Method::GET, &format!("{page}HJViewer-1.6.2.exe"), ip).await;
    assert_eq!((got.status, &got.body), (200, &newer));
    assert_eq!(
        bare(&h, Method::GET, &format!("{page}HJViewer-1.6.1.exe"), ip)
            .await
            .status,
        404
    );
    assert_eq!(
        bare(
            &h,
            Method::GET,
            &format!("{page}HJViewer-1.6.1-LICENSE.txt"),
            ip
        )
        .await
        .status,
        200
    );
    // without a policy the page says so, and there is no file
    std::fs::remove_file(area.dir.join("version-policy.json")).unwrap();
    let got = bare(&h, Method::GET, &page, ip).await;
    assert_eq!(got.status, 200);
    assert!(got
        .text()
        .contains("새 버전을 준비하고 있습니다. 잠시 뒤 다시 열어 주세요."));
    assert!(!got.text().contains(".exe"));
    assert_eq!(
        bare(&h, Method::GET, &format!("{page}HJViewer-1.6.2.exe"), ip)
            .await
            .status,
        404
    );
    area.usual();
    assert_eq!(
        bare(&h, Method::HEAD, &format!("{page}HJViewer-1.6.1.exe"), ip)
            .await
            .status,
        200
    );

    // another code in the file: the old one is over at once
    let other = "0123456789abcdef0123456789abcdef";
    area.door2(other, now() + 3600);
    assert_eq!(bare(&h, Method::GET, &page, ip).await.status, 404);
    assert_eq!(
        bare(&h, Method::GET, &format!("/api/hjv/get/{other}/"), ip)
            .await
            .status,
        200
    );
    // past its time the door is closed, though the file is there
    area.door2(DOOR2, now() - 1);
    for path in [
        page.clone(),
        format!("{page}HJViewer-1.6.1.exe"),
        door.clone(),
    ] {
        assert_eq!(bare(&h, Method::GET, &path, ip).await.status, 404, "{path}");
    }
    area.door2(DOOR2, now() + 60);
    assert_eq!(bare(&h, Method::GET, &page, ip).await.status, 200);
    // a file that is not door 2's file opens nothing
    std::fs::write(area.dir.join("door2.conf"), format!("CODE={DOOR2}\n")).unwrap();
    assert_eq!(bare(&h, Method::GET, &page, ip).await.status, 404);

    // each package through door 2 is a record: the address as its tag, nobody's account
    let results = h.results().await;
    let count = |result: &str| results.iter().filter(|r| *r == result).count();
    assert_eq!(
        (count("update:door2-windows"), count("update:door2-android")),
        (3, 1)
    );
    let rows: Vec<(Option<String>, Option<String>, String)> = h
        .state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT DISTINCT account_tag, device_tag, ip FROM events WHERE kind = 'update'",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect()
        })
        .await
        .unwrap();
    assert_eq!(rows, [(None, None, h.state.tags.ip(ip))]);
}

#[tokio::test]
async fn a_download_waits_for_a_slot_and_loses_nothing_meanwhile() {
    let area = Area::new();
    let (exe, _) = area.usual();
    area.put(
        "notices-1.6.1/LICENSE.txt",
        b"GNU AFFERO GENERAL PUBLIC LICENSE",
    );
    area.door2(DOOR2, now() + 3600);
    let h = start(&area, &[("HJV_API_DOWNLOADS_MAX", "1")]).await;
    let token = h.login_ok(HONG, None).await;
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    let notice = format!("/api/hjv/get/{DOOR2}/HJViewer-1.6.1-LICENSE.txt");
    let ip = "203.0.113.60";

    // the one slot is taken, as by a download that goes on: a short link and a notice file wait
    // with the rest, and HEAD is free
    let taken = h.state.updates.slots.take().unwrap();
    for path in [url.as_str(), notice.as_str()] {
        for _ in 0..4 {
            let got = bare(&h, Method::GET, path, ip).await;
            assert_eq!(
                (got.status, got.header("retry-after")),
                (503, "60"),
                "{path}"
            );
            assert_eq!(got.json()["hjv"], "busy");
            assert_eq!(got.header("cache-control"), "private, no-store");
        }
        assert_eq!(bare(&h, Method::HEAD, path, ip).await.status, 200, "{path}");
    }
    drop(taken);
    // a busy answer took none of the link's three downloads, nor counted against the address
    for i in 0..3 {
        let got = bare(&h, Method::GET, &url, ip).await;
        assert_eq!((got.status, &got.body), (200, &exe), "{i}");
    }
    assert_eq!(bare(&h, Method::GET, &url, ip).await.status, 404);
    let got = bare(&h, Method::GET, &notice, ip).await;
    assert_eq!(
        (got.status, got.text().as_str()),
        (200, "GNU AFFERO GENERAL PUBLIC LICENSE")
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(ip)).await,
        1
    );
    assert_eq!(h.state.updates.slots.free(), 1);
}

#[tokio::test]
async fn door_2s_records_stop_at_twenty_a_minute() {
    let area = Area::new();
    let (exe, _) = area.usual();
    area.door2(DOOR2, now() + 3600);
    let h = start(&area, &[]).await;
    let path = format!("/api/hjv/get/{DOOR2}/HJViewer-1.6.1.exe");
    // whoever has the link downloads as often as they like: the packages go out, the records and
    // the journal's lines of them stop
    for i in 0..25 {
        let got = bare(&h, Method::GET, &path, "203.0.113.40").await;
        assert_eq!((got.status, got.body.len()), (200, exe.len()), "{i}");
    }
    assert_eq!(update_results(&h).await, vec!["update:door2-windows"; 20]);
    let journal = hjv_api::log::captured();
    let ip_tag = h.state.tags.ip("203.0.113.40");
    let lines = |part: &str| journal.iter().filter(|l| l.contains(part)).count();
    assert_eq!(
        lines(&format!(
            "update: windows 1.6.1 through door 2 (ip={ip_tag}, "
        )),
        20
    );
    assert!(lines("update: more than 20 downloads through door 2 this minute") >= 1);
}

#[tokio::test]
async fn a_browsers_origin_is_taken_on_its_two_paths_only() {
    let area = Area::new();
    let (exe, _) = area.usual();
    area.door2(DOOR2, now() + 3600);
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;
    let pc = bound_pc(&h, "830000001", HONG).await;
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    let origin = ("origin", "https://elsewhere.example");

    // the short link and door 2: a browser may say where a click came from
    for path in [
        url.clone(),
        format!("/api/hjv/get/{DOOR2}/"),
        format!("/api/hjv/get/{DOOR2}/HJViewer-1.6.1.exe"),
    ] {
        let got = send(&h, Method::GET, &path, &[origin], None).await;
        assert_eq!(got.status, 200, "{path}");
        // and no page elsewhere may read the answer
        assert!(got.headers.get("access-control-allow-origin").is_none());
    }
    // door 1 and making a link: an app sends no Origin, so a web page cannot use these
    let auth = format!("Bearer {token}");
    for (method, path, body) in [
        (Method::GET, WINDOWS, None),
        (Method::HEAD, WINDOWS, None),
        (
            Method::POST,
            "/api/hjv/update/windows/link",
            Some("{\"installed\":true}"),
        ),
    ] {
        let proof = h
            .proof_for(
                method.as_str(),
                path,
                body.unwrap_or_default().as_bytes(),
                &token,
            )
            .unwrap();
        let mut headers = vec![
            ("authorization", auth.as_str()),
            ("x-hjv-proof", proof.as_str()),
        ];
        if body.is_some() {
            headers.push(("content-type", "application/json"));
        }
        let got = send(&h, method.clone(), path, &headers, body).await;
        assert_eq!(got.status, 200, "{method} {path}");
        let proof = h
            .proof_for(
                method.as_str(),
                path,
                body.unwrap_or_default().as_bytes(),
                &token,
            )
            .unwrap();
        headers[1] = ("x-hjv-proof", proof.as_str());
        headers.push(origin);
        let got = send(&h, method.clone(), path, &headers, body).await;
        assert_eq!(got.status, 403, "{method} {path}");
        if method != Method::HEAD {
            assert_eq!(got.json()["error"], "접근이 차단되었습니다");
        }
    }
    let signed = device_headers(&pc.id, &pc.seed, "windows", "1.6.1", now());
    let mut headers: Vec<(&str, &str)> = signed.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let got = send(&h, Method::GET, WINDOWS, &headers, None).await;
    assert_eq!((got.status, &got.body), (200, &exe));
    headers.push(origin);
    assert_eq!(
        send(&h, Method::GET, WINDOWS, &headers, None).await.status,
        403
    );
    // the other paths keep the rule
    let got = send(&h, Method::GET, "/api/hjv/source", &[origin], None).await;
    assert_eq!(got.status, 403);
}

#[tokio::test]
async fn failures_count_against_an_address_and_right_requests_never() {
    let area = Area::new();
    let (exe, _) = area.usual();
    area.door2(DOOR2, now() + 3600);
    // the limits of the design: thirty failures a day for an address, ten downloads a day
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;
    let pc = bound_pc(&h, "840000001", HONG).await;
    let ip = "198.51.100.30";
    let ip_tag = h.state.tags.ip(ip);
    let page = format!("/api/hjv/get/{DOOR2}/");

    // the HEAD without any header of its caller, which the app's HTTP client sends first to find
    // out how to connect: 401, and no failure, however many
    for _ in 0..40 {
        let got = bare(&h, Method::HEAD, WINDOWS, ip).await;
        assert_eq!(got.status, 401);
    }
    assert_eq!(h.counted(SCOPE_UPDATE_FAIL_IP, ip_tag.clone()).await, 0);

    // thirty failures of every kind
    let wrong_sig = device_headers(&pc.id, &[9; 32], "windows", "1.6.1", now());
    for i in 0..30 {
        let got = match i % 5 {
            0 => bare(&h, Method::GET, WINDOWS, ip).await,
            1 => {
                send(
                    &h,
                    Method::HEAD,
                    WINDOWS,
                    &[("authorization", "Bearer hjv_x"), ("x-real-ip", ip)],
                    None,
                )
                .await
            }
            2 => with_device_headers(&h, Method::GET, WINDOWS, &wrong_sig, ip).await,
            3 => bare(&h, Method::GET, "/api/hjv/u/0000-0000", ip).await,
            _ => {
                bare(
                    &h,
                    Method::GET,
                    "/api/hjv/get/00000000000000000000000000000000/",
                    ip,
                )
                .await
            }
        };
        assert_eq!(got.status, [401, 401, 403, 404, 404][i % 5], "{i}");
    }
    assert_eq!(h.counted(SCOPE_UPDATE_FAIL_IP, ip_tag.clone()).await, 30);
    // more of them: 429 in place of the 401, 403 or 404, and not counted on
    for i in 0..10 {
        let got = match i % 5 {
            0 => bare(&h, Method::GET, WINDOWS, ip).await,
            1 => {
                send(
                    &h,
                    Method::HEAD,
                    WINDOWS,
                    &[("authorization", "Bearer hjv_x"), ("x-real-ip", ip)],
                    None,
                )
                .await
            }
            2 => with_device_headers(&h, Method::GET, WINDOWS, &wrong_sig, ip).await,
            3 => bare(&h, Method::GET, "/api/hjv/u/0000-0000", ip).await,
            _ => {
                bare(
                    &h,
                    Method::GET,
                    "/api/hjv/get/00000000000000000000000000000000/",
                    ip,
                )
                .await
            }
        };
        assert_eq!(got.status, 429, "{i}");
        let wait: i64 = got.header("retry-after").parse().unwrap();
        assert!((86_000..=86_400).contains(&wait), "{wait}");
        if i % 5 != 1 {
            assert_eq!(got.json()["hjv"], "rate_limited");
            assert_eq!(
                got.json()["error"],
                "요청이 너무 많습니다. 잠시 뒤 다시 시도해 주세요."
            );
        }
    }
    assert_eq!(h.counted(SCOPE_UPDATE_FAIL_IP, ip_tag.clone()).await, 30);

    // behind the same address, whoever has the right token, key or code is not held up: more
    // than thirty right requests, of every kind
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    for i in 0..8 {
        assert_eq!(
            by_login(&h, Method::HEAD, WINDOWS, &token, ip, None)
                .await
                .status,
            200,
            "{i}"
        );
        assert_eq!(
            by_device(&h, Method::HEAD, &pc, WINDOWS, ip).await.status,
            200,
            "{i}"
        );
        assert_eq!(bare(&h, Method::HEAD, &url, ip).await.status, 200, "{i}");
        assert_eq!(bare(&h, Method::GET, &page, ip).await.status, 200, "{i}");
        let got = bare(&h, Method::GET, &format!("{page}HJViewer-1.6.1.exe"), ip).await;
        assert_eq!((got.status, got.body.len()), (200, exe.len()), "{i}");
    }
    let got = by_login(&h, Method::GET, WINDOWS, &token, ip, None).await;
    assert_eq!((got.status, &got.body), (200, &exe));
    let got = by_device(&h, Method::GET, &pc, WINDOWS, ip).await;
    assert_eq!((got.status, &got.body), (200, &exe));
    let got = bare(&h, Method::GET, &url, ip).await;
    assert_eq!((got.status, &got.body), (200, &exe));
    let got = by_login(
        &h,
        Method::POST,
        "/api/hjv/update/windows/link",
        &token,
        ip,
        Some("{}"),
    )
    .await;
    assert_eq!(got.status, 200);
    let again = got.json()["url"].as_str().unwrap().to_owned();
    assert_eq!(bare(&h, Method::HEAD, &again, ip).await.status, 200);
    assert_eq!(h.counted(SCOPE_UPDATE_FAIL_IP, ip_tag.clone()).await, 30);
    // and another address has failures of its own
    assert_eq!(
        bare(&h, Method::GET, WINDOWS, "198.51.100.31").await.status,
        401
    );
    // an IPv6 address counts with its /64, which one subscriber holds whole
    for from in ["2001:db8:1:2::1", "2001:db8:1:2:aaaa::2", "2001:db8:1:3::1"] {
        assert_eq!(bare(&h, Method::GET, WINDOWS, from).await.status, 401);
    }
    assert_eq!(
        h.state.tags.ip("2001:db8:1:2::1"),
        h.state.tags.ip("2001:db8:1:2:aaaa::2")
    );
    for (from, failures) in [("2001:db8:1:2::9", 2), ("2001:db8:1:3::9", 1)] {
        assert_eq!(
            h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip(from)).await,
            failures,
            "{from}"
        );
    }

    // the journal heard of the address once, by its tag
    let journal = hjv_api::log::captured();
    let told = journal
        .iter()
        .filter(|l| {
            l.contains(&format!(
                "update: an address has had its failed requests (ip={ip_tag})"
            ))
        })
        .count();
    assert_eq!(told, 1);
    // the operator can clear it
    let out = h
        .admin(&["unlock", &format!("ip:{ip}")], false, None)
        .await
        .unwrap();
    assert!(out.contains("cleared 30"), "{out}");
    assert_eq!(bare(&h, Method::GET, WINDOWS, ip).await.status, 401);
}

#[tokio::test]
async fn downloads_count_for_an_account_and_for_a_pc() {
    let area = Area::new();
    let (exe, _) = area.usual();
    let h = start(&area, &[("HJV_API_LIMIT_UPDATE", "3/86400")]).await;
    let hong = h.login_ok(HONG, None).await;
    let hong_phone = h.login_ok(HONG, None).await;
    let kim = h.login_ok(KIM, None).await;
    let pc = bound_pc(&h, "850000001", HONG).await;
    let other_pc = bound_pc(&h, "850000002", HONG).await;

    // an account: its logins together, the downloads that began
    for token in [&hong, &hong_phone, &hong] {
        let got = by_login(&h, Method::GET, WINDOWS, token, "10.3.5.1", None).await;
        assert_eq!((got.status, &got.body), (200, &exe));
    }
    for (token, path) in [(&hong, WINDOWS), (&hong_phone, ANDROID)] {
        let got = by_login(&h, Method::GET, path, token, "10.3.5.1", None).await;
        assert_eq!(got.status, 429);
        assert_eq!(got.json()["hjv"], "rate_limited");
        assert!(got.header("retry-after").parse::<i64>().unwrap() > 86_000);
        assert!(got.headers.get("content-disposition").is_none());
    }
    // looking is free, another account has its own count, and what is refused counts nowhere
    assert_eq!(
        by_login(&h, Method::HEAD, WINDOWS, &hong, "10.3.5.1", None)
            .await
            .status,
        200
    );
    assert_eq!(
        by_login(&h, Method::GET, WINDOWS, &kim, "10.3.5.1", None)
            .await
            .status,
        200
    );
    assert_eq!(
        by_login(
            &h,
            Method::GET,
            "/api/hjv/update/windows/1.6.0",
            &kim,
            "10.3.5.1",
            None
        )
        .await
        .status,
        404
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_ACCOUNT, h.account_tag(HONG.0))
            .await,
        3
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_ACCOUNT, h.account_tag(KIM.0))
            .await,
        1
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("10.3.5.1"))
            .await,
        0
    );

    // a PC: its own count, whatever its account's logins and its account's other PCs did
    for i in 0..3 {
        let got = by_device(&h, Method::GET, &pc, WINDOWS, "10.3.5.2").await;
        assert_eq!((got.status, &got.body), (200, &exe), "{i}");
    }
    let got = by_device(&h, Method::GET, &pc, WINDOWS, "10.3.5.2").await;
    assert_eq!(got.status, 429);
    assert_eq!(
        by_device(&h, Method::HEAD, &pc, WINDOWS, "10.3.5.2")
            .await
            .status,
        200
    );
    assert_eq!(
        by_device(&h, Method::GET, &other_pc, WINDOWS, "10.3.5.2")
            .await
            .status,
        200
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_DEVICE, h.state.tags.device(&pc.id))
            .await,
        3
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_DEVICE, h.state.tags.device(&other_pc.id))
            .await,
        1
    );
    // a link's downloads are the link's: three of them, whatever the account downloaded itself
    let (url, _) = link(&h, &hong, "windows", "{}").await;
    assert_eq!(bare(&h, Method::GET, &url, "10.3.5.3").await.status, 200);
    // what the limit refused is no record, and the report shows the counters
    let results = h.results().await;
    assert_eq!(
        results
            .iter()
            .filter(|r| *r == "update:token-windows")
            .count(),
        4
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| *r == "update:device-windows")
            .count(),
        4
    );
    let report = h.admin(&["report"], false, None).await.unwrap();
    for line in [
        "update token-windows",
        "update device-windows",
        "update-ok-account",
        "update-ok-device",
        "update-link",
    ] {
        assert!(report.contains(line), "{line}\n{report}");
    }
    // the operator clears an account's by its ID, a PC's by its tag
    h.admin(&["unlock", HONG.0], false, None).await.unwrap();
    assert_eq!(
        by_login(&h, Method::GET, WINDOWS, &hong, "10.3.5.1", None)
            .await
            .status,
        200
    );
    let tag = h.state.tags.device(&pc.id);
    h.admin(&["unlock", &format!("tag:{tag}")], false, None)
        .await
        .unwrap();
    assert_eq!(
        by_device(&h, Method::GET, &pc, WINDOWS, "10.3.5.2")
            .await
            .status,
        200
    );
}

/// The Windows package is refused at door 1, at door 2 and for a new link.
async fn nobody_gets_windows(h: &Harness, token: &str) {
    let door = format!("/api/hjv/get/{DOOR2}/HJViewer-1.6.1.exe");
    for method in [Method::GET, Method::HEAD] {
        let got = by_login(h, method.clone(), WINDOWS, token, "10.3.6.1", None).await;
        assert_eq!(got.status, 404, "{method} door 1");
        let got = bare(h, method.clone(), &door, "10.3.6.1").await;
        assert_eq!(got.status, 404, "{method} door 2");
    }
    let post = "/api/hjv/update/windows/link";
    let got = by_login(h, Method::POST, post, token, "10.3.6.1", Some("{}")).await;
    assert_eq!(got.status, 404);
}

#[tokio::test]
async fn a_package_that_is_not_as_the_policy_says_goes_to_nobody() {
    let area = Area::new();
    let (exe, apk) = area.usual();
    area.door2(DOOR2, now() + 3600);
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;
    let door = format!("/api/hjv/get/{DOOR2}/HJViewer-1.6.1.exe");
    let (url, _) = link(&h, &token, "windows", "{}").await;

    // another size than the policy's: a file half copied, or another build under the name
    area.put(EXE, &exe[..exe.len() - 1]);
    nobody_gets_windows(&h, &token).await;
    assert_eq!(bare(&h, Method::GET, &url, "10.3.6.1").await.status, 404);
    let mut longer = exe.clone();
    longer.push(0);
    area.put(EXE, &longer);
    nobody_gets_windows(&h, &token).await;
    // not there
    std::fs::remove_file(area.pkg(EXE)).unwrap();
    nobody_gets_windows(&h, &token).await;
    // a link in its place, even to the very file
    area.put("elsewhere.exe", &exe);
    std::os::unix::fs::symlink(area.pkg("elsewhere.exe"), area.pkg(EXE)).unwrap();
    nobody_gets_windows(&h, &token).await;
    std::fs::remove_file(area.pkg(EXE)).unwrap();
    // a directory
    std::fs::create_dir(area.pkg(EXE)).unwrap();
    nobody_gets_windows(&h, &token).await;
    std::fs::remove_dir(area.pkg(EXE)).unwrap();
    // the other platform's package is not held up by it
    let got = by_login(&h, Method::GET, ANDROID, &token, "10.3.6.1", None).await;
    assert_eq!((got.status, &got.body), (200, &apk));
    // and the journal says what is wrong, without a path
    let journal = hjv_api::log::captured();
    assert!(journal.iter().any(|l| l.starts_with(
        "update: the windows package the policy names has another size than the policy says"
    )));
    // nothing of that was a failure of the address, a download or a record
    assert_eq!(
        h.counted(SCOPE_UPDATE_FAIL_IP, h.state.tags.ip("10.3.6.1"))
            .await,
        0
    );
    assert_eq!(
        h.counted(SCOPE_UPDATE_OK_ACCOUNT, h.account_tag(HONG.0))
            .await,
        1
    );

    // the file as the policy says: there again for everyone, the link of before too
    area.put(EXE, &exe);
    let got = by_login(&h, Method::GET, WINDOWS, &token, "10.3.6.1", None).await;
    assert_eq!((got.status, &got.body), (200, &exe));
    assert_eq!(bare(&h, Method::GET, &door, "10.3.6.1").await.body, exe);
    assert_eq!(bare(&h, Method::GET, &url, "10.3.6.1").await.body, exe);

    // a policy that is none hands out nothing, and the last good one is not used in its place
    std::fs::write(area.dir.join("version-policy.json"), b"{\"v\":1}").unwrap();
    nobody_gets_windows(&h, &token).await;
    std::fs::remove_file(area.dir.join("version-policy.json")).unwrap();
    nobody_gets_windows(&h, &token).await;
    // a policy whose package name is no name of the package directory is no policy
    std::fs::write(area.dir.join("secret.exe"), &exe).unwrap();
    area.policy(5, json!({"windows": entry("1.6.1", "../secret.exe", &exe)}));
    nobody_gets_windows(&h, &token).await;
    area.policy(6, json!({"windows": entry("1.6.1", EXE, &exe), "android": {"min": "1.5.0", "latest": "1.6.1"}}));
    assert_eq!(
        by_login(&h, Method::HEAD, WINDOWS, &token, "10.3.6.1", None)
            .await
            .status,
        200
    );
    // an entry without a package: nothing to get
    assert_eq!(
        by_login(&h, Method::HEAD, ANDROID, &token, "10.3.6.1", None)
            .await
            .status,
        404
    );
}

#[tokio::test]
async fn without_the_operators_files_every_path_is_closed() {
    // hjv-api as it is installed before the first policy: an empty package directory, no policy,
    // no file of door 2
    let area = Area::new();
    let h = start(&area, &[]).await;
    let token = h.login_ok(HONG, None).await;
    assert_eq!(
        by_login(&h, Method::GET, WINDOWS, &token, "10.3.7.1", None)
            .await
            .status,
        404
    );
    let got = by_login(
        &h,
        Method::POST,
        "/api/hjv/update/windows/link",
        &token,
        "10.3.7.1",
        Some("{}"),
    )
    .await;
    assert_eq!(got.status, 404);
    assert_eq!(
        bare(
            &h,
            Method::GET,
            &format!("/api/hjv/get/{DOOR2}/"),
            "10.3.7.1"
        )
        .await
        .status,
        404
    );
    assert_eq!(
        bare(&h, Method::GET, "/api/hjv/u/K7M2-Q9XD", "10.3.7.1")
            .await
            .status,
        404
    );
    // and the account paths are as they were
    assert_eq!(h.get("/api/users", &token).await.status, 200);
}

#[tokio::test]
async fn the_journal_and_the_records_hold_tags_only() {
    let area = Area::new();
    area.usual();
    area.door2(DOOR2, now() + 3600);
    let h = start(&area, &[("HJV_API_LIMIT_UPDATE_FAIL_IP", "4/86400")]).await;
    let pc = bound_pc(&h, "860000001", LEE).await;
    let token = h.login_ok(LEE, None).await;
    let ip = "203.0.113.77";
    let auth = format!("Bearer {token}");
    let proof = h.proof_for("GET", WINDOWS, b"", &token).unwrap();
    let got = send(
        &h,
        Method::GET,
        WINDOWS,
        &[
            ("authorization", &auth),
            ("x-hjv-proof", &proof),
            ("x-real-ip", ip),
        ],
        None,
    )
    .await;
    assert_eq!(got.status, 200);
    let signed = device_headers(&pc.id, &pc.seed, "windows", "1.6.1", now());
    assert_eq!(
        with_device_headers(&h, Method::GET, WINDOWS, &signed, ip)
            .await
            .status,
        200
    );
    let (url, _) = link(&h, &token, "windows", "{\"installed\":true}").await;
    assert_eq!(bare(&h, Method::GET, &url, ip).await.status, 200);
    let door = format!("/api/hjv/get/{DOOR2}/HJViewer-1.6.1.exe");
    assert_eq!(bare(&h, Method::GET, &door, ip).await.status, 200);
    // and what is refused: a wrong token, a wrong signature, wrong codes, until the address has
    // had its failures
    let wrong_token = format!("Bearer hjv_{}", "w".repeat(43));
    let wrong_sig = device_headers(&pc.id, &[3; 32], "windows", "1.6.1", now());
    let wrong_code = "/api/hjv/u/ZZZZ-ZZZZ";
    let wrong_door = "/api/hjv/get/ffffffffffffffffffffffffffffffff/";
    send(
        &h,
        Method::GET,
        WINDOWS,
        &[("authorization", &wrong_token), ("x-real-ip", ip)],
        None,
    )
    .await;
    with_device_headers(&h, Method::GET, WINDOWS, &wrong_sig, ip).await;
    for _ in 0..3 {
        bare(&h, Method::GET, wrong_code, ip).await;
        bare(&h, Method::GET, wrong_door, ip).await;
    }

    let code = &url["/api/hjv/u/".len()..];
    // the start of a code's SHA-256: anyone could compute it for each of a short link's 2^40
    // codes, and so find the few hundred that a line of the journal leaves
    let plain_hash = |code: &str| hex(&sha256(code.as_bytes()))[..8].to_owned();
    let (short_hash, door_hash) = (plain_hash(&code.replace('-', "")), plain_hash(DOOR2));
    let secrets = [
        token.as_str(),
        &token[4..],
        proof.as_str(),
        proof.rsplit('.').next().unwrap(),
        signed[2].1.as_str(),
        wrong_sig[2].1.as_str(),
        &wrong_token[7..],
        code,
        &code.replace('-', ""),
        "ZZZZ-ZZZZ",
        "ZZZZZZZZ",
        DOOR2,
        "ffffffffffffffffffffffffffffffff",
        &short_hash,
        &door_hash,
        // and who and where: the journal and the update records know people, devices and
        // addresses by their tags
        LEE.0,
        pc.id.as_str(),
        ip,
    ];
    let journal = hjv_api::log::captured();
    let mine: Vec<&String> = journal
        .iter()
        .filter(|l| l.starts_with("update: "))
        .collect();
    assert!(mine.len() >= 8, "{mine:#?}");
    for line in &mine {
        for secret in secrets {
            assert!(!line.contains(secret), "{secret} in: {line}");
        }
    }
    // what the journal does say: the package, the tags, a code as the start of its tag (keyed
    // with the log key)
    let (account, device, ip_tag) = (
        h.account_tag(LEE.0),
        h.state.tags.device(&pc.id),
        h.state.tags.ip(ip),
    );
    let short = &h.state.tags.tag("code", &code.replace('-', ""))[..8];
    let door_tag = &h.state.tags.tag("code", DOOR2)[..8];
    for line in [
        format!("update: windows 1.6.1 to a login (acct={account}, ip={ip_tag})"),
        format!("update: windows 1.6.1 to a PC (acct={account}, dev={device}, ip={ip_tag})"),
        format!("update: a short link for windows 1.6.1 (acct={account}, code={short})"),
        format!("update: windows 1.6.1 by a short link (acct={account}, ip={ip_tag}, code={short}, 2 left)"),
        format!("update: windows 1.6.1 through door 2 (ip={ip_tag}, code={door_tag})"),
        format!("update: refused, no login with its proof (ip={ip_tag}); the next ten minutes are not logged"),
        format!("update: refused, a signature that is not the PC's (ip={ip_tag}); the next ten minutes are not logged"),
        format!("update: refused, no short link of that code (ip={ip_tag}); the next ten minutes are not logged"),
    ] {
        assert!(mine.iter().any(|l| l.contains(&line)), "{line}\n{mine:#?}");
    }
    assert!(mine.iter().any(|l| {
        l.starts_with("update: door 2's file (HJV_API_DOOR2_FILE) read: open until ")
            && l.ends_with(&format!(" (code={door_tag})"))
    }));
    // the records of the downloads
    let rows: Vec<String> = h
        .state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT result || ' ' || COALESCE(account_tag, '-') || ' ' ||
                        COALESCE(device_tag, '-') || ' ' || COALESCE(ip, '-')
                 FROM events WHERE kind = 'update' ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap();
    assert_eq!(
        rows,
        [
            format!("token-windows {account} - {ip_tag}"),
            format!("device-windows {account} {device} {ip_tag}"),
            format!("device-new-address {account} {device} {ip_tag}"),
            format!(
                "link-made-windows {account} - {}",
                h.state.tags.ip("10.3.0.1")
            ),
            format!("link-windows {account} - {ip_tag}"),
            format!("door2-windows - - {ip_tag}"),
        ]
    );
    for row in &rows {
        for secret in secrets {
            assert!(!row.contains(secret), "{secret} in: {row}");
        }
    }
    // the operator record: the one line of the PC's first address, tags only
    let record = std::fs::read_to_string(&h.oplog).unwrap();
    assert_eq!(record.lines().count(), 1, "{record}");
    assert!(record.contains(&format!(
        "cmd=\"update device-new-address\" targets={device},{ip_tag} reveal=0"
    )));
    for secret in secrets {
        assert!(!record.contains(secret), "{secret} in: {record}");
    }
    // and the limit counters are under tags
    let keys: Vec<String> = h
        .state
        .db
        .call(|c| {
            let mut stmt =
                c.prepare("SELECT DISTINCT key FROM attempts WHERE scope LIKE 'update-%'")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect()
        })
        .await
        .unwrap();
    assert_eq!(keys.len(), 3, "{keys:?}");
    for key in &keys {
        assert!([&account, &device, &ip_tag].contains(&key), "{key}");
    }
}
