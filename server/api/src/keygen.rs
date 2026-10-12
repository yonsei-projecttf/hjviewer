// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api keygen seal|log-key` (design v2 §10-3 step 3): makes a secret for a credential
//! without showing it on the screen.
//!
//! - `keygen seal`: an X25519 key pair for sealed passwords. The secret key goes to the credential
//!   hjviewer-api-login-key; the public key (for the app build variable HJV_LOGIN_SEAL_PUBKEY) and
//!   its key ID go to stderr.
//! - `keygen log-key`: 32 random bytes for the tags in logs and records (hjviewer-api-log-key).
//!
//! With `--encrypt-to <file>` the secret goes straight into `systemd-creds encrypt` (credential
//! name: the file name), else it goes to stdout, which must not be a terminal: for example
//! `hjv-api keygen seal | tee /media/usb/seal.key | systemd-creds encrypt --name=hjviewer-api-login-key - /etc/credstore.encrypted/hjviewer-api-login-key`,
//! which also leaves the offline copy the user keeps. A seal key credential may hold several keys,
//! one per line (the current one first), while the apps move to a new key.

use crate::{
    seal::key_id,
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

pub fn run(kind: &str, encrypt_to: Option<&Path>) -> Result<(), String> {
    let (secret, note) = match kind {
        "seal" => {
            let (secret, public, kid) = seal_key();
            (
                secret,
                format!("seal key {kid}\npublic key (HJV_LOGIN_SEAL_PUBKEY): {public}\n"),
            )
        }
        "log-key" => (log_key(), "log key made\n".to_owned()),
        _ => return Err("keygen seal | keygen log-key".into()),
    };
    match encrypt_to {
        Some(path) => encrypt(&secret, path)?,
        None => {
            let mut out = std::io::stdout().lock();
            if out.is_terminal() {
                return Err(
                    "keygen writes the secret to stdout, which is a terminal: pipe it into systemd-creds or use --encrypt-to <file>"
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
    }
}
