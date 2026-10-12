// SPDX-License-Identifier: AGPL-3.0-only

//! A small HTTPS/1.1 client for the relay's two POST requests. rustls with the ring provider and
//! Mozilla's root certificates compiled in, so the relay does not depend on the NAS's certificate
//! store; certificates are always verified (there is no insecure fallback), against the NAS's
//! clock. One connection per request, an overall deadline that every socket read and write under
//! rustls is held to (a peer that sends a byte at a time cannot stretch it), and size limits on
//! what the server sends back. DNS is the C library's resolver (musl: /etc/resolv.conf and
//! /etc/hosts); proxies are not supported.

use crate::config::{Server, VERSION};
use rustls::{pki_types::ServerName, ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use std::{
    fmt,
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::Arc,
    time::{Duration, Instant},
};

pub const MAX_RESPONSE_BODY: usize = 256 * 1024;
const MAX_HEAD: usize = 16 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The trust anchors compiled into the relay (webpki-roots, from Mozilla's list).
pub fn builtin_roots() -> RootCertStore {
    RootCertStore {
        roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
    }
}

/// The certificates of a PEM file as the only trust anchors (`--ca`, for tests).
pub fn roots_from_pem(pem: &[u8]) -> Result<RootCertStore, String> {
    let mut store = RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(pem) {
        let cert = cert.map_err(|e| format!("bad PEM: {e}"))?;
        store
            .add(cert)
            .map_err(|e| format!("bad certificate: {e}"))?;
    }
    if store.is_empty() {
        return Err("no certificates in the file".to_owned());
    }
    Ok(store)
}

#[derive(Debug)]
pub enum HttpError {
    Resolve(io::Error),
    Connect(io::Error),
    Tls(String),
    Io(io::Error),
    Timeout,
    Protocol(String),
    TooLarge,
}

impl fmt::Display for HttpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HttpError::Resolve(e) => write!(f, "DNS: {e}"),
            HttpError::Connect(e) => write!(f, "connect: {e}"),
            HttpError::Tls(e) => write!(f, "TLS: {e}"),
            HttpError::Io(e) => write!(f, "I/O: {e}"),
            HttpError::Timeout => f.write_str("timed out"),
            HttpError::Protocol(e) => write!(f, "HTTP: {e}"),
            HttpError::TooLarge => f.write_str("response too large"),
        }
    }
}

fn io_error(e: io::Error) -> HttpError {
    match e.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HttpError::Timeout,
        io::ErrorKind::InvalidData => {
            match e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) {
                Some(tls) => HttpError::Tls(tls_text(tls)),
                None => HttpError::Io(e),
            }
        }
        _ => HttpError::Io(e),
    }
}

/// The TLS error, with a hint when the certificate looked expired or not yet valid: rustls
/// checks it against this machine's clock, which on a NAS after a power cut may be far off. A
/// certificate for another name is described without the names it holds, which whoever
/// presented it chose.
fn tls_text(e: &rustls::Error) -> String {
    use rustls::{CertificateError as C, Error};
    match e {
        Error::InvalidCertificate(
            C::Expired | C::ExpiredContext { .. } | C::NotValidYet | C::NotValidYetContext { .. },
        ) => format!("{e} (NAS의 시계가 맞는지 확인하세요)"),
        Error::InvalidCertificate(C::NotValidForName | C::NotValidForNameContext { .. }) => {
            "invalid peer certificate: not valid for the server's name".to_owned()
        }
        _ => e.to_string(),
    }
}

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

pub struct Client {
    server: Server,
    config: Arc<ClientConfig>,
    name: ServerName<'static>,
    user_agent: String,
}

impl Client {
    pub fn new(server: Server, roots: RootCertStore) -> Result<Client, String> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = ServerName::try_from(server.host.clone()).map_err(|e| e.to_string())?;
        Ok(Client {
            server,
            config: Arc::new(config),
            name,
            user_agent: format!("hjv-relay/{VERSION}"),
        })
    }

    pub fn server(&self) -> &Server {
        &self.server
    }

    /// POSTs a JSON body and returns the response, whatever its status.
    pub fn post_json(
        &self,
        path: &str,
        body: &[u8],
        timeout: Duration,
    ) -> Result<Response, HttpError> {
        let deadline = Instant::now() + timeout;
        let tcp = self.connect(deadline)?;
        let conn = ClientConnection::new(self.config.clone(), self.name.clone())
            .map_err(|e| HttpError::Tls(e.to_string()))?;
        let mut stream = Conn {
            tls: StreamOwned::new(conn, DeadlineSock { tcp, deadline }),
        };
        let mut request = format!(
            "POST {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}\r\nContent-Type: application/json\r\n\
             Accept: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.server.authority(),
            self.user_agent,
            body.len()
        )
        .into_bytes();
        request.extend_from_slice(body);
        stream.write_all(&request).map_err(io_error)?;
        stream.flush().map_err(io_error)?;
        let response = read_response(&mut stream, MAX_RESPONSE_BODY)?;
        stream.tls.conn.send_close_notify();
        let _ = stream.tls.flush();
        Ok(response)
    }

    fn connect(&self, deadline: Instant) -> Result<TcpStream, HttpError> {
        let addrs: Vec<SocketAddr> = (self.server.host.as_str(), self.server.port)
            .to_socket_addrs()
            .map_err(HttpError::Resolve)?
            .collect();
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
        for addr in addrs {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(HttpError::Timeout);
            }
            match TcpStream::connect_timeout(&addr, left.min(CONNECT_TIMEOUT)) {
                Ok(s) => {
                    let _ = s.set_nodelay(true);
                    return Ok(s);
                }
                Err(e) => last = e,
            }
        }
        Err(match last.kind() {
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => HttpError::Timeout,
            _ => HttpError::Connect(last),
        })
    }
}

/// The TCP socket under rustls. rustls loops over socket reads and writes within one call (the
/// handshake, a record that arrives in pieces), so the deadline is enforced here, on every one
/// of them: each gets only the time left, and none starts after the deadline.
struct DeadlineSock {
    tcp: TcpStream,
    deadline: Instant,
}

impl DeadlineSock {
    fn left(&self) -> io::Result<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            Err(io::ErrorKind::TimedOut.into())
        } else {
            Ok(left)
        }
    }
}

impl Read for DeadlineSock {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.left()?;
        self.tcp.set_read_timeout(Some(left))?;
        self.tcp.read(buf)
    }
}

impl Write for DeadlineSock {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let left = self.left()?;
        self.tcp.set_write_timeout(Some(left))?;
        self.tcp.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}

/// The TLS stream of one request.
struct Conn {
    tls: StreamOwned<ClientConnection, DeadlineSock>,
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.tls.read(buf) {
            // a peer that closes without close_notify: the framing below decides if that is ok
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(0),
            r => r,
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.tls.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tls.flush()
    }
}

fn read_more<R: Read>(r: &mut R, buf: &mut Vec<u8>) -> Result<usize, HttpError> {
    let mut chunk = [0u8; 8192];
    loop {
        match r.read(&mut chunk) {
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                return Ok(n);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(io_error(e)),
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Reads one HTTP/1.x response: skips 1xx responses, then the body framed by chunked transfer
/// coding, Content-Length, or the end of the connection.
pub fn read_response<R: Read>(r: &mut R, max_body: usize) -> Result<Response, HttpError> {
    let mut buf = Vec::with_capacity(8192);
    loop {
        let head_end = loop {
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i + 4;
            }
            if buf.len() > MAX_HEAD {
                return Err(HttpError::Protocol("response head too large".to_owned()));
            }
            if read_more(r, &mut buf)? == 0 {
                return Err(HttpError::Protocol(
                    "connection closed before a response".to_owned(),
                ));
            }
        };
        let head = std::str::from_utf8(&buf[..head_end])
            .map_err(|_| HttpError::Protocol("response head is not text".to_owned()))?;
        let (status, headers) = parse_head(head)?;
        buf.drain(..head_end);
        if (100..200).contains(&status) {
            if status == 101 {
                return Err(HttpError::Protocol("unexpected 101 response".to_owned()));
            }
            continue;
        }
        let body = read_body(r, buf, &headers, max_body)?;
        return Ok(Response { status, body });
    }
}

type Headers = Vec<(String, String)>;

fn parse_head(head: &str) -> Result<(u16, Headers), HttpError> {
    let bad = |what: &str| HttpError::Protocol(what.to_owned());
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let mut parts = status_line.splitn(3, ' ');
    if !parts.next().unwrap_or_default().starts_with("HTTP/1.") {
        return Err(bad("not an HTTP/1.x response"));
    }
    let code = parts.next().unwrap_or_default();
    let status: u16 = (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()))
        .then(|| code.parse().ok())
        .flatten()
        .ok_or_else(|| bad("bad status code"))?;
    let mut headers = Vec::new();
    for line in lines.filter(|l| !l.is_empty()) {
        let (name, value) = line.split_once(':').ok_or_else(|| bad("bad header line"))?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    Ok((status, headers))
}

fn read_body<R: Read>(
    r: &mut R,
    mut buf: Vec<u8>,
    headers: &Headers,
    max: usize,
) -> Result<Vec<u8>, HttpError> {
    let values = |name: &str| -> Vec<&str> {
        headers
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
            .collect()
    };
    let codings = values("transfer-encoding");
    if !codings.is_empty() {
        let last = codings
            .last()
            .and_then(|v| v.rsplit(',').next())
            .map(|v| v.trim().to_ascii_lowercase());
        return match last.as_deref() {
            Some("chunked") => read_chunked(r, buf, max),
            _ => Err(HttpError::Protocol(
                "unsupported transfer coding".to_owned(),
            )),
        };
    }
    let lengths = values("content-length");
    if let Some(first) = lengths.first() {
        if lengths.iter().any(|l| l != first)
            || first.is_empty()
            || !first.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(HttpError::Protocol("bad Content-Length".to_owned()));
        }
        let n: usize = first.parse().map_err(|_| HttpError::TooLarge)?;
        if n > max {
            return Err(HttpError::TooLarge);
        }
        while buf.len() < n {
            if read_more(r, &mut buf)? == 0 {
                return Err(HttpError::Protocol("response body cut short".to_owned()));
            }
        }
        buf.truncate(n);
        return Ok(buf);
    }
    loop {
        if buf.len() > max {
            return Err(HttpError::TooLarge);
        }
        if read_more(r, &mut buf)? == 0 {
            return Ok(buf);
        }
    }
}

fn read_chunked<R: Read>(r: &mut R, mut buf: Vec<u8>, max: usize) -> Result<Vec<u8>, HttpError> {
    let mut body = Vec::new();
    let line = |r: &mut R, buf: &mut Vec<u8>| -> Result<String, HttpError> {
        loop {
            if let Some(i) = find(buf, b"\r\n") {
                let text = String::from_utf8(buf[..i].to_vec())
                    .map_err(|_| HttpError::Protocol("bad chunk line".to_owned()))?;
                buf.drain(..i + 2);
                return Ok(text);
            }
            if buf.len() > 4096 {
                return Err(HttpError::Protocol("chunk line too long".to_owned()));
            }
            if read_more(r, buf)? == 0 {
                return Err(HttpError::Protocol("chunked body cut short".to_owned()));
            }
        }
    };
    loop {
        let size_line = line(r, &mut buf)?;
        let size = size_line.split(';').next().unwrap_or_default().trim();
        if size.is_empty() || size.len() > 8 || !size.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(HttpError::Protocol("bad chunk size".to_owned()));
        }
        let size = usize::from_str_radix(size, 16).map_err(|_| HttpError::TooLarge)?;
        if size == 0 {
            // trailer fields, up to the empty line
            while !line(r, &mut buf)?.is_empty() {}
            return Ok(body);
        }
        if body.len() + size > max {
            return Err(HttpError::TooLarge);
        }
        while buf.len() < size + 2 {
            if read_more(r, &mut buf)? == 0 {
                return Err(HttpError::Protocol("chunked body cut short".to_owned()));
            }
        }
        if &buf[size..size + 2] != b"\r\n" {
            return Err(HttpError::Protocol("chunk not followed by CRLF".to_owned()));
        }
        body.extend_from_slice(&buf[..size]);
        buf.drain(..size + 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hands out the bytes a few at a time, like a socket.
    struct Trickle<'a>(&'a [u8]);

    impl Read for Trickle<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = self.0.len().min(buf.len()).min(7);
            buf[..n].copy_from_slice(&self.0[..n]);
            self.0 = &self.0[n..];
            Ok(n)
        }
    }

    fn parse(raw: &str) -> Result<Response, HttpError> {
        read_response(&mut Trickle(raw.as_bytes()), 64)
    }

    #[test]
    fn framings() {
        let r = parse("HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello and more").unwrap();
        assert_eq!((r.status, r.body.as_slice()), (200, &b"hello"[..]));
        let r =
            parse("HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\n\r\n{\"error\":1}")
                .unwrap();
        assert_eq!((r.status, r.body.as_slice()), (404, &b"{\"error\":1}"[..]));
        let r = parse(
            "HTTP/1.1 100 Continue\r\n\r\nHTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n\
             4\r\nWiki\r\n6;ext=1\r\npedia \r\nE\r\nin \r\n\r\nchunks.\r\n0\r\nX-Trailer: 1\r\n\r\n",
        )
        .unwrap();
        assert_eq!(r.body, b"Wikipedia in \r\n\r\nchunks.");
        let r = parse("HTTP/1.0 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n").unwrap();
        assert_eq!((r.status, r.body.len()), (503, 0));
        let r =
            parse("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 2\r\n\r\nok").unwrap();
        assert_eq!(r.body, b"ok");
    }

    #[test]
    fn malformed_responses() {
        for raw in [
            "",
            "HTTP/1.1 200 OK\r\n",
            "SSH-2.0-OpenSSH\r\n\r\n",
            "HTTP/2 200\r\n\r\n",
            "HTTP/1.1 2000 OK\r\n\r\n",
            "HTTP/1.1 200 OK\r\nno colon\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
            "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\nab",
            "HTTP/1.1 200 OK\r\nContent-Length: -1\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWikiXX0\r\n\r\n",
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWi",
            "HTTP/1.1 101 Switching Protocols\r\n\r\n",
        ] {
            assert!(matches!(parse(raw), Err(HttpError::Protocol(_))), "{raw:?}");
        }
        let big = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: 65\r\n\r\n{}",
            "x".repeat(65)
        );
        assert!(matches!(parse(&big), Err(HttpError::TooLarge)));
        let big = format!("HTTP/1.1 200 OK\r\n\r\n{}", "x".repeat(100));
        assert!(matches!(parse(&big), Err(HttpError::TooLarge)));
        let big = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n41\r\n{}\r\n0\r\n\r\n",
            "x".repeat(65)
        );
        assert!(matches!(parse(&big), Err(HttpError::TooLarge)));
        let head = format!("HTTP/1.1 200 OK\r\nX: {}\r\n\r\n", "x".repeat(MAX_HEAD));
        assert!(matches!(parse(&head), Err(HttpError::Protocol(_))));
    }

    #[test]
    fn certificate_time_errors_point_at_the_clock() {
        use rustls::{CertificateError, Error};
        for e in [CertificateError::Expired, CertificateError::NotValidYet] {
            assert!(tls_text(&Error::InvalidCertificate(e)).contains("시계"));
        }
        let other = tls_text(&Error::InvalidCertificate(CertificateError::UnknownIssuer));
        assert!(!other.contains("시계"), "{other}");
        // the names a certificate for another name holds are not repeated
        let name = tls_text(&Error::InvalidCertificate(
            CertificateError::NotValidForNameContext {
                expected: ServerName::try_from("rd.example.com").unwrap(),
                presented: vec!["aaaa-bbbb-cccc-dddd.attacker.example".to_owned()],
            },
        ));
        assert!(!name.contains("aaaa"), "{name}");
        assert!(name.contains("not valid for the server's name"), "{name}");
    }

    #[test]
    fn pem_roots() {
        assert!(roots_from_pem(b"").is_err());
        assert!(
            roots_from_pem(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n")
                .is_err()
        );
        assert!(builtin_roots().len() > 100);
    }
}
