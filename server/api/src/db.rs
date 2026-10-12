// SPDX-License-Identifier: AGPL-3.0-only

//! The SQLite database (design v2 §9-3, §13-1): allowlist, accounts, token hashes, the devices in
//! each account, login-limit counters, login records and used seal nonces.
//!
//! Deleted data must not be recoverable from the file: `secure_delete` is on, the WAL is
//! truncated after deletions ([`checkpoint`]) and temporary tables stay in memory. The file is
//! created readable by its owner only.
//!
//! Schema changes are migrations in [`MIGRATIONS`]; later phases (password-less proofs, WOL) add
//! their tables with a new entry there.

use crate::util::DAY;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Each entry upgrades the schema by one version.
pub const MIGRATIONS: &[&str] = &[r#"
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;

-- who may log in (D3): case-folded portal IDs
CREATE TABLE allowlist (
    login_key TEXT PRIMARY KEY,
    added_at INTEGER NOT NULL
) STRICT;

CREATE TABLE accounts (
    id INTEGER PRIMARY KEY,
    login_key TEXT NOT NULL UNIQUE,      -- the case-folded portal ID typed at login
    portal_id TEXT NOT NULL,             -- perslNo as the SSO sent it
    display_name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_login_at INTEGER NOT NULL,
    last_active_at INTEGER NOT NULL
) STRICT;

CREATE TABLE tokens (
    hash BLOB PRIMARY KEY,               -- SHA-256 of the token; the token itself is not kept
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id TEXT NOT NULL,             -- the device the login came from ('' if none)
    device_verified INTEGER NOT NULL,    -- 1: that device's signature matched hbbs's key
    bind_status TEXT NOT NULL,           -- what the login did with the device (this-device)
    created_at INTEGER NOT NULL,
    last_used_at INTEGER NOT NULL
) STRICT;
CREATE INDEX tokens_account ON tokens(account_id);

CREATE TABLE devices (
    device_id TEXT PRIMARY KEY,          -- a device belongs to one account at most
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    public_key BLOB NOT NULL,            -- the Ed25519 key hbbs held when it was added
    os TEXT NOT NULL,
    hostname TEXT NOT NULL,
    bound_at INTEGER NOT NULL,
    last_seen_at INTEGER NOT NULL
) STRICT;
CREATE INDEX devices_account ON devices(account_id);

-- login-limit counters (limits.rs): one row per attempt, under a scope and a tag of the address,
-- the ID or the account
CREATE TABLE attempts (
    id INTEGER PRIMARY KEY,
    scope TEXT NOT NULL,
    key TEXT NOT NULL,
    at INTEGER NOT NULL,
    counts INTEGER NOT NULL              -- 0 while the attempt runs, 1 once it counts for good
) STRICT;
CREATE INDEX attempts_key ON attempts(scope, key, at);

-- login and device records (v2 §13-1: account and device as tags, the address, result, time)
CREATE TABLE events (
    id INTEGER PRIMARY KEY,
    at INTEGER NOT NULL,
    kind TEXT NOT NULL,
    account_tag TEXT,
    device_tag TEXT,
    ip TEXT,
    result TEXT NOT NULL
) STRICT;
CREATE INDEX events_at ON events(at);

-- seal nonces seen, until the seal could no longer be fresh (SHA-256 of the nonce)
CREATE TABLE seal_nonces (
    nonce BLOB PRIMARY KEY,
    expires_at INTEGER NOT NULL
) STRICT;
"#];

/// Opens (and creates or upgrades) the database at `path`.
pub fn open(path: &Path) -> Result<Connection, String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if !path.exists() {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    }
    // SQLite gives the WAL and shared-memory files the main file's mode
    let mode = std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("{} is readable by others: {e}", path.display()))?;
        log_notice!("the database was readable by others; now by its owner only");
    }
    let conn =
        Connection::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    prepare(conn)
}

/// A database in memory (tests).
pub fn open_in_memory() -> Result<Connection, String> {
    prepare(Connection::open_in_memory().map_err(|e| e.to_string())?)
}

fn prepare(conn: Connection) -> Result<Connection, String> {
    let setup = || -> rusqlite::Result<()> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "secure_delete", "ON")?;
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "temp_store", "MEMORY")?;
        Ok(())
    };
    setup().map_err(|e| format!("database setup: {e}"))?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> Result<(), String> {
    let version: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let known = MIGRATIONS.len() as i64;
    if version > known {
        return Err(format!(
            "the database has schema {version}; this hjv-api knows up to {known}"
        ));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        tx.execute_batch(sql)
            .and_then(|_| tx.pragma_update(None, "user_version", i as i64 + 1))
            .and_then(|_| tx.commit())
            .map_err(|e| format!("schema upgrade to {}: {e}", i + 1))?;
    }
    Ok(())
}

/// The connection, shared; calls run on tokio's blocking threads one at a time, so each call is
/// atomic with respect to the others.
#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug)]
pub struct DbError(pub String);

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "database: {}", self.0)
    }
}

impl Db {
    pub fn new(conn: Connection) -> Db {
        Db {
            conn: Arc::new(Mutex::new(conn)),
        }
    }

    pub async fn call<R, F>(&self, f: F) -> Result<R, DbError>
    where
        F: FnOnce(&mut Connection) -> rusqlite::Result<R> + Send + 'static,
        R: Send + 'static,
    {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = conn.lock().unwrap_or_else(|e| e.into_inner());
            f(&mut conn)
        })
        .await
        .map_err(|e| DbError(format!("task failed: {e}")))?
        .map_err(|e| DbError(e.to_string()))
    }
}

/// Truncates the WAL so that deleted rows do not linger in it.
pub fn checkpoint(c: &Connection) -> rusqlite::Result<()> {
    c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
}

// ---- allowlist ----

/// Adds case-folded IDs; returns (added, already there).
pub fn allow_add(c: &mut Connection, keys: &[String], at: i64) -> rusqlite::Result<(usize, usize)> {
    let tx = c.transaction()?;
    let mut added = 0;
    for key in keys {
        added += tx.execute(
            "INSERT OR IGNORE INTO allowlist (login_key, added_at) VALUES (?1, ?2)",
            params![key, at],
        )?;
    }
    tx.commit()?;
    Ok((added, keys.len() - added))
}

/// Removes IDs and revokes their accounts' tokens; returns (removed, tokens revoked).
pub fn allow_remove(c: &mut Connection, keys: &[String]) -> rusqlite::Result<(usize, usize)> {
    let tx = c.transaction()?;
    let (mut removed, mut revoked) = (0, 0);
    for key in keys {
        removed += tx.execute("DELETE FROM allowlist WHERE login_key = ?1", [key])?;
        revoked += tx.execute(
            "DELETE FROM tokens WHERE account_id IN (SELECT id FROM accounts WHERE login_key = ?1)",
            [key],
        )?;
    }
    tx.commit()?;
    Ok((removed, revoked))
}

pub fn allowed(c: &Connection, key: &str) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS (SELECT 1 FROM allowlist WHERE login_key = ?1)",
        [key],
        |r| r.get(0),
    )
}

pub fn allowlist(c: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = c.prepare("SELECT login_key, added_at FROM allowlist ORDER BY login_key")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

// ---- accounts ----

#[derive(Debug, Clone)]
pub struct Account {
    pub id: i64,
    pub login_key: String,
    pub portal_id: String,
    pub display_name: String,
    pub created_at: i64,
    pub last_login_at: i64,
    pub last_active_at: i64,
}

const ACCOUNT_COLUMNS: &str =
    "a.id, a.login_key, a.portal_id, a.display_name, a.created_at, a.last_login_at, a.last_active_at";

fn account_row(r: &rusqlite::Row<'_>, at: usize) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(at)?,
        login_key: r.get(at + 1)?,
        portal_id: r.get(at + 2)?,
        display_name: r.get(at + 3)?,
        created_at: r.get(at + 4)?,
        last_login_at: r.get(at + 5)?,
        last_active_at: r.get(at + 6)?,
    })
}

/// Creates or updates the account of a successful login; returns its id.
pub fn upsert_account(
    c: &Connection,
    login_key: &str,
    portal_id: &str,
    display_name: &str,
    at: i64,
) -> rusqlite::Result<i64> {
    c.query_row(
        "INSERT INTO accounts (login_key, portal_id, display_name, created_at, last_login_at, last_active_at)
         VALUES (?1, ?2, ?3, ?4, ?4, ?4)
         ON CONFLICT (login_key) DO UPDATE SET portal_id = excluded.portal_id,
             display_name = excluded.display_name, last_login_at = excluded.last_login_at,
             last_active_at = excluded.last_active_at
         RETURNING id",
        params![login_key, portal_id, display_name, at],
        |r| r.get(0),
    )
}

pub fn accounts(c: &Connection) -> rusqlite::Result<Vec<Account>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {ACCOUNT_COLUMNS} FROM accounts a ORDER BY a.login_key"
    ))?;
    let rows = stmt.query_map([], |r| account_row(r, 0))?;
    rows.collect()
}

pub fn account_by_key(c: &Connection, login_key: &str) -> rusqlite::Result<Option<Account>> {
    c.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts a WHERE a.login_key = ?1"),
        [login_key],
        |r| account_row(r, 0),
    )
    .optional()
}

/// Deletes an account with its tokens and devices.
pub fn delete_account(c: &Connection, account_id: i64) -> rusqlite::Result<bool> {
    Ok(c.execute("DELETE FROM accounts WHERE id = ?1", [account_id])? > 0)
}

// ---- tokens ----

/// A valid token with its account.
#[derive(Debug, Clone)]
pub struct Session {
    pub token_hash: Vec<u8>,
    pub account: Account,
    pub device_id: String,
    pub device_verified: bool,
    pub bind_status: String,
    pub created_at: i64,
    pub last_used_at: i64,
}

/// A token to store: only its hash, with what its login found.
pub struct NewToken<'a> {
    pub hash: &'a [u8],
    pub account_id: i64,
    /// the device the login came from ('' if none)
    pub device_id: &'a str,
    pub device_verified: bool,
    pub bind_status: &'a str,
}

/// Stores a token. The account's older tokens from the same device go (the app keeps one), and of
/// the rest only the `keep` most recently used stay; returns how many went.
pub fn insert_token(
    c: &mut Connection,
    t: &NewToken<'_>,
    at: i64,
    keep: usize,
) -> rusqlite::Result<usize> {
    let tx = c.transaction()?;
    let mut revoked = 0;
    if !t.device_id.is_empty() {
        revoked += tx.execute(
            "DELETE FROM tokens WHERE account_id = ?1 AND device_id = ?2",
            params![t.account_id, t.device_id],
        )?;
    }
    tx.execute(
        "INSERT INTO tokens (hash, account_id, device_id, device_verified, bind_status, created_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![
            t.hash,
            t.account_id,
            t.device_id,
            t.device_verified,
            t.bind_status,
            at
        ],
    )?;
    revoked += tx.execute(
        "DELETE FROM tokens WHERE account_id = ?1 AND rowid NOT IN (
             SELECT rowid FROM tokens WHERE account_id = ?1
             ORDER BY last_used_at DESC, created_at DESC, rowid DESC LIMIT ?2)",
        params![t.account_id, keep as i64],
    )?;
    tx.commit()?;
    Ok(revoked)
}

/// The session of a token hash, if the token is valid: not idle longer than `idle`, not older
/// than `max`, and its account still on the allowlist. An expired token is deleted.
pub fn session(
    c: &Connection,
    hash: &[u8],
    at: i64,
    idle: i64,
    max: i64,
) -> rusqlite::Result<Option<Session>> {
    let row = c
        .query_row(
            &format!(
                "SELECT {ACCOUNT_COLUMNS}, t.device_id, t.device_verified, t.bind_status,
                        t.created_at, t.last_used_at,
                        EXISTS (SELECT 1 FROM allowlist l WHERE l.login_key = a.login_key)
                 FROM tokens t JOIN accounts a ON a.id = t.account_id WHERE t.hash = ?1"
            ),
            [hash],
            |r| {
                Ok((
                    Session {
                        token_hash: hash.to_vec(),
                        account: account_row(r, 0)?,
                        device_id: r.get(7)?,
                        device_verified: r.get(8)?,
                        bind_status: r.get(9)?,
                        created_at: r.get(10)?,
                        last_used_at: r.get(11)?,
                    },
                    r.get::<_, bool>(12)?,
                ))
            },
        )
        .optional()?;
    let Some((session, allowed)) = row else {
        return Ok(None);
    };
    if session.last_used_at <= at - idle || session.created_at <= at - max {
        c.execute("DELETE FROM tokens WHERE hash = ?1", [hash])?;
        return Ok(None);
    }
    Ok(allowed.then_some(session))
}

/// Records that a session was used: the token's idle time, the account's activity and, when the
/// token's device is in the account, that the device was seen (v2 §6). At most once an hour.
pub fn touch_session(c: &Connection, s: &Session, at: i64) -> rusqlite::Result<()> {
    if at - s.last_used_at < 3600 {
        return Ok(());
    }
    c.execute(
        "UPDATE tokens SET last_used_at = ?2 WHERE hash = ?1",
        params![s.token_hash, at],
    )?;
    c.execute(
        "UPDATE accounts SET last_active_at = ?2 WHERE id = ?1",
        params![s.account.id, at],
    )?;
    if s.device_verified {
        c.execute(
            "UPDATE devices SET last_seen_at = MAX(last_seen_at, ?3)
             WHERE device_id = ?1 AND account_id = ?2",
            params![s.device_id, s.account.id, at],
        )?;
    }
    Ok(())
}

pub fn delete_token(c: &Connection, hash: &[u8]) -> rusqlite::Result<bool> {
    Ok(c.execute("DELETE FROM tokens WHERE hash = ?1", [hash])? > 0)
}

pub fn revoke_tokens(c: &Connection, account_id: i64) -> rusqlite::Result<usize> {
    c.execute("DELETE FROM tokens WHERE account_id = ?1", [account_id])
}

pub fn set_bind_status(c: &Connection, hash: &[u8], status: &str) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE tokens SET bind_status = ?2 WHERE hash = ?1",
        params![hash, status],
    )?;
    Ok(())
}

// ---- devices ----

#[derive(Debug, Clone)]
pub struct Device {
    pub device_id: String,
    pub account_id: i64,
    pub public_key: Vec<u8>,
    pub os: String,
    pub hostname: String,
    pub bound_at: i64,
    pub last_seen_at: i64,
}

const DEVICE_COLUMNS: &str =
    "d.device_id, d.account_id, d.public_key, d.os, d.hostname, d.bound_at, d.last_seen_at";

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        device_id: r.get(0)?,
        account_id: r.get(1)?,
        public_key: r.get(2)?,
        os: r.get(3)?,
        hostname: r.get(4)?,
        bound_at: r.get(5)?,
        last_seen_at: r.get(6)?,
    })
}

pub fn device(c: &Connection, device_id: &str) -> rusqlite::Result<Option<Device>> {
    c.query_row(
        &format!("SELECT {DEVICE_COLUMNS} FROM devices d WHERE d.device_id = ?1"),
        [device_id],
        device_row,
    )
    .optional()
}

/// The stored key of `device_id` if it is in the account of `login_key`.
pub fn bound_key(
    c: &Connection,
    device_id: &str,
    login_key: &str,
) -> rusqlite::Result<Option<Vec<u8>>> {
    c.query_row(
        "SELECT d.public_key FROM devices d JOIN accounts a ON a.id = d.account_id
         WHERE d.device_id = ?1 AND a.login_key = ?2",
        [device_id, login_key],
        |r| r.get(0),
    )
    .optional()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bound {
    Added,
    Already,
    OtherAccount,
}

/// Puts a verified device into an account, unless another account has it (design v2 §5: a
/// conflict is refused, not settled silently).
pub fn bind(
    c: &mut Connection,
    account_id: i64,
    device_id: &str,
    public_key: &[u8],
    os: &str,
    hostname: &str,
    at: i64,
) -> rusqlite::Result<Bound> {
    let tx = c.transaction()?;
    let owner: Option<i64> = tx
        .query_row(
            "SELECT account_id FROM devices WHERE device_id = ?1",
            [device_id],
            |r| r.get(0),
        )
        .optional()?;
    let result = match owner {
        Some(owner) if owner != account_id => Bound::OtherAccount,
        Some(_) => {
            tx.execute(
                "UPDATE devices SET public_key = ?2, os = ?3, hostname = ?4, last_seen_at = ?5
                 WHERE device_id = ?1",
                params![device_id, public_key, os, hostname, at],
            )?;
            Bound::Already
        }
        None => {
            tx.execute(
                "INSERT INTO devices (device_id, account_id, public_key, os, hostname, bound_at, last_seen_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![device_id, account_id, public_key, os, hostname, at],
            )?;
            Bound::Added
        }
    };
    tx.commit()?;
    Ok(result)
}

/// Takes a device out of an account; false if it is not in that account.
pub fn unbind(c: &Connection, device_id: &str, account_id: i64) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "DELETE FROM devices WHERE device_id = ?1 AND account_id = ?2",
        params![device_id, account_id],
    )? > 0)
}

pub fn unbind_any(c: &Connection, device_id: &str) -> rusqlite::Result<bool> {
    Ok(c.execute("DELETE FROM devices WHERE device_id = ?1", [device_id])? > 0)
}

pub fn devices_of(c: &Connection, account_id: i64) -> rusqlite::Result<Vec<Device>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices d WHERE d.account_id = ?1
         ORDER BY d.hostname COLLATE NOCASE, d.device_id"
    ))?;
    let rows = stmt.query_map([account_id], device_row)?;
    rows.collect()
}

pub fn all_devices(c: &Connection) -> rusqlite::Result<Vec<Device>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices d ORDER BY d.device_id"
    ))?;
    let rows = stmt.query_map([], device_row)?;
    rows.collect()
}

/// hbbs saw the device `secs_ago` seconds ago with `public_key`: counts as seen when the key is
/// the one stored (v2 §6).
pub fn seen_by_hbbs(
    c: &Connection,
    device_id: &str,
    public_key: &[u8],
    seen_at: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE devices SET last_seen_at = MAX(last_seen_at, ?3)
         WHERE device_id = ?1 AND public_key = ?2",
        params![device_id, public_key, seen_at],
    )? > 0)
}

// ---- records ----

/// A login or device record (v2 §13-1): the account and the device as tags, the client address,
/// the result and the time.
#[derive(Debug, Clone)]
pub struct Record {
    pub at: i64,
    pub kind: &'static str,
    pub account_tag: Option<String>,
    pub device_tag: Option<String>,
    pub ip: Option<String>,
    pub result: &'static str,
}

pub fn insert_record(c: &Connection, r: &Record) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO events (at, kind, account_tag, device_tag, ip, result) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![r.at, r.kind, r.account_tag, r.device_tag, r.ip, r.result],
    )?;
    Ok(())
}

/// Records a seal nonce until `expires_at`; false if it was seen before and has not expired at
/// `at`. One statement, so one write.
pub fn use_nonce(
    c: &Connection,
    nonce_hash: &[u8],
    at: i64,
    expires_at: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "INSERT INTO seal_nonces (nonce, expires_at) VALUES (?1, ?2)
         ON CONFLICT (nonce) DO UPDATE SET expires_at = excluded.expires_at
         WHERE seal_nonces.expires_at < ?3",
        params![nonce_hash, expires_at, at],
    )? == 1)
}

// ---- retention (v2 §13-1) ----

pub struct Retention {
    pub token_idle: i64,
    pub token_max: i64,
    pub account: i64,
    pub device_unseen: i64,
    pub events: i64,
    /// login-limit counters: the longest window of the limits, or a day for the report
    pub attempts: i64,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Purged {
    pub tokens: usize,
    pub devices: usize,
    pub accounts: usize,
    pub events: usize,
    pub other: usize,
}

impl Purged {
    pub fn any(&self) -> bool {
        self.tokens + self.devices + self.accounts + self.events + self.other > 0
    }
}

/// Deletes what is past its keeping time, then truncates the WAL if anything went.
///
/// An account unused for the account time goes with its devices and tokens, unless it is still on
/// the allowlist and one of its PCs was seen within the device time: a lab PC logged in once stays
/// listed while it is in use, whether or not its owner opens the app (v2 §5 lists the ways a PC
/// leaves the list; the account's own expiry is not one of them).
pub fn purge(c: &Connection, at: i64, r: &Retention) -> rusqlite::Result<Purged> {
    let p = Purged {
        tokens: c.execute(
            "DELETE FROM tokens WHERE last_used_at <= ?1 OR created_at <= ?2",
            params![at - r.token_idle, at - r.token_max],
        )?,
        devices: c.execute(
            "DELETE FROM devices WHERE last_seen_at <= ?1",
            [at - r.device_unseen],
        )?,
        accounts: c.execute(
            "DELETE FROM accounts WHERE last_active_at <= ?1 AND NOT (
                 EXISTS (SELECT 1 FROM allowlist l WHERE l.login_key = accounts.login_key)
                 AND EXISTS (SELECT 1 FROM devices d
                             WHERE d.account_id = accounts.id AND d.last_seen_at > ?2))",
            params![at - r.account, at - r.device_unseen],
        )?,
        events: c.execute("DELETE FROM events WHERE at <= ?1", [at - r.events])?,
        other: c.execute("DELETE FROM seal_nonces WHERE expires_at < ?1", [at])?
            + c.execute("DELETE FROM attempts WHERE at <= ?1", [at - r.attempts])?,
    };
    if p.any() {
        checkpoint(c)?;
    }
    Ok(p)
}

// ---- report ----

#[derive(Debug, Default)]
pub struct Counts {
    pub allowlist: i64,
    pub accounts: i64,
    pub devices: i64,
    pub tokens: i64,
    /// login results over the last day and the last week: (result, day, week)
    pub results: Vec<(String, i64, i64)>,
}

pub fn counts(c: &Connection, at: i64) -> rusqlite::Result<Counts> {
    let one = |sql: &str| -> rusqlite::Result<i64> { c.query_row(sql, [], |r| r.get(0)) };
    let mut stmt = c.prepare(
        "SELECT kind || ' ' || result, SUM(at > ?1), COUNT(*) FROM events WHERE at > ?2
         GROUP BY kind, result ORDER BY kind, result",
    )?;
    let results = stmt
        .query_map(params![at - DAY, at - 7 * DAY], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Counts {
        allowlist: one("SELECT COUNT(*) FROM allowlist")?,
        accounts: one("SELECT COUNT(*) FROM accounts")?,
        devices: one("SELECT COUNT(*) FROM devices")?,
        tokens: one("SELECT COUNT(*) FROM tokens")?,
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> Connection {
        open_in_memory().unwrap()
    }

    fn token<'a>(hash: &'a [u8], account_id: i64, device_id: &'a str) -> NewToken<'a> {
        NewToken {
            hash,
            account_id,
            device_id,
            device_verified: !device_id.is_empty(),
            bind_status: if device_id.is_empty() {
                "none"
            } else {
                "added"
            },
        }
    }

    fn record(at: i64) -> Record {
        Record {
            at,
            kind: "login",
            account_tag: None,
            device_tag: None,
            ip: None,
            result: "ok",
        }
    }

    #[test]
    fn schema_and_files() {
        let dir = std::env::temp_dir().join(format!("hjv-api-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.sqlite3");
        let c = open(&path).unwrap();
        drop(c);
        // opening again keeps the schema
        let c = open(&path).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        let mode: String = c
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        let sd: i64 = c
            .query_row("PRAGMA secure_delete", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sd, 1);
        use std::os::unix::fs::PermissionsExt;
        let perm = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(perm, 0o600);
        c.pragma_update(None, "user_version", 99).unwrap();
        drop(c);
        assert!(open(&path).is_err());
        // a file others can read is closed to them
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = open(&path);
        let perm = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(perm, 0o600);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn allowlist_and_tokens() {
        let mut c = conn();
        let keys = vec!["a1".to_owned(), "b2".to_owned()];
        assert_eq!(allow_add(&mut c, &keys, 1).unwrap(), (2, 0));
        assert_eq!(allow_add(&mut c, &keys, 1).unwrap(), (0, 2));
        let id = upsert_account(&c, "a1", "A1", "홍길동", 10).unwrap();
        assert_eq!(upsert_account(&c, "a1", "A1", "홍길동2", 20).unwrap(), id);
        insert_token(&mut c, &token(b"h1", id, "123"), 100, 20).unwrap();
        let s = session(&c, b"h1", 200, 1000, 5000).unwrap().unwrap();
        assert_eq!(s.account.display_name, "홍길동2");
        assert!(s.device_verified);
        // idle and maximum age
        assert!(session(&c, b"h1", 1100, 1000, 5000).unwrap().is_none());
        assert!(
            session(&c, b"h1", 200, 1000, 5000).unwrap().is_none(),
            "expired tokens go"
        );
        insert_token(&mut c, &token(b"h2", id, ""), 100, 20).unwrap();
        assert!(session(&c, b"h2", 5100, 10_000, 5000).unwrap().is_none());
        // off the allowlist: no session, tokens revoked
        insert_token(&mut c, &token(b"h3", id, ""), 100, 20).unwrap();
        assert!(session(&c, b"h3", 200, 1000, 5000).unwrap().is_some());
        assert_eq!(allow_remove(&mut c, &["a1".to_owned()]).unwrap(), (1, 1));
        assert!(session(&c, b"h3", 200, 1000, 5000).unwrap().is_none());
        assert!(!allowed(&c, "a1").unwrap());
        assert!(allowed(&c, "b2").unwrap());
    }

    #[test]
    fn tokens_per_device_and_account() {
        let mut c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        let b = upsert_account(&c, "b", "B", "", 1).unwrap();
        let hashes = |c: &Connection| -> Vec<Vec<u8>> {
            let mut s = c.prepare("SELECT hash FROM tokens ORDER BY hash").unwrap();
            let rows = s.query_map([], |r| r.get(0)).unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        // a new login from a device replaces that device's token in the same account only
        assert_eq!(
            insert_token(&mut c, &token(b"1", a, "pc"), 10, 3).unwrap(),
            0
        );
        assert_eq!(
            insert_token(&mut c, &token(b"2", b, "pc"), 11, 3).unwrap(),
            0
        );
        assert_eq!(
            insert_token(&mut c, &token(b"3", a, "pc"), 12, 3).unwrap(),
            1
        );
        assert_eq!(hashes(&c), vec![b"2".to_vec(), b"3".to_vec()]);
        // logins without a device ID replace nothing; past the cap the least recently used go
        insert_token(&mut c, &token(b"4", a, ""), 13, 3).unwrap();
        insert_token(&mut c, &token(b"5", a, ""), 14, 3).unwrap();
        c.execute("UPDATE tokens SET last_used_at = 20 WHERE hash = X'34'", [])
            .unwrap();
        assert_eq!(insert_token(&mut c, &token(b"6", a, ""), 15, 3).unwrap(), 1);
        assert_eq!(
            hashes(&c),
            vec![b"2".to_vec(), b"4".to_vec(), b"5".to_vec(), b"6".to_vec()]
        );
    }

    #[test]
    fn devices_bind_once() {
        let mut c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        let b = upsert_account(&c, "b", "B", "", 1).unwrap();
        assert_eq!(
            bind(&mut c, a, "1", &[1; 32], "windows", "PC", 5).unwrap(),
            Bound::Added
        );
        assert_eq!(
            bind(&mut c, a, "1", &[2; 32], "windows", "PC2", 6).unwrap(),
            Bound::Already
        );
        assert_eq!(
            bind(&mut c, b, "1", &[2; 32], "windows", "X", 7).unwrap(),
            Bound::OtherAccount
        );
        let d = device(&c, "1").unwrap().unwrap();
        assert_eq!(
            (d.account_id, d.hostname.as_str(), d.public_key),
            (a, "PC2", vec![2; 32])
        );
        assert_eq!(bound_key(&c, "1", "a").unwrap(), Some(vec![2; 32]));
        assert_eq!(bound_key(&c, "1", "b").unwrap(), None);
        assert!(!unbind(&c, "1", b).unwrap());
        assert!(seen_by_hbbs(&c, "1", &[2; 32], 50).unwrap());
        assert!(!seen_by_hbbs(&c, "1", &[3; 32], 60).unwrap());
        assert_eq!(device(&c, "1").unwrap().unwrap().last_seen_at, 50);
        assert!(unbind(&c, "1", a).unwrap());
        assert!(device(&c, "1").unwrap().is_none());
        // deleting an account takes its devices and tokens
        bind(&mut c, a, "2", &[1; 32], "windows", "PC", 5).unwrap();
        insert_token(&mut c, &token(b"t", a, "2"), 5, 20).unwrap();
        assert!(delete_account(&c, a).unwrap());
        assert!(device(&c, "2").unwrap().is_none());
        assert!(!delete_token(&c, b"t").unwrap());
    }

    fn retention() -> Retention {
        Retention {
            token_idle: 90 * DAY,
            token_max: 365 * DAY,
            account: 365 * DAY,
            device_unseen: 180 * DAY,
            events: 183 * DAY,
            attempts: DAY,
        }
    }

    #[test]
    fn nonces_and_retention() {
        let mut c = conn();
        let t = crate::util::now();
        assert!(use_nonce(&c, b"n", t, t + 100).unwrap());
        assert!(!use_nonce(&c, b"n", t + 50, t + 150).unwrap());
        assert!(!use_nonce(&c, b"n", t + 100, t + 150).unwrap());
        assert!(use_nonce(&c, b"n", t + 101, t + 200).unwrap());
        assert!(!use_nonce(&c, b"n", t + 102, t + 200).unwrap());
        let a = upsert_account(&c, "a", "A", "", t - 400 * DAY).unwrap();
        let b = upsert_account(&c, "b", "B", "", t).unwrap();
        bind(&mut c, b, "1", &[1; 32], "windows", "PC", t - 200 * DAY).unwrap();
        bind(&mut c, b, "2", &[1; 32], "windows", "PC", t - DAY).unwrap();
        insert_token(&mut c, &token(b"old", b, ""), t - 100 * DAY, 20).unwrap();
        insert_token(&mut c, &token(b"new", b, ""), t, 20).unwrap();
        insert_record(&c, &record(t - 200 * DAY)).unwrap();
        insert_record(&c, &record(t)).unwrap();
        for (scope, at) in [("id", t - 2 * DAY), ("id", t - 3600)] {
            c.execute(
                "INSERT INTO attempts (scope, key, at, counts) VALUES (?1, 'k', ?2, 1)",
                params![scope, at],
            )
            .unwrap();
        }
        let p = purge(&c, t, &retention()).unwrap();
        assert_eq!((p.tokens, p.devices, p.accounts, p.events), (1, 1, 1, 1));
        assert!(account_by_key(&c, "a").unwrap().is_none());
        assert!(device(&c, "2").unwrap().is_some());
        let _ = a;
        let left: i64 = c
            .query_row("SELECT COUNT(*) FROM attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1, "counters older than the longest window go");
        let counts = counts(&c, t).unwrap();
        assert_eq!((counts.accounts, counts.devices, counts.tokens), (1, 1, 1));
        assert_eq!(counts.results, vec![("login ok".to_owned(), 1, 1)]);
    }

    #[test]
    fn an_account_stays_while_its_pc_is_seen() {
        let mut c = conn();
        let t = crate::util::now();
        let old = t - 400 * DAY;
        allow_add(&mut c, &["lab".to_owned(), "gone".to_owned()], old).unwrap();
        // unused for over a year, its PC seen by hbbs yesterday: kept
        let lab = upsert_account(&c, "lab", "LAB", "", old).unwrap();
        bind(&mut c, lab, "1", &[1; 32], "windows", "PC", old).unwrap();
        assert!(seen_by_hbbs(&c, "1", &[1; 32], t - DAY).unwrap());
        // the same, but off the allowlist since: deleted with its PC
        let gone = upsert_account(&c, "gone", "GONE", "", old).unwrap();
        bind(&mut c, gone, "2", &[1; 32], "windows", "PC", old).unwrap();
        assert!(seen_by_hbbs(&c, "2", &[1; 32], t - DAY).unwrap());
        allow_remove(&mut c, &["gone".to_owned()]).unwrap();
        // on the list, its PC not seen for 200 days: the PC goes, then the account
        let idle = upsert_account(&c, "idle", "IDLE", "", old).unwrap();
        allow_add(&mut c, &["idle".to_owned()], old).unwrap();
        bind(&mut c, idle, "3", &[1; 32], "windows", "PC", t - 200 * DAY).unwrap();
        let p = purge(&c, t, &retention()).unwrap();
        assert_eq!((p.accounts, p.devices), (2, 1));
        assert!(account_by_key(&c, "lab").unwrap().is_some());
        assert!(device(&c, "1").unwrap().is_some());
        assert!(account_by_key(&c, "gone").unwrap().is_none());
        assert!(device(&c, "2").unwrap().is_none());
        assert!(account_by_key(&c, "idle").unwrap().is_none());
    }
}
