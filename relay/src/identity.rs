// SPDX-License-Identifier: AGPL-3.0-only

//! The relay's own key and the names derived from it (PROTOCOL.md, "Relay identity"). The key is
//! an Ed25519 key made on first run and kept in the state folder as `relay.key` (mode 0600, read
//! before the relay drops its privileges). The relay ID is `r-` and the first 80 bits of
//! SHA-256(public key) in lowercase base32; the enrollment code an administrator approves is the
//! same 16 characters in uppercase, in groups of four.

use base64::{engine::general_purpose::STANDARD, Engine};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const KEY_FILE: &str = "relay.key";
const KEY_HEADER: &str = "hjv-relay-key-v1";
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

pub struct Identity {
    key: SigningKey,
    pub pk: [u8; 32],
    pub id: String,
    pub code: String,
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl Identity {
    pub fn from_seed(seed: &[u8; 32]) -> Identity {
        let key = SigningKey::from_bytes(seed);
        let pk = key.verifying_key().to_bytes();
        Identity {
            id: relay_id(&pk),
            code: enrollment_code(&pk),
            key,
            pk,
        }
    }

    pub fn generate() -> io::Result<Identity> {
        let mut seed = [0u8; 32];
        getrandom::getrandom(&mut seed).map_err(|e| io::Error::other(e.to_string()))?;
        Ok(Identity::from_seed(&seed))
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.key.sign(msg).to_bytes()
    }

    pub fn pk_base64(&self) -> String {
        STANDARD.encode(self.pk)
    }

    fn file_contents(&self) -> String {
        format!("{KEY_HEADER}\n{}\n", STANDARD.encode(self.key.to_bytes()))
    }
}

/// RFC 4648 base32 (uppercase, no padding).
pub fn base32(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(5) * 8);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for &byte in data {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
        buffer &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// The first 80 bits of SHA-256 over the public key.
pub fn fingerprint(pk: &[u8; 32]) -> [u8; 10] {
    let digest = Sha256::digest(pk);
    let mut out = [0u8; 10];
    out.copy_from_slice(&digest[..10]);
    out
}

/// `r-` and the fingerprint in lowercase base32 (18 characters).
pub fn relay_id(pk: &[u8; 32]) -> String {
    format!("r-{}", base32(&fingerprint(pk)).to_ascii_lowercase())
}

/// The fingerprint in uppercase base32 as `XXXX-XXXX-XXXX-XXXX`.
pub fn enrollment_code(pk: &[u8; 32]) -> String {
    let chars: Vec<char> = base32(&fingerprint(pk)).chars().collect();
    chars
        .chunks(4)
        .map(|group| group.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

#[derive(Debug)]
pub enum KeyError {
    /// Reading or writing failed; worth retrying.
    Io(io::Error),
    /// The key file is there but unusable; a person has to look at it.
    Invalid(String),
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::Io(e) => write!(f, "{e}"),
            KeyError::Invalid(m) => f.write_str(m),
        }
    }
}

pub fn key_path(dir: &Path) -> PathBuf {
    dir.join(KEY_FILE)
}

/// Reads the key in `dir`, if there is one. A key file with group or other permission bits is set
/// back to 0600.
pub fn load(dir: &Path) -> Result<Option<Identity>, KeyError> {
    let path = key_path(dir);
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path);
    let file = match file {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(KeyError::Invalid(format!(
                "{} is a symbolic link",
                path.display()
            )))
        }
        Err(e) => return Err(KeyError::Io(e)),
    };
    let meta = file.metadata().map_err(KeyError::Io)?;
    if !meta.is_file() {
        return Err(KeyError::Invalid(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    // SAFETY: geteuid has no preconditions.
    let euid = unsafe { libc::geteuid() };
    if meta.uid() != euid {
        return Err(KeyError::Invalid(format!(
            "{} belongs to uid {}, not to uid {euid}",
            path.display(),
            meta.uid()
        )));
    }
    if meta.mode() & 0o077 != 0 {
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|e| {
                KeyError::Invalid(format!("cannot make {} mode 0600: {e}", path.display()))
            })?;
        crate::logln!("키 파일 권한을 0600으로 고쳤습니다: {}", path.display());
    }
    let mut text = String::new();
    file.take(4096)
        .read_to_string(&mut text)
        .map_err(|e| match e.kind() {
            io::ErrorKind::InvalidData => {
                KeyError::Invalid(format!("{} is not a key file", path.display()))
            }
            _ => KeyError::Io(e),
        })?;
    parse_key_file(&text)
        .map(Some)
        .map_err(|m| KeyError::Invalid(format!("{}: {m}", path.display())))
}

fn parse_key_file(text: &str) -> Result<Identity, String> {
    let mut lines = text.lines();
    if lines.next() != Some(KEY_HEADER) {
        return Err("not an hjv-relay key file".to_owned());
    }
    let seed = STANDARD
        .decode(lines.next().unwrap_or("").trim())
        .map_err(|_| "the key is not base64".to_owned())?;
    let seed: [u8; 32] = seed
        .try_into()
        .map_err(|_| "the key is not 32 bytes".to_owned())?;
    if lines.any(|l| !l.trim().is_empty()) {
        return Err("unexpected text after the key".to_owned());
    }
    Ok(Identity::from_seed(&seed))
}

/// Reads the key in `dir`, or makes one. The new key is written to a temporary file and linked
/// into place, so a crash never leaves a partial key and two processes starting at once end up
/// with the same key. Returns whether the key is new.
pub fn load_or_create(dir: &Path) -> Result<(Identity, bool), KeyError> {
    if let Some(identity) = load(dir)? {
        return Ok((identity, false));
    }
    let identity = Identity::generate().map_err(KeyError::Io)?;
    let tmp = dir.join(format!(".{KEY_FILE}.{}", std::process::id()));
    let _ = fs::remove_file(&tmp);
    let written = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        file.write_all(identity.file_contents().as_bytes())?;
        file.sync_all()
    })();
    let linked = written.and_then(|()| fs::hard_link(&tmp, key_path(dir)));
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => {
            if let Ok(d) = fs::File::open(dir) {
                let _ = d.sync_all();
            }
            Ok((identity, true))
        }
        // another process made the key first: use that one
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => match load(dir)? {
            Some(identity) => Ok((identity, false)),
            None => Err(KeyError::Io(e)),
        },
        Err(e) => Err(KeyError::Io(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base32_matches_rfc4648() {
        // RFC 4648 section 10, without padding
        for (input, output) in [
            ("", ""),
            ("f", "MY"),
            ("fo", "MZXQ"),
            ("foo", "MZXW6"),
            ("foob", "MZXW6YQ"),
            ("fooba", "MZXW6YTB"),
            ("foobar", "MZXW6YTBOI"),
        ] {
            assert_eq!(base32(input.as_bytes()), output);
        }
        assert_eq!(base32(&[0xff; 10]), "7".repeat(16));
    }

    #[test]
    fn id_and_code_come_from_the_key_fingerprint() {
        let identity = Identity::from_seed(&[7; 32]);
        let fp = base32(&fingerprint(&identity.pk));
        assert_eq!(fp.len(), 16);
        assert_eq!(identity.id, format!("r-{}", fp.to_ascii_lowercase()));
        assert_eq!(
            identity.code,
            format!("{}-{}-{}-{}", &fp[..4], &fp[4..8], &fp[8..12], &fp[12..])
        );
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("hjv-relay-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        dir
    }

    #[test]
    fn key_file_round_trip() {
        let dir = temp_dir("key");
        assert!(load(&dir).unwrap().is_none());
        let (made, new) = load_or_create(&dir).unwrap();
        assert!(new);
        let meta = fs::metadata(key_path(&dir)).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        let (again, new) = load_or_create(&dir).unwrap();
        assert!(!new);
        assert_eq!(made.pk, again.pk);
        assert_eq!(made.sign(b"x"), again.sign(b"x"));
        // left with loose permissions, it is tightened on the next read
        fs::set_permissions(key_path(&dir), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load(&dir).unwrap().unwrap().pk, made.pk);
        let meta = fs::metadata(key_path(&dir)).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        // no temporary files are left behind
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn broken_key_files_are_refused_not_replaced() {
        let dir = temp_dir("badkey");
        for text in [
            "",
            "hjv-relay-key-v1\n",
            "hjv-relay-key-v1\nAAAA\n",
            "something else\nAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n",
            "hjv-relay-key-v1\nAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\nmore\n",
        ] {
            fs::write(key_path(&dir), text).unwrap();
            fs::set_permissions(key_path(&dir), fs::Permissions::from_mode(0o600)).unwrap();
            assert!(
                matches!(load_or_create(&dir), Err(KeyError::Invalid(_))),
                "{text:?}"
            );
            assert_eq!(fs::read_to_string(key_path(&dir)).unwrap(), text);
        }
        fs::remove_file(key_path(&dir)).unwrap();
        std::os::unix::fs::symlink("/dev/null", key_path(&dir)).unwrap();
        assert!(matches!(load(&dir), Err(KeyError::Invalid(_))));
        fs::remove_dir_all(&dir).unwrap();
    }
}
