// SPDX-License-Identifier: AGPL-3.0-only

//! HTTP/1.1 on the listening socket: the Unix socket systemd holds and passes in (socket
//! activation; deploy/systemd/hjviewer-api.socket), or for tests a TCP port on 127.0.0.1.

use crate::{
    api::{self, ApiRequest, BodyError, Reply, MAX_BODY},
    state::AppState,
};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::{Bytes, Incoming},
    header::{HeaderMap, CONTENT_LENGTH},
    server::conn::http1,
    service::service_fn,
    Method, Request, Response, StatusCode,
};
use hyper_util::{
    rt::{TokioIo, TokioTimer},
    server::graceful::GracefulShutdown,
};
use std::{
    convert::Infallible,
    future::Future,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::{TcpListener, UnixListener},
};

const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
const BODY_TIMEOUT: Duration = Duration::from_secs(10);
const DRAIN_LIMIT: usize = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(20);

pub enum Listener {
    Unix(UnixListener),
    Tcp(TcpListener),
}

/// The socket systemd passes in, if it does: exactly one listening Unix stream socket, fd 3.
pub fn systemd_socket() -> Result<Option<std::os::unix::net::UnixListener>, String> {
    use std::os::fd::FromRawFd;
    let (Ok(pid), Ok(fds)) = (std::env::var("LISTEN_PID"), std::env::var("LISTEN_FDS")) else {
        return Ok(None);
    };
    // children must not take the socket for theirs
    std::env::remove_var("LISTEN_PID");
    std::env::remove_var("LISTEN_FDS");
    std::env::remove_var("LISTEN_FDNAMES");
    if pid.parse::<u32>().ok() != Some(std::process::id()) {
        return Err("LISTEN_PID names another process".into());
    }
    if fds != "1" {
        return Err(format!(
            "systemd passed {fds} sockets; hjviewer-api.socket has one"
        ));
    }
    // SAFETY: with LISTEN_PID naming this process, systemd passed fd 3 to it and nothing else
    // in the process uses that fd.
    let listener = unsafe { std::os::unix::net::UnixListener::from_raw_fd(3) };
    listener
        .local_addr()
        .map_err(|_| "the socket from systemd is not a Unix socket".to_owned())?;
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("socket from systemd: {e}"))?;
    Ok(Some(listener))
}

/// Serves requests until `shutdown` completes, then waits a while for open ones.
pub async fn serve(state: Arc<AppState>, listener: Listener, shutdown: impl Future<Output = ()>) {
    let graceful = GracefulShutdown::new();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            accepted = accept(&listener) => match accepted {
                Ok((Connection::Unix(stream), _)) => spawn(&graceful, state.clone(), stream, None),
                Ok((Connection::Tcp(stream), peer)) => {
                    spawn(&graceful, state.clone(), stream, peer.map(|p| p.ip()))
                }
                Err(e) => {
                    log_err!("accept: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            },
        }
    }
    drop(listener);
    if tokio::time::timeout(SHUTDOWN_GRACE, graceful.shutdown())
        .await
        .is_err()
    {
        log_warn!("stopping with requests still open");
    }
}

enum Connection {
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
}

async fn accept(listener: &Listener) -> std::io::Result<(Connection, Option<SocketAddr>)> {
    match listener {
        Listener::Unix(l) => l.accept().await.map(|(s, _)| (Connection::Unix(s), None)),
        Listener::Tcp(l) => l
            .accept()
            .await
            .map(|(s, peer)| (Connection::Tcp(s), Some(peer))),
    }
}

fn spawn<S>(graceful: &GracefulShutdown, state: Arc<AppState>, stream: S, peer: Option<IpAddr>)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let service = service_fn(move |req| {
        let state = state.clone();
        async move { Ok::<_, Infallible>(handle(state, req, peer).await) }
    });
    let conn = http1::Builder::new()
        .timer(TokioTimer::new())
        .header_read_timeout(HEADER_TIMEOUT)
        .max_buf_size(MAX_BODY)
        .serve_connection(TokioIo::new(stream), service);
    let conn = graceful.watch(conn);
    tokio::spawn(async move {
        let _ = conn.await;
    });
}

async fn handle(
    state: Arc<AppState>,
    req: Request<Incoming>,
    peer: Option<IpAddr>,
) -> Response<Full<Bytes>> {
    let (parts, body) = req.into_parts();
    let head = parts.method == Method::HEAD;
    let declared = parts
        .headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let body = read_body(body, declared).await;
    let request = ApiRequest {
        ip: client_ip(&parts.headers, peer),
        method: parts.method,
        path: parts.uri.path().to_owned(),
        query: parts.uri.query().unwrap_or("").to_owned(),
        target: parts
            .uri
            .path_and_query()
            .map_or_else(|| parts.uri.path().to_owned(), |pq| pq.as_str().to_owned()),
        headers: parts.headers,
        body,
        retry: Default::default(),
    };
    let reply = match tokio::time::timeout(REQUEST_TIMEOUT, api::route(&state, request)).await {
        Ok(reply) => reply,
        Err(_) => {
            log_err!("a request took longer than {} s", REQUEST_TIMEOUT.as_secs());
            Reply::error(
                StatusCode::SERVICE_UNAVAILABLE,
                "HJViewer 서버를 지금 쓸 수 없습니다. 잠시 뒤 다시 시도해 주세요",
            )
        }
    };
    reply.into_response(head)
}

/// Reads the body. Up to 64 KiB is kept; a larger one is still read on, up to 1 MiB, and refused,
/// so that the client gets the answer rather than a reset connection. (nginx refuses bodies over
/// 64 KiB itself.)
async fn read_body(mut body: Incoming, declared: Option<u64>) -> Result<Bytes, BodyError> {
    if declared.is_some_and(|n| n > DRAIN_LIMIT as u64) {
        return Err(BodyError::TooLarge);
    }
    let read = async {
        let mut kept = Vec::new();
        let mut total = 0usize;
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|_| BodyError::Unreadable)?;
            if let Ok(data) = frame.into_data() {
                total += data.len();
                if total > DRAIN_LIMIT {
                    return Err(BodyError::TooLarge);
                }
                if total <= MAX_BODY {
                    kept.extend_from_slice(&data);
                }
            }
        }
        if total > MAX_BODY {
            Err(BodyError::TooLarge)
        } else {
            Ok(Bytes::from(kept))
        }
    };
    tokio::time::timeout(BODY_TIMEOUT, read)
        .await
        .unwrap_or(Err(BodyError::Unreadable))
}

/// The client's address: nginx always sets X-Real-IP to it (deploy/nginx-hjviewer-api.conf), and
/// only nginx and root can reach the socket.
fn client_ip(headers: &HeaderMap, peer: Option<IpAddr>) -> String {
    let canonical = |ip: IpAddr| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    };
    match headers.get("x-real-ip") {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|v| v.trim().parse::<IpAddr>().ok())
            .map_or_else(|| "invalid".to_owned(), |ip| canonical(ip).to_string()),
        None => peer.map_or_else(|| "unknown".to_owned(), |ip| canonical(ip).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::header::HeaderValue;

    #[test]
    fn client_addresses() {
        let mut h = HeaderMap::new();
        assert_eq!(client_ip(&h, None), "unknown");
        assert_eq!(
            client_ip(&h, Some("127.0.0.1".parse().unwrap())),
            "127.0.0.1"
        );
        h.insert("x-real-ip", HeaderValue::from_static("::ffff:10.1.2.3"));
        assert_eq!(
            client_ip(&h, Some("127.0.0.1".parse().unwrap())),
            "10.1.2.3"
        );
        h.insert("x-real-ip", HeaderValue::from_static("2001:db8::1"));
        assert_eq!(client_ip(&h, None), "2001:db8::1");
        h.insert("x-real-ip", HeaderValue::from_static("10.0.0.1, 10.0.0.2"));
        assert_eq!(client_ip(&h, None), "invalid");
    }
}
