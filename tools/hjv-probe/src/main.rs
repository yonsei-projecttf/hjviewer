// SPDX-License-Identifier: AGPL-3.0-only

//! Checks, from the network a client is on, that the hjviewer server is reachable the way the
//! apps reach it: TLS on 443, WebSocket upgrades on /ws/id (hbbs) and /ws/relay (hbbr), and a
//! rendezvous round trip that only the patched hbbs answers over WebSocket.
//!
//!     hjv-probe <server> [--port 443] [--ca root.pem] [--peer <device id>]...

use futures_util::{SinkExt, StreamExt};
use std::{
    process::ExitCode,
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{tungstenite::Message, Connector};

const STEP_TIMEOUT: Duration = Duration::from_secs(10);

struct Args {
    host: String,
    port: u16,
    ca: Option<String>,
    peers: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = std::env::args().skip(1);
    let mut host = None;
    let mut port = 443;
    let mut ca = None;
    let mut peers = Vec::new();
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--port" => {
                port = value("--port")?
                    .parse()
                    .map_err(|_| "--port must be a number".to_owned())?
            }
            "--ca" => ca = Some(value("--ca")?),
            "--peer" => peers.push(value("--peer")?),
            "-h" | "--help" => return Err(String::new()),
            _ if host.is_none() && !arg.starts_with('-') => host = Some(arg),
            _ => return Err(format!("unexpected argument: {arg}")),
        }
    }
    let host = host.ok_or("missing <server>")?;
    Ok(Args {
        host,
        port,
        ca,
        peers,
    })
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

fn report(name: &str, started: Instant, res: Result<String, String>, hint: &str) -> bool {
    let ms = started.elapsed().as_millis();
    match res {
        Ok(detail) => {
            println!("[ OK ] {name} ({ms} ms) {detail}");
            true
        }
        Err(err) => {
            println!("[FAIL] {name} ({ms} ms) {err}");
            println!("       -> {hint}");
            false
        }
    }
}

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;

async fn open_ws(args: &Args, path: &str) -> Result<Ws, String> {
    let url = format!("wss://{}:{}{path}", args.host, args.port);
    let connector = connector(args.ca.as_deref())?;
    let connect = tokio_tungstenite::connect_async_tls_with_config(url, None, false, Some(connector));
    match timeout(STEP_TIMEOUT, connect).await {
        Err(_) => Err("timed out".to_owned()),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok((ws, _))) => Ok(ws),
    }
}

/// Asks hbbs for the online state of `peers`; returns one bool per peer.
async fn online_states(ws: &mut Ws, peers: &[String]) -> Result<Vec<bool>, String> {
    let mut request = Vec::new();
    put_bytes(&mut request, 1, b"hjv-probe");
    for peer in peers {
        put_bytes(&mut request, 2, peer.as_bytes());
    }
    let mut msg = Vec::new();
    put_bytes(&mut msg, RENDEZVOUS_ONLINE_REQUEST, &request);
    ws.send(Message::Binary(msg))
        .await
        .map_err(|e| e.to_string())?;
    loop {
        let frame = match timeout(STEP_TIMEOUT, ws.next()).await {
            Err(_) => return Err("no answer".to_owned()),
            Ok(None) => return Err("connection closed without an answer".to_owned()),
            Ok(Some(Err(e))) => return Err(e.to_string()),
            Ok(Some(Ok(frame))) => frame,
        };
        let Message::Binary(bytes) = frame else {
            continue;
        };
        let response = find_bytes(&bytes, RENDEZVOUS_ONLINE_RESPONSE)
            .ok_or("unexpected answer (not an OnlineResponse)")?;
        let states = find_bytes(response, 1).unwrap_or_default();
        return Ok((0..peers.len())
            .map(|i| {
                states
                    .get(i / 8)
                    .map_or(false, |b| b & (0x80 >> (i % 8)) != 0)
            })
            .collect());
    }
}

// RendezvousMessage fields (hbb_common protos/rendezvous.proto)
const RENDEZVOUS_ONLINE_REQUEST: u32 = 23;
const RENDEZVOUS_ONLINE_RESPONSE: u32 = 24;

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

/// The length-delimited field `field` of a protobuf message.
fn find_bytes(buf: &[u8], field: u32) -> Option<&[u8]> {
    let mut pos = 0;
    while pos < buf.len() {
        let tag = get_varint(buf, &mut pos)?;
        match tag & 7 {
            0 => {
                get_varint(buf, &mut pos)?;
            }
            1 => pos += 8,
            2 => {
                let len = usize::try_from(get_varint(buf, &mut pos)?).ok()?;
                let data = buf.get(pos..pos.checked_add(len)?)?;
                if tag >> 3 == u64::from(field) {
                    return Some(data);
                }
                pos += len;
            }
            5 => pos += 4,
            _ => return None,
        }
    }
    None
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(err) => {
            if !err.is_empty() {
                eprintln!("{err}");
            }
            eprintln!("usage: hjv-probe <server> [--port 443] [--ca root.pem] [--peer <device id>]...");
            return ExitCode::from(2);
        }
    };
    println!("hjv-probe: {}:{}", args.host, args.port);
    let mut ok = true;

    let started = Instant::now();
    let tcp = timeout(STEP_TIMEOUT, TcpStream::connect((args.host.as_str(), args.port))).await;
    let res = match tcp {
        Err(_) => Err("timed out".to_owned()),
        Ok(Err(e)) => Err(e.to_string()),
        Ok(Ok(s)) => Ok(s.peer_addr().map(|a| a.to_string()).unwrap_or_default()),
    };
    if !report(
        "TCP connect",
        started,
        res,
        "DNS is wrong or this network blocks outgoing connections to the server; nothing else can work",
    ) {
        return ExitCode::FAILURE;
    }

    let started = Instant::now();
    let ws = open_ws(&args, "/ws/id").await;
    let mut ws_id = match ws {
        Ok(ws) => {
            report("TLS + WebSocket /ws/id", started, Ok(String::new()), "");
            Some(ws)
        }
        Err(err) => {
            let hint = if err.contains("certificate") || err.contains("tls") || err.contains("TLS") {
                "TLS failed: a TLS-inspecting proxy replaces the server certificate (ask IT to exempt the domain), or the certificate is not issued yet"
            } else {
                "the WebSocket upgrade was refused: a proxy strips Upgrade headers, or the reverse proxy does not route /ws/id to hbbs"
            };
            report("TLS + WebSocket /ws/id", started, Err(err), hint);
            ok = false;
            None
        }
    };

    if let Some(ws) = ws_id.as_mut() {
        let started = Instant::now();
        let res = online_states(ws, &args.peers).await.map(|states| {
            args.peers
                .iter()
                .zip(states)
                .map(|(peer, online)| format!("{peer}={}", if online { "online" } else { "offline" }))
                .collect::<Vec<_>>()
                .join(" ")
        });
        ok &= report(
            "rendezvous round trip (hbbs)",
            started,
            res,
            "hbbs did not answer over WebSocket: it is down, or it is not hjviewer's patched hbbs (upstream hbbs ignores WebSocket clients)",
        );
        ws.close(None).await.ok();
    }

    let started = Instant::now();
    let res = open_ws(&args, "/ws/relay").await.map(|_| String::new());
    ok &= report(
        "TLS + WebSocket /ws/relay (hbbr)",
        started,
        res,
        "the relay fallback is unreachable: check that the reverse proxy routes /ws/relay to hbbr",
    );

    if ok {
        println!("all checks passed: apps on this network can reach the server over 443");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
