// SPDX-License-Identifier: AGPL-3.0-only

//! The SQLite database (design v2 §9-3, §13-1): allowlist, accounts, token hashes, the devices in
//! each account, login-limit counters, login records and used seal nonces; since schema 2 (design
//! v3 §5-4) the account keys, the nonces of signed account requests, the password-less switches
//! and the change serial.
//!
//! Deleted data must not be recoverable from the file: `secure_delete` is on, the WAL is
//! truncated after deletions ([`checkpoint`]) and temporary tables stay in memory. The file is
//! created readable by its owner only.
//!
//! Schema changes are migrations in [`MIGRATIONS`]; later phases (WOL) add their tables with a new
//! entry there.
//!
//! Every change that alters what a PC's trust list would say (a key registered or revoked, a
//! switch turned, an account deleted, leaving or coming back on the allowlist, a binding released,
//! a PC's key found changed or back) raises the change serial ([`bump_serial`]) in the same
//! transaction; a new key carries the serial its registration made (design v3 §3-5, §5-2).
//!
//! A PC leaves an account in one place only, [`release_binding`]: its row goes, its binding ID
//! goes to the tombstones, so that the PC's next sync gets a signed `revoked` (design v3 §4-3),
//! and the change serial goes up, in one transaction. Nothing else deletes a `devices` row, and an
//! account is deleted only by [`delete_account`], which releases its PCs first (the `ON DELETE
//! CASCADE` of `devices` would leave no tombstone).

use crate::util::DAY;
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// Each entry upgrades the schema by one version.
pub const MIGRATIONS: &[&str] = &[
    r#"
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
"#,
    // Schema 2 (design v3 §5-4, "이전 2번"). Part A, the account keys (WP-S1); part B (the PC
    // sync of WP-S2) adds its columns and tables at the end of this same entry.
    r#"
-- one Ed25519 account key per login of a device (design v3 §4-1): it signs every account request
-- (proof.rs) and the same-account connection proofs that the PCs check against their trust lists
CREATE TABLE ctl_keys (
    ckid INTEGER PRIMARY KEY AUTOINCREMENT,  -- never reused: the apps name keys by it
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    pk BLOB NOT NULL UNIQUE,             -- the 32-byte public key from the sealed login
    dev_tag TEXT NOT NULL,               -- tag of the app's uuid (log.rs), '' without one; never the uuid
    ctl_id TEXT NOT NULL,                -- the device ID the login came from ('' if none)
    platform TEXT NOT NULL,              -- deviceInfo.os as hjv-api took it (devmodel.rs)
    model TEXT NOT NULL,                 -- the device name hjv-api chose (devmodel.rs)
    app_name TEXT NOT NULL,              -- the device name the app sent (deviceInfo.name)
    app_ver TEXT NOT NULL,               -- the app version the login said ('' if none)
    login_net TEXT NOT NULL,             -- 'on-campus', 'off-campus' or '' (no address is kept)
    key_serial INTEGER NOT NULL,         -- the change serial its registration made
    created_at INTEGER NOT NULL,
    last_login_at INTEGER NOT NULL,
    ack_at INTEGER,                      -- when one of the account's devices first confirmed it
    ack_by INTEGER,                      -- the oldest key that confirmed it (its ckid; keys.rs `new`)
    revoked_at INTEGER,
    revoke_reason TEXT
) STRICT;
CREATE INDEX ctl_keys_account ON ctl_keys(account_id);

-- nonces of signed account requests that change something, per key, for 20 minutes (proof.rs)
CREATE TABLE req_nonces (
    ckid INTEGER NOT NULL REFERENCES ctl_keys(ckid) ON DELETE CASCADE,
    nonce BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (ckid, nonce)
) STRICT, WITHOUT ROWID;

-- a token belongs to the account key of its login; the tokens of schema 1 have none and could
-- only fail now that every account request is signed (the apps log in again)
DELETE FROM tokens;
ALTER TABLE tokens ADD COLUMN ckid INTEGER REFERENCES ctl_keys(ckid) ON DELETE CASCADE;
CREATE INDEX tokens_key ON tokens(ckid);

-- the account's password-less switch: 'on', 'off', or NULL for the global one (meta)
ALTER TABLE accounts ADD COLUMN passwordless TEXT;

-- 128 random bits, new with each binding (the same account logging in again keeps it), and the
-- last login on the PC whose device signature hjv-api checked (server time)
ALTER TABLE devices ADD COLUMN binding_id BLOB;
ALTER TABLE devices ADD COLUMN login_at INTEGER;
UPDATE devices SET binding_id = randomblob(16);
CREATE UNIQUE INDEX devices_binding ON devices(binding_id);

-- the change serial, the global password-less switch (off when installed) and 'connection by
-- connection' checks of the PCs (strict, for incidents)
INSERT OR IGNORE INTO meta (key, value)
    VALUES ('change_serial', '0'), ('passwordless', 'off'), ('strict', 'off');

-- Part B, the PC sync (WP-S2, design v3 §3-6, §4-5, §5-2): what each bound PC said in its last
-- sync signed with its device key. The binding ID and login time above are part A.
ALTER TABLE devices ADD COLUMN activated_at INTEGER;   -- its first sync that carried the binding
ALTER TABLE devices ADD COLUMN last_sync_at INTEGER;   -- overwritten by every sync
ALTER TABLE devices ADD COLUMN app_ver TEXT;           -- the app version the sync said
ALTER TABLE devices ADD COLUMN off INTEGER;            -- 1: switched off on the PC
ALTER TABLE devices ADD COLUMN sync_state TEXT;        -- the PC's mark: 'present', 'absent', 'stale'
ALTER TABLE devices ADD COLUMN key_changed_at INTEGER; -- needs-relogin: hbbs holds another key

-- released bindings, so that the PC gets a signed 'revoked' (design v3 §4-3): kept 180 days,
-- written by release_binding only; `reason` is the way it was released, for the report
CREATE TABLE binding_tombstones (
    binding_id BLOB NOT NULL PRIMARY KEY,
    device_id TEXT NOT NULL,
    public_key BLOB NOT NULL,            -- the PC's key when it was released
    revoked_at INTEGER NOT NULL,
    reason TEXT NOT NULL
) STRICT;
CREATE INDEX binding_tombstones_device ON binding_tombstones(device_id, revoked_at);

-- the PCs' reports of password-less connections they let in (U6): per account key, PC and UTC
-- day the latest time and that day's count, each merged by taking the larger value. Kept 30 days
-- and deleted with the key (cascade) or the PC (release_binding), design v3 §10-1. No row per
-- connection.
CREATE TABLE key_use (
    ckid INTEGER NOT NULL REFERENCES ctl_keys(ckid) ON DELETE CASCADE,
    device_id TEXT NOT NULL,
    day TEXT NOT NULL,                   -- 'YYYY-MM-DD', UTC
    last_used_at INTEGER NOT NULL,
    n INTEGER NOT NULL,
    PRIMARY KEY (ckid, device_id, day)
) STRICT, WITHOUT ROWID;
CREATE INDEX key_use_day ON key_use(day);

-- the PCs' counts of refused proofs by reason, per server day; counts only, 30 days
CREATE TABLE pc_refusals (
    device_id TEXT NOT NULL REFERENCES devices(device_id) ON DELETE CASCADE,
    reason TEXT NOT NULL,
    day TEXT NOT NULL,
    count INTEGER NOT NULL,
    PRIMARY KEY (device_id, reason, day)
) STRICT, WITHOUT ROWID;

-- the (id, n) of accepted sync requests, for 600 seconds (envelope.rs)
CREATE TABLE pc_nonces (
    id TEXT NOT NULL,
    nonce BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (id, nonce)
) STRICT, WITHOUT ROWID;
"#,
];

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
    // a restored backup carries the backups' application ID (backup.rs); the database served is
    // never taken for a backup that `--keep-days` may delete
    let application_id: i32 = conn
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(|e| format!("database setup: {e}"))?;
    if application_id == crate::backup::APPLICATION_ID {
        conn.pragma_update(None, "application_id", 0)
            .map_err(|e| format!("database setup: {e}"))?;
    }
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

/// The first column of a query's rows.
fn column<T: rusqlite::types::FromSql>(
    c: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> rusqlite::Result<Vec<T>> {
    let mut stmt = c.prepare(sql)?;
    let rows = stmt.query_map(params, |r| r.get(0))?;
    rows.collect()
}

/// Runs `f` in one transaction: all of its changes or none.
pub fn in_tx<R>(
    c: &mut Connection,
    f: impl FnOnce(&Connection) -> rusqlite::Result<R>,
) -> rusqlite::Result<R> {
    let tx = c.transaction()?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

// ---- the change serial and the switches (design v3 §5-4, §5-6) ----

/// The change serial: it goes up with every change to what a trust list would say.
pub fn change_serial(c: &Connection) -> rusqlite::Result<i64> {
    Ok(c.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'change_serial'",
        [],
        |r| r.get(0),
    )
    .optional()?
    .unwrap_or(0))
}

/// Raises the change serial by one; returns the new value.
pub fn bump_serial(c: &Connection) -> rusqlite::Result<i64> {
    c.query_row(
        "INSERT INTO meta (key, value) VALUES ('change_serial', '1')
         ON CONFLICT (key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)
         RETURNING CAST(value AS INTEGER)",
        [],
        |r| r.get(0),
    )
}

fn flag(c: &Connection, key: &str) -> rusqlite::Result<bool> {
    let value: Option<String> = c
        .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
        .optional()?;
    Ok(value.as_deref() == Some("on"))
}

fn set_flag(c: &Connection, key: &str, on: bool) -> rusqlite::Result<bool> {
    let value = if on { "on" } else { "off" };
    let changed = c.execute(
        "INSERT INTO meta (key, value) VALUES (?1, ?2)
         ON CONFLICT (key) DO UPDATE SET value = excluded.value WHERE meta.value <> excluded.value",
        [key, value],
    )? > 0;
    if changed {
        bump_serial(c)?;
        switched(c)?;
    }
    Ok(changed)
}

/// Notes the time of a switch change (a password-less switch, strict, an account leaving or
/// coming back on the allowlist): the report counts the PCs that have not synced since, which
/// hjv-api must stay up for after an incident switch (design v3 §5-6).
fn switched(c: &Connection) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO meta (key, value) VALUES ('switched_at', CAST(strftime('%s', 'now') AS TEXT))
         ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        [],
    )?;
    Ok(())
}

/// When a switch last changed, if ever.
pub fn switched_at(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = 'switched_at'",
        [],
        |r| r.get(0),
    )
    .optional()
}

/// The global password-less switch (off when installed).
pub fn passwordless_global(c: &Connection) -> rusqlite::Result<bool> {
    flag(c, "passwordless")
}

/// Turns the global switch. Off is the incident switch (design v3 §5-3, §11): it also clears the
/// accounts switched on one by one, so that afterwards no account is on. On leaves the accounts
/// switched off one by one as they are. Returns whether anything changed.
pub fn set_passwordless_global(c: &Connection, on: bool) -> rusqlite::Result<bool> {
    let mut changed = set_flag(c, "passwordless", on)?;
    if !on {
        let cleared = c.execute(
            "UPDATE accounts SET passwordless = NULL WHERE passwordless = 'on'",
            [],
        )?;
        if cleared > 0 && !changed {
            bump_serial(c)?;
            switched(c)?;
        }
        changed |= cleared > 0;
    }
    Ok(changed)
}

/// An account's own switch: `Some(on)`, or `None` to follow the global one.
pub fn account_passwordless(c: &Connection, account_id: i64) -> rusqlite::Result<Option<bool>> {
    let value: Option<Option<String>> = c
        .query_row(
            "SELECT passwordless FROM accounts WHERE id = ?1",
            [account_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(value.flatten().map(|v| v == "on"))
}

/// Sets an account's own switch (`None`: follow the global one); returns whether it changed.
pub fn set_account_passwordless(
    c: &Connection,
    account_id: i64,
    on: Option<bool>,
) -> rusqlite::Result<bool> {
    let value = on.map(|on| if on { "on" } else { "off" });
    let changed = c.execute(
        "UPDATE accounts SET passwordless = ?2 WHERE id = ?1 AND passwordless IS NOT ?2",
        params![account_id, value],
    )? > 0;
    if changed {
        bump_serial(c)?;
        switched(c)?;
    }
    Ok(changed)
}

/// Whether password-less connections are on for an account: its own switch, else the global one.
pub fn passwordless_on(c: &Connection, account_id: i64) -> rusqlite::Result<bool> {
    match account_passwordless(c, account_id)? {
        Some(on) => Ok(on),
        None => passwordless_global(c),
    }
}

/// Whether the trust lists of an account's PCs are `active` (design v3 §5-2): the account is on
/// the allowlist and password-less connections are on for it. Off the allowlist, its PCs stay
/// bound with `disabled` lists without keys (§5-6). The PC sync and `admin binding list` both
/// judge by it.
pub fn list_on(c: &Connection, account_id: i64) -> rusqlite::Result<bool> {
    let listed: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM accounts a JOIN allowlist l ON l.login_key = a.login_key
                        WHERE a.id = ?1)",
        [account_id],
        |r| r.get(0),
    )?;
    Ok(listed && passwordless_on(c, account_id)?)
}

/// 'Connection by connection' checks of the PCs, for incidents (design v3 §3-5 step 7, §5-6).
pub fn strict(c: &Connection) -> rusqlite::Result<bool> {
    flag(c, "strict")
}

pub fn set_strict(c: &Connection, on: bool) -> rusqlite::Result<bool> {
    set_flag(c, "strict", on)
}

// ---- allowlist ----

/// Adds case-folded IDs; returns (added, already there). An account that comes back on the list
/// changes its PCs' trust lists (design v3 §5-2): the change serial goes up.
pub fn allow_add(c: &mut Connection, keys: &[String], at: i64) -> rusqlite::Result<(usize, usize)> {
    in_tx(c, |tx| {
        let (mut added, mut relisted) = (0, false);
        for key in keys {
            let n = tx.execute(
                "INSERT OR IGNORE INTO allowlist (login_key, added_at) VALUES (?1, ?2)",
                params![key, at],
            )?;
            added += n;
            if n > 0 && !relisted {
                relisted = tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM accounts WHERE login_key = ?1)",
                    [key],
                    |r| r.get(0),
                )?;
            }
        }
        if relisted {
            bump_serial(tx)?;
            switched(tx)?;
        }
        Ok((added, keys.len() - added))
    })
}

/// Removes IDs and revokes their accounts' tokens and account keys (design v3 §5-6; their PCs
/// stay bound, with lists without keys: §5-2, so the change serial goes up, keys or not);
/// returns (removed, tokens revoked, keys revoked).
pub fn allow_remove(
    c: &mut Connection,
    keys: &[String],
    at: i64,
) -> rusqlite::Result<(usize, usize, usize)> {
    in_tx(c, |tx| {
        let (mut removed, mut tokens, mut revoked, mut delisted) = (0, 0, 0, false);
        for key in keys {
            let n = tx.execute("DELETE FROM allowlist WHERE login_key = ?1", [key])?;
            removed += n;
            let account: Option<i64> = tx
                .query_row("SELECT id FROM accounts WHERE login_key = ?1", [key], |r| {
                    r.get(0)
                })
                .optional()?;
            if let Some(account_id) = account {
                tokens += revoke_tokens(tx, account_id)?;
                revoked += revoke_account_keys(tx, account_id, "allow-remove", at)?;
                delisted |= n > 0;
            }
        }
        // revoking keys raised it already
        if delisted && revoked == 0 {
            bump_serial(tx)?;
        }
        if delisted {
            switched(tx)?;
        }
        Ok((removed, tokens, revoked))
    })
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

pub fn account(c: &Connection, account_id: i64) -> rusqlite::Result<Option<Account>> {
    c.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts a WHERE a.id = ?1"),
        [account_id],
        |r| account_row(r, 0),
    )
    .optional()
}

pub fn account_by_key(c: &Connection, login_key: &str) -> rusqlite::Result<Option<Account>> {
    c.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM accounts a WHERE a.login_key = ?1"),
        [login_key],
        |r| account_row(r, 0),
    )
    .optional()
}

/// Deletes an account (design v3 §5-4): each of its PCs leaves through [`release_binding`]
/// first (reason `account-delete`), so that it gets a signed `revoked`; then the account row
/// goes and with it (cascade) its tokens, account keys, their nonces and the key use reported for
/// them. Inside the caller's transaction. What stays is the tombstones and the operator record.
pub fn delete_account(c: &Connection, account_id: i64, at: i64) -> rusqlite::Result<bool> {
    let pcs: Vec<String> = column(
        c,
        "SELECT device_id FROM devices WHERE account_id = ?1",
        [account_id],
    )?;
    for device_id in pcs {
        release_binding(c, &device_id, "account-delete", at)?;
    }
    let deleted = c.execute("DELETE FROM accounts WHERE id = ?1", [account_id])? > 0;
    if deleted {
        bump_serial(c)?;
    }
    Ok(deleted)
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
    /// the account key of the token's login, while it is not revoked
    pub key: Option<SessionKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionKey {
    pub ckid: i64,
    pub public_key: [u8; 32],
}

/// A token to store: only its hash, with what its login found.
pub struct NewToken<'a> {
    pub hash: &'a [u8],
    pub account_id: i64,
    /// the device the login came from ('' if none)
    pub device_id: &'a str,
    pub device_verified: bool,
    pub bind_status: &'a str,
    /// the account key registered by the same login
    pub ckid: Option<i64>,
}

/// Stores a token. The account's older tokens from the same device go (the app keeps one), and of
/// the rest only the `keep` most recently used stay; returns how many went.
pub fn insert_token(
    c: &mut Connection,
    t: &NewToken<'_>,
    at: i64,
    keep: usize,
) -> rusqlite::Result<usize> {
    in_tx(c, |tx| insert_token_tx(tx, t, at, keep))
}

/// [`insert_token`] inside the caller's transaction.
pub fn insert_token_tx(
    c: &Connection,
    t: &NewToken<'_>,
    at: i64,
    keep: usize,
) -> rusqlite::Result<usize> {
    let mut revoked = 0;
    if !t.device_id.is_empty() {
        revoked += c.execute(
            "DELETE FROM tokens WHERE account_id = ?1 AND device_id = ?2",
            params![t.account_id, t.device_id],
        )?;
    }
    c.execute(
        "INSERT INTO tokens (hash, account_id, device_id, device_verified, bind_status, created_at, last_used_at, ckid)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
        params![
            t.hash,
            t.account_id,
            t.device_id,
            t.device_verified,
            t.bind_status,
            at,
            t.ckid
        ],
    )?;
    revoked += c.execute(
        "DELETE FROM tokens WHERE account_id = ?1 AND rowid NOT IN (
             SELECT rowid FROM tokens WHERE account_id = ?1
             ORDER BY last_used_at DESC, created_at DESC, rowid DESC LIMIT ?2)",
        params![t.account_id, keep as i64],
    )?;
    Ok(revoked)
}

/// The session of a token hash, if the token is valid: not idle longer than `idle`, not older
/// than `max`, and its account still on the allowlist. An expired token is deleted. Whether the
/// request is signed by the session's key is the caller's to check (api.rs, proof.rs).
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
                        EXISTS (SELECT 1 FROM allowlist l WHERE l.login_key = a.login_key),
                        k.ckid, k.pk
                 FROM tokens t JOIN accounts a ON a.id = t.account_id
                 LEFT JOIN ctl_keys k ON k.ckid = t.ckid AND k.revoked_at IS NULL
                 WHERE t.hash = ?1"
            ),
            [hash],
            |r| {
                let ckid: Option<i64> = r.get(13)?;
                let pk: Option<Vec<u8>> = r.get(14)?;
                let key = match (ckid, pk.and_then(|pk| <[u8; 32]>::try_from(pk).ok())) {
                    (Some(ckid), Some(public_key)) => Some(SessionKey { ckid, public_key }),
                    _ => None,
                };
                Ok((
                    Session {
                        token_hash: hash.to_vec(),
                        account: account_row(r, 0)?,
                        device_id: r.get(7)?,
                        device_verified: r.get(8)?,
                        bind_status: r.get(9)?,
                        created_at: r.get(10)?,
                        last_used_at: r.get(11)?,
                        key,
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

/// Records the nonce of a signed request of key `ckid` until `expires_at`; false if the key used
/// it before and it has not expired at `at` (proof.rs). One statement, so one write.
pub fn use_req_nonce(
    c: &Connection,
    ckid: i64,
    nonce: &[u8],
    at: i64,
    expires_at: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "INSERT INTO req_nonces (ckid, nonce, expires_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (ckid, nonce) DO UPDATE SET expires_at = excluded.expires_at
         WHERE req_nonces.expires_at < ?4",
        params![ckid, nonce, expires_at, at],
    )? == 1)
}

// ---- account keys (design v3 §4-1, §5-4) ----

/// An account key.
#[derive(Debug, Clone)]
pub struct Key {
    pub ckid: i64,
    pub account_id: i64,
    pub public_key: Vec<u8>,
    pub dev_tag: String,
    pub ctl_id: String,
    pub platform: String,
    pub model: String,
    pub app_name: String,
    pub app_ver: String,
    pub login_net: String,
    pub key_serial: i64,
    pub created_at: i64,
    pub last_login_at: i64,
    pub ack_at: Option<i64>,
    /// the oldest of the account's keys that confirmed this one
    pub ack_by: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revoke_reason: Option<String>,
}

const KEY_COLUMNS: &str = "k.ckid, k.account_id, k.pk, k.dev_tag, k.ctl_id, k.platform, k.model,
    k.app_name, k.app_ver, k.login_net, k.key_serial, k.created_at, k.last_login_at, k.ack_at,
    k.ack_by, k.revoked_at, k.revoke_reason";

fn key_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Key> {
    Ok(Key {
        ckid: r.get(0)?,
        account_id: r.get(1)?,
        public_key: r.get(2)?,
        dev_tag: r.get(3)?,
        ctl_id: r.get(4)?,
        platform: r.get(5)?,
        model: r.get(6)?,
        app_name: r.get(7)?,
        app_ver: r.get(8)?,
        login_net: r.get(9)?,
        key_serial: r.get(10)?,
        created_at: r.get(11)?,
        last_login_at: r.get(12)?,
        ack_at: r.get(13)?,
        ack_by: r.get(14)?,
        revoked_at: r.get(15)?,
        revoke_reason: r.get(16)?,
    })
}

/// What a login registers about its key; the server's own view (design v3 §4-1 step 7).
pub struct NewKey<'a> {
    pub account_id: i64,
    pub public_key: &'a [u8; 32],
    pub dev_tag: &'a str,
    pub ctl_id: &'a str,
    pub platform: &'a str,
    pub model: &'a str,
    pub app_name: &'a str,
    pub app_ver: &'a str,
    pub login_net: &'a str,
}

/// What [`add_key`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAdmission {
    /// registered with the serial its registration made; `replaced` keys of the same device went
    Added {
        ckid: i64,
        key_serial: i64,
        replaced: usize,
    },
    /// another row has this public key, in any account, revoked or not
    InUse,
    /// the account would have more than the active keys allowed
    TooMany,
}

/// Registers a login's account key, inside the caller's transaction (design v3 §4-1 steps 3-5):
/// a key no row has yet, at most `max_active` active keys per account counting the ones this
/// login replaces as gone, then the account's older keys of the same device (`dev_tag`) are
/// revoked with their tokens and the change serial goes up once.
pub fn add_key(
    c: &Connection,
    k: &NewKey<'_>,
    at: i64,
    max_active: usize,
) -> rusqlite::Result<KeyAdmission> {
    let in_use: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM ctl_keys WHERE pk = ?1)",
        [k.public_key.as_slice()],
        |r| r.get(0),
    )?;
    if in_use {
        return Ok(KeyAdmission::InUse);
    }
    let (active, same_device): (i64, i64) = c.query_row(
        "SELECT COUNT(*), COALESCE(SUM(?2 <> '' AND dev_tag = ?2), 0) FROM ctl_keys
         WHERE account_id = ?1 AND revoked_at IS NULL",
        params![k.account_id, k.dev_tag],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if active - same_device >= max_active as i64 {
        return Ok(KeyAdmission::TooMany);
    }
    let replaced = if k.dev_tag.is_empty() {
        0
    } else {
        c.execute(
            "UPDATE ctl_keys SET revoked_at = ?3, revoke_reason = 'same-device'
             WHERE account_id = ?1 AND dev_tag = ?2 AND revoked_at IS NULL",
            params![k.account_id, k.dev_tag, at],
        )?
    };
    if replaced > 0 {
        drop_revoked_tokens(c)?;
    }
    let key_serial = bump_serial(c)?;
    c.execute(
        "INSERT INTO ctl_keys (account_id, pk, dev_tag, ctl_id, platform, model, app_name, app_ver,
             login_net, key_serial, created_at, last_login_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            k.account_id,
            k.public_key.as_slice(),
            k.dev_tag,
            k.ctl_id,
            k.platform,
            k.model,
            k.app_name,
            k.app_ver,
            k.login_net,
            key_serial,
            at
        ],
    )?;
    Ok(KeyAdmission::Added {
        ckid: c.last_insert_rowid(),
        key_serial,
        replaced,
    })
}

/// The tokens of revoked keys go with them.
fn drop_revoked_tokens(c: &Connection) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM tokens WHERE ckid IN (SELECT ckid FROM ctl_keys WHERE revoked_at IS NOT NULL)",
        [],
    )
}

/// After keys were revoked: their tokens go and the change serial goes up.
fn revoked(c: &Connection, count: usize) -> rusqlite::Result<usize> {
    if count > 0 {
        drop_revoked_tokens(c)?;
        bump_serial(c)?;
    }
    Ok(count)
}

/// Revokes one active key of an account with its token; false if the account has no such key.
pub fn revoke_key(
    c: &Connection,
    account_id: i64,
    ckid: i64,
    reason: &str,
    at: i64,
) -> rusqlite::Result<bool> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?3, revoke_reason = ?4
         WHERE ckid = ?1 AND account_id = ?2 AND revoked_at IS NULL",
        params![ckid, account_id, at, reason],
    )?;
    Ok(revoked(c, n)? > 0)
}

/// Revokes an active key whatever its account (admin); false if there is none.
pub fn revoke_any_key(c: &Connection, ckid: i64, reason: &str, at: i64) -> rusqlite::Result<bool> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?2, revoke_reason = ?3
         WHERE ckid = ?1 AND revoked_at IS NULL",
        params![ckid, at, reason],
    )?;
    Ok(revoked(c, n)? > 0)
}

/// Revokes the account's active keys but `keep`, with their tokens; returns how many.
pub fn revoke_other_keys(
    c: &Connection,
    account_id: i64,
    keep: i64,
    reason: &str,
    at: i64,
) -> rusqlite::Result<usize> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?3, revoke_reason = ?4
         WHERE account_id = ?1 AND ckid <> ?2 AND revoked_at IS NULL",
        params![account_id, keep, at, reason],
    )?;
    revoked(c, n)
}

/// Revokes every active key of an account with its tokens; returns how many keys.
pub fn revoke_account_keys(
    c: &Connection,
    account_id: i64,
    reason: &str,
    at: i64,
) -> rusqlite::Result<usize> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?2, revoke_reason = ?3
         WHERE account_id = ?1 AND revoked_at IS NULL",
        params![account_id, at, reason],
    )?;
    revoked(c, n)
}

/// Revokes every active key of every account with its tokens; returns how many keys.
pub fn revoke_all_keys(c: &Connection, reason: &str, at: i64) -> rusqlite::Result<usize> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?1, revoke_reason = ?2 WHERE revoked_at IS NULL",
        params![at, reason],
    )?;
    revoked(c, n)
}

/// Revokes the keys no valid token is left for: their logins expired (`idle` unused, `max`
/// old) or their tokens went without them (another login from the same device ID, the
/// account's token cap). A key counts only while its device is logged in (design v3 §1-8).
/// Returns how many.
pub fn revoke_idle_keys(c: &Connection, at: i64, idle: i64, max: i64) -> rusqlite::Result<usize> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?1, revoke_reason = 'idle'
         WHERE revoked_at IS NULL AND NOT EXISTS (
             SELECT 1 FROM tokens t WHERE t.ckid = ctl_keys.ckid
             AND t.last_used_at > ?2 AND t.created_at > ?3)",
        params![at, at - idle, at - max],
    )?;
    revoked(c, n)
}

/// Marks key `ckid` as confirmed by the account's active key `by` ("내 기기 맞음"), keeping the
/// oldest key that confirmed it. A key confirms only keys made after it: false, as for a key the
/// account does not have, for itself, an older key or one that is not active (keys.rs `new`).
pub fn ack_key(
    c: &Connection,
    account_id: i64,
    ckid: i64,
    by: i64,
    at: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE ctl_keys SET ack_at = COALESCE(ack_at, ?4), ack_by = MIN(COALESCE(ack_by, ?3), ?3)
         WHERE ckid = ?1 AND account_id = ?2 AND revoked_at IS NULL AND ckid > ?3
         AND EXISTS (SELECT 1 FROM ctl_keys b
                     WHERE b.ckid = ?3 AND b.account_id = ?2 AND b.revoked_at IS NULL)",
        params![ckid, account_id, by, at],
    )? > 0)
}

/// The account's active keys, newest first.
pub fn keys_of(c: &Connection, account_id: i64) -> rusqlite::Result<Vec<Key>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {KEY_COLUMNS} FROM ctl_keys k WHERE k.account_id = ?1 AND k.revoked_at IS NULL
         ORDER BY k.ckid DESC"
    ))?;
    let rows = stmt.query_map([account_id], key_row)?;
    rows.collect()
}

/// Every key, revoked ones too (until they are deleted), oldest first (admin).
pub fn all_keys(c: &Connection) -> rusqlite::Result<Vec<Key>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {KEY_COLUMNS} FROM ctl_keys k ORDER BY k.ckid"
    ))?;
    let rows = stmt.query_map([], key_row)?;
    rows.collect()
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
    /// the binding's 128-bit ID (design v3 §4-3)
    pub binding_id: Option<[u8; 16]>,
    /// the last login on the PC whose device signature hjv-api checked (server time)
    pub login_at: Option<i64>,
    /// from the PC's syncs: the first one that carried the binding, the last one, the app
    /// version and the switch-off it said, and the PC's mark of the binding
    pub activated_at: Option<i64>,
    pub last_sync_at: Option<i64>,
    pub app_ver: Option<String>,
    pub off: bool,
    pub sync_state: Option<String>,
    /// since when hbbs holds another key for the PC than the one stored (needs-relogin)
    pub key_changed_at: Option<i64>,
}

const DEVICE_COLUMNS: &str =
    "d.device_id, d.account_id, d.public_key, d.os, d.hostname, d.bound_at, d.last_seen_at,
     d.binding_id, d.login_at, d.activated_at, d.last_sync_at, d.app_ver, d.off, d.sync_state,
     d.key_changed_at";

fn device_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Device> {
    Ok(Device {
        device_id: r.get(0)?,
        account_id: r.get(1)?,
        public_key: r.get(2)?,
        os: r.get(3)?,
        hostname: r.get(4)?,
        bound_at: r.get(5)?,
        last_seen_at: r.get(6)?,
        binding_id: r
            .get::<_, Option<Vec<u8>>>(7)?
            .and_then(|b| <[u8; 16]>::try_from(b).ok()),
        login_at: r.get(8)?,
        activated_at: r.get(9)?,
        last_sync_at: r.get(10)?,
        app_ver: r.get(11)?,
        off: r.get::<_, Option<bool>>(12)?.unwrap_or(false),
        sync_state: r.get(13)?,
        key_changed_at: r.get(14)?,
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

/// What [`bind`] did, with the binding's ID and its login time for the login's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Binding {
    pub bound: Bound,
    /// added or already in the account: the binding ID (design v3 §4-3) and `login_at`, the
    /// server time of this login, which the trust list repeats (§3-6, §4-4)
    pub binding_id: Option<[u8; 16]>,
    pub login_at: Option<i64>,
}

/// Puts a verified device into an account, unless another account has it (design v2 §5: a
/// conflict is refused, not settled silently). A new binding gets a random 128-bit ID; the same
/// account logging in again keeps it and only moves the device's `login_at` to `at`, storing the
/// key hbbs holds now, which ends a needs-relogin (design v3 §4-6). Inside the caller's
/// transaction, which raises the change serial with the login's account key.
pub fn bind(
    c: &Connection,
    account_id: i64,
    device_id: &str,
    public_key: &[u8],
    os: &str,
    hostname: &str,
    at: i64,
) -> rusqlite::Result<Binding> {
    let owner: Option<i64> = c
        .query_row(
            "SELECT account_id FROM devices WHERE device_id = ?1",
            [device_id],
            |r| r.get(0),
        )
        .optional()?;
    let fresh = crate::util::random_bytes::<16>();
    let binding_id: Vec<u8> = match owner {
        Some(owner) if owner != account_id => {
            return Ok(Binding {
                bound: Bound::OtherAccount,
                binding_id: None,
                login_at: None,
            })
        }
        Some(_) => c.query_row(
            "UPDATE devices SET public_key = ?2, os = ?3, hostname = ?4, last_seen_at = ?5,
                 login_at = ?5, binding_id = COALESCE(binding_id, ?6), key_changed_at = NULL
             WHERE device_id = ?1 RETURNING binding_id",
            params![device_id, public_key, os, hostname, at, fresh.as_slice()],
            |r| r.get(0),
        )?,
        None => c.query_row(
            "INSERT INTO devices (device_id, account_id, public_key, os, hostname, bound_at,
                 last_seen_at, binding_id, login_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?6) RETURNING binding_id",
            params![
                device_id,
                account_id,
                public_key,
                os,
                hostname,
                at,
                fresh.as_slice()
            ],
            |r| r.get(0),
        )?,
    };
    Ok(Binding {
        bound: if owner.is_some() {
            Bound::Already
        } else {
            Bound::Added
        },
        binding_id: <[u8; 16]>::try_from(binding_id).ok(),
        login_at: Some(at),
    })
}

/// How long a released binding's tombstone is kept (design v3 §4-3, §5-4).
pub const TOMBSTONE_RETENTION: i64 = 180 * DAY;

/// The one way a PC leaves its account (design v3 §4-3): the row goes, its binding ID goes to
/// the tombstones with the PC's key and `reason`, and the change serial goes up, so that the PC's
/// next sync with that binding gets a signed `revoked` and drops its mark. What the PC reported
/// goes with it (design v3 §10-1): its refusals (cascade) and its key use. The reasons: `logout`,
/// `undo`, `remove`, `admin`, `account-delete`, `unseen-purge`. Inside the caller's transaction;
/// false if no account has the PC.
pub fn release_binding(
    c: &Connection,
    device_id: &str,
    reason: &str,
    at: i64,
) -> rusqlite::Result<bool> {
    let row: Option<(Option<Vec<u8>>, Vec<u8>)> = c
        .query_row(
            "SELECT binding_id, public_key FROM devices WHERE device_id = ?1",
            [device_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((binding_id, public_key)) = row else {
        return Ok(false);
    };
    if let Some(binding_id) = binding_id {
        c.execute(
            "INSERT INTO binding_tombstones (binding_id, device_id, public_key, revoked_at, reason)
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (binding_id) DO NOTHING",
            params![binding_id, device_id, public_key, at, reason],
        )?;
    }
    c.execute("DELETE FROM key_use WHERE device_id = ?1", [device_id])?;
    c.execute("DELETE FROM devices WHERE device_id = ?1", [device_id])?;
    bump_serial(c)?;
    Ok(true)
}

/// [`release_binding`] of a PC of the account `account_id` only (the paths of its users); false
/// when the account does not have the PC.
pub fn release_binding_in(
    c: &Connection,
    device_id: &str,
    account_id: i64,
    reason: &str,
    at: i64,
) -> rusqlite::Result<bool> {
    let owner: Option<i64> = c
        .query_row(
            "SELECT account_id FROM devices WHERE device_id = ?1",
            [device_id],
            |r| r.get(0),
        )
        .optional()?;
    if owner != Some(account_id) {
        return Ok(false);
    }
    release_binding(c, device_id, reason, at)
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

/// hbbs holds another key for the PC than the stored `stored` one (design v3 §4-6): the PC is
/// needs-relogin from `at`, and the change serial goes up. Only while `stored` is still the
/// stored key (a login may have stored hbbs's new one meanwhile); false if nothing changed.
pub fn mark_key_changed(
    c: &Connection,
    device_id: &str,
    stored: &[u8],
    at: i64,
) -> rusqlite::Result<bool> {
    let changed = c.execute(
        "UPDATE devices SET key_changed_at = ?3
         WHERE device_id = ?1 AND public_key = ?2 AND key_changed_at IS NULL",
        params![device_id, stored, at],
    )? > 0;
    if changed {
        bump_serial(c)?;
    }
    Ok(changed)
}

/// The PC proved its stored key again (or hbbs holds it again): no longer needs-relogin; false
/// if it was not.
pub fn clear_key_changed(c: &Connection, device_id: &str) -> rusqlite::Result<bool> {
    let changed = c.execute(
        "UPDATE devices SET key_changed_at = NULL
         WHERE device_id = ?1 AND key_changed_at IS NOT NULL",
        [device_id],
    )? > 0;
    if changed {
        bump_serial(c)?;
    }
    Ok(changed)
}

// ---- the PC sync (design v3 §3-6, §5-2; pcsync.rs) ----

/// A released binding (design v3 §4-3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    pub binding_id: Vec<u8>,
    pub device_id: String,
    pub public_key: Vec<u8>,
    pub revoked_at: i64,
    pub reason: String,
}

fn tombstone_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Tombstone> {
    Ok(Tombstone {
        binding_id: r.get(0)?,
        device_id: r.get(1)?,
        public_key: r.get(2)?,
        revoked_at: r.get(3)?,
        reason: r.get(4)?,
    })
}

/// What a sync from a PC ID is checked against (design v3 §5-2 step 3): the PC's binding, or else
/// the latest tombstone of the ID.
#[derive(Debug, Clone)]
pub enum SyncRow {
    Bound(Device),
    Released(Tombstone),
}

pub fn sync_row(c: &Connection, device_id: &str) -> rusqlite::Result<Option<SyncRow>> {
    if let Some(d) = device(c, device_id)? {
        return Ok(Some(SyncRow::Bound(d)));
    }
    Ok(c.query_row(
        "SELECT binding_id, device_id, public_key, revoked_at, reason FROM binding_tombstones
         WHERE device_id = ?1 ORDER BY revoked_at DESC, rowid DESC LIMIT 1",
        [device_id],
        tombstone_row,
    )
    .optional()?
    .map(SyncRow::Released))
}

/// The tombstone of `binding_id` when it was a binding of `device_id`.
pub fn tombstone(
    c: &Connection,
    device_id: &str,
    binding_id: &[u8; 16],
) -> rusqlite::Result<Option<Tombstone>> {
    c.query_row(
        "SELECT binding_id, device_id, public_key, revoked_at, reason FROM binding_tombstones
         WHERE binding_id = ?1 AND device_id = ?2",
        params![binding_id.as_slice(), device_id],
        tombstone_row,
    )
    .optional()
}

/// Records the (`id`, `n`) of an accepted sync until `expires_at`; false if it was accepted
/// before and has not expired at `at` (envelope.rs step 6). One statement, so one write.
pub fn use_pc_nonce(
    c: &Connection,
    id: &str,
    nonce: &[u8],
    at: i64,
    expires_at: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "INSERT INTO pc_nonces (id, nonce, expires_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (id, nonce) DO UPDATE SET expires_at = excluded.expires_at
         WHERE pc_nonces.expires_at < ?4",
        params![id, nonce, expires_at, at],
    )? == 1)
}

/// What a PC's sync said about itself.
#[derive(Debug, Clone, Copy)]
pub struct SyncNote<'a> {
    /// the PC's mark of the binding: 'present' (the binding it sent is its binding), 'absent'
    /// (it sent none) or 'stale' (it sent another one)
    pub mark: &'a str,
    /// the app version, '' when it said none that is one
    pub app_ver: &'a str,
    pub off: bool,
}

/// Stores what a bound PC's sync said; the PC counts as seen, so that the upkeep does not take a
/// PC in use out of its account (design v3 §4-3 path 6). The first sync carrying the binding sets
/// `activated_at`.
pub fn store_sync(
    c: &Connection,
    device_id: &str,
    note: &SyncNote<'_>,
    at: i64,
) -> rusqlite::Result<()> {
    c.execute(
        "UPDATE devices SET last_sync_at = ?2, last_seen_at = MAX(last_seen_at, ?2),
             app_ver = ?3, off = ?4, sync_state = ?5,
             activated_at = CASE WHEN ?5 = 'present' THEN COALESCE(activated_at, ?2)
                                 ELSE activated_at END
         WHERE device_id = ?1",
        params![device_id, at, note.app_ver, note.off, note.mark],
    )?;
    Ok(())
}

/// One line of a sync's `used` (design v3 §3-6, U6): the proofs of the account key `pk` that the
/// PC let in on the UTC day `day`, the latest at `last` by the PC's clock (at most the server's
/// time of the report), `n` that day's running count (not the increase since the last report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UseLine {
    pub pk: [u8; 32],
    pub day: String,
    pub last: i64,
    pub n: i64,
}

/// Days of key use and refusals kept: today and the 29 days before (UTC).
pub const USE_DAYS: i64 = 30;

/// The first day of the USE_DAYS that end with the day of `at`.
pub fn use_since(at: i64) -> String {
    crate::util::date(at - (USE_DAYS - 1) * DAY)
}

/// Merges a PC's `used` lines (U6, design v3 §5-2): a line of a key of the PC's account counts
/// under (key, PC, day), its time and its count each the larger of the stored and the reported
/// value. A report repeated changes nothing, and an earlier time or a smaller count does not
/// lower what is stored, so a local user with the PC's key cannot wipe the trace of a use. Lines
/// of other keys are left out. Returns how many lines counted.
pub fn merge_use(
    c: &Connection,
    account_id: i64,
    device_id: &str,
    lines: &[UseLine],
) -> rusqlite::Result<usize> {
    let mut counted = 0;
    for line in lines {
        let ckid: Option<i64> = c
            .query_row(
                "SELECT ckid FROM ctl_keys WHERE pk = ?1 AND account_id = ?2",
                params![line.pk.as_slice(), account_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(ckid) = ckid else {
            continue;
        };
        c.execute(
            "INSERT INTO key_use (ckid, device_id, day, last_used_at, n) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (ckid, device_id, day) DO UPDATE SET
                 last_used_at = MAX(last_used_at, excluded.last_used_at),
                 n = MAX(n, excluded.n)",
            params![ckid, device_id, line.day, line.last, line.n],
        )?;
        counted += 1;
    }
    Ok(counted)
}

/// The largest refusal count kept for one PC, reason and day.
const MAX_REFUSALS: i64 = 1_000_000_000;

/// Adds a PC's counts of refused proofs since its last report, by reason, under the server's day
/// `day` (counts only, no names: design v3 §4-5).
pub fn add_refusals(
    c: &Connection,
    device_id: &str,
    refusals: &[(String, i64)],
    day: &str,
) -> rusqlite::Result<()> {
    for (reason, count) in refusals.iter().filter(|(_, n)| *n > 0) {
        c.execute(
            "INSERT INTO pc_refusals (device_id, reason, day, count) VALUES (?1, ?2, ?3, MIN(?4, ?5))
             ON CONFLICT (device_id, reason, day) DO UPDATE SET
                 count = MIN(count + excluded.count, ?5)",
            params![device_id, reason, day, count, MAX_REFUSALS],
        )?;
    }
    Ok(())
}

/// What the PCs reported about one key's use from a day on (U6): per key and PC, the latest
/// time and the count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyUse {
    pub ckid: i64,
    pub device_id: String,
    /// the PC's host name ('' should the PC not be in the key's account)
    pub pc: String,
    pub last: i64,
    pub n: i64,
}

fn key_use_rows(
    c: &Connection,
    account_id: Option<i64>,
    since: &str,
) -> rusqlite::Result<Vec<KeyUse>> {
    let mut stmt = c.prepare(
        "SELECT u.ckid, u.device_id, COALESCE(MAX(d.hostname), ''), MAX(u.last_used_at), SUM(u.n)
         FROM key_use u JOIN ctl_keys k ON k.ckid = u.ckid
         LEFT JOIN devices d ON d.device_id = u.device_id AND d.account_id = k.account_id
         WHERE (?1 IS NULL OR k.account_id = ?1) AND u.day >= ?2
         GROUP BY u.ckid, u.device_id ORDER BY MAX(u.last_used_at) DESC, u.device_id",
    )?;
    let rows = stmt.query_map(params![account_id, since], |r| {
        Ok(KeyUse {
            ckid: r.get(0)?,
            device_id: r.get(1)?,
            pc: r.get(2)?,
            last: r.get(3)?,
            n: r.get(4)?,
        })
    })?;
    rows.collect()
}

/// The use of an account's keys on its PCs from the day `since` on, latest first.
pub fn key_use_of(c: &Connection, account_id: i64, since: &str) -> rusqlite::Result<Vec<KeyUse>> {
    key_use_rows(c, Some(account_id), since)
}

/// The same for every account (admin).
pub fn all_key_use(c: &Connection, since: &str) -> rusqlite::Result<Vec<KeyUse>> {
    key_use_rows(c, None, since)
}

/// The tombstones kept, by the way their bindings were released.
pub fn tombstones_by_reason(c: &Connection) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = c.prepare(
        "SELECT reason, COUNT(*) FROM binding_tombstones GROUP BY reason ORDER BY reason",
    )?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
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
    /// PCs not seen for the device time, released (`unseen-purge`)
    pub devices: usize,
    pub accounts: usize,
    pub events: usize,
    pub other: usize,
    /// account keys revoked because no valid token was left for them
    pub keys_revoked: usize,
    /// revoked account keys deleted after KEY_RETENTION
    pub keys_deleted: usize,
    /// tombstones past TOMBSTONE_RETENTION, key use and refusals past USE_DAYS
    pub tombstones: usize,
    pub sync_reports: usize,
}

impl Purged {
    pub fn any(&self) -> bool {
        self.tokens
            + self.devices
            + self.accounts
            + self.events
            + self.other
            + self.keys_revoked
            + self.keys_deleted
            + self.tombstones
            + self.sync_reports
            > 0
    }
}

/// How long a revoked account key is kept (design v3 §5-4, §10-1).
pub const KEY_RETENTION: i64 = 30 * DAY;

/// Deletes what is past its keeping time, then truncates the WAL if anything went.
///
/// A PC not seen for the device time (by hbbs, a login or list request from it, or its sync)
/// leaves its account through [`release_binding`]. An account unused for the account time goes
/// with its devices, tokens and account keys ([`delete_account`]), unless it is still on the
/// allowlist and one of its PCs was seen within the device time: a lab PC logged in once stays
/// listed while it is in use, whether or not its owner opens the app (v2 §5 lists the ways a PC
/// leaves the list; the account's own expiry is not one of them). The account keys of expired
/// tokens are revoked with them (design v3 §14 question 6), and revoked keys are deleted after
/// KEY_RETENTION; tombstones after TOMBSTONE_RETENTION, key use and refusals after USE_DAYS.
pub fn purge(c: &mut Connection, at: i64, r: &Retention) -> rusqlite::Result<Purged> {
    let p = in_tx(c, |c| {
        let tokens = c.execute(
            "DELETE FROM tokens WHERE last_used_at <= ?1 OR created_at <= ?2",
            params![at - r.token_idle, at - r.token_max],
        )?;
        let keys_revoked = revoke_idle_keys(c, at, r.token_idle, r.token_max)?;
        let mut devices = 0;
        let unseen: Vec<String> = column(
            c,
            "SELECT device_id FROM devices WHERE last_seen_at <= ?1",
            [at - r.device_unseen],
        )?;
        for id in unseen {
            devices += usize::from(release_binding(c, &id, "unseen-purge", at)?);
        }
        let mut accounts = 0;
        let unused: Vec<i64> = column(
            c,
            "SELECT id FROM accounts WHERE last_active_at <= ?1 AND NOT (
                 EXISTS (SELECT 1 FROM allowlist l WHERE l.login_key = accounts.login_key)
                 AND EXISTS (SELECT 1 FROM devices d
                             WHERE d.account_id = accounts.id AND d.last_seen_at > ?2))",
            [at - r.account, at - r.device_unseen],
        )?;
        for id in unused {
            accounts += usize::from(delete_account(c, id, at)?);
        }
        let since = use_since(at);
        Ok(Purged {
            tokens,
            devices,
            accounts,
            events: c.execute("DELETE FROM events WHERE at <= ?1", [at - r.events])?,
            other: c.execute("DELETE FROM seal_nonces WHERE expires_at < ?1", [at])?
                + c.execute("DELETE FROM req_nonces WHERE expires_at < ?1", [at])?
                + c.execute("DELETE FROM pc_nonces WHERE expires_at < ?1", [at])?
                + c.execute("DELETE FROM attempts WHERE at <= ?1", [at - r.attempts])?,
            keys_revoked,
            keys_deleted: c.execute(
                "DELETE FROM ctl_keys WHERE revoked_at IS NOT NULL AND revoked_at <= ?1",
                [at - KEY_RETENTION],
            )?,
            tombstones: c.execute(
                "DELETE FROM binding_tombstones WHERE revoked_at <= ?1",
                [at - TOMBSTONE_RETENTION],
            )?,
            sync_reports: c.execute("DELETE FROM key_use WHERE day < ?1", [&since])?
                + c.execute("DELETE FROM pc_refusals WHERE day < ?1", [&since])?,
        })
    })?;
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
    /// account keys: active, and revoked ones still kept by why they went: (reason, count)
    pub keys_active: i64,
    pub keys_revoked: Vec<(String, i64)>,
    pub change_serial: i64,
    pub passwordless: bool,
    /// accounts switched on and off one by one
    pub accounts_on: i64,
    pub accounts_off: i64,
    pub strict: bool,
    /// the PCs where password-less connections are on (their last sync, within a list's lifetime,
    /// carried their binding), of them the ones that synced in the last ten minutes, and the ones
    /// that have not synced since the last switch change, so that the list they hold may still say
    /// what the switch said before (design v3 §5-6: keep hjv-api running until it is 0, or for a
    /// list's lifetime)
    pub pcs_holding: i64,
    pub pcs_synced_recently: i64,
    pub pcs_behind_switch: i64,
    pub switched_at: Option<i64>,
    /// PCs whose key hbbs holds is not the one stored (needs-relogin)
    pub needs_relogin: i64,
    /// PCs by the app version of their last sync ('-': none yet)
    pub versions: Vec<(String, i64)>,
    /// refused proofs the PCs reported in the last USE_DAYS, by reason
    pub refusals: Vec<(String, i64)>,
    /// tombstones kept, by the way the binding was released
    pub tombstones: Vec<(String, i64)>,
    /// password-less connections the PCs let in during the last USE_DAYS
    pub uses: i64,
}

/// The counts of `admin report` at `at`; `list_ttl` is the lifetime of a trust list.
pub fn counts(c: &Connection, at: i64, list_ttl: i64) -> rusqlite::Result<Counts> {
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
    let mut stmt = c.prepare(
        "SELECT COALESCE(revoke_reason, '-'), COUNT(*) FROM ctl_keys WHERE revoked_at IS NOT NULL
         GROUP BY revoke_reason ORDER BY revoke_reason",
    )?;
    let keys_revoked = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let pairs = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<Vec<(String, i64)>> {
        let mut stmt = c.prepare(sql)?;
        let rows = stmt.query_map(p, |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    };
    let switched_at = switched_at(c)?;
    let since = use_since(at);
    let holding = |extra: &str, p: &[&dyn rusqlite::ToSql]| -> rusqlite::Result<i64> {
        c.query_row(
            &format!(
                "SELECT COUNT(*) FROM devices WHERE sync_state = 'present' AND last_sync_at > ?1 {extra}"
            ),
            p,
            |r| r.get(0),
        )
    };
    let valid_from = at - list_ttl;
    Ok(Counts {
        pcs_holding: holding("", &[&valid_from])?,
        pcs_synced_recently: holding("AND last_sync_at > ?2", &[&valid_from, &(at - 600)])?,
        // a sync in the second of the switch counts as before it: better wait once more
        pcs_behind_switch: match switched_at {
            Some(switched) => holding("AND last_sync_at <= ?2", &[&valid_from, &switched])?,
            None => 0,
        },
        switched_at,
        needs_relogin: one("SELECT COUNT(*) FROM devices WHERE key_changed_at IS NOT NULL")?,
        versions: pairs(
            "SELECT COALESCE(NULLIF(app_ver, ''), '-'), COUNT(*) FROM devices GROUP BY 1 ORDER BY 1",
            &[],
        )?,
        refusals: pairs(
            "SELECT reason, SUM(count) FROM pc_refusals WHERE day >= ?1
             GROUP BY reason ORDER BY reason",
            &[&since],
        )?,
        tombstones: tombstones_by_reason(c)?,
        uses: c.query_row(
            "SELECT COALESCE(SUM(n), 0) FROM key_use WHERE day >= ?1",
            [&since],
            |r| r.get(0),
        )?,
        allowlist: one("SELECT COUNT(*) FROM allowlist")?,
        accounts: one("SELECT COUNT(*) FROM accounts")?,
        devices: one("SELECT COUNT(*) FROM devices")?,
        tokens: one("SELECT COUNT(*) FROM tokens")?,
        results,
        keys_active: one("SELECT COUNT(*) FROM ctl_keys WHERE revoked_at IS NULL")?,
        keys_revoked,
        change_serial: change_serial(c)?,
        passwordless: passwordless_global(c)?,
        accounts_on: one("SELECT COUNT(*) FROM accounts WHERE passwordless = 'on'")?,
        accounts_off: one("SELECT COUNT(*) FROM accounts WHERE passwordless = 'off'")?,
        strict: strict(c)?,
    })
}

// ---- health (check.rs, design v3 §5-9) ----

/// The PCs holding their binding at `at` (as in [`counts`]: their last sync, within a list's
/// lifetime `list_ttl`, carried it), with the time of that sync, for the five-minute check.
pub fn holding_pcs(c: &Connection, at: i64, list_ttl: i64) -> rusqlite::Result<Vec<(String, i64)>> {
    let mut stmt = c.prepare(
        "SELECT device_id, last_sync_at FROM devices
         WHERE sync_state = 'present' AND last_sync_at > ?1 ORDER BY device_id",
    )?;
    let rows = stmt.query_map([at - list_ttl], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect()
}

/// The NAS relays that are active (relay/PROTOCOL.md section 11) and, of them, the ones that have
/// not polled for `silent_after` seconds at `at`; `None` until phase 2's schema has the table
/// `relays` with its columns `state` and `last_poll_at` (design v3 §5-4).
pub fn relay_health(
    c: &Connection,
    at: i64,
    silent_after: i64,
) -> rusqlite::Result<Option<(i64, i64)>> {
    let columns: Vec<String> = column(c, "SELECT name FROM pragma_table_info('relays')", [])?;
    if !["state", "last_poll_at"]
        .iter()
        .all(|name| columns.iter().any(|c| c == name))
    {
        return Ok(None);
    }
    c.query_row(
        "SELECT COUNT(*), COALESCE(SUM(last_poll_at IS NULL OR last_poll_at <= ?1), 0)
         FROM relays WHERE state = 'active'",
        [at - silent_after],
        |r| Ok(Some((r.get(0)?, r.get(1)?))),
    )
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
            ckid: None,
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
    fn schema_one_moves_to_two() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(MIGRATIONS[0]).unwrap();
        c.pragma_update(None, "user_version", 1).unwrap();
        c.execute_batch(
            "INSERT INTO accounts VALUES (1, 'a', 'A', '', 1, 1, 1);
             INSERT INTO tokens VALUES (X'01', 1, '', 0, 'none', 1, 1);
             INSERT INTO devices VALUES ('1', 1, X'02', 'windows', 'PC', 1, 1);
             INSERT INTO devices VALUES ('2', 1, X'03', 'windows', 'PC', 1, 1);",
        )
        .unwrap();
        let c = prepare(c).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 2);
        // the tokens without a key went; the devices kept, each with a binding ID of its own
        let tokens: i64 = c
            .query_row("SELECT COUNT(*) FROM tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tokens, 0);
        let ids: Vec<Vec<u8>> = {
            let mut s = c.prepare("SELECT binding_id FROM devices").unwrap();
            let rows = s.query_map([], |r| r.get(0)).unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().all(|id| id.len() == 16) && ids[0] != ids[1]);
        // off when installed, serial 0
        assert!(!passwordless_global(&c).unwrap() && !strict(&c).unwrap());
        assert_eq!(change_serial(&c).unwrap(), 0);
        assert_eq!(account_passwordless(&c, 1).unwrap(), None);
        // part B: no PC has synced yet, no binding was released
        let d = device(&c, "1").unwrap().unwrap();
        assert_eq!(
            (d.last_sync_at, d.sync_state, d.off, d.key_changed_at),
            (None, None, false, None)
        );
        for table in ["binding_tombstones", "key_use", "pc_refusals", "pc_nonces"] {
            let n: i64 = c
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
        // a login on a PC of schema 1: the binding ID stays, the login time comes
        let id = ids[0].clone();
        let device: String = c
            .query_row(
                "SELECT device_id FROM devices WHERE binding_id = ?1",
                [&id],
                |r| r.get(0),
            )
            .unwrap();
        let b = bind(&c, 1, &device, &[2], "windows", "PC", 9).unwrap();
        assert_eq!(b.binding_id.map(|b| b.to_vec()), Some(id));
        assert_eq!(b.login_at, Some(9));
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
        assert_eq!(
            allow_remove(&mut c, &["a1".to_owned()], 300).unwrap(),
            (1, 1, 0)
        );
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
        let added = bind(&c, a, "1", &[1; 32], "windows", "PC", 5).unwrap();
        assert_eq!((added.bound, added.login_at), (Bound::Added, Some(5)));
        // the same account again keeps the binding ID and moves the login time
        let again = bind(&c, a, "1", &[2; 32], "windows", "PC2", 6).unwrap();
        assert_eq!((again.bound, again.login_at), (Bound::Already, Some(6)));
        assert!(added.binding_id.is_some());
        assert_eq!(again.binding_id, added.binding_id);
        assert_eq!(
            bind(&c, b, "1", &[2; 32], "windows", "X", 7).unwrap(),
            Binding {
                bound: Bound::OtherAccount,
                binding_id: None,
                login_at: None
            }
        );
        let d = device(&c, "1").unwrap().unwrap();
        assert_eq!(
            (d.account_id, d.hostname.as_str(), d.public_key),
            (a, "PC2", vec![2; 32])
        );
        assert_eq!(bound_key(&c, "1", "a").unwrap(), Some(vec![2; 32]));
        assert_eq!(bound_key(&c, "1", "b").unwrap(), None);
        assert!(!release_binding_in(&c, "1", b, "remove", 7).unwrap());
        assert!(seen_by_hbbs(&c, "1", &[2; 32], 50).unwrap());
        assert!(!seen_by_hbbs(&c, "1", &[3; 32], 60).unwrap());
        assert_eq!(device(&c, "1").unwrap().unwrap().last_seen_at, 50);
        let serial = change_serial(&c).unwrap();
        assert!(release_binding_in(&c, "1", a, "remove", 70).unwrap());
        assert!(device(&c, "1").unwrap().is_none());
        assert_eq!(change_serial(&c).unwrap(), serial + 1);
        // the binding ID stays as a tombstone, with the key the PC had
        let old = added.binding_id.unwrap();
        assert_eq!(
            tombstone(&c, "1", &old).unwrap(),
            Some(Tombstone {
                binding_id: old.to_vec(),
                device_id: "1".into(),
                public_key: vec![2; 32],
                revoked_at: 70,
                reason: "remove".into()
            })
        );
        assert_eq!(tombstone(&c, "2", &old).unwrap(), None);
        assert!(!release_binding(&c, "1", "admin", 71).unwrap());
        // bound again after it left: a new binding ID
        let rebound = bind(&c, a, "1", &[2; 32], "windows", "PC2", 8).unwrap();
        assert_eq!(rebound.bound, Bound::Added);
        assert_ne!(rebound.binding_id, added.binding_id);
        // deleting an account releases its devices first, then takes its tokens
        bind(&c, a, "2", &[1; 32], "windows", "PC", 5).unwrap();
        insert_token(&mut c, &token(b"t", a, "2"), 5, 20).unwrap();
        let two = device(&c, "2").unwrap().unwrap().binding_id.unwrap();
        let serial = change_serial(&c).unwrap();
        assert!(delete_account(&c, a, 80).unwrap());
        // two releases and the account
        assert_eq!(change_serial(&c).unwrap(), serial + 3);
        assert!(device(&c, "2").unwrap().is_none());
        assert_eq!(
            tombstone(&c, "2", &two).unwrap().map(|t| t.reason),
            Some("account-delete".into())
        );
        assert!(!delete_token(&c, b"t").unwrap());
    }

    #[test]
    fn sync_rows_and_tombstones() {
        let c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        assert!(sync_row(&c, "1").unwrap().is_none());
        let first = bind(&c, a, "1", &[1; 32], "windows", "PC", 5).unwrap();
        assert!(
            matches!(sync_row(&c, "1").unwrap(), Some(SyncRow::Bound(d)) if d.binding_id == first.binding_id)
        );
        release_binding(&c, "1", "logout", 10).unwrap();
        let second = bind(&c, a, "1", &[2; 32], "windows", "PC", 11).unwrap();
        release_binding(&c, "1", "admin", 20).unwrap();
        // without a binding, the latest tombstone of the ID
        let Some(SyncRow::Released(t)) = sync_row(&c, "1").unwrap() else {
            panic!("released")
        };
        assert_eq!(
            (t.binding_id, t.public_key, t.reason.as_str()),
            (second.binding_id.unwrap().to_vec(), vec![2; 32], "admin")
        );
        // nonces of accepted syncs, per ID, until they expire
        assert!(use_pc_nonce(&c, "1", b"n", 100, 700).unwrap());
        assert!(!use_pc_nonce(&c, "1", b"n", 700, 1300).unwrap());
        assert!(use_pc_nonce(&c, "2", b"n", 700, 1300).unwrap());
        assert!(use_pc_nonce(&c, "1", b"n", 701, 1301).unwrap());
    }

    #[test]
    fn a_restored_backup_is_no_backup_once_served() {
        let dir = std::env::temp_dir().join(format!("hjv-api-restored-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("restored.sqlite3");
        {
            let c = open(&path).unwrap();
            c.pragma_update(None, "application_id", crate::backup::APPLICATION_ID)
                .unwrap();
        }
        let c = open(&path).unwrap();
        let id: i32 = c
            .query_row("PRAGMA application_id", [], |r| r.get(0))
            .unwrap();
        assert_eq!(id, 0);
        drop(c);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn health_queries() {
        let c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        for id in ["1", "2", "3", "4"] {
            bind(&c, a, id, &[1; 32], "windows", "PC", 5).unwrap();
        }
        let note = |mark| SyncNote {
            mark,
            app_ver: "1.6.0",
            off: false,
        };
        // the PCs of `counts`' "holding their binding": their binding in a sync within a list's
        // lifetime
        store_sync(&c, "1", &note("present"), 1000).unwrap();
        store_sync(&c, "2", &note("present"), 400).unwrap();
        store_sync(&c, "3", &note("absent"), 1000).unwrap();
        assert_eq!(
            holding_pcs(&c, 1100, 600).unwrap(),
            vec![("1".to_owned(), 1000)]
        );
        assert_eq!(
            holding_pcs(&c, 1100, 1000).unwrap(),
            vec![("1".to_owned(), 1000), ("2".to_owned(), 400)]
        );
        assert_eq!(counts(&c, 1100, 1000).unwrap().pcs_holding, 2);
        // the relays come with phase 2's schema; before it there is nothing to say
        assert_eq!(relay_health(&c, 5000, 3600).unwrap(), None);
        c.execute_batch(
            "CREATE TABLE relays (relay_id TEXT PRIMARY KEY, state TEXT NOT NULL, last_poll_at INTEGER);
             INSERT INTO relays VALUES ('r1', 'active', 4900), ('r2', 'active', 1000),
                 ('r3', 'active', NULL), ('r4', 'pending', NULL), ('r5', 'revoked', 10);",
        )
        .unwrap();
        assert_eq!(relay_health(&c, 5000, 3600).unwrap(), Some((3, 2)));
        assert_eq!(relay_health(&c, 5000, 4500).unwrap(), Some((3, 1)));
    }

    #[test]
    fn key_changes() {
        let c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        bind(&c, a, "1", &[1; 32], "windows", "PC", 5).unwrap();
        let serial = change_serial(&c).unwrap();
        // only while the stored key is the one the check found wanting
        assert!(!mark_key_changed(&c, "1", &[9; 32], 10).unwrap());
        assert!(mark_key_changed(&c, "1", &[1; 32], 10).unwrap());
        assert!(!mark_key_changed(&c, "1", &[1; 32], 11).unwrap());
        assert_eq!(device(&c, "1").unwrap().unwrap().key_changed_at, Some(10));
        assert_eq!(change_serial(&c).unwrap(), serial + 1);
        assert!(clear_key_changed(&c, "1").unwrap());
        assert!(!clear_key_changed(&c, "1").unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial + 2);
        // a login of the same account stores hbbs's new key and ends needs-relogin
        mark_key_changed(&c, "1", &[1; 32], 12).unwrap();
        let again = bind(&c, a, "1", &[2; 32], "windows", "PC", 13).unwrap();
        let d = device(&c, "1").unwrap().unwrap();
        assert_eq!((d.public_key, d.key_changed_at), (vec![2; 32], None));
        assert_eq!(again.binding_id, d.binding_id);
    }

    #[test]
    fn key_use_is_merged_by_the_larger_values() {
        let mut c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        let b = upsert_account(&c, "b", "B", "", 1).unwrap();
        let KeyAdmission::Added { ckid, .. } = login_with_key(&mut c, a, 1, "") else {
            panic!("added")
        };
        login_with_key(&mut c, b, 2, "");
        bind(&c, a, "pc", &[7; 32], "windows", "PC-1", 5).unwrap();
        let line = |pk: u8, day: &str, last: i64, n: i64| UseLine {
            pk: [pk; 32],
            day: day.into(),
            last,
            n,
        };
        let stored = |c: &Connection| -> Vec<(String, i64, i64)> {
            let mut s = c
                .prepare("SELECT day, last_used_at, n FROM key_use ORDER BY day")
                .unwrap();
            let rows = s
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
                .unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        let report = [line(1, "2026-10-11", 300, 2), line(1, "2026-10-12", 500, 1)];
        assert_eq!(merge_use(&c, a, "pc", &report).unwrap(), 2);
        // the same report again, an earlier time and a smaller count: nothing goes down
        assert_eq!(merge_use(&c, a, "pc", &report).unwrap(), 2);
        merge_use(&c, a, "pc", &[line(1, "2026-10-11", 100, 1)]).unwrap();
        assert_eq!(
            stored(&c),
            vec![("2026-10-11".into(), 300, 2), ("2026-10-12".into(), 500, 1)]
        );
        // more uses later the same day go up
        merge_use(&c, a, "pc", &[line(1, "2026-10-12", 900, 4)]).unwrap();
        assert_eq!(stored(&c)[1], ("2026-10-12".into(), 900, 4));
        // another account's key, or no key at all: left out
        assert_eq!(
            merge_use(
                &c,
                a,
                "pc",
                &[line(2, "2026-10-12", 1, 1), line(5, "2026-10-12", 1, 1)]
            )
            .unwrap(),
            0
        );
        let since = "2026-09-13";
        assert_eq!(
            key_use_of(&c, a, since).unwrap(),
            vec![KeyUse {
                ckid,
                device_id: "pc".into(),
                pc: "PC-1".into(),
                last: 900,
                n: 6
            }]
        );
        assert!(key_use_of(&c, b, since).unwrap().is_empty());
        assert_eq!(key_use_of(&c, a, "2026-10-12").unwrap()[0].n, 4);
        // refusals add up under the server's day, zeros left out
        let refused = [("unknown_key".to_owned(), 2), ("off".to_owned(), 0)];
        add_refusals(&c, "pc", &refused, "2026-10-12").unwrap();
        add_refusals(&c, "pc", &refused, "2026-10-12").unwrap();
        let rows: Vec<(String, i64)> = {
            let mut s = c.prepare("SELECT reason, count FROM pc_refusals").unwrap();
            let rows = s.query_map([], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
            rows.collect::<rusqlite::Result<_>>().unwrap()
        };
        assert_eq!(rows, vec![("unknown_key".to_owned(), 4)]);
        // the refusals and the key use go with the PC (design v3 §10-1); another PC's stay
        bind(&c, a, "pc2", &[8; 32], "windows", "PC-2", 5).unwrap();
        merge_use(&c, a, "pc2", &[line(1, "2026-10-12", 700, 1)]).unwrap();
        release_binding(&c, "pc", "remove", 10).unwrap();
        let left = |table: &str| -> i64 {
            c.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!((left("pc_refusals"), left("key_use")), (0, 1));
        assert_eq!(
            key_use_of(&c, a, since).unwrap(),
            vec![KeyUse {
                ckid,
                device_id: "pc2".into(),
                pc: "PC-2".into(),
                last: 700,
                n: 1
            }]
        );
        // and with the account
        delete_account(&c, a, 20).unwrap();
        assert_eq!(left("key_use"), 0);
    }

    fn new_key<'a>(account_id: i64, pk: &'a [u8; 32], dev_tag: &'a str) -> NewKey<'a> {
        NewKey {
            account_id,
            public_key: pk,
            dev_tag,
            ctl_id: "987654321",
            platform: "ios",
            model: "iPhone 14 Pro",
            app_name: "iPhone15,2",
            app_ver: "1.6.0",
            login_net: "",
        }
    }

    /// Registers a key and a token for it, as a login does.
    fn login_with_key(c: &mut Connection, account_id: i64, n: u8, dev_tag: &str) -> KeyAdmission {
        let pk = [n; 32];
        in_tx(c, |tx| {
            let admission = add_key(
                tx,
                &new_key(account_id, &pk, dev_tag),
                100 + i64::from(n),
                3,
            )?;
            if let KeyAdmission::Added { ckid, .. } = admission {
                let hash = [n; 32];
                let t = NewToken {
                    ckid: Some(ckid),
                    ..token(&hash, account_id, "")
                };
                insert_token_tx(tx, &t, 100 + i64::from(n), 20)?;
            }
            Ok(admission)
        })
        .unwrap()
    }

    fn active(c: &Connection, account_id: i64) -> Vec<Vec<u8>> {
        keys_of(c, account_id)
            .unwrap()
            .into_iter()
            .map(|k| k.public_key)
            .collect()
    }

    #[test]
    fn account_keys() {
        let mut c = conn();
        allow_add(&mut c, &["a".to_owned(), "b".to_owned()], 1).unwrap();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        let b = upsert_account(&c, "b", "B", "", 1).unwrap();
        // each registration raises the serial and carries it
        let KeyAdmission::Added {
            ckid: k1,
            key_serial,
            replaced: 0,
        } = login_with_key(&mut c, a, 1, "dev-1")
        else {
            panic!("added")
        };
        assert_eq!((key_serial, change_serial(&c).unwrap()), (1, 1));
        // the same public key again, in any account: refused
        assert_eq!(login_with_key(&mut c, b, 1, "dev-9"), KeyAdmission::InUse);
        // a new login of the same device replaces its key and the key's token
        let KeyAdmission::Added {
            ckid: k2,
            replaced: 1,
            ..
        } = login_with_key(&mut c, a, 2, "dev-1")
        else {
            panic!("replaced")
        };
        assert_eq!(active(&c, a), vec![vec![2; 32]]);
        assert!(session(&c, &[1; 32], 200, 1000, 5000).unwrap().is_none());
        let s = session(&c, &[2; 32], 200, 1000, 5000).unwrap().unwrap();
        assert_eq!(
            s.key,
            Some(SessionKey {
                ckid: k2,
                public_key: [2; 32]
            })
        );
        // at most three active keys here; a device that has one replaces it rather than adding
        assert!(matches!(
            login_with_key(&mut c, a, 3, "dev-3"),
            KeyAdmission::Added { .. }
        ));
        assert!(matches!(
            login_with_key(&mut c, a, 4, ""),
            KeyAdmission::Added { .. }
        ));
        assert_eq!(login_with_key(&mut c, a, 5, "dev-5"), KeyAdmission::TooMany);
        assert_eq!(login_with_key(&mut c, a, 6, ""), KeyAdmission::TooMany);
        assert!(matches!(
            login_with_key(&mut c, a, 7, "dev-3"),
            KeyAdmission::Added { replaced: 1, .. }
        ));
        assert_eq!(active(&c, a).len(), 3);
        // another account counts apart
        assert!(matches!(
            login_with_key(&mut c, b, 8, "dev-3"),
            KeyAdmission::Added { .. }
        ));
        // revoking: one key, the others, all; each takes the tokens along and raises the serial
        let serial = change_serial(&c).unwrap();
        assert!(!revoke_key(&c, b, k2, "user", 300).unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial);
        assert!(revoke_key(&c, a, k2, "user", 300).unwrap());
        assert!(!revoke_key(&c, a, k2, "user", 300).unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial + 1);
        assert!(session(&c, &[2; 32], 400, 1000, 5000).unwrap().is_none());
        let keep = keys_of(&c, a).unwrap()[0].ckid;
        assert_eq!(revoke_other_keys(&c, a, keep, "others", 300).unwrap(), 1);
        assert_eq!(keys_of(&c, a).unwrap().len(), 1);
        assert!(!revoke_key(&c, a, k1, "user", 300).unwrap());
        assert_eq!(revoke_all_keys(&c, "admin", 330).unwrap(), 2);
        let tokens: i64 = c
            .query_row("SELECT COUNT(*) FROM tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tokens, 0);
        let reasons: Vec<Option<String>> = all_keys(&c)
            .unwrap()
            .into_iter()
            .map(|k| k.revoke_reason)
            .collect();
        assert_eq!(
            reasons,
            [
                "same-device",
                "user",
                "same-device",
                "others",
                "admin",
                "admin"
            ]
            .map(|r| Some(r.to_owned()))
            .to_vec()
        );
    }

    #[test]
    fn confirming_keys() {
        let mut c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        let b = upsert_account(&c, "b", "B", "", 1).unwrap();
        let ckid = |admission: KeyAdmission| match admission {
            KeyAdmission::Added { ckid, .. } => ckid,
            other => panic!("{other:?}"),
        };
        let kb = ckid(login_with_key(&mut c, b, 9, ""));
        let k1 = ckid(login_with_key(&mut c, a, 1, "phone"));
        let k2 = ckid(login_with_key(&mut c, a, 2, "pc"));
        let k3 = ckid(login_with_key(&mut c, a, 3, "pad"));
        let confirmed = |c: &Connection, ckid: i64| {
            let k = keys_of(c, a).unwrap().into_iter().find(|k| k.ckid == ckid);
            k.map(|k| (k.ack_at, k.ack_by))
        };
        // a key confirms only later keys of its own account
        assert!(!ack_key(&c, a, k3, k3, 10).unwrap(), "itself");
        assert!(!ack_key(&c, a, k1, k2, 10).unwrap(), "an older key");
        assert!(
            !ack_key(&c, b, k3, kb, 10).unwrap(),
            "another account's key"
        );
        assert!(
            !ack_key(&c, a, k3, kb, 10).unwrap(),
            "by another account's key"
        );
        assert!(!ack_key(&c, a, 99, k1, 10).unwrap(), "no such key");
        assert_eq!(confirmed(&c, k3), Some((None, None)));
        // the first time stays, and so does the oldest key that confirmed it
        assert!(ack_key(&c, a, k3, k2, 10).unwrap());
        assert!(ack_key(&c, a, k3, k1, 20).unwrap());
        assert!(ack_key(&c, a, k3, k2, 30).unwrap());
        assert_eq!(confirmed(&c, k3), Some((Some(10), Some(k1))));
        // a revoked key neither confirms nor is confirmed
        assert!(revoke_key(&c, a, k1, "user", 40).unwrap());
        assert!(!ack_key(&c, a, k2, k1, 50).unwrap());
        assert!(revoke_key(&c, a, k3, "user", 40).unwrap());
        assert!(!ack_key(&c, a, k3, k2, 50).unwrap());
        // confirming changes no trust list: the serial stays
        let k4 = ckid(login_with_key(&mut c, a, 4, "phone"));
        let serial = change_serial(&c).unwrap();
        assert!(ack_key(&c, a, k4, k2, 60).unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial);
    }

    #[test]
    fn request_nonces() {
        let mut c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        let KeyAdmission::Added { ckid, .. } = login_with_key(&mut c, a, 1, "") else {
            panic!("added")
        };
        assert!(use_req_nonce(&c, ckid, b"n", 100, 1300).unwrap());
        assert!(!use_req_nonce(&c, ckid, b"n", 1300, 2500).unwrap());
        assert!(use_req_nonce(&c, ckid, b"n", 1301, 2501).unwrap());
        // per key
        let KeyAdmission::Added { ckid: other, .. } = login_with_key(&mut c, a, 2, "") else {
            panic!("added")
        };
        assert!(use_req_nonce(&c, other, b"n", 1302, 2502).unwrap());
        // they go with their key
        c.execute("DELETE FROM ctl_keys WHERE ckid = ?1", [ckid])
            .unwrap();
        let left: i64 = c
            .query_row("SELECT COUNT(*) FROM req_nonces", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1);
    }

    #[test]
    fn switches() {
        let c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        let b = upsert_account(&c, "b", "B", "", 1).unwrap();
        // off when installed
        assert!(!passwordless_global(&c).unwrap());
        assert!(!passwordless_on(&c, a).unwrap() && !strict(&c).unwrap());
        // one account first, then everyone
        assert!(set_account_passwordless(&c, a, Some(true)).unwrap());
        assert!(!set_account_passwordless(&c, a, Some(true)).unwrap());
        assert!(passwordless_on(&c, a).unwrap() && !passwordless_on(&c, b).unwrap());
        assert!(set_account_passwordless(&c, b, Some(false)).unwrap());
        assert!(set_passwordless_global(&c, true).unwrap());
        assert!(passwordless_on(&c, a).unwrap() && !passwordless_on(&c, b).unwrap());
        assert_eq!(account_passwordless(&c, b).unwrap(), Some(false));
        // off for everyone, also the account switched on alone
        assert!(set_account_passwordless(&c, b, None).unwrap());
        assert!(set_passwordless_global(&c, false).unwrap());
        assert!(!passwordless_on(&c, a).unwrap() && !passwordless_on(&c, b).unwrap());
        assert_eq!(account_passwordless(&c, a).unwrap(), None);
        assert!(!set_passwordless_global(&c, false).unwrap());
        // every change raises the serial once
        assert_eq!(change_serial(&c).unwrap(), 5);
        assert!(set_strict(&c, true).unwrap() && strict(&c).unwrap());
        assert_eq!(change_serial(&c).unwrap(), 6);
    }

    #[test]
    fn lists_are_on_for_accounts_on_the_allowlist() {
        let mut c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        set_account_passwordless(&c, a, Some(true)).unwrap();
        // its switch on, but the account is not on the allowlist
        assert!(passwordless_on(&c, a).unwrap() && !list_on(&c, a).unwrap());
        allow_add(&mut c, &["a".to_owned()], 1).unwrap();
        assert!(list_on(&c, a).unwrap());
        // off the allowlist, or its switch off
        allow_remove(&mut c, &["a".to_owned()], 2).unwrap();
        assert!(!list_on(&c, a).unwrap());
        allow_add(&mut c, &["a".to_owned()], 3).unwrap();
        set_account_passwordless(&c, a, Some(false)).unwrap();
        assert!(!list_on(&c, a).unwrap());
        // no such account
        assert!(!list_on(&c, a + 1).unwrap());
    }

    #[test]
    fn keys_expire_with_their_tokens() {
        let mut c = conn();
        let t = crate::util::now();
        let a = upsert_account(&c, "a", "A", "", t).unwrap();
        allow_add(&mut c, &["a".to_owned()], t).unwrap();
        for n in [1, 2, 3] {
            login_with_key(&mut c, a, n, "");
        }
        // one login unused for 91 days, one whose token went without its key (the token cap)
        c.execute(
            "UPDATE tokens SET last_used_at = ?1, created_at = ?1 WHERE hash = ?2",
            params![t - 91 * DAY, [1u8; 32].as_slice()],
        )
        .unwrap();
        c.execute(
            "UPDATE tokens SET last_used_at = ?1, created_at = ?1 WHERE hash <> ?2",
            params![t, [1u8; 32].as_slice()],
        )
        .unwrap();
        assert!(delete_token(&c, &[2; 32]).unwrap());
        let p = purge(&mut c, t, &retention()).unwrap();
        assert_eq!((p.tokens, p.keys_revoked), (1, 2));
        assert_eq!(active(&c, a), vec![vec![3; 32]]);
        // revoked keys are kept for 30 days
        let p = purge(&mut c, t + KEY_RETENTION - 1, &retention()).unwrap();
        assert_eq!(p.keys_deleted, 0);
        c.execute("UPDATE tokens SET last_used_at = ?1", [t + KEY_RETENTION])
            .unwrap();
        let p = purge(&mut c, t + KEY_RETENTION, &retention()).unwrap();
        assert_eq!((p.keys_revoked, p.keys_deleted), (0, 2));
        assert_eq!(all_keys(&c).unwrap().len(), 1);
    }

    #[test]
    fn removing_from_the_allowlist_revokes_keys() {
        let mut c = conn();
        allow_add(&mut c, &["a".to_owned()], 1).unwrap();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        login_with_key(&mut c, a, 1, "");
        login_with_key(&mut c, a, 2, "");
        assert_eq!(
            allow_remove(&mut c, &["a".to_owned(), "zz".to_owned()], 500).unwrap(),
            (1, 2, 2)
        );
        assert!(keys_of(&c, a).unwrap().is_empty());
        let counts = counts(&c, 600, DAY).unwrap();
        assert_eq!(counts.keys_active, 0);
        assert_eq!(counts.keys_revoked, vec![("allow-remove".to_owned(), 2)]);
    }

    #[test]
    fn the_allowlist_moves_the_change_serial() {
        let mut c = conn();
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // IDs nobody logged in with: no PC's list changes
        allow_add(&mut c, &ids(&["a", "b"]), 1).unwrap();
        allow_remove(&mut c, &ids(&["b"]), 1).unwrap();
        assert_eq!(change_serial(&c).unwrap(), 0);
        // an account without active keys leaves: its PCs' lists change all the same
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        assert_eq!(allow_remove(&mut c, &ids(&["a"]), 2).unwrap(), (1, 0, 0));
        assert_eq!(change_serial(&c).unwrap(), 1);
        assert_eq!(allow_remove(&mut c, &ids(&["a"]), 3).unwrap(), (0, 0, 0));
        assert_eq!(change_serial(&c).unwrap(), 1);
        // back on the list
        assert_eq!(allow_add(&mut c, &ids(&["a", "b"]), 4).unwrap(), (2, 0));
        assert_eq!(change_serial(&c).unwrap(), 2);
        assert_eq!(allow_add(&mut c, &ids(&["a"]), 5).unwrap(), (0, 1));
        assert_eq!(change_serial(&c).unwrap(), 2);
        // with keys: once, as they are revoked
        login_with_key(&mut c, a, 1, "");
        let serial = change_serial(&c).unwrap();
        assert_eq!(
            allow_remove(&mut c, &ids(&["a", "b"]), 6).unwrap(),
            (2, 1, 1)
        );
        assert_eq!(change_serial(&c).unwrap(), serial + 1);
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
        bind(&c, b, "1", &[1; 32], "windows", "PC", t - 200 * DAY).unwrap();
        bind(&c, b, "2", &[1; 32], "windows", "PC", t - DAY).unwrap();
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
        let p = purge(&mut c, t, &retention()).unwrap();
        assert_eq!((p.tokens, p.devices, p.accounts, p.events), (1, 1, 1, 1));
        assert!(account_by_key(&c, "a").unwrap().is_none());
        assert!(device(&c, "2").unwrap().is_some());
        let _ = a;
        let left: i64 = c
            .query_row("SELECT COUNT(*) FROM attempts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 1, "counters older than the longest window go");
        let counts = counts(&c, t, DAY).unwrap();
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
        bind(&c, lab, "1", &[1; 32], "windows", "PC", old).unwrap();
        assert!(seen_by_hbbs(&c, "1", &[1; 32], t - DAY).unwrap());
        // the same, but off the allowlist since: deleted with its PC
        let gone = upsert_account(&c, "gone", "GONE", "", old).unwrap();
        bind(&c, gone, "2", &[1; 32], "windows", "PC", old).unwrap();
        assert!(seen_by_hbbs(&c, "2", &[1; 32], t - DAY).unwrap());
        allow_remove(&mut c, &["gone".to_owned()], t).unwrap();
        // on the list, its PC not seen for 200 days: the PC goes, then the account
        let idle = upsert_account(&c, "idle", "IDLE", "", old).unwrap();
        allow_add(&mut c, &["idle".to_owned()], old).unwrap();
        bind(&c, idle, "3", &[1; 32], "windows", "PC", t - 200 * DAY).unwrap();
        let p = purge(&mut c, t, &retention()).unwrap();
        assert_eq!((p.accounts, p.devices), (2, 1));
        assert!(account_by_key(&c, "lab").unwrap().is_some());
        assert!(device(&c, "1").unwrap().is_some());
        assert!(account_by_key(&c, "gone").unwrap().is_none());
        assert!(device(&c, "2").unwrap().is_none());
        assert!(account_by_key(&c, "idle").unwrap().is_none());
    }
}
