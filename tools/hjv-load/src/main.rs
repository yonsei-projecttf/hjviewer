// SPDX-License-Identifier: AGPL-3.0-or-later

//! Synthetic load for an hjviewer server, to size it.
//!
//!     hjv-load peers <server> [--port 443] [--ca root.pem] [-n 25] [--secs 60]
//!         N fake PCs register over wss://<server>/ws/id and echo the heartbeats, as idle PCs do.
//!     hjv-load relay <server> [--port 443] [--ca root.pem] [-n 1] [--mbps 5] [--secs 30] [--key K]
//!         N sessions relayed through wss://<server>/ws/relay, each pushing --mbps one way.
//!         --key is the server's public key (data/id_ed25519.pub) when hbbr runs with -k.
//!
//! hbbs lets one IP register about 30 devices a minute and 300 a day, so keep `peers -n` small.

use futures_util::{SinkExt, StreamExt};
use std::{
    process::ExitCode,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{tungstenite::Message, Connector, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

// RendezvousMessage fields (hbb_common protos/rendezvous.proto)
const REGISTER_PK: u32 = 15;
const REGISTER_PK_RESPONSE: u32 = 16;
const REQUEST_RELAY: u32 = 18;

struct Opts {
    cmd: String,
    host: String,
    port: u16,
    ca: Option<String>,
    n: usize,
    secs: u64,
    mbps: f64,
    key: String,
}

fn parse() -> Result<Opts, String> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().ok_or("missing command: peers or relay")?;
    if cmd != "peers" && cmd != "relay" {
        return Err(format!("unknown command {cmd}"));
    }
    let host = args.next().ok_or("missing <server>")?;
    let mut o = Opts {
        cmd,
        host,
        port: 443,
        ca: None,
        n: 0,
        secs: 0,
        mbps: 5.0,
        key: String::new(),
    };
    while let Some(arg) = args.next() {
        let value = args.next().ok_or(format!("{arg} needs a value"))?;
        let bad = |what: &str| format!("{arg}: {what}");
        match arg.as_str() {
            "--port" => o.port = value.parse().map_err(|_| bad("not a port"))?,
            "--ca" => o.ca = Some(value),
            "-n" => o.n = value.parse().map_err(|_| bad("not a number"))?,
            "--secs" => o.secs = value.parse().map_err(|_| bad("not a number"))?,
            "--mbps" => o.mbps = value.parse().map_err(|_| bad("not a number"))?,
            "--key" => o.key = value,
            _ => return Err(format!("unexpected argument {arg}")),
        }
    }
    if o.n == 0 {
        o.n = if o.cmd == "peers" { 25 } else { 1 };
    }
    if o.secs == 0 {
        o.secs = if o.cmd == "peers" { 60 } else { 30 };
    }
    Ok(o)
}

fn connector(ca: Option<&str>) -> Result<Connector, String> {
    let mut builder = native_tls::TlsConnector::builder();
    if let Some(path) = ca {
        let pem = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
        let cert = native_tls::Certificate::from_pem(&pem).map_err(|e| format!("{path}: {e}"))?;
        builder.add_root_certificate(cert);
    }
    builder
        .build()
        .map(Connector::NativeTls)
        .map_err(|e| e.to_string())
}

async fn open(o: &Opts, path: &str) -> Result<Ws, String> {
    let url = format!("wss://{}:{}{path}", o.host, o.port);
    let connect = tokio_tungstenite::connect_async_tls_with_config(
        url,
        None,
        true,
        Some(connector(o.ca.as_deref())?),
    );
    match timeout(Duration::from_secs(10), connect).await {
        Err(_) => Err("connect timed out".to_owned()),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

fn put_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push(v as u8 | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn put_bytes(buf: &mut Vec<u8>, field: u32, data: &[u8]) {
    put_varint(buf, u64::from(field) << 3 | 2);
    put_varint(buf, data.len() as u64);
    buf.extend_from_slice(data);
}

fn get_varint(buf: &[u8], pos: &mut usize) -> Option<u64> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let b = *buf.get(*pos)?;
        *pos += 1;
        v |= u64::from(b & 0x7f) << shift;
        if b < 0x80 {
            return Some(v);
        }
    }
    None
}

enum Field<'a> {
    Varint(u64),
    Bytes(&'a [u8]),
}

/// Field `field` of a protobuf message, if present.
fn find(buf: &[u8], field: u32) -> Option<Field<'_>> {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = get_varint(buf, &mut pos)?;
        let hit = tag >> 3 == u64::from(field);
        match tag & 7 {
            0 => {
                let v = get_varint(buf, &mut pos)?;
                if hit {
                    return Some(Field::Varint(v));
                }
            }
            1 => pos += 8,
            2 => {
                let len = usize::try_from(get_varint(buf, &mut pos)?).ok()?;
                let data = buf.get(pos..pos.checked_add(len)?)?;
                if hit {
                    return Some(Field::Bytes(data));
                }
                pos += len;
            }
            5 => pos += 4,
            _ => return None,
        }
    }
    None
}

/// Bytes that differ per run and per index, for IDs and UUIDs (no RNG dependency).
fn unique(i: usize) -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or_default();
    nanos ^ (i as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
}

async fn peer(o: Arc<Opts>, run: u64, i: usize, beats: Arc<AtomicU64>) -> Result<(), String> {
    let mut ws = open(&o, "/ws/id").await?;
    let mut rk = Vec::new();
    put_bytes(&mut rk, 1, format!("9{:05}{:03}", run % 100_000, i).as_bytes());
    put_bytes(&mut rk, 2, &unique(i).to_le_bytes());
    let mut pk = [0u8; 32];
    pk[..8].copy_from_slice(&unique(i + 1).to_le_bytes());
    put_bytes(&mut rk, 3, &pk);
    let mut msg = Vec::new();
    put_bytes(&mut msg, REGISTER_PK, &rk);
    ws.send(Message::Binary(msg)).await.map_err(|e| e.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(o.secs);
    let mut registered = false;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let frame = match timeout(left, ws.next()).await {
            Err(_) => break,
            Ok(None) => return Err("closed by the server".to_owned()),
            Ok(Some(Err(e))) => return Err(e.to_string()),
            Ok(Some(Ok(frame))) => frame,
        };
        let Message::Binary(bytes) = frame else {
            continue;
        };
        if bytes.is_empty() {
            beats.fetch_add(1, Ordering::Relaxed);
            ws.send(Message::Binary(Vec::new()))
                .await
                .map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(Field::Bytes(res)) = find(&bytes, REGISTER_PK_RESPONSE) {
            match find(res, 1) {
                None | Some(Field::Varint(0)) => registered = true,
                Some(Field::Varint(code)) => return Err(format!("registration refused, code {code}")),
                Some(Field::Bytes(_)) => return Err("malformed response".to_owned()),
            }
        }
    }
    if registered {
        Ok(())
    } else {
        Err("no registration answer".to_owned())
    }
}

async fn relay_half(o: &Opts, uuid: &str) -> Result<Ws, String> {
    let mut ws = open(o, "/ws/relay").await?;
    let mut rr = Vec::new();
    put_bytes(&mut rr, 2, uuid.as_bytes());
    if !o.key.is_empty() {
        put_bytes(&mut rr, 6, o.key.as_bytes());
    }
    let mut msg = Vec::new();
    put_bytes(&mut msg, REQUEST_RELAY, &rr);
    ws.send(Message::Binary(msg)).await.map_err(|e| e.to_string())?;
    Ok(ws)
}

async fn session(o: Arc<Opts>, run: u64, i: usize, received: Arc<AtomicU64>) -> Result<(), String> {
    let uuid = format!("hjv-load-{run}-{i}");
    let (a, b) = tokio::join!(relay_half(&o, &uuid), relay_half(&o, &uuid));
    let (mut a, mut b) = (a?, b?);
    let secs = o.secs;
    let reader = tokio::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(secs + 2);
        while let Ok(Some(Ok(frame))) = timeout(deadline.saturating_duration_since(Instant::now()), b.next()).await {
            if let Message::Binary(bytes) = frame {
                received.fetch_add(bytes.len() as u64, Ordering::Relaxed);
            }
        }
    });
    // paced in 10 ms ticks, 16 KiB frames, the way a video stream arrives in bursts
    let per_tick = (o.mbps * 1_000_000.0 / 8.0 / 100.0) as usize;
    let chunk = vec![0x5a_u8; 16 * 1024];
    let mut tick = tokio::time::interval(Duration::from_millis(10));
    let end = Instant::now() + Duration::from_secs(o.secs);
    let mut owed = 0usize;
    while Instant::now() < end {
        tick.tick().await;
        owed += per_tick;
        while owed >= chunk.len() {
            a.send(Message::Binary(chunk.clone()))
                .await
                .map_err(|e| format!("relay send: {e}"))?;
            owed -= chunk.len();
        }
    }
    reader.await.ok();
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let o = match parse() {
        Ok(o) => Arc::new(o),
        Err(e) => {
            eprintln!("{e}");
            eprintln!("usage: hjv-load peers|relay <server> [--port 443] [--ca root.pem] [-n N] [--secs S] [--mbps R] [--key K]");
            return ExitCode::from(2);
        }
    };
    let run = unique(0) % 1_000_000;
    let started = Instant::now();
    let failed = Arc::new(AtomicUsize::new(0));
    let mut tasks = Vec::new();
    if o.cmd == "peers" {
        let beats = Arc::new(AtomicU64::new(0));
        for i in 0..o.n {
            let (o, beats, failed) = (o.clone(), beats.clone(), failed.clone());
            tasks.push(tokio::spawn(async move {
                if let Err(e) = peer(o, run, i, beats).await {
                    eprintln!("peer {i}: {e}");
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for t in tasks {
            t.await.ok();
        }
        let ok = o.n - failed.load(Ordering::Relaxed);
        println!(
            "peers: {ok}/{} registered and kept for {}s, {} heartbeats echoed",
            o.n,
            o.secs,
            beats.load(Ordering::Relaxed)
        );
    } else {
        let received = Arc::new(AtomicU64::new(0));
        for i in 0..o.n {
            let (o, received, failed) = (o.clone(), received.clone(), failed.clone());
            tasks.push(tokio::spawn(async move {
                if let Err(e) = session(o, run, i, received).await {
                    eprintln!("session {i}: {e}");
                    failed.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        for t in tasks {
            t.await.ok();
        }
        let bytes = received.load(Ordering::Relaxed) as f64;
        println!(
            "relay: {}/{} sessions, {:.1} MB through the relay in {:.1}s = {:.2} Mb/s total (asked {:.2})",
            o.n - failed.load(Ordering::Relaxed),
            o.n,
            bytes / 1e6,
            started.elapsed().as_secs_f64(),
            bytes * 8.0 / 1e6 / o.secs as f64,
            o.mbps * o.n as f64
        );
    }
    if failed.load(Ordering::Relaxed) > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
