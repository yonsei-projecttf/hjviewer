// SPDX-License-Identifier: AGPL-3.0-only

//! The operator access record (design v2 §10-2, §13-1): two lines per admin command, appended to
//! a file of its own (by default $LOGS_DIRECTORY/operator.log, /var/log/hjviewer-api/operator.log
//! under systemd). The first is written before the command runs (a command whose line cannot be
//! written does not run), the second when it has run, with what it changed (counts only) or that
//! it failed:
//!
//! ```text
//! 2026-10-12T09:30:00Z uid=0 sudo_user=alice cmd="allow import" targets=<tag>,<tag> reveal=0 prev=<64 hex>
//! 2026-10-12T09:30:00Z uid=0 sudo_user=alice cmd="allow import" done="ok: allowlist: 2 added, 0 already there" prev=<64 hex>
//! ```
//!
//! `prev` is the SHA-256 of the previous line (without its newline; 64 zeros for the first), so
//! that a changed or removed line shows: `hjv-api oplog-verify` checks the chain. Targets appear as
//! tags (log.rs), never as IDs or addresses; an allowlist file appears as the tags of its IDs, or,
//! past 32 IDs, as their number and a hash of their sorted tags. At installation root makes the
//! file append-only (`chattr +a`); hjv-api only ever opens it for appending.

use crate::util::{datetime, hex, now, sha256};
use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

pub struct OpLog {
    path: PathBuf,
    /// the hash of the last line
    last: Mutex<String>,
}

pub struct OpEntry<'a> {
    pub uid: u32,
    pub sudo_user: &'a str,
    pub command: &'a str,
    pub targets: &'a [String],
    pub reveal: bool,
}

/// What may stand in a line: letters, digits and a few signs; anything else becomes '?'.
fn safe(text: &str, max: usize) -> String {
    text.chars()
        .take(max)
        .map(|c| {
            if c.is_ascii_alphanumeric() || " ._:,()-".contains(c) {
                c
            } else {
                '?'
            }
        })
        .collect()
}

impl OpLog {
    /// Opens the record at `path`, created if missing, and reads where its chain stands.
    pub fn open(path: &Path) -> Result<OpLog, String> {
        let file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("operator log {}: {e}", path.display()))?;
        drop(file);
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("operator log {}: {e}", path.display()))?;
        let last = text
            .lines()
            .rev()
            .find(|l| !l.is_empty())
            .map_or_else(|| GENESIS.to_owned(), |l| hex(&sha256(l.as_bytes())));
        Ok(OpLog {
            path: path.to_owned(),
            last: Mutex::new(last),
        })
    }

    /// The line of a command, before it runs.
    pub fn append(&self, entry: &OpEntry<'_>) -> Result<(), String> {
        let targets = if entry.targets.is_empty() {
            "-".to_owned()
        } else {
            entry
                .targets
                .iter()
                .map(|t| safe(t, 40).replace(',', "?"))
                .collect::<Vec<_>>()
                .join(",")
        };
        self.write(
            entry,
            &format!("targets={targets} reveal={}", u8::from(entry.reveal)),
        )
    }

    /// The line of a command that has run: `done` says what it changed, in counts, or that it
    /// failed.
    pub fn append_done(&self, entry: &OpEntry<'_>, done: &str) -> Result<(), String> {
        self.write(entry, &format!("done=\"{}\"", safe(done, 160)))
    }

    fn write(&self, entry: &OpEntry<'_>, rest: &str) -> Result<(), String> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let sudo_user = if entry.sudo_user.is_empty() {
            "-".to_owned()
        } else {
            safe(entry.sudo_user, 32).replace([' ', ','], "?")
        };
        let line = format!(
            "{} uid={} sudo_user={} cmd=\"{}\" {rest} prev={}",
            datetime(now()),
            entry.uid,
            sudo_user,
            safe(entry.command, 60),
            last
        );
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("operator log: {e}"))?;
        file.write_all(format!("{line}\n").as_bytes())
            .and_then(|_| file.sync_data())
            .map_err(|e| format!("operator log: {e}"))?;
        *last = hex(&sha256(line.as_bytes()));
        Ok(())
    }
}

/// Checks the chain of a record; returns the number of lines.
pub fn verify(path: &Path) -> Result<usize, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut prev = GENESIS.to_owned();
    let mut count = 0;
    for (n, line) in text.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let linked = line
            .rsplit_once(" prev=")
            .is_some_and(|(_, hash)| hash == prev);
        if !linked {
            return Err(format!(
                "line {} does not follow the line before it: the record was changed",
                n + 1
            ));
        }
        prev = hex(&sha256(line.as_bytes()));
        count += 1;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain() {
        let dir = std::env::temp_dir().join(format!("hjv-api-oplog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("operator.log");
        let _ = std::fs::remove_file(&path);
        let log = OpLog::open(&path).unwrap();
        let entry = |cmd: &'static str| OpEntry {
            uid: 0,
            sudo_user: "alice",
            command: cmd,
            targets: &[],
            reveal: false,
        };
        log.append(&entry("report")).unwrap();
        log.append(&OpEntry {
            targets: &["1a2b".to_owned()],
            reveal: true,
            ..entry("unlock")
        })
        .unwrap();
        // a reopened record continues the chain
        let log = OpLog::open(&path).unwrap();
        log.append(&OpEntry {
            sudo_user: "bad\nname \"x\",y",
            ..entry("users")
        })
        .unwrap();
        log.append_done(
            &entry("allow import"),
            "ok: allowlist: 2 added, 0 already there\" prev=forged",
        )
        .unwrap();
        assert_eq!(verify(&path).unwrap(), 4);
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text.lines().count(), 4);
        assert!(text
            .lines()
            .next()
            .unwrap()
            .contains(&format!("prev={GENESIS}")));
        assert!(text.contains("sudo_user=bad?name??x??y"));
        assert!(text.contains("targets=1a2b reveal=1"));
        assert!(text.contains(
            "cmd=\"allow import\" done=\"ok: allowlist: 2 added, 0 already there? prev?forged\" prev="
        ));
        // removing or changing a line breaks the chain
        let lines: Vec<&str> = text.lines().collect();
        std::fs::write(&path, format!("{}\n{}\n", lines[0], lines[2])).unwrap();
        assert!(verify(&path).is_err());
        std::fs::write(&path, text.replace("report", "rep0rt")).unwrap();
        assert!(verify(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
