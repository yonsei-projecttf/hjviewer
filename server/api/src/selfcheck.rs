// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api selfcheck` (design v2 §10-2): from inside the service's sandbox, can hjv-api reach
//! what it needs? For each SSO host it resolves the name, opens TCP and, for https, completes a
//! TLS handshake with certificate checks; it sends no request, body or credential. It also asks
//! hbbs's console whether patch 0004 answers, and checks that HJV_API_HBBS_PUBKEY is set.
//!
//! The unit can run it after each start (`ExecStartPost=-… selfcheck --log-only`, commented out
//! in hjviewer-api.service until the operator agrees), and `hjv-api admin selfcheck` runs it
//! inside the running service.

use crate::{
    config::Config,
    hbbs::{self, HbbsError},
    sso::tls_config,
};
use hyper::Uri;
use rustls::pki_types::ServerName;
use std::{
    fmt::Write as _,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{net::TcpStream, time::timeout};

const STEP_TIMEOUT: Duration = Duration::from_secs(5);

/// Runs the checks; returns the report and whether all passed.
pub async fn run(config: &Config) -> (String, bool) {
    let mut report = String::new();
    let mut ok = true;
    let mut seen = Vec::new();
    for (name, uri) in [
        ("token URL", &config.sso.token_url),
        ("user-info URL", &config.sso.userinfo_url),
    ] {
        let key = (
            uri.scheme_str().map(str::to_owned),
            uri.host().map(str::to_owned),
            uri.port_u16(),
        );
        if seen.contains(&key) {
            let _ = writeln!(report, "[ OK ] {name}: same host as above");
            continue;
        }
        seen.push(key);
        match check_host(uri).await {
            Ok(detail) => {
                let _ = writeln!(report, "[ OK ] {name}: {detail}");
            }
            Err(e) => {
                ok = false;
                let _ = writeln!(report, "[FAIL] {name}: {e}");
            }
        }
    }
    if config.hbbs_pubkey.is_none() {
        ok = false;
        let _ = writeln!(
            report,
            "[FAIL] HJV_API_HBBS_PUBKEY is not set: logins work, no device is added"
        );
    }
    match hbbs::lookup(config.hbbs_console, config.hbbs_pubkey.as_ref(), "0").await {
        Ok(_) => {
            let _ = writeln!(report, "[ OK ] hbbs console: answers hjv-pk (patch 0004)");
        }
        Err(HbbsError::Unsupported) => {
            ok = false;
            let _ = writeln!(
                report,
                "[FAIL] hbbs console: no answer to hjv-pk; deploy hbbs with patch 0004 (logins work, no device is added)"
            );
        }
        Err(e) => {
            ok = false;
            let _ = writeln!(report, "[FAIL] hbbs console: {e}");
        }
    }
    (report, ok)
}

async fn check_host(uri: &Uri) -> Result<String, String> {
    let host = uri
        .host()
        .ok_or("no host")?
        .trim_start_matches('[')
        .trim_end_matches(']');
    let https = uri.scheme_str() == Some("https");
    let port = uri.port_u16().unwrap_or(if https { 443 } else { 80 });
    let started = Instant::now();
    let addrs: Vec<_> = timeout(STEP_TIMEOUT, tokio::net::lookup_host((host, port)))
        .await
        .map_err(|_| "name lookup timed out".to_owned())?
        .map_err(|e| format!("name lookup failed: {e}"))?
        .collect();
    let addr = *addrs.first().ok_or("the name has no address")?;
    let stream = timeout(STEP_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| format!("TCP to port {port} timed out (egress filter?)"))?
        .map_err(|e| format!("TCP to port {port}: {e}"))?;
    if !https {
        return Ok(format!(
            "{} addresses, TCP ok in {} ms (plain http: test setup)",
            addrs.len(),
            started.elapsed().as_millis()
        ));
    }
    let connector = tokio_rustls::TlsConnector::from(Arc::new(tls_config()?));
    let name =
        ServerName::try_from(host.to_owned()).map_err(|_| "not a valid TLS name".to_owned())?;
    let tls = timeout(STEP_TIMEOUT, connector.connect(name, stream))
        .await
        .map_err(|_| "TLS handshake timed out".to_owned())?
        .map_err(|e| format!("TLS handshake: {e}"))?;
    let version = tls
        .get_ref()
        .1
        .protocol_version()
        .map_or_else(|| "?".to_owned(), |v| format!("{v:?}"));
    Ok(format!(
        "{} addresses, TCP and TLS ({version}) ok in {} ms",
        addrs.len(),
        started.elapsed().as_millis()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reports_each_step() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                drop(stream);
            }
        });
        let plain: Uri = format!("http://{addr}/token").parse().unwrap();
        assert!(check_host(&plain).await.unwrap().contains("TCP ok"));
        // not a TLS server: the handshake fails, after rustls and the CA store were set up
        let tls: Uri = format!("https://{addr}/token").parse().unwrap();
        let err = check_host(&tls).await.unwrap_err();
        assert!(err.starts_with("TLS handshake"), "{err}");
        // bound and never listening: refused, and no other test gets the port meanwhile
        let held = tokio::net::TcpSocket::new_v4().unwrap();
        held.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let port = held.local_addr().unwrap().port();
        let closed: Uri = format!("http://127.0.0.1:{port}/token").parse().unwrap();
        let err = check_host(&closed).await.unwrap_err();
        assert!(err.contains(&format!("TCP to port {port}")), "{err}");
    }
}
