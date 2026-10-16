// SPDX-License-Identifier: AGPL-3.0-only

//! The App Review account: the one login that Apple's App Review uses to look at the iOS app.
//! Only a file of the operator's on the server names it (HJV_API_REVIEW_FILE, the user's decision
//! of 2026-10-16), never the repository: root:hjviewer-api 0640 like door 2's file
//! (deploy/review.conf.example; deploy/review-conf.py writes it), with each of these lines once and
//! `#` comments:
//!
//! ```text
//! ID=<the login ID>
//! HASH=pbkdf2-sha256$<iterations>$<salt, standard base64>$<derived key, standard base64>
//! UNTIL=<UTC time, 2026-11-29T15:00:00Z>
//! ```
//!
//! HASH is PBKDF2-HMAC-SHA256 of the password: 100 000 to 10 000 000 iterations (bounded, so that
//! a file cannot stall logins), a salt of 16 bytes or more, a key of 32. An ID that passes
//! HJViewer's ID rule (util.rs `staff_id`) refuses the whole file, so that a staff account is never
//! shadowed. The file is read again when it changes (update.rs `watched`, as door 2's); a missing,
//! unreadable or refused file, or one whose UNTIL has come, means there is no such account. The
//! journal names the account by its tag only.
//!
//! While the file is active, its ID (compared as the allowlist compares IDs) logs in without the
//! SSO (login.rs): the limits of any ID and the success limits, the password checked against HASH
//! in constant time (a wrong one fails as a wrong password the SSO refused), then the login's
//! transaction as for anyone, with the name [`NAME`], but never into an account it did not make (of
//! another name, or with a PC: one of before the ID rule). The operator puts the ID on the
//! allowlist ahead in either mode (docs/server-runbook.md 7-17): in open mode an ID not on it
//! counts under HJV_API_LIMIT_NEW_ID until its first login, in list mode it logs in only then. The
//! account needs no state of its own to be told apart: its key fails the ID rule, which every other
//! account's passes ([`is_review`]). It never holds a PC (login.rs `check_device`), gets no package
//! or short link (update.rs), is never a super operator (overview.rs), and its tokens hold only
//! while the file is active and names it ([`admits`], api.rs `authenticate`): else they are
//! unknown tokens, and the app logs out. A new HASH ends no session of the old password: `hjv-api
//! admin user revoke-all` does.

use crate::{
    config::Config,
    log::Tagger,
    state::AppState,
    update::{same, utc_time, watched, Watched},
    util::{b64_decode, datetime, login_key, now, staff_id, valid_portal_id},
};
use ring::pbkdf2;
use std::{
    num::NonZeroU32,
    sync::{Arc, Mutex},
};

/// The account's name, where the SSO's would be.
pub const NAME: &str = "App Review";
const MIN_ITERATIONS: u32 = 100_000;
const MAX_ITERATIONS: u32 = 10_000_000;
const MIN_SALT: usize = 16;
const MAX_FILE: u64 = 4096;

/// The file, as read.
pub struct Review {
    /// the login key of its ID (util.rs `login_key`)
    id: String,
    iterations: NonZeroU32,
    salt: Vec<u8>,
    key: [u8; 32],
    until: i64,
}

impl Review {
    /// Reads the file: `ID=`, `HASH=` and `UNTIL=` once each, and `#` comments. The errors never
    /// show what the file holds.
    pub fn parse(file: &[u8]) -> Result<Review, &'static str> {
        let text = std::str::from_utf8(file).map_err(|_| "the file is not text")?;
        let (mut id, mut hash, mut until) = (None, None, None);
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (field, value) = match line.split_once('=') {
                Some(("ID", value)) => (&mut id, value),
                Some(("HASH", value)) => (&mut hash, value),
                Some(("UNTIL", value)) => (&mut until, value),
                _ => return Err("a line that is neither ID=, HASH=, UNTIL= nor a # comment"),
            };
            if field.replace(value.trim()).is_some() {
                return Err("ID, HASH or UNTIL is there more than once");
            }
        }
        let (Some(id), Some(hash), Some(until)) = (id, hash, until) else {
            return Err("ID, HASH or UNTIL is missing");
        };
        if !valid_portal_id(id) {
            return Err("ID is not a login ID");
        }
        if staff_id(id).is_some() {
            return Err("ID passes HJViewer's ID rule, so it could be a staff account's");
        }
        let (iterations, salt, key) = hash_parts(hash)?;
        Ok(Review {
            id: login_key(id),
            iterations,
            salt,
            key,
            until: utc_time(until).ok_or("UNTIL is not a time")?,
        })
    }

    /// Whether `password` is the file's: PBKDF2-HMAC-SHA256 with its salt and iterations,
    /// compared in constant time (ring). It takes as long as the iterations: not for an async
    /// thread.
    pub fn verify(&self, password: &str) -> bool {
        pbkdf2::verify(
            pbkdf2::PBKDF2_HMAC_SHA256,
            self.iterations,
            &self.salt,
            password.as_bytes(),
            &self.key,
        )
        .is_ok()
    }

    /// Whether the file names the account of `login_key`.
    pub fn names(&self, login_key: &str) -> bool {
        same(self.id.as_bytes(), login_key.as_bytes())
    }

    fn summary(&self, tags: &Tagger) -> String {
        format!(
            "until {} (acct={})",
            datetime(self.until),
            tags.account(&self.id)
        )
    }
}

/// HASH's parts: `pbkdf2-sha256$<iterations>$<salt>$<key>`.
fn hash_parts(hash: &str) -> Result<(NonZeroU32, Vec<u8>, [u8; 32]), &'static str> {
    let mut parts = hash.split('$');
    let (Some("pbkdf2-sha256"), Some(iterations), Some(salt), Some(key), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err("HASH is not pbkdf2-sha256$<iterations>$<salt>$<key>");
    };
    let iterations = Some(iterations)
        .filter(|n| n.bytes().all(|b| b.is_ascii_digit()) && !n.starts_with('0'))
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|n| (MIN_ITERATIONS..=MAX_ITERATIONS).contains(n))
        .and_then(NonZeroU32::new)
        .ok_or("HASH's iterations are not 100000 to 10000000")?;
    let salt = b64_decode(salt)
        .filter(|salt| salt.len() >= MIN_SALT)
        .ok_or("HASH's salt is not base64 of 16 bytes or more")?;
    let key = b64_decode(key)
        .and_then(|key| <[u8; 32]>::try_from(key).ok())
        .ok_or("HASH's key is not base64 of 32 bytes")?;
    Ok((iterations, salt, key))
}

/// The file as hjv-api last read it.
#[derive(Default)]
pub struct ReviewFile(Mutex<Watched<Review>>);

impl ReviewFile {
    /// The file while it is active at `at`: there, taken, and before its UNTIL; `None` without
    /// HJV_API_REVIEW_FILE. It reads a file: not for an async thread.
    pub fn active(&self, config: &Config, tags: &Tagger, at: i64) -> Option<Arc<Review>> {
        let path = config.review_file.as_deref()?;
        watched(
            &self.0,
            path,
            MAX_FILE,
            "login: the App Review file (HJV_API_REVIEW_FILE)",
            Review::parse,
            |review| review.summary(tags),
        )
        .filter(|review| at < review.until)
    }
}

/// Whether the account of `login_key` is the App Review account: every other login passes
/// HJViewer's ID rule (login.rs step 4), and its ID never does.
pub fn is_review(login_key: &str) -> bool {
    staff_id(login_key).is_none()
}

/// Whether a token of the account of `login_key` holds at `at`: any other account's does, the App
/// Review account's while its file is active and names it. It may read a file: not for an async
/// thread.
pub fn admits(state: &AppState, login_key: &str, at: i64) -> bool {
    !is_review(login_key)
        || state
            .review
            .active(&state.config, &state.tags, at)
            .is_some_and(|review| review.names(login_key))
}

/// The file, when it is active and names the ID typed at a login (on a thread for files).
pub async fn named(state: &Arc<AppState>, username: &str) -> Option<Arc<Review>> {
    // without the setting, no thread is taken
    state.config.review_file.as_ref()?;
    let (state, key) = (state.clone(), login_key(username));
    tokio::task::spawn_blocking(move || {
        state
            .review
            .active(&state.config, &state.tags, now())
            .filter(|review| review.names(&key))
    })
    .await
    .ok()
    .flatten()
}

/// At the start: the file's own line says what was read or why it is not used; this one, that
/// there is no App Review account.
pub fn note_at_start(state: &AppState) {
    if state.config.review_file.is_some()
        && state
            .review
            .active(&state.config, &state.tags, now())
            .is_none()
    {
        log_notice!(
            "login: no App Review account (HJV_API_REVIEW_FILE is not there, not used, or past its UNTIL)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashMap, path::PathBuf};

    /// Made-up values, never a real account's.
    const ID: &str = "APPREVIEW-TEST";
    const PASSWORD: &str = "correct horse battery staple";
    /// PBKDF2-HMAC-SHA256 of PASSWORD with the salt 00 01 … 0f and 100 000 iterations, 32 bytes,
    /// by Python's hashlib (`hashlib.pbkdf2_hmac("sha256", PASSWORD, bytes(range(16)), 100000,
    /// 32)`): 49d49c25f597846209f0d92e7770ab64e1c75e94b4ce6c509265ee67175d2a1e.
    const HASH: &str =
        "pbkdf2-sha256$100000$AAECAwQFBgcICQoLDA0ODw==$SdScJfWXhGIJ8Nkud3CrZOHHXpS0zmxQkmXuZxddKh4=";

    fn file(id: &str, hash: &str, until: &str) -> String {
        format!("ID={id}\nHASH={hash}\nUNTIL={until}\n")
    }

    #[test]
    fn the_hash_is_pythons_pbkdf2() {
        let review = Review::parse(file(ID, HASH, "2026-11-29T15:00:00Z").as_bytes()).unwrap();
        assert_eq!(review.iterations.get(), 100_000);
        assert_eq!(review.salt, (0u8..16).collect::<Vec<u8>>());
        assert_eq!(
            crate::util::hex(&review.key),
            "49d49c25f597846209f0d92e7770ab64e1c75e94b4ce6c509265ee67175d2a1e"
        );
        assert!(review.verify(PASSWORD));
        for wrong in [
            "",
            "correct horse battery stapl",
            "correct horse battery staple ",
            "Correct horse battery staple",
        ] {
            assert!(!review.verify(wrong), "{wrong:?}");
        }
    }

    #[test]
    fn the_files_lines() {
        let until = "2026-11-29T15:00:00Z";
        let review = Review::parse(file(ID, HASH, until).as_bytes()).unwrap();
        assert_eq!(review.until, 1_795_964_400);
        assert_eq!(datetime(review.until), until);
        // the ID compared as the allowlist compares IDs: trimmed and case-folded
        assert!(review.names("appreview-test"));
        assert!(!review.names("appreview-tes"));
        assert!(!review.names("zztest01"));
        // comments, blank lines, Windows line ends, blanks around a value, any order
        let review = Review::parse(
            format!("# the App Review account\r\n\r\nUNTIL= {until} \r\n  HASH={HASH}\r\nID= appreview-Test \r\n")
                .as_bytes(),
        )
        .unwrap();
        assert!(review.names("appreview-test"));
        // the bounds of the iterations, which a file cannot leave
        let with = |iterations: &str| {
            let hash = HASH.replace("$100000$", &format!("${iterations}$"));
            Review::parse(file(ID, &hash, until).as_bytes()).map(|r| r.iterations.get())
        };
        assert_eq!(with("100000"), Ok(100_000));
        assert_eq!(with("10000000"), Ok(10_000_000));
        let iterations = Err("HASH's iterations are not 100000 to 10000000");
        for bad in [
            "99999",
            "10000001",
            "0",
            "0100000",
            "+100000",
            "",
            "4294967296",
        ] {
            assert_eq!(with(bad), iterations, "{bad:?}");
        }
        let rule = "ID passes HJViewer's ID rule, so it could be a staff account's";
        let form = "HASH is not pbkdf2-sha256$<iterations>$<salt>$<key>";
        let salt = "HASH's salt is not base64 of 16 bytes or more";
        let key = "HASH's key is not base64 of 32 bytes";
        let time = "UNTIL is not a time";
        let twice = "ID, HASH or UNTIL is there more than once";
        let line = "a line that is neither ID=, HASH=, UNTIL= nor a # comment";
        let hashed = |hash: &str| file(ID, hash, until);
        let salt_16 = "AAECAwQFBgcICQoLDA0ODw==";
        let refused = [
            // an ID of HJViewer's ID rule could be a staff account's: the whole file goes
            (file("ZZDEMO01", HASH, until), rule),
            (file("yzdemo", HASH, until), rule),
            (file("two words", HASH, until), "ID is not a login ID"),
            (file("", HASH, until), "ID is not a login ID"),
            (hashed(&HASH.replace("sha256", "sha1")), form),
            (hashed(&format!("{HASH}$")), form),
            (hashed(&HASH[..HASH.rfind('$').unwrap()]), form),
            (hashed(PASSWORD), form),
            // 15 bytes, and 16 without the padding
            (hashed(&HASH.replace(salt_16, "AAECAwQFBgcICQoLDA0O")), salt),
            (
                hashed(&HASH.replace(salt_16, "AAECAwQFBgcICQoLDA0ODw")),
                salt,
            ),
            // 31 bytes and 34
            (hashed(&HASH.replace("Kh4=", "Kg==")), key),
            (hashed(&HASH.replace("Kh4=", "Kh4AAA==")), key),
            (file(ID, HASH, "2026-11-30"), time),
            (file(ID, HASH, "2026-11-30T00:00:00+09:00"), time),
            (file(ID, HASH, "later"), time),
            // each line once
            (format!("{}ID={ID}\n", file(ID, HASH, until)), twice),
            (format!("{}HASH={HASH}\n", file(ID, HASH, until)), twice),
            (format!("UNTIL={until}\n{}", file(ID, HASH, until)), twice),
            (
                format!("ID={ID}\nHASH={HASH}\n"),
                "ID, HASH or UNTIL is missing",
            ),
            (String::new(), "ID, HASH or UNTIL is missing"),
            (
                format!("{}PASSWORD={PASSWORD}\n", file(ID, HASH, until)),
                line,
            ),
            (format!("id={ID}\nHASH={HASH}\nUNTIL={until}\n"), line),
        ];
        for (text, why) in refused {
            let got = Review::parse(text.as_bytes()).map(|_| ()).unwrap_err();
            assert_eq!(got, why, "{text:?}");
            // no error shows what the file holds
            assert!(!got.to_lowercase().contains("appreview"));
            assert!(!got.contains("AAECAw") && !got.contains("SdScJf"));
        }
        assert!(Review::parse(&[0xff, 0xfe]).is_err());
    }

    #[test]
    fn review_accounts_are_the_keys_outside_the_rule() {
        assert!(is_review("appreview-test"));
        assert!(is_review("zatest01"));
        assert!(is_review(""));
        assert!(!is_review("zztest01"));
        assert!(!is_review("yztest03"));
    }

    #[test]
    fn the_file_is_read_again_and_ends_at_its_until() {
        crate::log::capture();
        let dir = std::env::temp_dir().join(format!(
            "hjv-api-demo-account-{}-{}",
            std::process::id(),
            crate::util::hex(&crate::util::random_bytes::<6>())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("review.conf");
        let vars: HashMap<String, String> = [
            ("HJV_API_SSO_TOKEN_URL", "https://sso.example.ac.kr/token"),
            ("HJV_API_SSO_USERINFO_URL", "https://sso.example.ac.kr/user"),
            ("HJV_API_SSO_CLIENT_ID", "client"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .chain([("HJV_API_REVIEW_FILE".to_owned(), path.display().to_string())])
        .collect();
        let config = Config::from_vars(&vars).unwrap();
        let tags = Tagger::new(b"log key");
        let reviews = ReviewFile::default();
        let at = 1_795_000_000;
        // as the runbook puts it in place: a new file moved over the old one
        let put = |text: String| {
            let new = PathBuf::from(format!("{}.new", path.display()));
            std::fs::write(&new, text).unwrap();
            std::fs::rename(&new, &path).unwrap();
        };
        assert!(reviews.active(&config, &tags, at).is_none());
        put(file(ID, HASH, "2026-11-29T15:00:00Z"));
        assert!(reviews
            .active(&config, &tags, at)
            .unwrap()
            .names("appreview-test"));
        // not after its UNTIL, though the file is the same
        assert!(reviews.active(&config, &tags, 1_795_964_400).is_none());
        assert!(reviews.active(&config, &tags, 1_795_964_399).is_some());
        put(file("ZZDEMO01", HASH, "2026-11-29T15:00:00Z"));
        assert!(reviews.active(&config, &tags, at).is_none());
        put(file("other-demo", HASH, "2026-11-29T15:00:00Z"));
        let other = reviews.active(&config, &tags, at).unwrap();
        assert!(other.names("other-demo") && !other.names("appreview-test"));
        std::fs::remove_file(&path).unwrap();
        assert!(reviews.active(&config, &tags, at).is_none());
        // without the setting, no file is looked at
        let off = Config::from_vars(
            &vars
                .into_iter()
                .filter(|(k, _)| k != "HJV_API_REVIEW_FILE")
                .collect(),
        )
        .unwrap();
        assert_eq!(off.review_file, None);
        put(file(ID, HASH, "2026-11-29T15:00:00Z"));
        assert!(ReviewFile::default().active(&off, &tags, at).is_none());
        // the journal heard of each file, by the account's tag, never its ID or hash
        let lines = crate::log::captured();
        let mine: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with("login: the App Review file (HJV_API_REVIEW_FILE) "))
            .collect();
        let read = format!(
            "login: the App Review file (HJV_API_REVIEW_FILE) read: until 2026-11-29T15:00:00Z (acct={})",
            tags.account("appreview-test")
        );
        assert!(mine.iter().any(|l| **l == read), "{mine:?}");
        assert!(mine.iter().any(|l| l.ends_with(
            "is not used: ID passes HJViewer's ID rule, so it could be a staff account's"
        )));
        assert!(mine.iter().any(|l| l.ends_with(" is gone")));
        for line in &lines {
            let lower = line.to_lowercase();
            assert!(
                !lower.contains("appreview") && !lower.contains("zzdemo01"),
                "{line}"
            );
            assert!(
                !line.contains("SdScJf") && !line.contains(PASSWORD),
                "{line}"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
