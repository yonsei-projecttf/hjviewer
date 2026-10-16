// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api selfcheck` (design v2 §10-2): from inside the service's sandbox, can hjv-api reach
//! what it needs? For each SSO host it resolves the name, opens TCP and, for https, completes a
//! TLS handshake with certificate checks; it sends no request, body or credential. It also asks
//! hbbs's console whether patch 0004 answers, and checks that HJV_API_HBBS_PUBKEY is set.
//!
//! Since design v3 (§5-3) it also prints the SHA-256 fingerprints of the list keys the apps pin
//! (HJV_API_TRUST_PUBKEYS, the apps' HJV_TRUST_PUBKEYS), to compare with the fingerprints in the
//! summary of the app build, and checks that the list key in use (the credential
//! hjviewer-api-list-key, number HJV_API_LIST_KID) is the pinned key of that number: a wrong pin
//! would have every PC refuse every trust list, and the release build is to catch it first.
//!
//! The same for the command key that signs the wake commands (phase 2, relay/PROTOCOL.md §3.2):
//! the fingerprints `<kid>:<hex SHA-256>` of the command keys the relays pin
//! (HJV_API_COMMAND_PUBKEYS, the relays' HJV_COMMAND_PUBKEY), to compare with the summary of the
//! relay build and with `hjv-relay version`, and whether the key in use (the credential
//! hjviewer-api-command-key, number HJV_API_COMMAND_KID) is the pinned key of its number: a wrong
//! pin would have every relay refuse every command. Without the credential and the setting, Wake-
//! on-LAN is simply off.
//!
//! The same for the ticket key that signs the access tickets (docs/access-tickets.md 2-5):
//! the fingerprints of the keys hbbs takes (HJV_API_TICKET_PUBKEYS, the `KEY=` lines of its
//! access.conf), and whether the key in use (the credential hjviewer-api-ticket-key, number
//! HJV_API_TICKET_KID) is the one of its number there: otherwise hbbs refuses every ticket, and
//! with the access check on nobody connects. A public test key among them fails too (ticket.rs
//! `public_test_key`): anyone could sign the tickets hbbs takes. Without the credential and the
//! setting, no ticket is signed.
//!
//! It also says who may log in (HJV_API_LOGIN, the open login of 2026-10-14): `open` or `list`.
//!
//! The unit can run it after each start (`ExecStartPost=-… selfcheck --log-only`, commented out
//! in hjviewer-api.service until the operator agrees), and `hjv-api admin selfcheck` runs it
//! inside the running service, with the list key and the command key the service holds.

use crate::{
    config::Config,
    hbbs::{self, HbbsError},
    sso::tls_config,
    ticket, trust,
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

/// The list key the check compares with the pinned ones.
pub enum ListKeyCheck {
    /// the key in use, with its number
    Held { kid: u32, public_key: [u8; 32] },
    /// it could not be read here: why
    NotRead(String),
    /// no credentials here (a shell outside the unit): not compared
    Unavailable,
}

/// The command key the check compares with the pinned ones.
pub enum CommandKeyCheck {
    /// the key in use, with its number
    Held { kid: u32, public_key: [u8; 32] },
    /// the credentials hold none
    Absent,
    /// it could not be read here: why
    NotRead(String),
    /// no credentials here (a shell outside the unit): not compared
    Unavailable,
}

/// The ticket key the check compares with the keys hbbs takes.
pub enum TicketKeyCheck {
    /// the key in use, with its number
    Held { kid: u32, public_key: [u8; 32] },
    /// the credentials hold none
    Absent,
    /// it could not be read here: why
    NotRead(String),
    /// no credentials here (a shell outside the unit): not compared
    Unavailable,
}

/// The ticket key lines of the report; false when one failed.
fn ticket_keys(config: &Config, held: &TicketKeyCheck, report: &mut String) -> bool {
    let pinned = &config.ticket_pubkeys;
    if !pinned.is_empty() {
        let fingerprints: Vec<String> = pinned
            .iter()
            .map(|(kid, key)| trust::fingerprint(*kid, key))
            .collect();
        let _ = writeln!(
            report,
            "[ OK ] ticket keys hbbs takes (HJV_API_TICKET_PUBKEYS, its access.conf's KEY= lines), SHA-256: {}",
            fingerprints.join(", ")
        );
    }
    let fail = |report: &mut String, line: String| {
        let _ = writeln!(report, "[FAIL] {line}");
        false
    };
    let test_keys = ticket::test_keys_pinned(config);
    for kid in &test_keys {
        fail(report, format!(
            "ticket key {kid} of HJV_API_TICKET_PUBKEYS is a public test key, whose private key is in the repository: with it in hbbs's access.conf anyone can sign tickets"
        ));
    }
    let held_ok = match held {
        TicketKeyCheck::Held { kid, public_key } => {
            let mine = trust::fingerprint(*kid, public_key);
            match config.pinned_ticket_key(*kid) {
                Some(p) if p == *public_key => {
                    let _ = writeln!(
                        report,
                        "[ OK ] ticket key in use (HJV_API_TICKET_KID {kid}): the key {kid} hbbs takes, {mine}"
                    );
                    true
                }
                Some(_) => fail(report, format!(
                    "ticket key in use (HJV_API_TICKET_KID {kid}, {mine}) is not the key {kid} of HJV_API_TICKET_PUBKEYS: hbbs refuses every ticket it signs"
                )),
                None if pinned.is_empty() => fail(report, format!(
                    "HJV_API_TICKET_PUBKEYS is not set: the ticket key in use ({mine}) cannot be compared with the KEY= lines of hbbs's access.conf"
                )),
                None => fail(report, format!(
                    "ticket key in use (HJV_API_TICKET_KID {kid}, {mine}): no key of HJV_API_TICKET_PUBKEYS has the number {kid}"
                )),
            }
        }
        TicketKeyCheck::Absent if pinned.is_empty() => {
            let _ = writeln!(
                report,
                "[ -- ] ticket key: none (no credential hjviewer-api-ticket-key): no access ticket is signed"
            );
            true
        }
        TicketKeyCheck::Absent => fail(
            report,
            "ticket key: no credential hjviewer-api-ticket-key, while HJV_API_TICKET_PUBKEYS names hbbs's keys: no access ticket is signed".to_owned(),
        ),
        TicketKeyCheck::NotRead(e) => fail(report, format!("ticket key in use: {e}")),
        TicketKeyCheck::Unavailable => {
            let _ = writeln!(
                report,
                "[ -- ] ticket key in use: not read here (no credentials directory: give --credentials <dir>, or run hjv-api admin selfcheck)"
            );
            true
        }
    };
    held_ok && test_keys.is_empty()
}

/// The command key lines of the report; false when one failed.
fn command_keys(config: &Config, held: &CommandKeyCheck, report: &mut String) -> bool {
    let pinned = &config.command_pubkeys;
    if !pinned.is_empty() {
        let fingerprints: Vec<String> = pinned
            .iter()
            .map(|(kid, key)| trust::fingerprint(*kid, key))
            .collect();
        let _ = writeln!(
            report,
            "[ OK ] command keys the relays pin (HJV_API_COMMAND_PUBKEYS), SHA-256: {}",
            fingerprints.join(", ")
        );
    }
    let fail = |report: &mut String, line: String| {
        let _ = writeln!(report, "[FAIL] {line}");
        false
    };
    match held {
        CommandKeyCheck::Held { kid, public_key } => {
            let mine = trust::fingerprint(*kid, public_key);
            match config.pinned_command_key(*kid) {
                Some(p) if p == *public_key => {
                    let _ = writeln!(
                        report,
                        "[ OK ] command key in use (HJV_API_COMMAND_KID {kid}): the pinned key {kid}, {mine}"
                    );
                    true
                }
                Some(_) => fail(report, format!(
                    "command key in use (HJV_API_COMMAND_KID {kid}, {mine}) is not the pinned key {kid}: the relays refuse every command it signs"
                )),
                None if pinned.is_empty() => fail(report, format!(
                    "HJV_API_COMMAND_PUBKEYS is not set: the command key in use ({mine}) cannot be compared with the keys the relays pin (their HJV_COMMAND_PUBKEY)"
                )),
                None => fail(report, format!(
                    "command key in use (HJV_API_COMMAND_KID {kid}, {mine}): no pinned key has the number {kid}"
                )),
            }
        }
        CommandKeyCheck::Absent if pinned.is_empty() => {
            let _ = writeln!(
                report,
                "[ -- ] command key: none (no credential hjviewer-api-command-key): no PC is woken"
            );
            true
        }
        CommandKeyCheck::Absent => fail(
            report,
            "command key: no credential hjviewer-api-command-key, while HJV_API_COMMAND_PUBKEYS names the relays' keys: no PC can be woken".to_owned(),
        ),
        CommandKeyCheck::NotRead(e) => fail(report, format!("command key in use: {e}")),
        CommandKeyCheck::Unavailable => {
            let _ = writeln!(
                report,
                "[ -- ] command key in use: not read here (no credentials directory: give --credentials <dir>, or run hjv-api admin selfcheck)"
            );
            true
        }
    }
}

/// The list key lines of the report; false when one failed.
fn list_keys(config: &Config, held: &ListKeyCheck, report: &mut String) -> bool {
    let mut ok = true;
    if config.trust_pubkeys.is_empty() {
        let _ = writeln!(
            report,
            "[FAIL] HJV_API_TRUST_PUBKEYS is not set: the list key cannot be compared with the keys the apps pin (their HJV_TRUST_PUBKEYS)"
        );
        ok = false;
    } else {
        let fingerprints: Vec<String> = config
            .trust_pubkeys
            .iter()
            .map(|(kid, key)| trust::fingerprint(*kid, key))
            .collect();
        let _ = writeln!(
            report,
            "[ OK ] list keys the apps pin (HJV_API_TRUST_PUBKEYS), SHA-256: {}",
            fingerprints.join(", ")
        );
    }
    match held {
        ListKeyCheck::Held { kid, public_key } => {
            let mine = trust::fingerprint(*kid, public_key);
            match config.pinned(*kid) {
                Some(pinned) if pinned == *public_key => {
                    let _ = writeln!(
                        report,
                        "[ OK ] list key in use (HJV_API_LIST_KID {kid}): the pinned key {kid}, {mine}"
                    );
                }
                Some(_) => {
                    ok = false;
                    let _ = writeln!(
                        report,
                        "[FAIL] list key in use (HJV_API_LIST_KID {kid}, {mine}) is not the pinned key {kid}: the apps refuse every list it signs"
                    );
                }
                None if config.trust_pubkeys.is_empty() => {
                    let _ = writeln!(
                        report,
                        "[ -- ] list key in use (HJV_API_LIST_KID {kid}): {mine}"
                    );
                }
                None => {
                    ok = false;
                    let _ = writeln!(
                        report,
                        "[FAIL] list key in use (HJV_API_LIST_KID {kid}, {mine}): no pinned key has the number {kid}"
                    );
                }
            }
        }
        ListKeyCheck::NotRead(e) => {
            ok = false;
            let _ = writeln!(report, "[FAIL] list key in use: {e}");
        }
        ListKeyCheck::Unavailable => {
            let _ = writeln!(
                report,
                "[ -- ] list key in use: not read here (no credentials directory: give --credentials <dir>, or run hjv-api admin selfcheck, which checks the key the service holds)"
            );
        }
    }
    ok
}

/// Runs the checks; returns the report and whether all passed.
pub async fn run(
    config: &Config,
    list_key: &ListKeyCheck,
    command_key: &CommandKeyCheck,
    ticket_key: &TicketKeyCheck,
) -> (String, bool) {
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
    ok &= list_keys(config, list_key, &mut report);
    ok &= command_keys(config, command_key, &mut report);
    ok &= ticket_keys(config, ticket_key, &mut report);
    let _ = writeln!(
        report,
        "[ OK ] login (HJV_API_LOGIN): {}, {}",
        config.login.as_str(),
        config.login.describe()
    );
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
