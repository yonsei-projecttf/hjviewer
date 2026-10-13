// SPDX-License-Identifier: AGPL-3.0-only

//! The device public keys hbbs holds, read from its loopback console (design v2 §9-5).
//!
//! hbbs answers commands on its loopback-only console (TCP 127.0.0.1:21115 next to the
//! rendezvous port, no authentication, one command of at most 1 KiB per connection, read within
//! 1 s; it writes its answer and closes). Server patch 0004 adds the command
//!
//! ```text
//! hjv-pk <id>
//! ```
//!
//! and answers it with exactly one line, the only format hjv-api accepts:
//!
//! ```text
//! HJVPK1 <id> <signed> <age>\n
//! ```
//!
//! - `<id>`: the ID asked for, echoed;
//! - `<signed>`: standard base64 of the device's key as hbbs signs it for clients: the bytes
//!   `RendezvousServer::get_pk` puts into PunchHoleResponse.pk and RelayResponse.pk, that is
//!   `sign::sign(IdPk { id, pk }.write_to_bytes(), <hbbs's secret key>)`: the 64-byte Ed25519
//!   signature followed by the protobuf message `IdPk` (hbb_common rendezvous.proto: `string id =
//!   1; bytes pk = 2;`) with the ID and the 32-byte public key the device registered with (from
//!   memory or hbbs's database); or `-` when hbbs knows no key for the ID;
//! - `<age>`: whole seconds since the device last registered, when it registered since hbbs
//!   started (a device that is on registers every few seconds), or `-` when hbbs does not know
//!   (a peer only loaded from the database, or right after hbbs restarted). At most ten years.
//!
//! hjv-api takes a key only with a valid signature of hbbs's key pair (HJV_API_HBBS_PUBKEY, the
//! public key the apps are built with) over an `IdPk` that names the ID asked for. The console has
//! no authentication, and while hbbs restarts any local program could listen on its port: without
//! the signature it could hand out keys of its own and have other people's PCs added to an
//! account. hbbs's secret key stays out of hjv-api's reach (InaccessiblePaths= in the unit).
//!
//! An hbbs without patch 0004 answers unknown commands with nothing; [`HbbsError::Unsupported`]
//! reports that. [`format_reply`] and [`sign_id_pk`] write the line, for patch 0004's tests and
//! the fake console in hjv-api's tests.

use crate::util::{b64, b64_decode, valid_device_id};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use std::{net::SocketAddr, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};

pub const COMMAND: &str = "hjv-pk";
pub const REPLY_TAG: &str = "HJVPK1";
/// The largest `<age>` taken (ten years); anything larger is not from hbbs.
pub const MAX_AGE: u64 = 10 * 365 * 24 * 3600;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const REPLY_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_REPLY: u64 = 512;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup {
    Known {
        public_key: [u8; 32],
        seen_secs_ago: Option<u64>,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HbbsError {
    /// no connection or no answer in time
    Unreachable(String),
    /// an empty answer: hbbs without patch 0004
    Unsupported,
    /// an answer in another format
    Malformed,
    /// a key not signed by HJV_API_HBBS_PUBKEY, or signed for another ID
    BadSignature,
    /// a key came, but HJV_API_HBBS_PUBKEY is not set, so it cannot be checked
    NoServerKey,
    /// an ID that cannot be a device ID (never sent)
    BadId,
}

impl std::fmt::Display for HbbsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HbbsError::Unreachable(e) => write!(f, "hbbs console unreachable ({e})"),
            HbbsError::Unsupported => write!(f, "hbbs does not know hjv-pk (patch 0004 missing)"),
            HbbsError::Malformed => write!(f, "hbbs answered hjv-pk in an unknown format"),
            HbbsError::BadSignature => write!(
                f,
                "the hjv-pk answer is not signed with HJV_API_HBBS_PUBKEY (the wrong key, or another program on the console's port)"
            ),
            HbbsError::NoServerKey => {
                write!(f, "HJV_API_HBBS_PUBKEY is not set: device keys cannot be checked")
            }
            HbbsError::BadId => write!(f, "not a device ID"),
        }
    }
}

/// Asks hbbs at `console` for the public key of device `id`; a key counts only with the signature
/// of `server_key`.
pub async fn lookup(
    console: SocketAddr,
    server_key: Option<&[u8; 32]>,
    id: &str,
) -> Result<Lookup, HbbsError> {
    if !valid_device_id(id) {
        return Err(HbbsError::BadId);
    }
    let mut stream = timeout(CONNECT_TIMEOUT, TcpStream::connect(console))
        .await
        .map_err(|_| HbbsError::Unreachable("connect timed out".into()))?
        .map_err(|e| HbbsError::Unreachable(e.kind().to_string()))?;
    let reply = timeout(REPLY_TIMEOUT, async {
        // one write: hbbs reads the command with a single read
        stream
            .write_all(format!("{COMMAND} {id}").as_bytes())
            .await?;
        let mut reply = Vec::new();
        (&mut stream)
            .take(MAX_REPLY)
            .read_to_end(&mut reply)
            .await?;
        Ok::<_, std::io::Error>(reply)
    })
    .await
    .map_err(|_| HbbsError::Unreachable("no answer in time".into()))?
    .map_err(|e| HbbsError::Unreachable(e.kind().to_string()))?;
    let reply = String::from_utf8(reply).map_err(|_| HbbsError::Malformed)?;
    parse_reply(id, &reply, server_key)
}

/// Reads hbbs's answer to `hjv-pk <id>`.
pub fn parse_reply(
    id: &str,
    reply: &str,
    server_key: Option<&[u8; 32]>,
) -> Result<Lookup, HbbsError> {
    if reply.trim().is_empty() {
        return Err(HbbsError::Unsupported);
    }
    let line = reply.strip_suffix('\n').ok_or(HbbsError::Malformed)?;
    let fields: Vec<&str> = line.split(' ').collect();
    let [tag, reply_id, signed, age] = fields[..] else {
        return Err(HbbsError::Malformed);
    };
    if tag != REPLY_TAG || reply_id != id {
        return Err(HbbsError::Malformed);
    }
    let seen_secs_ago = match age {
        "-" => None,
        n => Some(
            n.parse::<u64>()
                .ok()
                .filter(|age| *age <= MAX_AGE)
                .ok_or(HbbsError::Malformed)?,
        ),
    };
    if signed == "-" {
        return Ok(Lookup::Unknown);
    }
    let signed = b64_decode(signed).ok_or(HbbsError::Malformed)?;
    let server_key = server_key.ok_or(HbbsError::NoServerKey)?;
    let (signed_id, pk) = open_id_pk(server_key, &signed)?;
    if signed_id != id {
        return Err(HbbsError::BadSignature);
    }
    if pk.is_empty() {
        // a peer that registered without a key
        return Ok(Lookup::Unknown);
    }
    let public_key = <[u8; 32]>::try_from(pk).map_err(|_| HbbsError::Malformed)?;
    Ok(Lookup::Known {
        public_key,
        seen_secs_ago,
    })
}

/// Checks hbbs's signature on a signed `IdPk` and reads it: (ID, public key).
fn open_id_pk(server_key: &[u8; 32], signed: &[u8]) -> Result<(String, Vec<u8>), HbbsError> {
    let (signature, message) = signed
        .split_first_chunk::<64>()
        .ok_or(HbbsError::Malformed)?;
    let key = VerifyingKey::from_bytes(server_key).map_err(|_| HbbsError::BadSignature)?;
    key.verify_strict(message, &Signature::from_bytes(signature))
        .map_err(|_| HbbsError::BadSignature)?;
    decode_id_pk(message).ok_or(HbbsError::Malformed)
}

/// The protobuf `IdPk { string id = 1; bytes pk = 2; }`, as hbbs's rust-protobuf writes it: the
/// two fields, each at most once, empty ones left out.
fn decode_id_pk(message: &[u8]) -> Option<(String, Vec<u8>)> {
    let (mut id, mut pk) = (None, None);
    let mut pos = 0;
    while pos < message.len() {
        let tag = get_varint(message, &mut pos)?;
        if tag & 7 != 2 {
            return None;
        }
        let len = usize::try_from(get_varint(message, &mut pos)?).ok()?;
        let data = message.get(pos..pos.checked_add(len)?)?;
        pos += len;
        let slot = match tag >> 3 {
            1 => &mut id,
            2 => &mut pk,
            _ => return None,
        };
        if slot.replace(data).is_some() {
            return None;
        }
    }
    let id = std::str::from_utf8(id.unwrap_or_default()).ok()?.to_owned();
    Some((id, pk.unwrap_or_default().to_vec()))
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

fn put_bytes(buf: &mut Vec<u8>, field: u8, data: &[u8]) {
    buf.push((field << 3) | 2);
    let mut len = data.len();
    while len >= 0x80 {
        buf.push(len as u8 | 0x80);
        len >>= 7;
    }
    buf.push(len as u8);
    buf.extend_from_slice(data);
}

/// What hbbs sends as a device's signed key (`get_pk`): the signature of the 32-byte Ed25519
/// seed `server_seed` over `IdPk { id, pk }`, then that message.
pub fn sign_id_pk(server_seed: &[u8; 32], id: &str, pk: &[u8]) -> Vec<u8> {
    let mut message = Vec::new();
    if !id.is_empty() {
        put_bytes(&mut message, 1, id.as_bytes());
    }
    if !pk.is_empty() {
        put_bytes(&mut message, 2, pk);
    }
    let signature = SigningKey::from_bytes(server_seed).sign(&message);
    [signature.to_bytes().as_slice(), &message].concat()
}

/// The answer line patch 0004 writes; `signed` as [`sign_id_pk`] makes it.
pub fn format_reply(id: &str, signed: Option<&[u8]>, seen_secs_ago: Option<u64>) -> String {
    let signed = signed.map_or_else(|| "-".to_owned(), b64);
    let age = seen_secs_ago.map_or_else(|| "-".to_owned(), |s| s.to_string());
    format!("{REPLY_TAG} {id} {signed} {age}\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with libsodium 1.0.18 (crypto_sign_seed_keypair, crypto_sign), as hbbs's sodiumoxide
    // `sign::sign` signs, over IdPk{id: "123456789", pk: 1..=32}.
    const SERVER_SEED: &str = "yMnKy8zNzs/Q0dLT1NXW19jZ2tvc3d7f4OHi4+Tl5uc=";
    const SERVER_PK: &str = "MrU+iC49qsGA16X2Ik1htkAbQ/kB2wnw5NhM5OSicYs=";
    const DEVICE_PK: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA=";
    const SIGNED: &str = "W/XYlnSIcSr8BWaXhBfRj6VuPOfIBXTb7klc8AKUCT866vhEtM+bA41CkCwwQls9Cn0w1IlkhhkifUJkxHLlCwoJMTIzNDU2Nzg5EiABAgMEBQYHCAkKCwwNDg8QERITFBUWFxgZGhscHR4fIA==";

    fn key(text: &str) -> [u8; 32] {
        b64_decode(text).unwrap().try_into().unwrap()
    }

    #[test]
    fn reads_what_libsodium_signed() {
        let (seed, server, device) = (key(SERVER_SEED), key(SERVER_PK), key(DEVICE_PK));
        assert_eq!(crate::devsig::public_key(&seed), server);
        // the same bytes as libsodium's: the signature is deterministic, the message the same
        assert_eq!(b64(&sign_id_pk(&seed, "123456789", &device)), SIGNED);
        let line = format!("HJVPK1 123456789 {SIGNED} 4\n");
        assert_eq!(
            parse_reply("123456789", &line, Some(&server)),
            Ok(Lookup::Known {
                public_key: device,
                seen_secs_ago: Some(4)
            })
        );
        // another server key, no key, another ID asked for
        let other = crate::devsig::public_key(&[1; 32]);
        assert_eq!(
            parse_reply("123456789", &line, Some(&other)),
            Err(HbbsError::BadSignature)
        );
        assert_eq!(
            parse_reply("123456789", &line, None),
            Err(HbbsError::NoServerKey)
        );
        let echoed = format!("HJVPK1 987654321 {SIGNED} 4\n");
        assert_eq!(
            parse_reply("987654321", &echoed, Some(&server)),
            Err(HbbsError::BadSignature)
        );
    }

    #[test]
    fn replies() {
        let seed = [3u8; 32];
        let server = crate::devsig::public_key(&seed);
        let pk = [5u8; 32];
        let signed = sign_id_pk(&seed, "1", &pk);
        assert_eq!(
            parse_reply("1", &format_reply("1", Some(&signed), None), Some(&server)),
            Ok(Lookup::Known {
                public_key: pk,
                seen_secs_ago: None
            })
        );
        assert_eq!(
            parse_reply("1", &format_reply("1", None, None), Some(&server)),
            Ok(Lookup::Unknown)
        );
        // an unknown ID needs no key
        assert_eq!(
            parse_reply("1", &format_reply("1", None, Some(2)), None),
            Ok(Lookup::Unknown)
        );
        // a peer without a key
        assert_eq!(
            parse_reply(
                "1",
                &format_reply("1", Some(&sign_id_pk(&seed, "1", &[])), None),
                Some(&server)
            ),
            Ok(Lookup::Unknown)
        );
        assert_eq!(parse_reply("1", "", None), Err(HbbsError::Unsupported));
        // a forged key: signed by someone else, or not signed at all
        let forged = sign_id_pk(&[4u8; 32], "1", &pk);
        assert_eq!(
            parse_reply(
                "1",
                &format_reply("1", Some(&forged), Some(1)),
                Some(&server)
            ),
            Err(HbbsError::BadSignature)
        );
        let mut altered = signed.clone();
        *altered.last_mut().unwrap() ^= 1;
        assert_eq!(
            parse_reply(
                "1",
                &format_reply("1", Some(&altered), Some(1)),
                Some(&server)
            ),
            Err(HbbsError::BadSignature)
        );
        let plain = format!("HJVPK1 1 {} 1\n", b64(&pk));
        assert_eq!(
            parse_reply("1", &plain, Some(&server)),
            Err(HbbsError::Malformed)
        );
        let good = b64(&signed);
        for bad in [
            format!("HJVPK1 2 {good} -\n"),
            format!("HJVPK1 1 {good} -"),
            format!("HJVPK2 1 {good} -\n"),
            "HJVPK1 1 abc -\n".to_owned(),
            "HJVPK1 1 - x\n".to_owned(),
            format!("HJVPK1 1 {good} {}\n", MAX_AGE + 1),
            format!("HJVPK1 1 {good} {}\n", u64::MAX),
            "HJVPK1 1 - - extra\n".to_owned(),
            "relay-servers\n".to_owned(),
        ] {
            assert_eq!(
                parse_reply("1", &bad, Some(&server)),
                Err(HbbsError::Malformed),
                "{bad:?}"
            );
        }
        assert_eq!(
            parse_reply("1", &format!("HJVPK1 1 {good} {MAX_AGE}\n"), Some(&server)),
            Ok(Lookup::Known {
                public_key: pk,
                seen_secs_ago: Some(MAX_AGE)
            })
        );
    }

    #[test]
    fn id_pk_messages() {
        assert_eq!(
            decode_id_pk(b"\x0a\x011\x12\x02ab"),
            Some(("1".to_owned(), b"ab".to_vec()))
        );
        // fields in either order, empty ones left out
        assert_eq!(
            decode_id_pk(b"\x12\x02ab\x0a\x011"),
            Some(("1".to_owned(), b"ab".to_vec()))
        );
        assert_eq!(decode_id_pk(b""), Some((String::new(), Vec::new())));
        for bad in [
            &b"\x0a\x011\x0a\x012"[..],
            b"\x1a\x011",
            b"\x08\x01",
            b"\x0a\x05ab",
            b"\x0a\x01\xff",
            b"\x0a",
        ] {
            assert_eq!(decode_id_pk(bad), None, "{bad:?}");
        }
    }

    #[tokio::test]
    async fn asks_over_tcp() {
        let seed = [6u8; 32];
        let server = crate::devsig::public_key(&seed);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"hjv-pk 42");
            let signed = sign_id_pk(&seed, "42", &[1u8; 32]);
            s.write_all(format_reply("42", Some(&signed), Some(9)).as_bytes())
                .await
                .unwrap();
        });
        assert_eq!(
            lookup(addr, Some(&server), "42").await,
            Ok(Lookup::Known {
                public_key: [1u8; 32],
                seen_secs_ago: Some(9)
            })
        );
        assert_eq!(
            lookup(addr, Some(&server), "4 2").await,
            Err(HbbsError::BadId)
        );
        // bound and never listening: refused, and no other test gets the port meanwhile
        let held = tokio::net::TcpSocket::new_v4().unwrap();
        held.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        assert!(matches!(
            lookup(held.local_addr().unwrap(), Some(&server), "42").await,
            Err(HbbsError::Unreachable(_))
        ));
    }
}
