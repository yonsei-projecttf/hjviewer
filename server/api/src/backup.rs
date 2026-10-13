// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api admin backup` (design v3 §5-6, §5-9): a copy of the database, taken by the running
//! service while it serves, and written by the command line into a directory only root may enter.
//!
//! The service takes the copy with `VACUUM INTO`, SQLite's online copy of a live database: one
//! read transaction, so the copy is consistent while requests go on. It writes it to a file of its
//! own next to the database (created 0600 before SQLite opens it), makes it a standalone file
//! (rollback journal, so that it opens without -wal and -shm files, and the application ID
//! [`APPLICATION_ID`], by which `--keep-days` tells its backups from other files), checks it
//! (`PRAGMA quick_check`) and counts the rows of each table. It answers the admin request with the
//! size, the SHA-256, the schema and those counts, sends the file's bytes after the answer's line,
//! and deletes its copy.
//!
//! The command line writes the bytes to a hidden temporary file in the destination's directory,
//! 0600, checks their size and SHA-256, opens the file and checks it again (quick_check, schema,
//! application ID, the row counts the service gave), and only then gives it its name, which must
//! not exist yet (a hard link, which never replaces a file). The directory must belong to whoever
//! runs the command and be closed to everyone else, as root's /var/lib/hjviewer-api/backup (0700)
//! is: the copy holds what the database holds. A missing directory is made (0700) only as that
//! daily backup's directory ([`DEFAULT_DIR`]) or in a directory of the caller's that is closed to
//! everyone else, and removed again when the backup fails; anywhere else (/etc, say) it must be
//! made first. With `--keep-days N` the command then deletes the backups in that directory
//! older than N days: regular files named *.sqlite3 that carry the application ID, and nothing
//! else, besides its own temporary files (`.<name>.sqlite3.<12 hex>.partial`) of runs that broke
//! off, after a day.
//!
//! The copy holds the database only: the credentials (/etc/credstore.encrypted,
//! /var/lib/systemd/credential.secret) and the operator record are not in it (design v3 §5-9). A
//! restore is an incident of its own: it brings back revoked keys and tokens, switches and
//! released bindings (README.md, Backup and restore). hjv-api therefore opens a restored backup
//! with password-less connections off for every account (db.rs).

use crate::{
    admin::{self, AdminRequest, AdminResponse},
    state::AppState,
    util::{datetime, hex, random_bytes, DAY},
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::Read,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

/// The application ID of a backup ("HJVA"): `PRAGMA application_id`, bytes 68-71 of the file.
pub const APPLICATION_ID: i32 = 0x484a_5641;
/// How long the service may take to make its copy, and the command line to wait for each part
/// of the bytes.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(300);
const CHUNK_TIMEOUT: Duration = Duration::from_secs(60);
const CHUNK: usize = 64 * 1024;
/// The longest `--keep-days`.
pub const MAX_KEEP_DAYS: i64 = 3650;
/// The daily backup's directory: root's, 0700, inside the service's state directory, made by
/// deploy/systemd/hjviewer-api-backup.service (`StateDirectory=`), and by the command line when
/// a backup by hand comes first.
pub const DEFAULT_DIR: &str = "/var/lib/hjviewer-api/backup";
const SUFFIX: &str = ".sqlite3";
const PARTIAL: &str = ".partial";

pub const USAGE: &str = "hjv-api admin backup [--keep-days N] <file>.sqlite3 | <directory>";

/// What the service says of its copy.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub size: u64,
    pub sha256: String,
    /// the database's schema (PRAGMA user_version)
    pub schema: i64,
    /// the rows of each table
    pub counts: BTreeMap<String, i64>,
}

impl BackupInfo {
    /// One line, counts only (the operator record carries it too).
    pub fn summary(&self) -> String {
        format!(
            "backup: schema {}, {} bytes, {} tables, {} rows",
            self.schema,
            self.size,
            self.counts.len(),
            self.counts.values().sum::<i64>()
        )
    }
}

/// The service's copy of the database; the file goes when this does.
pub struct Snapshot {
    pub info: BackupInfo,
    path: PathBuf,
}

impl Snapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A file deleted when this goes, unless kept.
struct Scratch(Option<PathBuf>);

impl Scratch {
    fn path(&self) -> &Path {
        self.0.as_deref().unwrap_or(Path::new(""))
    }

    fn keep(mut self) -> PathBuf {
        self.0.take().unwrap_or_default()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// A directory this run made for its backup: removed again when the backup fails (if empty by
/// then: the temporary file goes first), kept once the backup is in it.
struct MadeDir(Option<PathBuf>);

impl MadeDir {
    fn keep(mut self) {
        self.0 = None;
    }
}

impl Drop for MadeDir {
    fn drop(&mut self) {
        if let Some(dir) = self.0.take() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// Creates `path`, which must not exist, readable and writable by its owner only.
fn create_private(path: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// The rows of each table (the tables SQLite keeps for itself aside), the schema and the
/// application ID of a database.
fn inspect(c: &Connection) -> rusqlite::Result<(i64, BTreeMap<String, i64>, i32)> {
    let schema = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let application_id = c.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let mut stmt = c.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
         ORDER BY name",
    )?;
    let tables = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut counts = BTreeMap::new();
    for table in tables {
        let n: i64 = c.query_row(
            &format!("SELECT COUNT(*) FROM \"{}\"", table.replace('"', "\"\"")),
            [],
            |r| r.get(0),
        )?;
        counts.insert(table, n);
    }
    Ok((schema, counts, application_id))
}

/// `PRAGMA quick_check`: Ok when it says "ok".
fn quick_check(c: &Connection) -> Result<(), String> {
    let mut stmt = c
        .prepare("PRAGMA quick_check")
        .map_err(|e| format!("quick_check: {e}"))?;
    let lines = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .and_then(|rows| rows.collect::<rusqlite::Result<Vec<_>>>())
        .map_err(|e| format!("quick_check: {e}"))?;
    if lines == ["ok"] {
        Ok(())
    } else {
        Err(format!(
            "quick_check found {} problems, the first: {}",
            lines.len(),
            lines.first().map_or("", String::as_str)
        ))
    }
}

/// SHA-256 and size of a file.
fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += n as u64;
    }
    Ok((hex(&hasher.finalize()), size))
}

/// Makes the copy at `path` a standalone file with the backup's application ID, checks it and
/// describes it.
fn finish(path: &Path) -> Result<BackupInfo, String> {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("the copy: {e}"))?;
    let ready = || -> rusqlite::Result<(i64, BTreeMap<String, i64>, i32)> {
        let _: String = c.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0))?;
        c.pragma_update(None, "application_id", APPLICATION_ID)?;
        inspect(&c)
    };
    let (schema, counts, _) = ready().map_err(|e| format!("the copy: {e}"))?;
    quick_check(&c).map_err(|e| format!("the copy: {e}"))?;
    c.close().map_err(|(_, e)| format!("the copy: {e}"))?;
    let (sha256, size) = hash_file(path).map_err(|e| format!("the copy: {e}"))?;
    Ok(BackupInfo {
        size,
        sha256,
        schema,
        counts,
    })
}

/// The start of the names of the service's copies.
const COPY_PREFIX: &str = ".hjv-api-backup-";

/// Where the service makes its copies: next to the database, where it may write (a database in
/// memory, in tests, has no place: the temporary directory).
async fn copies_dir(state: &AppState) -> Result<PathBuf, String> {
    let db_path = state
        .db
        .call(|c| Ok(c.path().map(str::to_owned)))
        .await
        .map_err(|e| e.to_string())?;
    Ok(db_path
        .filter(|p| !p.is_empty())
        .and_then(|p| Path::new(&p).parent().map(Path::to_owned))
        .unwrap_or_else(std::env::temp_dir))
}

/// Deletes copies left by a backup that broke off with the service (a crash while it sent one),
/// an hour old or more, and their journals; the hourly upkeep calls it. Returns how many went.
pub async fn remove_stale_copies(state: &AppState) -> Result<usize, String> {
    let dir = copies_dir(state).await?;
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let old = entry
            .metadata()
            .ok()
            .filter(std::fs::Metadata::is_file)
            .and_then(|m| m.modified().ok())
            .and_then(|m| SystemTime::now().duration_since(m).ok())
            .is_some_and(|age| age >= Duration::from_secs(3600));
        if old
            && name.to_str().is_some_and(|n| n.starts_with(COPY_PREFIX))
            && std::fs::remove_file(entry.path()).is_ok()
        {
            removed += 1;
        }
    }
    Ok(removed)
}

/// The service's side: a consistent copy of the database it serves, checked and described.
pub async fn snapshot(state: &AppState) -> Result<Snapshot, String> {
    let dir = copies_dir(state).await?;
    let path = dir.join(format!(
        "{COPY_PREFIX}{}-{}{SUFFIX}",
        std::process::id(),
        hex(&random_bytes::<6>())
    ));
    create_private(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let scratch = Scratch(Some(path.clone()));
    let target = path.to_string_lossy().into_owned();
    state
        .db
        .call(move |c| c.execute("VACUUM INTO ?1", [target]).map(|_| ()))
        .await
        .map_err(|e| format!("VACUUM INTO: {e}"))?;
    let info = tokio::task::spawn_blocking(move || finish(&path))
        .await
        .map_err(|e| format!("the copy: {e}"))??;
    Ok(Snapshot {
        info,
        path: scratch.keep(),
    })
}

/// The service's side: sends the copy's bytes after the answer's line.
pub async fn send_file<W: AsyncWriteExt + Unpin>(
    snapshot: &Snapshot,
    write: &mut W,
) -> Result<(), String> {
    let mut file = tokio::fs::File::open(snapshot.path())
        .await
        .map_err(|e| format!("the copy: {e}"))?;
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file
            .read(&mut buf)
            .await
            .map_err(|e| format!("the copy: {e}"))?;
        if n == 0 {
            break;
        }
        tokio::time::timeout(CHUNK_TIMEOUT, write.write_all(&buf[..n]))
            .await
            .map_err(|_| "the command line stopped reading".to_owned())?
            .map_err(|e| e.to_string())?;
    }
    write.flush().await.map_err(|e| e.to_string())
}

/// "20261013-033012Z" for a Unix time.
fn stamp(at: i64) -> String {
    datetime(at).replace(['-', ':'], "").replace('T', "-")
}

/// What the command line was given.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    dest: String,
    keep_days: Option<i64>,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut dest = None;
    let mut keep_days = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--keep-days" => {
                let n = it
                    .next()
                    .and_then(|v| v.parse::<i64>().ok())
                    .filter(|n| (1..=MAX_KEEP_DAYS).contains(n))
                    .ok_or_else(|| format!("--keep-days takes 1 to {MAX_KEEP_DAYS}"))?;
                keep_days = Some(n);
            }
            a if a.starts_with("--") => return Err(format!("unknown option {a}\n{USAGE}")),
            a if dest.is_none() && !a.is_empty() => dest = Some(a.to_owned()),
            _ => return Err(USAGE.to_owned()),
        }
    }
    Ok(Args {
        dest: dest.ok_or(USAGE)?,
        keep_days,
    })
}

/// The directory and name of a backup: `<dir>/<name>.sqlite3` as given, or for a directory a
/// name of the time; `true` when the name was made here.
fn destination(arg: &str, at: i64) -> Result<(PathBuf, String, bool), String> {
    let path = Path::new(arg);
    if arg.ends_with('/') || path.is_dir() {
        return Ok((
            path.to_owned(),
            format!("hjv-api-{}{SUFFIX}", stamp(at)),
            true,
        ));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| format!("{arg}: not a file name\n{USAGE}"))?;
    if !name.ends_with(SUFFIX) || name.starts_with('.') || name.len() == SUFFIX.len() {
        return Err(format!(
            "{arg}: a backup's name ends in {SUFFIX} and does not start with a dot\n{USAGE}"
        ));
    }
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok((dir.to_owned(), name.to_owned(), false))
}

/// The user this process acts as: the effective user ID of /proc/self/status (Linux).
fn effective_uid() -> Option<u32> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("Uid:"))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// Makes the backup's directory (0700) when it is missing, only where a backup may go: as the
/// daily backup's directory [`DEFAULT_DIR`] (its parent, the service's state directory, is the
/// service user's), or in a directory of the caller's that is closed to everyone else, as
/// [`check_dir`] asks of the backup's directory itself. Anywhere else, /etc say, the command
/// makes nothing: the directory must be there first.
fn make_dir(dir: &Path) -> Result<MadeDir, String> {
    if dir.symlink_metadata().is_ok() {
        return Ok(MadeDir(None));
    }
    if dir != Path::new(DEFAULT_DIR) {
        let parent = dir
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let closed_and_mine = std::fs::metadata(parent).is_ok_and(|meta| {
            meta.is_dir()
                && meta.mode() & 0o077 == 0
                && effective_uid().is_some_and(|uid| uid == meta.uid())
        });
        if !closed_and_mine {
            return Err(format!(
                "{} is not there, and is made only in a directory of yours closed to others (or as {DEFAULT_DIR}): make it first (install -d -m 700 {})",
                dir.display(),
                dir.display()
            ));
        }
    }
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(MadeDir(Some(dir.to_owned())))
}

/// The directory of a backup: there (or made by [`make_dir`]), closed to everyone but its owner,
/// and owned by whoever writes `probe` in it (the backup's temporary file).
fn check_dir(dir: &Path, probe: &Path) -> Result<(), String> {
    let meta = std::fs::metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.is_dir() {
        return Err(format!("{}: not a directory", dir.display()));
    }
    if meta.mode() & 0o077 != 0 {
        return Err(format!(
            "{} is open to others (mode {:o}): a backup goes into a directory only its owner may enter (chmod 700)",
            dir.display(),
            meta.mode() & 0o7777
        ));
    }
    let mine = std::fs::metadata(probe)
        .map_err(|e| format!("{}: {e}", probe.display()))?
        .uid();
    if meta.uid() != mine {
        return Err(format!(
            "{} belongs to another user (uid {}): a backup goes into a directory of whoever takes it (as root: root's)",
            dir.display(),
            meta.uid()
        ));
    }
    Ok(())
}

/// Opens the received file and checks it against what the service said.
fn verify(path: &Path, info: &BackupInfo) -> Result<(), String> {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("the received copy: {e}"))?;
    quick_check(&c).map_err(|e| format!("the received copy: {e}"))?;
    let (schema, counts, application_id) =
        inspect(&c).map_err(|e| format!("the received copy: {e}"))?;
    if application_id != APPLICATION_ID {
        return Err("the received copy has not the backup's application ID".into());
    }
    if schema != info.schema || counts != info.counts {
        return Err("the received copy's schema or rows are not those the service counted".into());
    }
    Ok(())
}

/// Whether the first bytes of a file are those of an hjv-api backup: an SQLite database with
/// [`APPLICATION_ID`].
fn is_backup(path: &Path) -> bool {
    let mut head = [0u8; 72];
    let read = File::open(path).and_then(|mut f| f.read_exact(&mut head));
    read.is_ok()
        && &head[..16] == b"SQLite format 3\0"
        && i32::from_be_bytes([head[68], head[69], head[70], head[71]]) == APPLICATION_ID
}

/// The name of the temporary file a backup named `name` is written to: `.<name>.<12 hex>.partial`.
fn partial_name(name: &str, random: &[u8; 6]) -> String {
    format!(".{name}.{}{PARTIAL}", hex(random))
}

/// Whether `name` is one of [`partial_name`]'s, for a backup's name (`<name>.sqlite3`, not
/// starting with a dot).
fn is_partial(name: &str) -> bool {
    let Some((backup, random)) = name
        .strip_prefix('.')
        .and_then(|n| n.strip_suffix(PARTIAL))
        .and_then(|n| n.rsplit_once('.'))
    else {
        return false;
    };
    random.len() == 12
        && random
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        && backup.ends_with(SUFFIX)
        && backup.len() > SUFFIX.len()
        && !backup.starts_with('.')
}

/// What [`prune`] removed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Pruned {
    pub backups: usize,
    pub partial: usize,
}

/// Deletes the backups in `dir` older than `keep_days` (by modification time at `now`), and this
/// command's temporary files ([`is_partial`]) of broken-off runs older than a day; never `except`,
/// never anything else.
pub fn prune(dir: &Path, keep_days: i64, now: SystemTime, except: &Path) -> Result<Pruned, String> {
    let older = |meta: &std::fs::Metadata, secs: i64| {
        meta.modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age.as_secs() as i64 > secs)
    };
    let mut pruned = Pruned::default();
    let mut failed = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.file_type().is_file() || path == except {
            continue;
        }
        let partial = is_partial(name);
        let backup = !name.starts_with('.') && name.ends_with(SUFFIX);
        let gone = if partial && older(&meta, DAY) {
            &mut pruned.partial
        } else if backup && older(&meta, keep_days * DAY) && is_backup(&path) {
            &mut pruned.backups
        } else {
            continue;
        };
        match std::fs::remove_file(&path) {
            Ok(()) => *gone += 1,
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    if failed.is_empty() {
        Ok(pruned)
    } else {
        Err(format!(
            "could not remove old backups: {}",
            failed.join(", ")
        ))
    }
}

/// The command line's side: `hjv-api admin backup [--keep-days N] <file>|<directory>`; returns
/// what to print.
pub async fn cli(socket: &Path, args: &[String], sudo_user: String) -> Result<String, String> {
    let args = parse_args(args)?;
    let at = crate::util::now();
    let (dir, name, named_here) = destination(&args.dest, at)?;
    // a directory made here goes again on any failure below, after the temporary file (declared
    // later, so dropped first)
    let made = make_dir(&dir)?;
    if !named_here && dir.join(&name).symlink_metadata().is_ok() {
        return Err(format!("{} exists already", dir.join(&name).display()));
    }
    let temp = dir.join(partial_name(&name, &random_bytes::<6>()));
    let file = create_private(&temp).map_err(|e| format!("{}: {e}", temp.display()))?;
    let temp = Scratch(Some(temp));
    check_dir(&dir, temp.path())?;

    let mut stream = admin::connect(socket, admin::START_WAIT).await?;
    let request = AdminRequest {
        argv: vec!["backup".into(), name.clone()],
        reveal: false,
        sudo_user,
        input: None,
    };
    let mut line = serde_json::to_string(&request).map_err(|e| e.to_string())?;
    line.push('\n');
    stream
        .write_all(line.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut answer = String::new();
    tokio::time::timeout(ANSWER_TIMEOUT, reader.read_line(&mut answer))
        .await
        .map_err(|_| "no answer from the service".to_owned())?
        .map_err(|e| e.to_string())?;
    let response: AdminResponse =
        serde_json::from_str(&answer).map_err(|_| "the service answered nonsense".to_owned())?;
    if !response.ok {
        return Err(response.error);
    }
    let info = response
        .backup
        .ok_or("the service sent no copy (is it an hjv-api without backups?)")?;

    // the bytes, checked as they come
    let mut out = tokio::fs::File::from_std(file);
    let mut hasher = Sha256::new();
    let mut left = info.size;
    let mut buf = vec![0u8; CHUNK];
    while left > 0 {
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        let n = tokio::time::timeout(CHUNK_TIMEOUT, reader.read(&mut buf[..want]))
            .await
            .map_err(|_| "the service stopped sending".to_owned())?
            .map_err(|e| e.to_string())?;
        if n == 0 {
            return Err(format!(
                "the service sent {} of {} bytes",
                info.size - left,
                info.size
            ));
        }
        hasher.update(&buf[..n]);
        out.write_all(&buf[..n])
            .await
            .map_err(|e| format!("{}: {e}", temp.path().display()))?;
        left -= n as u64;
    }
    out.sync_all()
        .await
        .map_err(|e| format!("{}: {e}", temp.path().display()))?;
    drop(out);
    if hex(&hasher.finalize()) != info.sha256 {
        return Err("the received bytes are not those the service sent (SHA-256)".into());
    }
    let (checked_path, checked_info) = (temp.path().to_owned(), info.clone());
    tokio::task::spawn_blocking(move || verify(&checked_path, &checked_info))
        .await
        .map_err(|e| e.to_string())??;

    // its name, which must not exist yet: a link never replaces a file
    let mut target = dir.join(&name);
    let mut tries = 1;
    loop {
        match std::fs::hard_link(temp.path(), &target) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && named_here && tries < 9 => {
                tries += 1;
                let stem = name.trim_end_matches(SUFFIX);
                target = dir.join(format!("{stem}-{tries}{SUFFIX}"));
            }
            Err(e) => return Err(format!("{}: {e}", target.display())),
        }
    }
    drop(temp);
    made.keep();
    File::open(&dir)
        .and_then(|d| d.sync_all())
        .map_err(|e| format!("{}: {e}", dir.display()))?;

    let mut text = format!(
        "{}\n  {}, sha256 {}; checked (quick_check, schema and rows as the service counted)\n  rows: {}\n",
        target.display(),
        info.summary(),
        info.sha256.get(..16).unwrap_or(&info.sha256),
        info.counts
            .iter()
            .map(|(t, n)| format!("{t} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if let Some(days) = args.keep_days {
        let pruned = prune(&dir, days, SystemTime::now(), &target)
            .map_err(|e| format!("{text}the backup is written, but {e}"))?;
        text.push_str(&format!(
            "removed {} backups older than {days} days{}\n",
            pruned.backups,
            if pruned.partial > 0 {
                format!(" and {} broken-off temporary files", pruned.partial)
            } else {
                String::new()
            }
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "hjv-backup-{name}-{}-{}",
            std::process::id(),
            hex(&random_bytes::<4>())
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&d).unwrap();
        d
    }

    fn database(path: &Path, application_id: i32) {
        let c = Connection::open(path).unwrap();
        c.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (1), (2);")
            .unwrap();
        c.pragma_update(None, "application_id", application_id)
            .unwrap();
    }

    fn age(path: &Path, days: u64) {
        let when = SystemTime::now() - Duration::from_secs(days * 86_400 + 60);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(when)
            .unwrap();
    }

    #[test]
    fn arguments_and_names() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(
            parse_args(&s(&["--keep-days", "14", "/b/"])).unwrap(),
            Args {
                dest: "/b/".into(),
                keep_days: Some(14)
            }
        );
        assert_eq!(parse_args(&s(&["/b/x.sqlite3"])).unwrap().keep_days, None);
        for bad in [
            &[][..],
            &["--keep-days", "0", "/b/"],
            &["--keep-days", "x", "/b/"],
            &["--keep-days", "3651", "/b/"],
            &["--keep", "/b/"],
            &["/a.sqlite3", "/b.sqlite3"],
        ] {
            assert!(parse_args(&s(bad)).is_err(), "{bad:?}");
        }
        assert_eq!(stamp(1_791_856_812), "20261013-020012Z");
        let (d, name, made) = destination("/nonexistent-hjv/", 1_791_856_812).unwrap();
        assert_eq!(
            (d, name.as_str(), made),
            (
                PathBuf::from("/nonexistent-hjv/"),
                "hjv-api-20261013-020012Z.sqlite3",
                true
            )
        );
        let (d, name, made) = destination("/b/before-1.6.1.sqlite3", 0).unwrap();
        assert_eq!(
            (d, name.as_str(), made),
            (PathBuf::from("/b"), "before-1.6.1.sqlite3", false)
        );
        assert_eq!(destination("x.sqlite3", 0).unwrap().0, PathBuf::from("."));
        for bad in [
            "/b/x.db",
            "/b/.x.sqlite3",
            "/b/.sqlite3",
            "/b/x.sqlite3.bak",
        ] {
            assert!(destination(bad, 0).is_err(), "{bad}");
        }
    }

    #[test]
    fn temporary_names() {
        let name = partial_name("hjv-api-20261013-020012Z.sqlite3", &[0xab; 6]);
        assert_eq!(
            name,
            ".hjv-api-20261013-020012Z.sqlite3.abababababab.partial"
        );
        assert!(is_partial(&name));
        assert!(is_partial(&partial_name("b.sqlite3", &random_bytes::<6>())));
        for other in [
            ".b.sqlite3.ababab.partial",
            ".b.sqlite3.ABABABABABAB.partial",
            ".b.sqlite3.abababababag.partial",
            ".sqlite3.abababababab.partial",
            "..b.sqlite3.abababababab.partial",
            ".b.db.abababababab.partial",
            "b.sqlite3.abababababab.partial",
            ".b.sqlite3.abababababab.partial.x",
            ".abababababab.partial",
            ".partial",
            ".x.partial",
        ] {
            assert!(!is_partial(other), "{other}");
        }
    }

    #[test]
    fn directories_are_made_where_backups_go_only() {
        let d = dir("make");
        let uid = effective_uid().unwrap();
        assert_eq!(uid, std::fs::metadata(&d).unwrap().uid());
        // there already: nothing to make or remove
        drop(make_dir(&d).unwrap());
        assert!(d.is_dir());
        // in a directory of ours closed to others: made 0700, gone again unless kept
        let new = d.join("new");
        let made = make_dir(&new).unwrap();
        assert_eq!(std::fs::metadata(&new).unwrap().mode() & 0o777, 0o700);
        drop(made);
        assert!(!new.exists());
        make_dir(&new).unwrap().keep();
        assert!(new.is_dir());
        // a directory made here and not empty stays (the backup is in it)
        let full = d.join("full");
        let made = make_dir(&full).unwrap();
        std::fs::write(full.join("b.sqlite3"), "").unwrap();
        drop(made);
        assert!(full.join("b.sqlite3").exists());
        // not in a directory others may enter, a missing one, or one of another user's
        std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let err = make_dir(&d.join("open")).err().unwrap();
        assert!(err.contains("make it first (install -d -m 700"), "{err}");
        assert!(!d.join("open").exists());
        std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        assert!(make_dir(&d.join("none").join("new")).is_err());
        let root = Path::new("/root");
        if root
            .metadata()
            .is_ok_and(|m| m.mode() & 0o077 == 0 && m.uid() != uid)
        {
            assert!(make_dir(&root.join("hjv-api-backup-test")).is_err());
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn directories() {
        let d = dir("dirs");
        let probe = d.join("probe");
        create_private(&probe).unwrap();
        assert_eq!(check_dir(&d, &probe), Ok(()));
        for mode in [0o750, 0o705, 0o777] {
            std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(mode))
                .unwrap();
            let err = check_dir(&d, &probe).unwrap_err();
            assert!(err.contains("open to others"), "{err}");
        }
        std::fs::set_permissions(&d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
        assert!(check_dir(&probe, &probe)
            .unwrap_err()
            .contains("not a directory"));
        // a directory closed to others but of another owner (root's, where tests do not run as
        // root)
        let root = Path::new("/root");
        if root.metadata().is_ok_and(|m| {
            m.mode() & 0o077 == 0 && m.uid() != std::fs::metadata(&probe).unwrap().uid()
        }) {
            let err = check_dir(root, &probe).unwrap_err();
            assert!(err.contains("belongs to another user"), "{err}");
        }
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn copies_are_standalone_and_checked() {
        let d = dir("finish");
        let live = d.join("live.sqlite3");
        let c = crate::db::open(&live).unwrap();
        c.execute(
            "INSERT INTO allowlist (login_key, added_at) VALUES ('2026000001', 1)",
            [],
        )
        .unwrap();
        let copy = d.join("copy.sqlite3");
        create_private(&copy).unwrap();
        c.execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
            .unwrap();
        let info = finish(&copy).unwrap();
        assert_eq!(info.schema, crate::db::MIGRATIONS.len() as i64);
        assert_eq!(info.counts["allowlist"], 1);
        assert_eq!(info.counts["meta"], 3);
        assert!(!info.counts.contains_key("sqlite_sequence"));
        assert_eq!(hash_file(&copy).unwrap(), (info.sha256.clone(), info.size));
        assert!(is_backup(&copy));
        assert!(!is_backup(&live));
        // no WAL beside it: it opens read-only as it is
        assert!(!d.join("copy.sqlite3-wal").exists() && !d.join("copy.sqlite3-shm").exists());
        assert_eq!(verify(&copy, &info), Ok(()));
        assert!(!d.join("copy.sqlite3-wal").exists() && !d.join("copy.sqlite3-shm").exists());
        let mut other = info.clone();
        other.counts.insert("allowlist".into(), 2);
        assert!(verify(&copy, &other).is_err());
        // a damaged copy fails the check
        let mut bytes = std::fs::read(&copy).unwrap();
        let page = 4096.min(bytes.len() - 8);
        for b in &mut bytes[page..page + 64] {
            *b = 0xff;
        }
        std::fs::write(&copy, &bytes).unwrap();
        assert!(verify(&copy, &info).is_err());
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn pruning_takes_old_backups_only() {
        let d = dir("prune");
        let old = d.join("hjv-api-20260901-033000Z.sqlite3");
        let snapshot = d.join("before-1.6.1.sqlite3");
        let new = d.join("hjv-api-20261013-033000Z.sqlite3");
        for p in [&old, &snapshot, &new] {
            database(p, APPLICATION_ID);
        }
        // old, but not hjv-api backups: another database, a file of another name, a link
        let foreign = d.join("other.sqlite3");
        database(&foreign, 0);
        let text = d.join("notes.txt");
        std::fs::write(&text, "x").unwrap();
        let link = d.join("link.sqlite3");
        std::os::unix::fs::symlink(&old, &link).unwrap();
        let partial = d.join(partial_name(
            "x.sqlite3",
            &[0x01, 0x23, 0x45, 0x67, 0x89, 0xab],
        ));
        std::fs::write(&partial, "").unwrap();
        let fresh_partial = d.join(partial_name("y.sqlite3", &[0; 6]));
        std::fs::write(&fresh_partial, "").unwrap();
        // old hidden files that are not this command's temporary files
        let others: Vec<PathBuf> = [
            ".x.sqlite3.0123.partial",
            ".keep.partial",
            ".x.sqlite3.0123456789AB.partial",
            ".x.db.0123456789ab.partial",
            "..x.sqlite3.0123456789ab.partial",
            "x.sqlite3.0123456789ab.partial",
        ]
        .iter()
        .map(|n| d.join(n))
        .collect();
        for p in &others {
            std::fs::write(p, "").unwrap();
        }
        for p in [&old, &snapshot, &foreign, &text, &partial]
            .into_iter()
            .chain(&others)
        {
            age(p, 20);
        }
        age(&new, 3);
        let pruned = prune(&d, 14, SystemTime::now(), &new).unwrap();
        assert_eq!(
            pruned,
            Pruned {
                backups: 2,
                partial: 1
            }
        );
        assert!(!old.exists() && !snapshot.exists() && !partial.exists());
        for p in [&new, &foreign, &text, &fresh_partial]
            .into_iter()
            .chain(&others)
        {
            assert!(p.exists(), "{}", p.display());
        }
        assert!(link.symlink_metadata().is_ok());
        // the backup just written stays whatever its age
        age(&new, 30);
        assert_eq!(prune(&d, 14, SystemTime::now(), &new).unwrap().backups, 0);
        assert_eq!(prune(&d, 14, SystemTime::now(), &old).unwrap().backups, 1);
        std::fs::remove_dir_all(&d).unwrap();
    }
}
