// SPDX-License-Identifier: AGPL-3.0-only

//! Passphrase encryption in the age format (age-encryption.org/v1, the scrypt recipient), enough
//! for one small secret: what `age -p` writes and reads, without ASCII armor.
//!
//! ```text
//! age-encryption.org/v1
//! -> scrypt <salt, 16 bytes> <work factor, log2 N>
//! <file key (16 bytes) wrapped with ChaCha20-Poly1305 under the scrypt key>
//! --- <HMAC-SHA-256 of the header up to and including "---">
//! <nonce, 16 bytes><the payload in 64 KiB chunks, each ChaCha20-Poly1305>
//! ```
//!
//! - scrypt key = scrypt(passphrase, "age-encryption.org/v1/scrypt" + salt, N = 2^factor, r = 8,
//!   p = 1), 32 bytes; the file key is wrapped under it with a zero nonce.
//! - header MAC key = HKDF-SHA-256(file key, no salt, "header"); payload key = HKDF-SHA-256(file
//!   key, salt = nonce, "payload"); chunk nonces are an 11-byte counter and a last-chunk flag.
//! - base64 is the standard alphabet without padding.
//!
//! The format is public, so a key file can still be opened with the age tool itself if this
//! program is ever lost.

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit},
    ChaCha20Poly1305, Key, Nonce,
};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use zeroize::Zeroizing;

const VERSION_LINE: &str = "age-encryption.org/v1";
const SCRYPT_LABEL: &[u8] = b"age-encryption.org/v1/scrypt";
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;
/// What `age -p` uses: about a second and 256 MiB.
pub const DEFAULT_WORK_FACTOR: u8 = 18;
/// As the age tool, refuse a file that asks for more work than this (1 GiB at 22).
pub const MAX_WORK_FACTOR: u8 = 22;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a file this module reads.
    Format(&'static str),
    /// The passphrase does not open the file.
    Passphrase,
    /// The header opened, the content is damaged.
    Damaged,
}

type Secret32 = Zeroizing<[u8; 32]>;

fn scrypt_key(passphrase: &[u8], salt: &[u8; 16], work_factor: u8) -> Result<Secret32, Error> {
    let params = scrypt::Params::new(work_factor, 8, 1, 32)
        .map_err(|_| Error::Format("the scrypt work factor is out of range"))?;
    let mut full_salt = Vec::with_capacity(SCRYPT_LABEL.len() + salt.len());
    full_salt.extend_from_slice(SCRYPT_LABEL);
    full_salt.extend_from_slice(salt);
    let mut key = Zeroizing::new([0u8; 32]);
    scrypt::scrypt(passphrase, &full_salt, &params, key.as_mut())
        .map_err(|_| Error::Format("scrypt refused the parameters"))?;
    Ok(key)
}

fn hkdf(file_key: &[u8; 16], salt: Option<&[u8]>, info: &[u8]) -> Secret32 {
    let mut out = Zeroizing::new([0u8; 32]);
    // 32 bytes is a valid output length for HKDF-SHA-256, so this cannot fail
    let _ = Hkdf::<Sha256>::new(salt, file_key).expand(info, out.as_mut());
    out
}

fn header_mac(file_key: &[u8; 16], header: &[u8]) -> Hmac<Sha256> {
    let key = hkdf(file_key, None, b"header");
    // HMAC takes a key of any length, so this cannot fail
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key.as_ref()).expect("any key length");
    mac.update(header);
    mac
}

fn chunk_nonce(counter: u64, last: bool) -> Nonce {
    let mut nonce = [0u8; 12];
    nonce[3..11].copy_from_slice(&counter.to_be_bytes());
    nonce[11] = u8::from(last);
    Nonce::from(nonce)
}

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0u8; N];
    getrandom::getrandom(&mut bytes).map_err(|_| Error::Format("no randomness from the system"))?;
    Ok(bytes)
}

/// Encrypts `plaintext` under `passphrase`.
pub fn encrypt(plaintext: &[u8], passphrase: &[u8], work_factor: u8) -> Result<Vec<u8>, Error> {
    let file_key = Zeroizing::new(random::<16>()?);
    encrypt_with(
        plaintext,
        passphrase,
        work_factor,
        &file_key,
        &random()?,
        &random()?,
    )
}

fn encrypt_with(
    plaintext: &[u8],
    passphrase: &[u8],
    work_factor: u8,
    file_key: &[u8; 16],
    salt: &[u8; 16],
    payload_nonce: &[u8; 16],
) -> Result<Vec<u8>, Error> {
    if work_factor == 0 || work_factor > MAX_WORK_FACTOR {
        return Err(Error::Format("the scrypt work factor is out of range"));
    }
    let wrap_key = scrypt_key(passphrase, salt, work_factor)?;
    let wrapped = ChaCha20Poly1305::new(Key::from_slice(wrap_key.as_ref()))
        .encrypt(&Nonce::default(), file_key.as_ref())
        .map_err(|_| Error::Format("could not wrap the file key"))?;
    let mut out = format!(
        "{VERSION_LINE}\n-> scrypt {} {work_factor}\n{}\n---",
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(&wrapped)
    )
    .into_bytes();
    let mac = header_mac(file_key, &out).finalize().into_bytes();
    out.extend_from_slice(format!(" {}\n", STANDARD_NO_PAD.encode(mac)).as_bytes());

    out.extend_from_slice(payload_nonce);
    let payload_key = hkdf(file_key, Some(payload_nonce), b"payload");
    let cipher = ChaCha20Poly1305::new(Key::from_slice(payload_key.as_ref()));
    let chunks: Vec<&[u8]> = if plaintext.is_empty() {
        vec![&[]]
    } else {
        plaintext.chunks(CHUNK).collect()
    };
    for (i, chunk) in chunks.iter().enumerate() {
        let sealed = cipher
            .encrypt(&chunk_nonce(i as u64, i + 1 == chunks.len()), *chunk)
            .map_err(|_| Error::Format("could not encrypt"))?;
        out.extend_from_slice(&sealed);
    }
    Ok(out)
}

/// One header line, up to the line feed; the rest follows.
fn line(input: &[u8]) -> Result<(&str, &[u8]), Error> {
    let end = input
        .iter()
        .position(|b| *b == b'\n')
        .ok_or(Error::Format("the header ends early"))?;
    let text =
        std::str::from_utf8(&input[..end]).map_err(|_| Error::Format("the header is not text"))?;
    Ok((text, &input[end + 1..]))
}

fn unpadded<const N: usize>(text: &str) -> Result<[u8; N], Error> {
    STANDARD_NO_PAD
        .decode(text)
        .ok()
        .and_then(|bytes| <[u8; N]>::try_from(bytes).ok())
        .ok_or(Error::Format(
            "a header value has the wrong length or is not base64",
        ))
}

/// Decrypts what [`encrypt`] or `age -p` wrote.
pub fn decrypt(file: &[u8], passphrase: &[u8]) -> Result<Zeroizing<Vec<u8>>, Error> {
    let (version, rest) = line(file)?;
    if version != VERSION_LINE {
        return Err(Error::Format(
            "not an age file (armored files are not read)",
        ));
    }
    let (stanza, rest) = line(rest)?;
    let mut args = stanza.split(' ');
    let (salt, work_factor) = match (
        args.next(),
        args.next(),
        args.next(),
        args.next(),
        args.next(),
    ) {
        (Some("->"), Some("scrypt"), Some(salt), Some(factor), None) => (salt, factor),
        _ => {
            return Err(Error::Format(
                "not encrypted with a passphrase (no scrypt stanza)",
            ))
        }
    };
    let salt: [u8; 16] = unpadded(salt)?;
    let canonical = !work_factor.is_empty()
        && work_factor.len() <= 2
        && work_factor.bytes().all(|b| b.is_ascii_digit())
        && !work_factor.starts_with('0');
    let work_factor: u8 = canonical
        .then(|| work_factor.parse().ok())
        .flatten()
        .filter(|f| (1..=MAX_WORK_FACTOR).contains(f))
        .ok_or(Error::Format("the scrypt work factor is out of range"))?;
    let (body, rest) = line(rest)?;
    let wrapped: [u8; 32] = unpadded(body)?;
    let (footer, payload) = line(rest)?;
    let mac: [u8; 32] = footer
        .strip_prefix("--- ")
        .ok_or(Error::Format("more than one stanza, or no header MAC"))
        .and_then(unpadded)?;
    let header = &file[..file.len() - rest.len() + 3];

    let wrap_key = scrypt_key(passphrase, &salt, work_factor)?;
    let file_key = ChaCha20Poly1305::new(Key::from_slice(wrap_key.as_ref()))
        .decrypt(&Nonce::default(), wrapped.as_ref())
        .map(Zeroizing::new)
        .map_err(|_| Error::Passphrase)?;
    let file_key: Zeroizing<[u8; 16]> =
        Zeroizing::new(<[u8; 16]>::try_from(file_key.as_slice()).map_err(|_| Error::Damaged)?);
    header_mac(&file_key, header)
        .verify_slice(&mac)
        .map_err(|_| Error::Damaged)?;

    if payload.len() < 16 + TAG {
        return Err(Error::Damaged);
    }
    let (nonce, sealed) = payload.split_at(16);
    let payload_key = hkdf(&file_key, Some(nonce), b"payload");
    let cipher = ChaCha20Poly1305::new(Key::from_slice(payload_key.as_ref()));
    let chunks: Vec<&[u8]> = sealed.chunks(CHUNK + TAG).collect();
    let mut plaintext = Zeroizing::new(Vec::with_capacity(sealed.len()));
    for (i, chunk) in chunks.iter().enumerate() {
        let last = i + 1 == chunks.len();
        let opened = cipher
            .decrypt(&chunk_nonce(i as u64, last), *chunk)
            .map(Zeroizing::new)
            .map_err(|_| Error::Damaged)?;
        // only a file that is empty altogether may end with an empty chunk
        if last && opened.is_empty() && chunks.len() > 1 {
            return Err(Error::Damaged);
        }
        plaintext.extend_from_slice(&opened);
    }
    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAST: u8 = 6;

    #[test]
    fn round_trip() {
        for plaintext in [
            &b""[..],
            b"x",
            b"hjv-update-key-v1\nrole=signing\n",
            &[7u8; CHUNK],
            &[9u8; CHUNK + 1],
        ] {
            let file = encrypt(plaintext, "암호 passphrase".as_bytes(), FAST).unwrap();
            assert!(file.starts_with(b"age-encryption.org/v1\n-> scrypt "));
            assert_eq!(
                decrypt(&file, "암호 passphrase".as_bytes())
                    .unwrap()
                    .as_slice(),
                plaintext
            );
            assert_eq!(decrypt(&file, b"other"), Err(Error::Passphrase));
            assert_eq!(decrypt(&file, b""), Err(Error::Passphrase));
        }
        // two encryptions of the same thing share nothing
        let (a, b) = (
            encrypt(b"same", b"p", FAST).unwrap(),
            encrypt(b"same", b"p", FAST).unwrap(),
        );
        assert_ne!(a, b);
        assert_ne!(a[a.len() - 20..], b[b.len() - 20..]);
    }

    #[test]
    fn the_plaintext_is_not_in_the_file() {
        let secret = b"hjv-update-key-v1 seed=QEFCQ0RFRkdISUpLTE1OT1BRUlNUVVZXWFlaW1xdXl8=";
        let file = encrypt(secret, b"p", FAST).unwrap();
        for window in secret.windows(8) {
            assert!(!file.windows(8).any(|w| w == window));
        }
    }

    #[test]
    fn damage_is_noticed() {
        let file = encrypt(b"the secret", b"p", FAST).unwrap();
        let header_end = file.windows(4).position(|w| w == b"\n---").unwrap();
        for at in 0..file.len() {
            let mut bad = file.clone();
            bad[at] ^= 1;
            let got = decrypt(&bad, b"p");
            assert!(got.is_err(), "a flipped bit at {at} went unnoticed");
            // a change in the wrapped key reads as a wrong passphrase; after the header it is damage
            if at > header_end + 50 {
                assert_eq!(got, Err(Error::Damaged), "at {at}");
            }
        }
        for len in 0..file.len() {
            assert!(decrypt(&file[..len], b"p").is_err(), "truncated to {len}");
        }
        let mut longer = file.clone();
        longer.push(0);
        assert_eq!(decrypt(&longer, b"p"), Err(Error::Damaged));
    }

    #[test]
    fn header_rules() {
        let file = encrypt(b"x", b"p", FAST).unwrap();
        let text_end = file.windows(5).position(|w| w == b"\n--- ").unwrap() + 49;
        let (header, payload) = file.split_at(text_end);
        let header = std::str::from_utf8(header).unwrap();
        let rebuilt = |h: String| [h.as_bytes(), payload].concat();
        assert!(decrypt(&rebuilt(header.to_owned()), b"p").is_ok());
        for (from, to) in [
            ("age-encryption.org/v1", "age-encryption.org/v2"),
            ("-> scrypt", "-> X25519"),
            ("-> scrypt", "->  scrypt"),
            (" 6\n", " 06\n"),
            (" 6\n", " 0\n"),
            (" 6\n", " 23\n"),
            (" 6\n", " 6 x\n"),
            (" 6\n", "\n"),
            ("\n---", "\n-> scrypt AAAAAAAAAAAAAAAAAAAAAA 6\nAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n---"),
        ] {
            assert!(header.contains(from), "{from:?}");
            let bad = rebuilt(header.replacen(from, to, 1));
            assert!(matches!(decrypt(&bad, b"p"), Err(Error::Format(_))), "{to:?}");
        }
        // another work factor is another key: the passphrase no longer opens the file
        let bad = rebuilt(header.replacen(" 6\n", " 7\n", 1));
        assert_eq!(decrypt(&bad, b"p"), Err(Error::Passphrase));
        assert!(matches!(
            decrypt(b"-----BEGIN AGE ENCRYPTED FILE-----\n", b"p"),
            Err(Error::Format(_))
        ));
        assert!(matches!(decrypt(b"", b"p"), Err(Error::Format(_))));
        assert!(matches!(encrypt(b"x", b"p", 0), Err(Error::Format(_))));
        assert!(matches!(encrypt(b"x", b"p", 23), Err(Error::Format(_))));
    }

    #[test]
    fn fixed_inputs_give_fixed_bytes() {
        let file = encrypt_with(b"hello", b"password", FAST, &[1; 16], &[2; 16], &[3; 16]).unwrap();
        let again =
            encrypt_with(b"hello", b"password", FAST, &[1; 16], &[2; 16], &[3; 16]).unwrap();
        assert_eq!(file, again);
        assert!(file.starts_with(b"age-encryption.org/v1\n-> scrypt AgICAgICAgICAgICAgICAg 6\n"));
        assert_eq!(decrypt(&file, b"password").unwrap().as_slice(), b"hello");
        // These very bytes were opened with the age crate (0.11.5): what this module writes is
        // what age reads. tests/vectors/keyfile holds the other direction, a file age wrote.
        use base64::engine::general_purpose::STANDARD;
        assert_eq!(
            STANDARD.encode(&file),
            "YWdlLWVuY3J5cHRpb24ub3JnL3YxCi0+IHNjcnlwdCBBZ0lDQWdJQ0FnSUNBZ0lDQWdJQ0FnIDYKcFdPdkwr\
             R3lXT3lmTGpJU3Z1SzBxSkxaMTJsR1B6MVFZNGZsUDVzcjErcwotLS0gV1NkMVVSYk5wRWNqQXZqOWEzY1Rz\
             SzF3TWkyaUVxU0xVV1BxdUxOQnR5VQoDAwMDAwMDAwMDAwMDAwMDHs2Pw5XCoL4/Zp2nJ6nlSYTFw4Qi"
        );
    }
}
