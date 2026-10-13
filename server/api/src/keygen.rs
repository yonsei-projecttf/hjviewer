// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api keygen seal|log-key|list` (design v2 §10-3 step 3, design v3 §5-3): makes a secret
//! for a credential without showing it on the screen.
//!
//! - `keygen seal`: an X25519 key pair for sealed passwords. The secret key goes to the credential
//!   hjviewer-api-login-key; the public key (for the app build variable HJV_LOGIN_SEAL_PUBKEY) and
//!   its key ID go to stderr.
//! - `keygen log-key`: 32 random bytes for the tags in logs and records (hjviewer-api-log-key).
//! - `keygen list --kid N`: an Ed25519 list key with the number N, which signs the trust lists.
//!   Its 32-byte seed goes to the credential hjviewer-api-list-key; stderr shows the public key as
//!   the entry `N:<base64>` of the app build variable HJV_TRUST_PUBKEYS (and of hjv-api's
//!   HJV_API_TRUST_PUBKEYS) and its fingerprint `N:<hex SHA-256>`.
//! - `keygen command --kid N`: an Ed25519 command key with the number N, which signs the wake
//!   commands (design v3 §5-3, phase 2). Its 32-byte seed goes to the credential
//!   hjviewer-api-command-key; stderr shows exactly one line, the public key as `N:<base64>`, the
//!   entry of the relay build variable HJV_COMMAND_PUBKEY and of hjv-api's HJV_API_COMMAND_PUBKEYS
//!   (relay/PROTOCOL.md §3.2: one text form wherever a command public key is written; `hjv-api
//!   selfcheck` shows its fingerprint).
//!
//! The secret goes, base64 on one line, straight into `systemd-creds encrypt` with
//! `--encrypt-to <file>` (credential name: the file name), into a new file with `--out <file>`
//! (removable media, for a key kept offline: the seal key, list key 2, command key 2), or else to
//! stdout, which
//! must not be a terminal: for example `hjv-api keygen list --kid 1 | systemd-creds encrypt
//! --name=hjviewer-api-list-key - /etc/credstore.encrypted/hjviewer-api-list-key`. A seal key
//! credential may hold several keys, one per line (the current one first), while the apps move to
//! a new key; a list key credential holds the key in use only.

use crate::{
    seal::key_id,
    trust,
    util::{b64, random_bytes},
};
use std::{
    io::{IsTerminal, Write},
    path::Path,
    process::{Command, Stdio},
};
use zeroize::Zeroizing;

/// A new seal key: (secret key line, public key, key ID).
pub fn seal_key() -> (Zeroizing<String>, String, String) {
    let secret = Zeroizing::new(random_bytes::<32>());
    let public = *crypto_box::SecretKey::from_bytes(*secret)
        .public_key()
        .as_bytes();
    (Zeroizing::new(b64(&*secret)), b64(&public), key_id(&public))
}

/// A new log key line.
pub fn log_key() -> Zeroizing<String> {
    Zeroizing::new(b64(&*Zeroizing::new(random_bytes::<32>())))
}

/// A new list key: (seed line, public key).
pub fn list_key() -> (Zeroizing<String>, [u8; 32]) {
    let seed = Zeroizing::new(random_bytes::<32>());
    let public = crate::devsig::public_key(&seed);
    (Zeroizing::new(b64(&*seed)), public)
}

/// Where the secret goes.
pub enum Target<'a> {
    Stdout,
    EncryptTo(&'a Path),
    Out(&'a Path),
}

pub fn run(kind: &str, kid: Option<u32>, target: Target<'_>) -> Result<(), String> {
    let (secret, note) = match (kind, kid) {
        ("seal", None) => {
            let (secret, public, kid) = seal_key();
            (
                secret,
                format!("seal key {kid}\npublic key (HJV_LOGIN_SEAL_PUBKEY): {public}\n"),
            )
        }
        ("log-key", None) => (log_key(), "log key made\n".to_owned()),
        ("list", Some(kid)) if (1..=trust::MAX_KID).contains(&kid) => {
            let (secret, public) = list_key();
            (
                secret,
                format!(
                    "list key {kid}\npublic key (an entry of HJV_TRUST_PUBKEYS and HJV_API_TRUST_PUBKEYS): {}\nfingerprint: {}\n",
                    trust::entry(kid, &public),
                    trust::fingerprint(kid, &public)
                ),
            )
        }
        ("list", _) => return Err("keygen list --kid <1-65535>".into()),
        ("command", Some(kid)) if (1..=trust::MAX_KID).contains(&kid) => {
            // the same kind of key as a list key: an Ed25519 seed, its public key in one form
            let (secret, public) = list_key();
            (secret, format!("{}\n", trust::entry(kid, &public)))
        }
        ("command", _) => return Err("keygen command --kid <1-65535>".into()),
        _ => return Err(USAGE.into()),
    };
    match target {
        Target::EncryptTo(path) => encrypt(&secret, path)?,
        Target::Out(path) => write_new(&secret, path)?,
        Target::Stdout => {
            let mut out = std::io::stdout().lock();
            if out.is_terminal() {
                return Err(
                    "keygen writes the secret to stdout, which is a terminal: pipe it into systemd-creds or use --encrypt-to <file> or --out <file>"
                        .into(),
                );
            }
            out.write_all(format!("{}\n", secret.as_str()).as_bytes())
                .and_then(|_| out.flush())
                .map_err(|e| e.to_string())?;
        }
    }
    eprint!("{note}");
    Ok(())
}

pub const USAGE: &str = "keygen seal | keygen log-key | keygen list --kid N | keygen command --kid N  [--encrypt-to <file> | --out <file>]";

fn encrypt(secret: &str, path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("--encrypt-to needs a file name")?;
    let mut child = Command::new("systemd-creds")
        .args(["encrypt", &format!("--name={name}"), "-"])
        .arg(path)
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("systemd-creds: {e}"))?;
    let line = Zeroizing::new(format!("{secret}\n"));
    child
        .stdin
        .take()
        .ok_or("systemd-creds: no stdin")?
        .write_all(line.as_bytes())
        .map_err(|e| format!("systemd-creds: {e}"))?;
    let status = child.wait().map_err(|e| format!("systemd-creds: {e}"))?;
    if !status.success() {
        return Err(format!("systemd-creds encrypt failed ({status})"));
    }
    Ok(())
}

/// Writes the secret line to a new file only its owner can read; an existing file stays as it
/// is (a key is never overwritten).
fn write_new(secret: &str, path: &Path) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let line = Zeroizing::new(format!("{secret}\n"));
    file.write_all(line.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::parse_seal_keys, seal::SealKeys};

    #[test]
    fn keys_work() {
        let (secret, public, kid) = seal_key();
        let keys = parse_seal_keys(&secret).unwrap();
        assert_eq!(SealKeys::new(&keys).key_ids(), vec![kid.clone()]);
        let pk: [u8; 32] = crate::util::b64_decode(&public)
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(key_id(&pk), kid);
        assert_eq!(crate::util::b64_decode(&log_key()).unwrap().len(), 32);
        let (seed, public) = list_key();
        let seed = crate::config::parse_list_key(&seed).unwrap();
        assert_eq!(trust::ListKey::new(1, &seed).public_key(), public);
    }

    #[test]
    fn out_files_are_new_and_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("hjv-api-keygen-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("list-2.key");
        run("list", Some(2), Target::Out(&path)).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(crate::config::parse_list_key(&text).is_ok());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        // never over an existing key
        assert!(run("list", Some(2), Target::Out(&path)).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        // a command key: the seed for the credential, the public key in the relays' one form
        let path = dir.join("command-2.key");
        run("command", Some(2), Target::Out(&path)).unwrap();
        let seed = crate::config::parse_list_key(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(crate::wake::CommandKey::new(2, &seed).kid(), 2);
        for (kind, kid) in [
            ("list", None),
            ("list", Some(0)),
            ("list", Some(65_536)),
            ("seal", Some(1)),
            ("command", None),
            ("command", Some(0)),
            ("command", Some(65_536)),
        ] {
            assert!(
                run(kind, kid, Target::Out(&dir.join("x"))).is_err(),
                "{kind} {kid:?}"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
