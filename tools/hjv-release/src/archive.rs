// SPDX-License-Identifier: AGPL-3.0-only

//! The archive (trust rules 4-2): every file `sign` wrote, the marks `void` writes, and the key
//! sets of the builds in use. The tool compares every new policy and declaration with it: a
//! signed file counts as published unless it is voided.
//!
//! ```text
//! ~/hjviewer-policy-archive/
//!   policy-e<epoch>-s<seq>.json      an envelope `sign` wrote (or `archive adopt` took)
//!   declaration-e<epoch>.json        the root part {k, p, s} of a declaration `sign` signed
//!   void-e<epoch>-s<seq>             `void`: that policy never left the operator's machines
//!   keyset-<16 hex digits>.env       a recorded key set: its two key lines and
//!                                    "# versions: 1.6.0 1.6.1" (the builds made with it)
//! ```
//!
//! It lives outside the repository (agents make and delete worktrees under `build/`) and apart
//! from the key files. Nothing enters it but what `sign`, `floor --live`, `void` and
//! `archive adopt` write; `policy` and `verify` only read it. Only the operator deletes a file,
//! and only a key set record, when the admin report shows no device on a build with that key set
//! (runbook 5-9). After every `sign` the operator copies it to the signing key's offline medium.

use crate::{
    envelope::{Envelope, Part},
    keys::{parse_key_list, KeySet, PublicKey, Role, ROOT_VAR, SIGNING_VAR},
    time,
    trust::hex,
};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

/// The archive's place under the operator's home directory.
pub const DEFAULT_NAME: &str = "hjviewer-policy-archive";
/// An archive file is small: an envelope (16 KiB) or a line of keys.
const MAX_FILE: u64 = 64 * 1024;

/// A policy file of the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Archived {
    pub epoch: u64,
    pub seq: u64,
    /// The envelope file, byte for byte.
    pub file: String,
    pub path: PathBuf,
}

/// A recorded key set: the keys a build pins, and the app versions built with them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySetRecord {
    pub path: PathBuf,
    pub signing: Vec<PublicKey>,
    pub roots: Vec<PublicKey>,
    pub versions: Vec<String>,
}

impl KeySetRecord {
    /// Whether this build pins `key` under its number and role.
    pub fn pins(&self, role: Role, key: &PublicKey) -> bool {
        let list = match role {
            Role::Signing => &self.signing,
            Role::Root => &self.roots,
        };
        list.iter().any(|k| k == key)
    }

    pub fn name(&self) -> String {
        let file = self
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if self.versions.is_empty() {
            file
        } else {
            format!("{file}(버전 {})", self.versions.join(" "))
        }
    }
}

pub struct Archive {
    pub dir: PathBuf,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Archive({})", self.dir.display())
    }
}

fn number(s: &str) -> Option<u64> {
    let canonical = !s.is_empty()
        && s.len() <= 15
        && s.bytes().all(|b| b.is_ascii_digit())
        && (s == "0" || !s.starts_with('0'));
    canonical.then(|| s.parse().ok()).flatten()
}

/// `e<epoch>-s<seq>` of a file name.
fn position(rest: &str) -> Option<(u64, u64)> {
    let (e, s) = rest.strip_prefix('e')?.split_once("-s")?;
    Some((number(e)?, number(s)?))
}

fn read_small(path: &Path) -> Result<String, String> {
    let meta = fs::symlink_metadata(path)
        .map_err(|e| format!("{}: 읽을 수 없습니다({e})", path.display()))?;
    if !meta.is_file() || meta.len() > MAX_FILE {
        return Err(format!(
            "{}: 보관소의 파일이 아니거나 너무 큽니다",
            path.display()
        ));
    }
    fs::read_to_string(path).map_err(|e| format!("{}: 읽을 수 없습니다({e})", path.display()))
}

/// Creates a file that must not exist yet.
fn create(path: &Path, content: &str) -> Result<(), String> {
    let at = |e: std::io::Error| format!("{}: 쓸 수 없습니다({e})", path.display());
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .map_err(at)?;
    file.write_all(content.as_bytes()).map_err(at)?;
    file.sync_all().map_err(at)
}

impl Archive {
    /// `~/hjviewer-policy-archive`.
    pub fn default_dir() -> Result<PathBuf, String> {
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or("HOME이 없어 기본 보관소를 정할 수 없습니다(--archive DIR)")?;
        Ok(PathBuf::from(home).join(DEFAULT_NAME))
    }

    /// Opens the archive at `dir`, making the directory when `create` is set (the writers). An
    /// archive that does not exist yet reads as empty (the first policy).
    pub fn open(dir: &Path, create: bool) -> Result<Archive, String> {
        if !dir.exists() {
            if !create {
                return Ok(Archive {
                    dir: dir.to_path_buf(),
                });
            }
            fs::create_dir_all(dir)
                .map_err(|e| format!("{}: 보관소를 만들 수 없습니다({e})", dir.display()))?;
        }
        let meta = fs::symlink_metadata(dir)
            .map_err(|e| format!("{}: 읽을 수 없습니다({e})", dir.display()))?;
        if !meta.is_dir() {
            return Err(format!("{}: 보관소가 디렉터리가 아닙니다", dir.display()));
        }
        Ok(Archive {
            dir: dir.to_path_buf(),
        })
    }

    fn names(&self) -> Result<Vec<String>, String> {
        let mut names = Vec::new();
        if !self.dir.exists() {
            return Ok(names);
        }
        let entries = fs::read_dir(&self.dir)
            .map_err(|e| format!("{}: 읽을 수 없습니다({e})", self.dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("{}: {e}", self.dir.display()))?;
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort();
        Ok(names)
    }

    /// Every policy file, by position (voided ones included).
    pub fn policies(&self) -> Result<Vec<Archived>, String> {
        let mut out = Vec::new();
        for name in self.names()? {
            let Some(rest) = name
                .strip_prefix("policy-")
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            let (epoch, seq) = position(rest)
                .ok_or_else(|| format!("보관소의 파일 이름이 맞지 않습니다: {name}"))?;
            let path = self.dir.join(&name);
            out.push(Archived {
                epoch,
                seq,
                file: read_small(&path)?,
                path,
            });
        }
        out.sort_by_key(|a| (a.epoch, a.seq));
        Ok(out)
    }

    /// The positions `void` marked.
    pub fn voided(&self) -> Result<BTreeSet<(u64, u64)>, String> {
        let mut out = BTreeSet::new();
        for name in self.names()? {
            if let Some(rest) = name.strip_prefix("void-") {
                out.insert(
                    position(rest)
                        .ok_or_else(|| format!("보관소의 파일 이름이 맞지 않습니다: {name}"))?,
                );
            }
        }
        Ok(out)
    }

    /// Every declaration `sign` signed, by epoch.
    pub fn declarations(&self) -> Result<Vec<(u64, Part)>, String> {
        let mut out = Vec::new();
        for name in self.names()? {
            let Some(epoch) = name
                .strip_prefix("declaration-e")
                .and_then(|n| n.strip_suffix(".json"))
            else {
                continue;
            };
            let epoch = number(epoch)
                .ok_or_else(|| format!("보관소의 파일 이름이 맞지 않습니다: {name}"))?;
            let text = read_small(&self.dir.join(&name))?;
            let part = Part::parse(text.trim_end_matches('\n'), "보관소의 루트 선언")?;
            out.push((epoch, part));
        }
        out.sort_by_key(|(e, _)| *e);
        Ok(out)
    }

    /// The archive's highest position that is not voided: what `--prev` must be (X11).
    pub fn highest(&self) -> Result<Option<Archived>, String> {
        let voided = self.voided()?;
        Ok(self
            .policies()?
            .into_iter()
            .rfind(|a| !voided.contains(&(a.epoch, a.seq))))
    }

    /// The highest epoch of any file the archive holds (0 when it is empty).
    pub fn max_epoch(&self) -> Result<u64, String> {
        let policies = self.policies()?.iter().map(|a| a.epoch).max().unwrap_or(0);
        let declarations = self
            .declarations()?
            .iter()
            .map(|(e, _)| *e)
            .max()
            .unwrap_or(0);
        Ok(policies.max(declarations))
    }

    /// The highest `seq` the archive holds in `epoch`, voided files included.
    pub fn max_seq(&self, epoch: u64) -> Result<Option<u64>, String> {
        Ok(self
            .policies()?
            .iter()
            .filter(|a| a.epoch == epoch)
            .map(|a| a.seq)
            .max())
    }

    /// The archive's file that is `file` byte for byte.
    pub fn find(&self, file: &str) -> Result<Option<Archived>, String> {
        Ok(self.policies()?.into_iter().find(|a| a.file == file))
    }

    /// The archive's file at a position.
    pub fn at(&self, epoch: u64, seq: u64) -> Result<Option<Archived>, String> {
        Ok(self
            .policies()?
            .into_iter()
            .find(|a| (a.epoch, a.seq) == (epoch, seq)))
    }

    /// The declaration the archive holds for `epoch`.
    pub fn declaration(&self, epoch: u64) -> Result<Option<Part>, String> {
        Ok(self
            .declarations()?
            .into_iter()
            .find(|(e, _)| *e == epoch)
            .map(|(_, p)| p))
    }

    /// Adds a signed envelope at its position. The same bytes again change nothing; another
    /// text at a position the archive holds is refused (X11).
    pub fn add_policy(&self, epoch: u64, seq: u64, file: &str) -> Result<Option<PathBuf>, String> {
        match self.at(epoch, seq)? {
            Some(a) if a.file == file => return Ok(None),
            Some(a) => {
                return Err(format!(
                    "보관소에 같은 위치({epoch}:{seq})의 다른 정책이 있습니다: {}",
                    a.path.display()
                ))
            }
            None => {}
        }
        let path = self.dir.join(format!("policy-e{epoch}-s{seq}.json"));
        create(&path, file)?;
        Ok(Some(path))
    }

    /// Adds a declaration's root part. The same text again changes nothing; another text for an
    /// epoch the archive holds is refused (an epoch number is never used twice).
    pub fn add_declaration(&self, epoch: u64, part: &Part) -> Result<Option<PathBuf>, String> {
        match self.declaration(epoch)? {
            Some(p) if p.p == part.p => return Ok(None),
            Some(_) => {
                return Err(format!(
                    "보관소에 세대 {epoch}의 다른 루트 선언이 있습니다. 세대 번호는 두 번 쓰지 않습니다"
                ))
            }
            None => {}
        }
        let path = self.dir.join(format!("declaration-e{epoch}.json"));
        create(&path, &format!("{}\n", part.to_text()))?;
        Ok(Some(path))
    }

    /// Marks a position as voided (`void`).
    pub fn add_void(&self, epoch: u64, seq: u64, now: i64) -> Result<PathBuf, String> {
        let path = self.dir.join(format!("void-e{epoch}-s{seq}"));
        create(
            &path,
            &format!(
                "# 이 정책은 운영자의 컴퓨터 밖으로 나가지 않았습니다(hjv-release void)\nvoided {}\n",
                time::format_utc(now)
            ),
        )?;
        Ok(path)
    }

    /// Every recorded key set.
    pub fn keysets(&self) -> Result<Vec<KeySetRecord>, String> {
        let mut out = Vec::new();
        for name in self.names()? {
            if !(name.starts_with("keyset-") && name.ends_with(".env")) {
                continue;
            }
            let path = self.dir.join(&name);
            let text = read_small(&path)?;
            let (mut signing, mut roots, mut versions) = (None, None, Vec::new());
            for line in text.lines() {
                let line = line.trim();
                if let Some(value) = line.strip_prefix(&format!("{SIGNING_VAR}=")) {
                    signing = Some(parse_key_list(value, 8, SIGNING_VAR)?);
                } else if let Some(value) = line.strip_prefix(&format!("{ROOT_VAR}=")) {
                    roots = Some(parse_key_list(value, 8, ROOT_VAR)?);
                } else if let Some(list) = line.strip_prefix("# versions:") {
                    versions = list.split_whitespace().map(str::to_owned).collect();
                } else if !(line.is_empty() || line.starts_with('#')) {
                    return Err(format!("{}: 모르는 줄이 있습니다", path.display()));
                }
            }
            let (Some(signing), Some(roots)) = (signing, roots) else {
                return Err(format!(
                    "{}: 두 키 줄({SIGNING_VAR}, {ROOT_VAR})이 있어야 합니다",
                    path.display()
                ));
            };
            out.push(KeySetRecord {
                path,
                signing,
                roots,
                versions,
            });
        }
        Ok(out)
    }

    /// The file name of a key set's record.
    pub fn keyset_name(keys: &KeySet, server: &str) -> String {
        format!("keyset-{}.env", &hex(&keys.fingerprint(server))[..16])
    }

    /// Whether the archive records this key set.
    pub fn records(&self, keys: &KeySet, server: &str) -> Result<bool, String> {
        Ok(self.dir.join(Archive::keyset_name(keys, server)).exists())
    }

    /// Records the key set of a build (and the app version built with it). Returns the record
    /// written, if anything changed.
    pub fn record_keyset(
        &self,
        keys: &KeySet,
        server: &str,
        version: Option<&str>,
    ) -> Result<Option<PathBuf>, String> {
        let path = self.dir.join(Archive::keyset_name(keys, server));
        let mut versions: Vec<String> = Vec::new();
        if path.exists() {
            let text = read_small(&path)?;
            if let Some(list) = text
                .lines()
                .find_map(|l| l.trim().strip_prefix("# versions:"))
            {
                versions = list.split_whitespace().map(str::to_owned).collect();
            }
            match version {
                Some(v) if !versions.iter().any(|x| x == v) => versions.push(v.to_owned()),
                _ => return Ok(None),
            }
        } else if let Some(v) = version {
            versions.push(v.to_owned());
        }
        let line = |list: &[PublicKey]| {
            let mut list: Vec<&PublicKey> = list.iter().collect();
            list.sort_by_key(|k| k.id);
            list.iter()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
                .join(",")
        };
        let text = format!(
            "# 기록된 키 집합(hjv-release): 이 키로 만든 빌드가 쓰이는 동안 모든 서명은 이 집합이 박은 키로 합니다\n\
             {SIGNING_VAR}={}\n{ROOT_VAR}={}\n# versions: {}\n",
            line(&keys.signing),
            line(&keys.roots),
            versions.join(" ")
        );
        let temporary = self.dir.join(format!(
            ".{}.new-{}",
            Archive::keyset_name(keys, server),
            std::process::id()
        ));
        let _ = fs::remove_file(&temporary);
        create(&temporary, &text)?;
        fs::rename(&temporary, &path).map_err(|e| {
            let _ = fs::remove_file(&temporary);
            format!("{}: 쓸 수 없습니다({e})", path.display())
        })?;
        Ok(Some(path))
    }

    /// The recorded key sets that do not pin `key` in `role` (X12: a key serves the records when
    /// this is empty).
    pub fn not_pinning(&self, role: Role, key: &PublicKey) -> Result<Vec<KeySetRecord>, String> {
        Ok(self
            .keysets()?
            .into_iter()
            .filter(|r| !r.pins(role, key))
            .collect())
    }
}

/// The position of an envelope file, read for its form only.
pub fn position_of(file: &str) -> Result<(u64, u64), String> {
    let (_, envelope) = Envelope::read_form(file)?;
    let form = crate::policy::PolicyForm::read(&envelope.policy.p)?;
    Ok((form.epoch, form.seq))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{keys::test_key_set, release::tests::temp_dir};

    #[test]
    fn files_and_positions() {
        let dir = temp_dir("archive");
        let archive = Archive::open(&dir.join("a"), true).unwrap();
        assert!(archive.highest().unwrap().is_none());
        assert_eq!(archive.max_epoch().unwrap(), 0);
        archive.add_policy(1, 1, "one\n").unwrap();
        archive.add_policy(1, 2, "two\n").unwrap();
        archive.add_policy(2, 1, "three\n").unwrap();
        // the same bytes again change nothing; another text at a position is refused
        assert_eq!(archive.add_policy(1, 2, "two\n").unwrap(), None);
        assert!(archive
            .add_policy(1, 2, "other\n")
            .unwrap_err()
            .contains("1:2"));
        assert_eq!(archive.highest().unwrap().unwrap().file, "three\n");
        assert_eq!(archive.max_seq(1).unwrap(), Some(2));
        assert_eq!(archive.find("two\n").unwrap().unwrap().seq, 2);
        assert!(archive.find("four\n").unwrap().is_none());
        // a voided file still counts for the positions, not as the highest
        archive.add_void(2, 1, 0).unwrap();
        assert_eq!(archive.highest().unwrap().unwrap().file, "two\n");
        assert_eq!(archive.max_seq(2).unwrap(), Some(1));
        assert_eq!(archive.max_epoch().unwrap(), 2);
        // a declaration's epoch counts too, and an epoch is used once
        let part = Part {
            k: 101,
            p: "{\"v\":1}".to_owned(),
            s: [7; 64],
        };
        archive.add_declaration(3, &part).unwrap();
        assert_eq!(archive.add_declaration(3, &part).unwrap(), None);
        let other = Part {
            p: "{\"v\":1,\"x\":1}".to_owned(),
            ..part.clone()
        };
        assert!(archive.add_declaration(3, &other).is_err());
        assert_eq!(archive.max_epoch().unwrap(), 3);
        assert_eq!(archive.declaration(3).unwrap().unwrap(), part);
        // a file that is not the archive's is refused, not ignored
        fs::write(dir.join("a/policy-e01-s1.json"), "x").unwrap();
        assert!(archive.policies().is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn key_set_records() {
        let dir = temp_dir("archive-keys");
        let archive = Archive::open(&dir, true).unwrap();
        let keys = test_key_set();
        assert!(!archive.records(&keys, "rd.example.com").unwrap());
        let written = archive
            .record_keyset(&keys, "rd.example.com", Some("1.6.0"))
            .unwrap();
        assert!(written.is_some());
        assert!(archive.records(&keys, "rd.example.com").unwrap());
        assert_eq!(
            archive
                .record_keyset(&keys, "rd.example.com", Some("1.6.0"))
                .unwrap(),
            None
        );
        archive
            .record_keyset(&keys, "rd.example.com", Some("1.6.1"))
            .unwrap();
        let records = archive.keysets().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].versions, ["1.6.0", "1.6.1"]);
        assert!(records[0].pins(Role::Signing, &keys.signing[0]));
        assert!(records[0].pins(Role::Root, &keys.roots[1]));
        assert!(!records[0].pins(Role::Root, &keys.signing[0]));
        assert!(archive
            .not_pinning(Role::Signing, &keys.signing[1])
            .unwrap()
            .is_empty());
        fs::remove_dir_all(dir).unwrap();
    }
}
