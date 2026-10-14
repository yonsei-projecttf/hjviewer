// SPDX-License-Identifier: AGPL-3.0-only

//! How a package goes out (body.rs, server.rs; the design of the minimum version and forced
//! update, §4-6): a piece at a time whatever the size of the file and the speed of its reader,
//! longer than a request may take, at most HJV_API_DOWNLOADS_MAX at once, and never held by a
//! reader that stops, nor past its hour by one that takes a few bytes at a time.
//!
//! The tests of time run on tokio's paused clock, with hjv-api on one end of a connection in
//! memory (`server::serve_stream`), so that a minute and an hour pass at once and nothing depends
//! on how busy the machine is. The test of memory runs the program itself.

mod common;

use common::temp_dir;
use hjv_api::{
    body::{DOWNLOAD_TIME, PIECE, RATE, WRITE_STALL},
    config::{Config, Secrets},
    db, server,
    state::AppState,
    util::{b64, datetime, now},
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    os::unix::fs::FileExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    net::TcpStream,
    time::Instant,
};
use zeroize::Zeroizing;

const DOOR2: &str = "5ca1ab1e0ddba11c0ffee15dec0dedaa";
const EXE: &str = "HJViewer-9.9.9-windows-x86_64.exe";
/// The package through door 2, which needs no login.
const DOWNLOAD: &str = "/api/hjv/get/5ca1ab1e0ddba11c0ffee15dec0dedaa/HJViewer-9.9.9.exe";
const PAGE: &str = "/api/hjv/get/5ca1ab1e0ddba11c0ffee15dec0dedaa/";

/// The byte of a package at `at`: its place shows in it.
fn byte_at(at: u64) -> u8 {
    (at % 251) as u8 ^ (at >> 16) as u8
}

/// The operator's files: door 2 open, and a policy whose Windows package has `size` bytes.
/// `sparse` leaves the file empty but for a few marks (a large file that takes no disk).
fn area(size: u64, sparse: bool) -> PathBuf {
    let dir = temp_dir("stream");
    std::fs::create_dir_all(dir.join("pkg")).unwrap();
    let file = std::fs::File::create(dir.join("pkg").join(EXE)).unwrap();
    if sparse {
        file.set_len(size).unwrap();
        for at in marks(size) {
            file.write_all_at(&[byte_at(at) | 1], at).unwrap();
        }
    } else {
        let bytes: Vec<u8> = (0..size).map(byte_at).collect();
        file.write_all_at(&bytes, 0).unwrap();
    }
    let policy = json!({"v": 1, "epoch": 1, "seq": 1, "issued": "2026-10-05T03:00:00Z",
        "server": "rd.example.com",
        "platforms": {"windows": {"min": "1.6.0", "latest": "9.9.9", "package": {
            "file": EXE, "size": size, "sha256": "00".repeat(32), "build": "9.9.9.1"}}}});
    let envelope = json!({"v": 1, "k": 1, "p": policy.to_string(), "s": "AAAA"});
    std::fs::write(dir.join("version-policy.json"), envelope.to_string()).unwrap();
    std::fs::write(
        dir.join("door2.conf"),
        format!("CODE={DOOR2}\nUNTIL={}\n", datetime(now() + 365 * 86_400)),
    )
    .unwrap();
    dir
}

/// The places of a sparse file that are not zero: its ends, and around the seams of pieces.
fn marks(size: u64) -> Vec<u64> {
    let piece = PIECE as u64;
    let mut marks = vec![
        0,
        piece - 1,
        piece,
        size / 2 + 12_345,
        size - piece,
        size - 1,
    ];
    marks.retain(|at| *at < size);
    marks
}

fn settings(dir: &Path, more: &[(&str, &str)]) -> HashMap<String, String> {
    let path = |name: &str| dir.join(name).display().to_string();
    let mut vars: HashMap<String, String> = [
        (
            "HJV_API_SSO_TOKEN_URL",
            "http://127.0.0.1:9/token".to_owned(),
        ),
        (
            "HJV_API_SSO_USERINFO_URL",
            "http://127.0.0.1:9/user".to_owned(),
        ),
        ("HJV_API_SSO_CLIENT_ID", "client".to_owned()),
        ("HJV_API_VERSION_POLICY", path("version-policy.json")),
        ("HJV_API_PACKAGE_DIR", path("pkg")),
        ("HJV_API_DOOR2_FILE", path("door2.conf")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    for (k, v) in more {
        vars.insert((*k).to_owned(), (*v).to_owned());
    }
    vars
}

/// hjv-api's state over the files of `dir`, without a socket.
fn state(dir: &Path, more: &[(&str, &str)]) -> Arc<AppState> {
    hjv_api::log::capture();
    let config = Config::from_vars(&settings(dir, more)).unwrap();
    let secrets = Secrets {
        sso_basic: Zeroizing::new("basic".to_owned()),
        sso_api_uuid: Zeroizing::new("uuid".to_owned()),
        seal_keys: vec![Zeroizing::new([42; 32])],
        log_key: Zeroizing::new(vec![7; 32]),
        list_key: Zeroizing::new([1; 32]),
        command_key: None,
    };
    AppState::new(config, secrets, db::open_in_memory().unwrap(), None).unwrap()
}

/// A connection to hjv-api in memory, whose pipe holds `buffer` bytes each way.
fn connect(state: &Arc<AppState>, buffer: usize) -> DuplexStream {
    let (near, far) = tokio::io::duplex(buffer);
    tokio::spawn(server::serve_stream(state.clone(), far));
    near
}

/// The head of an answer: its status and its header lines, in lower case.
struct Head {
    status: u16,
    lines: Vec<String>,
}

impl Head {
    fn header(&self, name: &str) -> &str {
        self.lines
            .iter()
            .find_map(|line| line.strip_prefix(&format!("{name}: ")))
            .unwrap_or("")
    }

    fn length(&self) -> u64 {
        self.header("content-length").parse().unwrap()
    }
}

/// Sends a request and reads the head of its answer, not a byte of the body.
async fn ask<S>(stream: &mut S, method: &str, path: &str) -> Head
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = format!("{method} {path} HTTP/1.1\r\nHost: rd.example.com\r\n\r\n");
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        head.push(stream.read_u8().await.expect("the head of an answer"));
    }
    let head = String::from_utf8(head).unwrap().to_ascii_lowercase();
    let mut lines = head.trim_end().split("\r\n").map(str::to_owned);
    let status = lines.next().unwrap()[9..12].parse().unwrap();
    Head {
        status,
        lines: lines.collect(),
    }
}

/// Reads `want` bytes of a body that starts at `from` and checks each against the package.
async fn read_checked<S: AsyncRead + Unpin>(stream: &mut S, from: u64, want: u64) {
    let mut buf = vec![0u8; PIECE];
    let mut at = from;
    while at < from + want {
        let room = ((from + want - at) as usize).min(buf.len());
        let n = stream.read(&mut buf[..room]).await.unwrap();
        assert!(n > 0, "the body ended at {at}");
        for (i, b) in buf[..n].iter().enumerate() {
            assert_eq!(*b, byte_at(at + i as u64), "the byte at {}", at + i as u64);
        }
        at += n as u64;
    }
}

/// Reads on until the connection ends; how many bytes came.
async fn read_to_the_end<S: AsyncRead + Unpin>(stream: &mut S) -> u64 {
    let mut buf = vec![0u8; PIECE];
    let mut total = 0;
    loop {
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => return total,
            Ok(n) => total += n as u64,
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_slow_reader_gets_the_whole_package() {
    let size = 16 * PIECE as u64 + 4321;
    let dir = area(size, false);
    let state = state(&dir, &[]);
    let mut stream = connect(&state, 4096);
    let started = Instant::now();
    let head = ask(&mut stream, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    assert_eq!(state.updates.slots.free(), 3);
    // 40 seconds of nothing, then 16 KiB, again and again: well past the minute a request has
    // for its answer's head, and never a minute without taking anything
    let mut at = 0;
    while at < size {
        tokio::time::sleep(Duration::from_secs(40)).await;
        let step = (size - at).min(16 * 1024);
        read_checked(&mut stream, at, step).await;
        at += step;
    }
    assert!(started.elapsed() > Duration::from_secs(40 * 60));
    assert!(started.elapsed() < DOWNLOAD_TIME);
    // the connection is as good as before: the next request on it is answered
    let head = ask(&mut stream, "HEAD", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    assert_eq!(state.updates.slots.free(), 4);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_reader_that_stops_loses_its_connection_and_its_slot() {
    let size = 64 * PIECE as u64;
    let dir = area(size, false);
    let state = state(&dir, &[("HJV_API_DOWNLOADS_MAX", "1")]);
    let mut stuck = connect(&state, 4096);
    let head = ask(&mut stuck, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    read_checked(&mut stuck, 0, 10_000).await;
    assert_eq!(state.updates.slots.free(), 0);
    // meanwhile nobody else gets a package, and is told when to try again
    let mut other = connect(&state, 4096);
    let head = ask(&mut other, "GET", DOWNLOAD).await;
    assert_eq!(head.status, 503);
    assert_eq!(head.header("retry-after"), "60");
    assert_eq!(
        head.header("content-type"),
        "application/json; charset=utf-8"
    );
    let mut body = vec![0u8; head.length() as usize];
    other.read_exact(&mut body).await.unwrap();
    let answer: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        (answer["hjv"].as_str(), answer["retry_after"].as_i64()),
        (Some("busy"), Some(60))
    );
    // the reader takes nothing for a minute: hjv-api gives the connection up
    let stopped = Instant::now();
    tokio::time::sleep(WRITE_STALL - Duration::from_secs(1)).await;
    assert_eq!(state.updates.slots.free(), 0);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(state.updates.slots.free(), 1);
    // what was on the way still comes, and then the end, far from the whole file
    let rest = read_to_the_end(&mut stuck).await;
    assert!(10_000 + rest < size / 2, "{rest}");
    assert!(stopped.elapsed() < WRITE_STALL + Duration::from_secs(5));
    // and the slot is for the next one, who gets all of it
    let mut next = connect(&state, 4096);
    let head = ask(&mut next, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    read_checked(&mut next, 0, size).await;
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(start_paused = true)]
async fn the_fifth_download_at_once_waits_its_turn() {
    let size = 8 * PIECE as u64;
    let dir = area(size, false);
    let state = state(&dir, &[]);
    assert_eq!(state.updates.slots.free(), 4);
    let mut running = Vec::new();
    for i in 0..4 {
        let mut stream = connect(&state, 4096);
        let head = ask(&mut stream, "GET", DOWNLOAD).await;
        assert_eq!((head.status, head.length()), (200, size), "{i}");
        running.push(stream);
    }
    assert_eq!(state.updates.slots.free(), 0);
    let mut fifth = connect(&state, 4096);
    for _ in 0..3 {
        let head = ask(&mut fifth, "GET", DOWNLOAD).await;
        assert_eq!((head.status, head.header("retry-after")), (503, "60"));
        let mut body = vec![0u8; head.length() as usize];
        fifth.read_exact(&mut body).await.unwrap();
        assert!(String::from_utf8(body)
            .unwrap()
            .contains("\"hjv\":\"busy\""));
    }
    // what is no download of a package needs no slot: looking at one, and door 2's page
    let head = ask(&mut fifth, "HEAD", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    let head = ask(&mut fifth, "GET", PAGE).await;
    assert_eq!(head.status, 200);
    let mut page = vec![0u8; head.length() as usize];
    fifth.read_exact(&mut page).await.unwrap();
    assert!(String::from_utf8(page)
        .unwrap()
        .contains("HJViewer-9.9.9-install.exe"));
    // one that is through gives its slot on
    let mut first = running.remove(0);
    read_checked(&mut first, 0, size).await;
    tokio::task::yield_now().await;
    assert_eq!(state.updates.slots.free(), 1);
    let head = ask(&mut fifth, "GET", DOWNLOAD).await;
    assert_eq!(head.status, 200);
    assert_eq!(state.updates.slots.free(), 0);
    let mut sixth = connect(&state, 4096);
    let head = ask(&mut sixth, "GET", DOWNLOAD).await;
    assert_eq!(head.status, 503);
    let mut body = vec![0u8; head.length() as usize];
    sixth.read_exact(&mut body).await.unwrap();
    // and so does one whose reader went away in the middle
    drop(running.remove(0));
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert_eq!(state.updates.slots.free(), 1);
    assert_eq!(ask(&mut sixth, "GET", DOWNLOAD).await.status, 200);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_download_ends_after_an_hour() {
    let size = 256 * PIECE as u64;
    let dir = area(size, true);
    let state = state(&dir, &[("HJV_API_DOWNLOADS_MAX", "1")]);
    let mut stream = connect(&state, PIECE);
    let started = Instant::now();
    let head = ask(&mut stream, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    // a piece every 50 seconds: never a minute without taking any, and far too slow
    let mut buf = vec![0u8; PIECE];
    let mut total = 0u64;
    loop {
        tokio::time::sleep(Duration::from_secs(50)).await;
        match stream.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => total += n as u64,
        }
        assert!(started.elapsed() < DOWNLOAD_TIME + Duration::from_secs(600));
    }
    assert!(
        started.elapsed() >= DOWNLOAD_TIME,
        "{:?}",
        started.elapsed()
    );
    assert!(total > 32 * PIECE as u64 && total < size / 2, "{total}");
    assert_eq!(state.updates.slots.free(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_reader_that_takes_a_few_bytes_at_a_time_loses_its_slot_after_an_hour() {
    let size = 256 * PIECE as u64;
    let dir = area(size, true);
    let state = state(&dir, &[("HJV_API_DOWNLOADS_MAX", "1")]);
    let mut stream = connect(&state, 4096);
    let started = Instant::now();
    let head = ask(&mut stream, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    assert_eq!(state.updates.slots.free(), 0);
    // 200 bytes every 50 seconds: never a minute without taking any, and so slow that hyper
    // never gets to ask the body for its second piece
    let mut buf = [0u8; 200];
    let freed = loop {
        tokio::time::sleep(Duration::from_secs(50)).await;
        if state.updates.slots.free() == 1 {
            break started.elapsed();
        }
        assert!(
            started.elapsed() < DOWNLOAD_TIME + Duration::from_secs(300),
            "the slot is still held after {:?}",
            started.elapsed()
        );
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "the connection ended, and the slot is still held");
    };
    // the first write after the hour fails, and this loop looks every 50 seconds
    assert!(freed >= DOWNLOAD_TIME, "{freed:?}");
    assert!(
        freed <= DOWNLOAD_TIME + Duration::from_secs(100),
        "{freed:?}"
    );
    // what was on its way still comes, and then the end
    let rest = read_to_the_end(&mut stream).await;
    assert!(rest <= 4096, "{rest}");
    // and the slot is for the next one
    let mut next = connect(&state, PIECE);
    let head = ask(&mut next, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    std::fs::remove_dir_all(dir).unwrap();
}

/// The hour is the answer's, not the connection's: a download that ends a few seconds before its
/// hour leaves the connection as it was, and the next request on it, past the hour, is answered,
/// also when its head comes in pieces of 16 bytes, between which hyper flushes (the review of the
/// sweep: server.rs handle() clears the deadline for each request, body.rs looks at it in the
/// writes of the body only).
#[tokio::test(start_paused = true)]
async fn the_hour_of_a_download_is_not_the_connections() {
    for (buffer, chunk) in [(4096, 4096u64), (16, 16)] {
        // 72 chunks for one every 50 seconds, and the rest at once at the end
        let size = 72 * chunk + 4 * 1024;
        let dir = area(size, false);
        let state = state(&dir, &[]);
        let mut stream = connect(&state, buffer);
        let started = Instant::now();
        let head = ask(&mut stream, "GET", DOWNLOAD).await;
        assert_eq!((head.status, head.length()), (200, size));
        let mut buf = vec![0u8; chunk as usize];
        let mut at = 0u64;
        while started.elapsed() < Duration::from_secs(3540) {
            tokio::time::sleep(Duration::from_secs(50)).await;
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n > 0, "the body ended at {at}");
            at += n as u64;
        }
        // the rest six seconds before the hour: the answer is complete
        tokio::time::sleep_until(started + Duration::from_secs(3594)).await;
        read_checked(&mut stream, at, size - at).await;
        assert!(started.elapsed() < DOWNLOAD_TIME);
        // the next request on the connection, past the hour
        tokio::time::sleep_until(started + DOWNLOAD_TIME + Duration::from_secs(2)).await;
        let head = ask(&mut stream, "HEAD", DOWNLOAD).await;
        assert_eq!(
            (head.status, head.length()),
            (200, size),
            "a pipe of {buffer} bytes"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// The largest package a policy names, all of it, on the paused clock: every mark in its place,
/// no faster than the rate files go out at, and this process (hjv-api and its reader in one)
/// holds no more memory after it than before.
#[tokio::test(start_paused = true)]
async fn the_largest_package_goes_out_whole_at_the_rate() {
    let size: u64 = 200 * 1024 * 1024;
    let dir = area(size, true);
    let state = state(&dir, &[]);
    let mut stream = connect(&state, PIECE);
    let (before, _) = memory(std::process::id());
    let started = Instant::now();
    let head = ask(&mut stream, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    read_sparse(&mut stream, 0, size).await;
    let took = started.elapsed().as_secs_f64();
    // 200 MiB at 3 840 000 bytes a second is 54.6 seconds
    let fastest = size as f64 / RATE as f64;
    assert!(took >= fastest * 0.999 && took < fastest * 1.05, "{took}");
    assert_eq!(state.updates.slots.free(), 4);
    let (after, _) = memory(std::process::id());
    assert!(
        after.saturating_sub(before) < 32 * 1024,
        "{before} KiB, then {after} KiB"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

/// Reads `want` bytes of a sparse package's body from `from` on: each mark in its place, and
/// zero wherever a read begins that is no mark.
async fn read_sparse<S: AsyncRead + Unpin>(stream: &mut S, from: u64, want: u64) {
    let size = 200 * 1024 * 1024;
    let marks = marks(size);
    let mut buf = vec![0u8; PIECE];
    let mut at = from;
    while at < from + want {
        let room = ((from + want - at) as usize).min(buf.len());
        let n = stream.read(&mut buf[..room]).await.unwrap();
        assert!(n > 0, "the body ended at {at}");
        for mark in marks.iter().filter(|m| (at..at + n as u64).contains(m)) {
            assert_eq!(
                buf[(mark - at) as usize],
                byte_at(*mark) | 1,
                "the byte at {mark}"
            );
        }
        if !marks.contains(&at) {
            assert_eq!(buf[0], 0, "the byte at {at}");
        }
        at += n as u64;
    }
}

const BIN: &str = env!("CARGO_BIN_EXE_hjv-api");

/// `VmHWM` (the most memory the process ever held) and `VmRSS` of a process, in KiB.
fn memory(pid: u32) -> (u64, u64) {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let field = |name: &str| -> u64 {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
            .unwrap_or_else(|| panic!("no {name} in /proc/{pid}/status"))
    };
    (field("VmHWM:"), field("VmRSS:"))
}

struct Child(std::process::Child);

impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The program itself, with a package of 200 MiB: while it sends it, to a reader that takes
/// what comes and then to one that stops for a while, it holds no more memory than for a small
/// answer. (At the rate files go out at, all of 200 MiB takes 55 seconds: the test above sends
/// all of it, on the paused clock; this one takes the first megabytes from the real program.)
#[tokio::test]
async fn the_program_holds_a_piece_of_a_package_not_the_package() {
    let size: u64 = 200 * 1024 * 1024;
    let dir = area(size, true);
    let env = dir.join("hjviewer-api.env");
    let text: String = settings(&dir, &[])
        .iter()
        .map(|(k, v)| format!("{k}={v}\n"))
        .collect();
    std::fs::write(&env, text).unwrap();
    let credentials = dir.join("credentials");
    std::fs::create_dir_all(&credentials).unwrap();
    for (name, value) in [
        ("hjviewer-sso-client-secret", "basic".to_owned()),
        ("hjviewer-sso-api-uuid", "uuid".to_owned()),
        ("hjviewer-api-login-key", b64(&[11u8; 32])),
        ("hjviewer-api-log-key", b64(&[5u8; 32])),
        ("hjviewer-api-list-key", b64(&[1u8; 32])),
    ] {
        std::fs::write(credentials.join(name), value).unwrap();
    }
    // a port that is free now
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let child = Command::new(BIN)
        .arg("serve")
        .arg("--db")
        .arg(dir.join("hjv-api.sqlite3"))
        .arg("--env-file")
        .arg(&env)
        .arg("--credentials")
        .arg(&credentials)
        .args(["--listen", &addr.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let child = Child(child);
    let pid = child.0.id();
    let mut stream = None;
    for _ in 0..200 {
        match TcpStream::connect(addr).await {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    let mut stream = stream.unwrap_or_else(|| panic!("hjv-api does not listen on {addr}"));

    // a small answer first, so that what is measured after it is the package's
    let head = ask(&mut stream, "GET", PAGE).await;
    assert_eq!(head.status, 200);
    let mut page = vec![0u8; head.length() as usize];
    stream.read_exact(&mut page).await.unwrap();
    let head = ask(&mut stream, "HEAD", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    let (before, _) = memory(pid);

    let started = std::time::Instant::now();
    let head = ask(&mut stream, "GET", DOWNLOAD).await;
    assert_eq!((head.status, head.length()), (200, size));
    assert_eq!(
        head.header("content-type"),
        "application/vnd.microsoft.portable-executable"
    );
    // the first megabytes, as fast as they come: no faster than the rate
    let taken = 6 * 1024 * 1024;
    read_sparse(&mut stream, 0, taken).await;
    let took = started.elapsed().as_secs_f64();
    assert!(
        taken as f64 / took <= RATE as f64 * 1.1,
        "{taken} bytes in {took} s"
    );
    let (reading, _) = memory(pid);
    // then nothing for a while: the file is not read ahead of its reader
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let (paused, _) = memory(pid);
    read_sparse(&mut stream, taken, 1024 * 1024).await;
    drop(stream);

    eprintln!(
        "hjv-api's most memory: {before} KiB before the package, {reading} KiB while it goes out, {paused} KiB with its reader paused"
    );
    // the unit allows 256 MiB (MemoryMax=); the file is 204800 KiB
    let grown = reading.max(paused).saturating_sub(before);
    assert!(grown < 16 * 1024, "{grown} KiB more for a package");
    assert!(
        reading.max(paused) < 96 * 1024,
        "{reading} KiB, {paused} KiB"
    );
    drop(child);
    std::fs::remove_dir_all(dir).unwrap();
}
