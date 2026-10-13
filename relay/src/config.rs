// SPDX-License-Identifier: AGPL-3.0-only

//! What a build fixes at compile time, like the client's pinned builds: the server (`HJV_SERVER`,
//! a host name with an optional `:port`) and the public keys wake commands are signed with
//! (`HJV_COMMAND_PUBKEY`, `<key id>:<base64 key>[,<key id>:<base64 key>]`, PROTOCOL.md "Command
//! keys"). The public repository builds without them: such a build refuses `run` and still does
//! `test-wake`, `init`, `status` and `version`. `HJV_BUILD_COMMIT` names the source commit.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::VerifyingKey;
use sha2::{Digest, Sha256};
use std::fmt;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const BUILD_SERVER: Option<&str> = option_env!("HJV_SERVER");
const BUILD_COMMAND_PUBKEY: Option<&str> = option_env!("HJV_COMMAND_PUBKEY");
const BUILD_COMMIT: Option<&str> = option_env!("HJV_BUILD_COMMIT");

/// At most this many command keys: the current one and the next, for key rotation.
pub const MAX_COMMAND_KEYS: usize = 2;

/// The HJViewer server: requests go to `https://<host>:<port>/api/hjv/relay/...`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub host: String,
    pub port: u16,
}

impl Server {
    /// `host` or `host:port`: a DNS name or an IPv4 address, without a scheme or a path.
    pub fn parse(s: &str) -> Result<Server, String> {
        let s = s.trim();
        let (host, port) = match s.rsplit_once(':') {
            Some((host, port)) => {
                let port = port
                    .parse::<u16>()
                    .ok()
                    .filter(|p| *p != 0 && port.bytes().all(|b| b.is_ascii_digit()))
                    .ok_or_else(|| format!("bad port in server {s:?}"))?;
                (host, port)
            }
            None => (s, 443),
        };
        let label_ok = |label: &str| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        };
        if host.is_empty() || host.len() > 253 || !host.split('.').all(label_ok) {
            return Err(format!("not a host name: {s:?}"));
        }
        Ok(Server {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    /// The `Host` header value: the name, and the port unless it is 443.
    pub fn authority(&self) -> String {
        if self.port == 443 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

impl fmt::Display for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.authority())
    }
}

/// A public key wake commands may be signed with, and the number the server names it by (`k`).
#[derive(Clone, Debug)]
pub struct CommandKey {
    pub id: u32,
    pub key: VerifyingKey,
}

impl CommandKey {
    /// The key's fingerprint (PROTOCOL.md, "Command keys"): the lowercase hex SHA-256 of the 32
    /// public key bytes, which `hjv-api selfcheck` and the build summary also show.
    pub fn fingerprint(&self) -> String {
        Sha256::digest(self.key.to_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

impl fmt::Display for CommandKey {
    /// `<key id>:<base64 key>`, one entry of `HJV_COMMAND_PUBKEY`, the line
    /// `hjv-api keygen command --kid <key id>` prints.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.id, STANDARD.encode(self.key.to_bytes()))
    }
}

/// Parses `<key id>:<base64 key>[,<key id>:<base64 key>]`: ids 1-65535, standard base64 of the
/// 32-byte Ed25519 public key, at most [`MAX_COMMAND_KEYS`] keys with distinct ids.
pub fn parse_command_keys(s: &str) -> Result<Vec<CommandKey>, String> {
    let mut keys: Vec<CommandKey> = Vec::new();
    for entry in s.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (id, key) = entry
            .split_once(':')
            .ok_or_else(|| format!("{entry:?} is not <key id>:<base64 key>"))?;
        let id = id.trim();
        let id: u32 = id
            .parse()
            .ok()
            .filter(|n| (1..=65535).contains(n) && id.bytes().all(|b| b.is_ascii_digit()))
            .ok_or_else(|| format!("bad key id {id:?}"))?;
        let bytes = STANDARD
            .decode(key.trim())
            .map_err(|_| format!("command key {id} is not base64"))?;
        let bytes: [u8; 32] = bytes
            .try_into()
            .map_err(|_| format!("command key {id} is not 32 bytes"))?;
        let key = VerifyingKey::from_bytes(&bytes)
            .map_err(|_| format!("command key {id} is not an Ed25519 public key"))?;
        if key.is_weak() {
            return Err(format!("command key {id} is a weak key"));
        }
        if keys.iter().any(|k| k.id == id) {
            return Err(format!("command key id {id} appears twice"));
        }
        keys.push(CommandKey { id, key });
    }
    if keys.is_empty() {
        return Err("no command keys".to_owned());
    }
    if keys.len() > MAX_COMMAND_KEYS {
        return Err(format!("more than {MAX_COMMAND_KEYS} command keys"));
    }
    Ok(keys)
}

/// The server this build is pinned to, if any.
pub fn server() -> Option<Result<Server, String>> {
    BUILD_SERVER
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Server::parse)
}

/// The command keys this build trusts, if any.
pub fn command_keys() -> Option<Result<Vec<CommandKey>, String>> {
    BUILD_COMMAND_PUBKEY
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(parse_command_keys)
}

/// The source commit the release build was made from.
pub fn commit() -> Option<&'static str> {
    BUILD_COMMIT.map(str::trim).filter(|s| !s.is_empty())
}

/// The CPU architecture reported to the server (`x86_64`, `aarch64`, ...).
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers() {
        let s = Server::parse("rd.example.com").unwrap();
        assert_eq!((s.host.as_str(), s.port), ("rd.example.com", 443));
        assert_eq!(s.authority(), "rd.example.com");
        let s = Server::parse(" Localhost:8443 ").unwrap();
        assert_eq!((s.host.as_str(), s.port), ("localhost", 8443));
        assert_eq!(s.authority(), "localhost:8443");
        assert_eq!(Server::parse("127.0.0.1:1").unwrap().port, 1);
        for bad in [
            "",
            "https://rd.example.com",
            "rd.example.com/api",
            "rd.example.com:0",
            "rd.example.com:+1",
            "rd.example.com:65536",
            "-rd.example.com",
            "rd..example.com",
            "user@rd.example.com",
            "[::1]:443",
        ] {
            assert!(Server::parse(bad).is_err(), "{bad:?} parsed");
        }
    }

    fn public_key(seed: u8) -> String {
        let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
        STANDARD.encode(key.verifying_key().to_bytes())
    }

    #[test]
    fn command_keys() {
        let (a, b) = (public_key(1), public_key(2));
        let keys = parse_command_keys(&format!("1:{a}")).unwrap();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].id, 1);
        let two = parse_command_keys(&format!(" 1:{a} , 2:{b} ")).unwrap();
        assert_eq!(two.iter().map(|k| k.id).collect::<Vec<_>>(), [1, 2]);
        assert_ne!(two[0].key, two[1].key);
        // shown the way it is given
        assert_eq!(two[0].to_string(), format!("1:{a}"));
        assert_eq!(two[1].to_string(), format!("2:{b}"));
        // fingerprints: hex SHA-256 of the key bytes
        let bytes = STANDARD.decode(&a).unwrap();
        let hex: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(two[0].fingerprint(), hex);
        assert_eq!(hex.len(), 64);
        assert_ne!(two[0].fingerprint(), two[1].fingerprint());
        for bad in [
            String::new(),
            a.clone(),
            format!("0:{a}"),
            format!("x:{a}"),
            format!("+1:{a}"),
            format!("65536:{a}"),
            format!("1:{}", &a[..a.len() - 2]),
            "1:AAAA".to_owned(),
            format!("1:{a},1:{b}"),
            format!("1:{a},2:{b},3:{a}"),
            // the identity point, a weak key
            "1:AQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_owned(),
        ] {
            assert!(parse_command_keys(&bad).is_err(), "{bad:?} parsed");
        }
    }
}
