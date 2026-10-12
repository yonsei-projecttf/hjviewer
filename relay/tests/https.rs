// SPDX-License-Identifier: AGPL-3.0-only

//! The HTTPS client against a peer that sends its TLS bytes one at a time, each well within any
//! socket timeout: the request must still end at its deadline (PROTOCOL.md: 35 s from connect to
//! the last byte), during the handshake and while reading the response. Only 127.0.0.1.

mod common;

use hjv_relay::{
    config::Server,
    https::{roots_from_pem, Client, HttpError},
};
use rustls::ServerConnection;
use std::{
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    thread,
    time::{Duration, Instant},
};

/// A socket that, once `slow` is set, writes one byte per call, `gap` apart.
struct Trickle {
    tcp: TcpStream,
    slow: bool,
    gap: Duration,
}

impl Read for Trickle {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.tcp.read(buf)
    }
}

impl Write for Trickle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.slow && !buf.is_empty() {
            thread::sleep(self.gap);
            self.tcp.write(&buf[..1])
        } else {
            self.tcp.write(buf)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

/// A one-request TLS server for "localhost" that trickles from the start (`from_handshake`) or
/// only its response; returns the port and its CA.
fn trickling_server(from_handshake: bool) -> (u16, String, thread::JoinHandle<()>) {
    let (ca_pem, config) = common::tls();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        let mut conn = ServerConnection::new(config).unwrap();
        let mut sock = Trickle {
            tcp,
            slow: from_handshake,
            gap: Duration::from_millis(50),
        };
        while conn.is_handshaking() {
            if conn.complete_io(&mut sock).is_err() {
                return;
            }
        }
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            if conn.complete_io(&mut sock).is_err() {
                return;
            }
            let n = conn.reader().read(&mut buf).unwrap_or(0);
            request.extend_from_slice(&buf[..n]);
        }
        let body = br#"{"server_time":1760000000,"state":"active"}"#;
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        conn.writer().write_all(head.as_bytes()).unwrap();
        conn.writer().write_all(body).unwrap();
        sock.slow = true;
        while conn.wants_write() {
            if conn.write_tls(&mut sock).is_err() {
                return;
            }
        }
    });
    (port, ca_pem, server)
}

fn post(port: u16, ca_pem: &str, timeout: Duration) -> (Result<u16, HttpError>, Duration) {
    let client = Client::new(
        Server::parse(&format!("localhost:{port}")).unwrap(),
        roots_from_pem(ca_pem.as_bytes()).unwrap(),
    )
    .unwrap();
    let started = Instant::now();
    let result = client
        .post_json("/api/hjv/relay/poll", b"{}", timeout)
        .map(|r| r.status);
    (result, started.elapsed())
}

#[test]
fn a_trickled_response_does_not_stretch_the_deadline() {
    let (port, ca, server) = trickling_server(false);
    let (result, took) = post(port, &ca, Duration::from_secs(1));
    assert!(matches!(result, Err(HttpError::Timeout)), "{result:?}");
    assert!(took < Duration::from_millis(2500), "{took:?}");
    server.join().unwrap();
}

#[test]
fn a_trickled_handshake_does_not_stretch_the_deadline() {
    let (port, ca, server) = trickling_server(true);
    let (result, took) = post(port, &ca, Duration::from_secs(1));
    assert!(matches!(result, Err(HttpError::Timeout)), "{result:?}");
    assert!(took < Duration::from_millis(2500), "{took:?}");
    server.join().unwrap();
}
