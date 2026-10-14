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
//! Schema changes are migrations in [`MIGRATIONS`]. Schema 3 (design v3 §5-4, §8) is Wake-on-LAN:
//! the PCs' wake reports, the NAS relays with their pre-approvals and nonces, and the wake jobs
//! with their signed commands (wolinfo.rs, relay.rs, wake.rs). Schema 4 (design v3 §5-2) keeps
//! who ended an account key and what the account's devices answered about it (keys.rs). Schema 5
//! (open login, 2026-10-14) gives each allowlist entry how it came (an import or a login) and its
//! block: the list means "may use HJViewer" as before, an entry that is not blocked, and every
//! check of it ([`allowed`], [`session`], [`list_on`]) leaves blocked entries out. Schema 6 keeps
//! the client address of each bound PC's last sync for the super operator's view (overview.rs).
//!
//! A restored backup (backup.rs, known by its application ID) is opened with password-less
//! connections off for every account, before the service answers anything: its switches are
//! those of its day, and its lists would carry the account keys revoked since (design v3 §5-9).
//! Its relays are those of its day as well, which the journal says when it has active ones: a
//! relay revoked since is revoked again by the operator.
//!
//! Every change that alters what a PC's trust list would say (a key registered or revoked, a
//! switch turned, an account deleted, blocked, unblocked or coming back on the allowlist, a
//! binding released, a PC's key found changed or back) raises the change serial
//! ([`bump_serial`]) in the same transaction; a new key carries the serial its registration made
//! (design v3 §3-5, §5-2).
//!
//! A PC leaves an account in one place only, [`release_binding`]: its row goes, its binding ID
//! goes to the tombstones, so that the PC's next sync gets a signed `revoked` (design v3 §4-3),
//! and the change serial goes up, in one transaction. Nothing else deletes a `devices` row, and an
//! account is deleted only by [`delete_account`], which releases its PCs first (the `ON DELETE
//! CASCADE` of `devices` would leave no tombstone).

use crate::{
    util::DAY,
    wolinfo::{RelayIface, Report},
};
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
    // Schema 3 (design v3 §5-4, "이전 3번"; §8): Wake-on-LAN through NAS relays (WP-W2).
    r#"
-- the wake report of each bound PC (design v3 §8-2; wolinfo.rs), from its last sync that carried
-- one, signed with its device key; it goes with the binding (release_binding)
CREATE TABLE wol_info (
    device_id TEXT NOT NULL PRIMARY KEY,
    reported_at INTEGER NOT NULL,        -- the server's time of the sync that carried it
    info_json TEXT NOT NULL,             -- the report as hjv-api read it (no MAC ever leaves)
    info_hash TEXT NOT NULL,             -- the PC's wol_hash that came with it (compared only)
    nets TEXT NOT NULL,                  -- the networks of its wired cards, canonical, comma-separated
    egress_ip TEXT NOT NULL              -- where its syncs come from (X-Real-IP), the latest
) STRICT;

-- NAS relays (relay/PROTOCOL.md §11; relay.rs): pending until an admin approves them for the
-- account of a PC, active, or revoked, which keeps the ID, the key and the times only
CREATE TABLE relays (
    relay_id TEXT NOT NULL PRIMARY KEY,  -- 'r-' and base32 of the first 10 bytes of SHA-256(pk)
    pk BLOB NOT NULL UNIQUE,
    state TEXT NOT NULL,                 -- 'pending', 'active', 'revoked'
    owner_account INTEGER REFERENCES accounts(id) ON DELETE SET NULL,
    scope TEXT NOT NULL,                 -- 'own'; 'shared' (admin relay share) is kept, not used (W2)
    name TEXT NOT NULL,                  -- what the apps call it
    created_at INTEGER NOT NULL,
    source TEXT,                         -- the address group of its first hello (pending limits)
    approved_at INTEGER,
    revoked_at INTEGER,
    last_hello_at INTEGER,
    last_poll_at INTEGER,                -- its last accepted poll: online when within 60 seconds
    version TEXT,
    arch TEXT,
    os TEXT,
    kernel TEXT,
    ifaces_json TEXT,                    -- its interfaces as hjv-api read them (wolinfo.rs)
    ifaces_hash TEXT,                    -- the ifaces_hash that came with them (compared only)
    egress_ip TEXT
) STRICT;
CREATE INDEX relays_owner ON relays(owner_account, state);
CREATE INDEX relays_state ON relays(state, created_at);

-- codes approved before their relay said hello (PROTOCOL.md §11), for 7 days
CREATE TABLE relay_preapprovals (
    relay_id TEXT NOT NULL PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;

-- the (id, n) of accepted relay requests, for 600 seconds (envelope.rs); 100 000 at most
CREATE TABLE relay_nonces (
    relay_id TEXT NOT NULL,
    nonce BLOB NOT NULL,
    expires_at INTEGER NOT NULL,
    PRIMARY KEY (relay_id, nonce)
) STRICT, WITHOUT ROWID;
CREATE INDEX relay_nonces_expiry ON relay_nonces(expires_at);

-- wake jobs (design v3 §8-4; wake.rs): one PC, one relay, one signed command; the wake record,
-- kept six months (the account as its tag; the rows go with the account)
CREATE TABLE wake_jobs (
    job TEXT NOT NULL PRIMARY KEY,       -- 32 hex digits, 128 random bits
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    account_tag TEXT NOT NULL,
    device_id TEXT NOT NULL,
    relay_id TEXT NOT NULL,
    grade TEXT NOT NULL,                 -- how the relay matched: 'exact', 'overlap', 'history', 'any'
    net TEXT NOT NULL,                   -- the PC's network the command names
    cmd TEXT NOT NULL,                   -- hex SHA-256 of the command text (the results name it)
    created_at INTEGER NOT NULL,
    exp INTEGER NOT NULL,                -- the command's exp
    state TEXT NOT NULL,                 -- 'queued', 'sent', 'confirmed', 'not_confirmed', 'failed'
    sent_at INTEGER,
    packets INTEGER,
    confirmed_at INTEGER,
    error TEXT,
    finished_at INTEGER
) STRICT;
CREATE INDEX wake_jobs_device ON wake_jobs(device_id, created_at);
CREATE INDEX wake_jobs_account ON wake_jobs(account_id, created_at);
CREATE INDEX wake_jobs_relay ON wake_jobs(relay_id, created_at);
CREATE INDEX wake_jobs_state ON wake_jobs(state);
CREATE INDEX wake_jobs_cmd ON wake_jobs(cmd);

-- the commands waiting for their relay (PROTOCOL.md §7.4): the exact text, signature and key
-- number, sent again byte for byte until a result names them or they expire
CREATE TABLE wake_commands (
    cmd_sha256 TEXT NOT NULL PRIMARY KEY,
    relay_id TEXT NOT NULL,
    job TEXT NOT NULL REFERENCES wake_jobs(job) ON DELETE CASCADE,
    text TEXT NOT NULL,
    sig TEXT NOT NULL,
    kid INTEGER NOT NULL,
    iat INTEGER NOT NULL,
    exp INTEGER NOT NULL
) STRICT;
CREATE INDEX wake_commands_relay ON wake_commands(relay_id, iat);
"#,
    // Schema 4 (design v3 §5-2, the decisions U1-a to U1-c of 2026-10-13): the new-device notice
    // waits for an answer, also for keys made before the device that sees it and for keys that
    // ended before anyone answered; the logged-in devices keep the ended ones for 30 days.
    r#"
-- the key whose request ended this one ('user': 로그아웃시키기, 'others': 다른 기기 모두
-- 로그아웃, also the key itself), and the [아님] of a device of the account with that device's key
ALTER TABLE ctl_keys ADD COLUMN revoked_by INTEGER;
ALTER TABLE ctl_keys ADD COLUMN not_mine_at INTEGER;
ALTER TABLE ctl_keys ADD COLUMN not_mine_by INTEGER;

-- [내 기기 맞음] for a key made before the device that answered: it counts for that device only,
-- so that a second login of whoever has the portal password cannot answer for its first
CREATE TABLE key_answers (
    ckid INTEGER NOT NULL REFERENCES ctl_keys(ckid) ON DELETE CASCADE,
    by_ckid INTEGER NOT NULL REFERENCES ctl_keys(ckid) ON DELETE CASCADE,
    at INTEGER NOT NULL,
    PRIMARY KEY (ckid, by_ckid)
) STRICT, WITHOUT ROWID;
CREATE INDEX key_answers_by ON key_answers(by_ckid);
"#,
    // Schema 5 (open login, the user's decision of 2026-10-14): everyone the Yonsei SSO accepts
    // may log in, and a portal ID that is not on the allowlist is put on it at its first login;
    // the operator blocks an entry where removing it was the way to keep someone out (login.rs,
    // admin.rs).
    r#"
-- how an entry came on the list: 'import' (the operator's file, or a block of an ID that had no
-- entry) or 'login' (its first login); blocked_at is set while the operator blocks the entry,
-- which keeps its account out whatever HJV_API_LOGIN says
ALTER TABLE allowlist ADD COLUMN source TEXT NOT NULL DEFAULT 'import';
ALTER TABLE allowlist ADD COLUMN blocked_at INTEGER;
-- an account whose portal ID was taken off the list (allow remove, before blocks) stays out:
-- that is what taking it off meant, and without an entry its next login would register it
INSERT INTO allowlist (login_key, added_at, source, blocked_at)
    SELECT login_key, CAST(strftime('%s', 'now') AS INTEGER), 'import',
           CAST(strftime('%s', 'now') AS INTEGER)
    FROM accounts WHERE login_key NOT IN (SELECT login_key FROM allowlist);
"#,
    // Schema 6 (the super operator's view, overview.rs): where each bound PC's last sync came from.
    r#"
-- the client address of the PC's last accepted sync, by the rule of every client address
-- (X-Real-IP, which nginx sets: server.rs); overwritten by each sync, NULL before the first one
-- or when it was no address, and gone with the row when the PC leaves its account
ALTER TABLE devices ADD COLUMN last_sync_ip TEXT;
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
    let application_id: i32 = conn
        .query_row("PRAGMA application_id", [], |r| r.get(0))
        .map_err(|e| format!("database setup: {e}"))?;
    if application_id == crate::backup::APPLICATION_ID {
        let was_on = restored(&conn).map_err(|e| format!("database setup: {e}"))?;
        log_notice!(
            "the database is a restored backup: password-less connections {} off for every account until `hjv-api admin passwordless on` (README.md, Backup and restore)",
            if was_on { "turned" } else { "stay" }
        );
        // its relays are the backup's too: one revoked since is active again, which only the
        // operator can tell (the record of `relay revoke` is not in the database)
        let relays: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM relays WHERE state = 'active'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if relays > 0 {
            log_notice!(
                "the restored backup has {relays} active NAS {}: one revoked since the backup is active again (`hjv-api admin relay list`, then `relay revoke` what was revoked since; README.md, Backup and restore)",
                if relays == 1 { "relay" } else { "relays" }
            );
        }
    }
    Ok(conn)
}

/// A restored backup, served for the first time (it carries the backups' application ID,
/// backup.rs): password-less connections off for every account, as `passwordless off` turns them
/// (design v3 §5-9). The backup's switches are those of its day, and the lists it would sign carry
/// the account keys revoked since, a lost phone's among them; the restore's `user revoke-all
/// --all` comes after the start, and the PCs sync meanwhile. In the same transaction the database
/// loses the backups' ID, so that it is never taken for a backup that `--keep-days` may delete,
/// and a start that breaks off in between does all of it again. Returns whether a switch was on.
fn restored(conn: &Connection) -> rusqlite::Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let was_on = set_passwordless_global(&tx, false)?;
    tx.pragma_update(None, "application_id", 0)?;
    tx.commit()?;
    Ok(was_on)
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

/// Notes the time of a switch change (a password-less switch, strict, an account blocked,
/// unblocked or coming back on the allowlist): the report counts the PCs that have not synced
/// since, which hjv-api must stay up for after an incident switch (design v3 §5-6).
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
/// the allowlist, not blocked, and password-less connections are on for it. Off the allowlist or
/// blocked, its PCs stay bound with `disabled` lists without keys (§5-6). The PC sync and `admin
/// binding list` both judge by it.
pub fn list_on(c: &Connection, account_id: i64) -> rusqlite::Result<bool> {
    let listed: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM accounts a JOIN allowlist l ON l.login_key = a.login_key
                        WHERE a.id = ?1 AND l.blocked_at IS NULL)",
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

/// Where a case-folded portal ID stands with the allowlist (schema 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// on the list and not blocked: the account may use HJViewer
    Registered,
    /// on the list and blocked by the operator: its login is refused after the SSO
    Blocked,
    /// not on the list
    Absent,
}

pub fn listing(c: &Connection, key: &str) -> rusqlite::Result<Listing> {
    let blocked: Option<bool> = c
        .query_row(
            "SELECT blocked_at IS NOT NULL FROM allowlist WHERE login_key = ?1",
            [key],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match blocked {
        Some(false) => Listing::Registered,
        Some(true) => Listing::Blocked,
        None => Listing::Absent,
    })
}

/// Where a login that the SSO accepted stands with the allowlist: blocked when the entry of the ID
/// typed (`key`) is blocked, or that of the ID the SSO names for the person (`persl_key`, its
/// perslNo case-folded), or that of an account the SSO named so before under another ID, so that
/// a block holds whatever form of the portal ID the SSO takes; else as the typed ID's entry.
pub fn login_listing(c: &Connection, key: &str, persl_key: &str) -> rusqlite::Result<Listing> {
    let typed = listing(c, key)?;
    if typed == Listing::Blocked || (persl_key != key && listing(c, persl_key)? == Listing::Blocked)
    {
        return Ok(Listing::Blocked);
    }
    let blocked: Vec<String> = column(
        c,
        "SELECT a.portal_id FROM accounts a JOIN allowlist l ON l.login_key = a.login_key
         WHERE l.blocked_at IS NOT NULL",
        [],
    )?;
    if blocked
        .iter()
        .any(|portal_id| crate::util::login_key(portal_id) == persl_key)
    {
        return Ok(Listing::Blocked);
    }
    Ok(typed)
}

/// The case-folded portal IDs that the server knows as the same person's as `key`: `key` itself,
/// the ID the SSO named for its account (the account's perslNo), and the IDs of the other accounts
/// the SSO named so. Whether the portal ID typed is always the perslNo is not known (design v2
/// §4-1), so a block and its lifting take them all ([`allow_block`], [`allow_unblock`]).
fn person_keys(c: &Connection, key: &str) -> rusqlite::Result<Vec<String>> {
    let persl = match account_by_key(c, key)? {
        Some(account) => crate::util::login_key(&account.portal_id),
        None => key.to_owned(),
    };
    let mut keys = vec![key.to_owned()];
    if persl != key {
        keys.push(persl.clone());
    }
    for account in accounts(c)? {
        if crate::util::login_key(&account.portal_id) == persl && !keys.contains(&account.login_key)
        {
            keys.push(account.login_key);
        }
    }
    Ok(keys)
}

/// What the allowlist says to a login the SSO accepted ([`admit_login`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    /// on the list already
    Listed,
    /// not on the list, and put on it now (open mode)
    Registered,
    /// blocked by the operator
    Blocked,
    /// not on the list, and no login puts one on it (list mode)
    Refused,
}

/// Admits a login the SSO accepted, inside the login's transaction (login.rs): an ID on the list
/// goes on, a blocked one is refused (also when the ID the SSO names is blocked:
/// [`login_listing`]), and one not on the list is put on it (`source` 'login') when `register`
/// (HJV_API_LOGIN=open), else refused. An account that exists without an entry and comes back on
/// the list this way changes its PCs' trust lists, as with [`allow_add`].
pub fn admit_login(
    c: &Connection,
    key: &str,
    persl_key: &str,
    register: bool,
    at: i64,
) -> rusqlite::Result<Admission> {
    match login_listing(c, key, persl_key)? {
        Listing::Registered => Ok(Admission::Listed),
        Listing::Blocked => Ok(Admission::Blocked),
        Listing::Absent if !register => Ok(Admission::Refused),
        Listing::Absent => {
            c.execute(
                "INSERT INTO allowlist (login_key, added_at, source) VALUES (?1, ?2, 'login')",
                params![key, at],
            )?;
            if account_by_key(c, key)?.is_some() {
                bump_serial(c)?;
                switched(c)?;
            }
            Ok(Admission::Registered)
        }
    }
}

/// Adds case-folded IDs (`source` 'import'); returns (added, already there). An entry already
/// there keeps its source and its block: only [`allow_unblock`] lifts a block, so that importing
/// an old file again lets nobody back in. An account that comes back on the list changes its
/// PCs' trust lists (design v3 §5-2): the change serial goes up.
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

/// What [`allow_block`] did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Blocked {
    /// entries blocked now, and of them the ones that were not on the list (put on it blocked)
    pub blocked: usize,
    pub new: usize,
    /// entries blocked before: nothing changed for them
    pub already: usize,
    pub tokens: usize,
    pub keys: usize,
    /// of the entries blocked now, those of another ID of the same person ([`person_keys`])
    pub person: usize,
}

/// Blocks case-folded IDs (admin block, allow remove): where removing an ID kept its account out
/// while only the list let anyone in, the entry now stays, blocked, so that its next login is
/// refused after the SSO, in either HJV_API_LOGIN mode; an ID not on the list is put on it
/// blocked (`source` 'import'). The other IDs the server knows for the same person are blocked
/// with it ([`person_keys`]: the ID the SSO named for its account and that person's other
/// accounts), so that the block stays when one of those accounts is deleted. Their accounts'
/// tokens and account keys are revoked at once (design v3 §5-6), and their PCs stay bound with
/// lists without keys (§5-2), so the change serial goes up, keys or not.
pub fn allow_block(c: &mut Connection, keys: &[String], at: i64) -> rusqlite::Result<Blocked> {
    in_tx(c, |tx| {
        let (mut b, mut delisted) = (Blocked::default(), false);
        let mut done: Vec<String> = Vec::new();
        for named in keys {
            for key in person_keys(tx, named)? {
                if done.contains(&key) {
                    continue;
                }
                done.push(key.clone());
                let other = &key != named;
                let changed = block_entry(tx, &key, at, &mut b, other)?;
                b.person += usize::from(changed && other);
                if let Some(account) = account_by_key(tx, &key)? {
                    b.tokens += revoke_tokens(tx, account.id)?;
                    b.keys += revoke_account_keys(tx, account.id, "blocked", at)?;
                    delisted |= changed;
                }
            }
        }
        // revoking keys raised it already
        if delisted && b.keys == 0 {
            bump_serial(tx)?;
        }
        if delisted {
            switched(tx)?;
        }
        Ok(b)
    })
}

/// Blocks one entry for [`allow_block`]; whether it changed. An entry blocked before counts as
/// `already` when it is one the operator named (`other` false).
fn block_entry(
    tx: &Connection,
    key: &str,
    at: i64,
    b: &mut Blocked,
    other: bool,
) -> rusqlite::Result<bool> {
    let changed = match listing(tx, key)? {
        Listing::Blocked => {
            b.already += usize::from(!other);
            false
        }
        Listing::Registered => {
            tx.execute(
                "UPDATE allowlist SET blocked_at = ?2 WHERE login_key = ?1",
                params![key, at],
            )?;
            true
        }
        Listing::Absent => {
            tx.execute(
                "INSERT INTO allowlist (login_key, added_at, source, blocked_at)
                 VALUES (?1, ?2, 'import', ?2)",
                params![key, at],
            )?;
            b.new += 1;
            true
        }
    };
    b.blocked += usize::from(changed);
    Ok(changed)
}

/// Lifts the blocks of case-folded IDs (admin unblock), with those of the other IDs the server
/// knows for the same person ([`person_keys`], as [`allow_block`] took them): the entry is on the
/// list again as of `at` (an entry without an account keeps from then on), and its account may
/// log in again; its PCs' lists change with it. Returns (entries unblocked, IDs named that had
/// no block to lift).
pub fn allow_unblock(
    c: &mut Connection,
    keys: &[String],
    at: i64,
) -> rusqlite::Result<(usize, usize)> {
    in_tx(c, |tx| {
        let (mut unblocked, mut untouched, mut relisted) = (0, 0, false);
        let mut done: Vec<String> = Vec::new();
        for named in keys {
            let mut lifted = false;
            for key in person_keys(tx, named)? {
                if done.contains(&key) {
                    continue;
                }
                done.push(key.clone());
                let n = tx.execute(
                    "UPDATE allowlist SET blocked_at = NULL, added_at = ?2
                     WHERE login_key = ?1 AND blocked_at IS NOT NULL",
                    params![key, at],
                )?;
                unblocked += n;
                lifted |= n > 0;
                if n > 0 && !relisted {
                    relisted = account_by_key(tx, &key)?.is_some();
                }
            }
            untouched += usize::from(!lifted);
        }
        if relisted {
            bump_serial(tx)?;
            switched(tx)?;
        }
        Ok((unblocked, untouched))
    })
}

/// Whether the account of a case-folded portal ID may use HJViewer: its entry is on the allowlist
/// and not blocked (schema 5). Everything that asks whether an account is on the list means this.
pub fn allowed(c: &Connection, key: &str) -> rusqlite::Result<bool> {
    c.query_row(
        "SELECT EXISTS (SELECT 1 FROM allowlist WHERE login_key = ?1 AND blocked_at IS NULL)",
        [key],
        |r| r.get(0),
    )
}

/// An entry of the allowlist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListEntry {
    pub login_key: String,
    pub added_at: i64,
    /// 'import' or 'login'
    pub source: String,
    pub blocked_at: Option<i64>,
}

pub fn allowlist(c: &Connection) -> rusqlite::Result<Vec<ListEntry>> {
    let mut stmt = c.prepare(
        "SELECT login_key, added_at, source, blocked_at FROM allowlist ORDER BY login_key",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok(ListEntry {
            login_key: r.get(0)?,
            added_at: r.get(1)?,
            source: r.get(2)?,
            blocked_at: r.get(3)?,
        })
    })?;
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
/// first (reason `account-delete`), so that it gets a signed `revoked`, and each of its relays is
/// revoked; then the account row goes and with it (cascade) its tokens, account keys, their
/// nonces, the key use reported for them, its relay pre-approvals and its wake jobs with their
/// commands. Inside the caller's transaction. Its allowlist entry goes with it unless it is
/// blocked (schema 5: a block stays until it is lifted). What stays is the tombstones, the revoked
/// relays' keys, a blocked entry and the operator record.
pub fn delete_account(c: &Connection, account_id: i64, at: i64) -> rusqlite::Result<bool> {
    let pcs: Vec<String> = column(
        c,
        "SELECT device_id FROM devices WHERE account_id = ?1",
        [account_id],
    )?;
    for device_id in pcs {
        release_binding(c, &device_id, "account-delete", at)?;
    }
    // its relays are revoked, keeping their keys (design v3 §5-4); its pre-approvals and wake
    // jobs go with the account row (cascade)
    let relays: Vec<String> = column(
        c,
        "SELECT relay_id FROM relays WHERE owner_account = ?1",
        [account_id],
    )?;
    for relay_id in relays {
        revoke_relay(c, &relay_id, at)?;
    }
    c.execute(
        "DELETE FROM allowlist WHERE blocked_at IS NULL
             AND login_key = (SELECT login_key FROM accounts WHERE id = ?1)",
        [account_id],
    )?;
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

/// Stores a token. The account's older tokens from the same device go (the app keeps one), and so
/// do their account keys: a login from a device ID that already has one replaces its key
/// ('replaced', design v3 U1-c; the new key gets the new-device notice like any other). Of the
/// rest only the `keep` most recently used tokens stay; returns how many tokens went.
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
        let keys = c.execute(
            "UPDATE ctl_keys SET revoked_at = ?3, revoke_reason = 'replaced'
             WHERE revoked_at IS NULL AND ckid IN (
                 SELECT ckid FROM tokens WHERE account_id = ?1 AND device_id = ?2)",
            params![t.account_id, t.device_id, at],
        )?;
        if keys > 0 {
            bump_serial(c)?;
        }
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
/// than `max`, and its account still on the allowlist and not blocked. An expired token is
/// deleted. Whether the request is signed by the session's key is the caller's to check (api.rs,
/// proof.rs).
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
                        EXISTS (SELECT 1 FROM allowlist l
                                WHERE l.login_key = a.login_key AND l.blocked_at IS NULL),
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
    /// the oldest of the account's keys made before this one that confirmed it
    pub ack_by: Option<i64>,
    pub revoked_at: Option<i64>,
    pub revoke_reason: Option<String>,
    /// the key whose request revoked this one ('user', 'others'; the key itself for its own logout)
    pub revoked_by: Option<i64>,
    /// a device of the account answered [아님] for this key: when, and that device's key
    pub not_mine_at: Option<i64>,
    pub not_mine_by: Option<i64>,
}

const KEY_COLUMNS: &str = "k.ckid, k.account_id, k.pk, k.dev_tag, k.ctl_id, k.platform, k.model,
    k.app_name, k.app_ver, k.login_net, k.key_serial, k.created_at, k.last_login_at, k.ack_at,
    k.ack_by, k.revoked_at, k.revoke_reason, k.revoked_by, k.not_mine_at, k.not_mine_by";

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
        revoked_by: r.get(17)?,
        not_mine_at: r.get(18)?,
        not_mine_by: r.get(19)?,
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
    /// registered with the serial its registration made; `replaced` keys of the same device went,
    /// and `evicted` least recently used ones, which made room
    Added {
        ckid: i64,
        key_serial: i64,
        replaced: usize,
        evicted: usize,
    },
    /// another row has this public key, in any account, revoked or not
    InUse,
}

/// Registers a login's account key, inside the caller's transaction (design v3 §4-1 steps 3-5):
/// a key no row has yet; the account's older keys of the same device (`dev_tag`) are revoked
/// with their tokens; at most `max_active` active keys per account, counting the ones this login
/// replaces as gone: past that, the account's least recently used key is revoked ('evicted') so
/// that whoever knows the portal password now can always log in and log the others out (a full
/// account refused logins, and whoever filled it kept its keys); the change serial goes up once.
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
    let replaced = if k.dev_tag.is_empty() {
        0
    } else {
        c.execute(
            "UPDATE ctl_keys SET revoked_at = ?3, revoke_reason = 'same-device'
             WHERE account_id = ?1 AND dev_tag = ?2 AND revoked_at IS NULL",
            params![k.account_id, k.dev_tag, at],
        )?
    };
    let mut evicted = 0;
    loop {
        let active: i64 = c.query_row(
            "SELECT COUNT(*) FROM ctl_keys WHERE account_id = ?1 AND revoked_at IS NULL",
            [k.account_id],
            |r| r.get(0),
        )?;
        if active < max_active as i64 {
            break;
        }
        // the least recently used: its token's last use, or its last login without one
        let n = c.execute(
            "UPDATE ctl_keys SET revoked_at = ?2, revoke_reason = 'evicted'
             WHERE ckid = (
                 SELECT k.ckid FROM ctl_keys k
                 WHERE k.account_id = ?1 AND k.revoked_at IS NULL
                 ORDER BY COALESCE((SELECT MAX(t.last_used_at) FROM tokens t WHERE t.ckid = k.ckid),
                                   k.last_login_at), k.ckid
                 LIMIT 1)",
            params![k.account_id, at],
        )?;
        if n == 0 {
            break;
        }
        evicted += n;
    }
    if replaced + evicted > 0 {
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
        evicted,
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
/// `by` is the key whose request revoked it, when a device asked for it (keys.rs).
pub fn revoke_key(
    c: &Connection,
    account_id: i64,
    ckid: i64,
    reason: &str,
    by: Option<i64>,
    at: i64,
) -> rusqlite::Result<bool> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?3, revoke_reason = ?4, revoked_by = ?5
         WHERE ckid = ?1 AND account_id = ?2 AND revoked_at IS NULL",
        params![ckid, account_id, at, reason, by],
    )?;
    Ok(revoked(c, n)? > 0)
}

/// A device of the account answered [아님] for key `ckid` (design v3 §1-1, U1-a): the key is
/// marked, revoked with its token if it is still active, and the confirmations it gave count no
/// more, so that a key confirmed only by it gets the notice again. `by` is the asking key, which
/// cannot answer for itself. None if the account has no such key (or it is `by`); else whether
/// the key was active.
pub fn mark_not_mine(
    c: &Connection,
    account_id: i64,
    ckid: i64,
    by: i64,
    at: i64,
) -> rusqlite::Result<Option<bool>> {
    let marked = c.execute(
        "UPDATE ctl_keys SET not_mine_at = COALESCE(not_mine_at, ?3),
                             not_mine_by = COALESCE(not_mine_by, ?4)
         WHERE ckid = ?1 AND account_id = ?2 AND ckid <> ?4",
        params![ckid, account_id, at, by],
    )?;
    if marked == 0 {
        return Ok(None);
    }
    c.execute(
        "UPDATE ctl_keys SET ack_by = NULL, ack_at = NULL WHERE account_id = ?1 AND ack_by = ?2",
        params![account_id, ckid],
    )?;
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?2, revoke_reason = 'user', revoked_by = ?3
         WHERE ckid = ?1 AND revoked_at IS NULL",
        params![ckid, at, by],
    )?;
    Ok(Some(revoked(c, n)? > 0))
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

/// Revokes the account's active keys but `keep`, whose request it is, with their tokens; returns
/// how many.
pub fn revoke_other_keys(
    c: &Connection,
    account_id: i64,
    keep: i64,
    reason: &str,
    at: i64,
) -> rusqlite::Result<usize> {
    let n = c.execute(
        "UPDATE ctl_keys SET revoked_at = ?3, revoke_reason = ?4, revoked_by = ?2
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

/// Key `ckid` of the account, active or ended (until it is deleted), was answered [내 기기 맞음]
/// by the account's active key `by` (design v3 §1-1, U1-a, U1-c). For a key made after `by` the
/// oldest key that confirmed it is kept, and the answer counts for `by` and the devices that
/// logged in after it (keys.rs `new`); for one made before `by`, it counts for `by` only
/// (the `key_answers` table): a later key may be a second login of whoever has the portal
/// password. False for a key the account does not have and for the key itself.
pub fn ack_key(
    c: &Connection,
    account_id: i64,
    ckid: i64,
    by: i64,
    at: i64,
) -> rusqlite::Result<bool> {
    let asker_active: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM ctl_keys
                        WHERE ckid = ?1 AND account_id = ?2 AND revoked_at IS NULL)",
        params![by, account_id],
        |r| r.get(0),
    )?;
    if !asker_active || ckid == by {
        return Ok(false);
    }
    if ckid > by {
        return Ok(c.execute(
            "UPDATE ctl_keys SET ack_at = COALESCE(ack_at, ?4),
                                 ack_by = MIN(COALESCE(ack_by, ?3), ?3)
             WHERE ckid = ?1 AND account_id = ?2",
            params![ckid, account_id, by, at],
        )? > 0);
    }
    let known: bool = c.query_row(
        "SELECT EXISTS (SELECT 1 FROM ctl_keys WHERE ckid = ?1 AND account_id = ?2)",
        params![ckid, account_id],
        |r| r.get(0),
    )?;
    if known {
        c.execute(
            "INSERT OR IGNORE INTO key_answers (ckid, by_ckid, at) VALUES (?1, ?2, ?3)",
            params![ckid, by, at],
        )?;
    }
    Ok(known)
}

/// The keys made before key `by` that it answered [내 기기 맞음] ([`ack_key`]).
pub fn answered_by(c: &Connection, by: i64) -> rusqlite::Result<Vec<i64>> {
    column(c, "SELECT ckid FROM key_answers WHERE by_ckid = ?1", [by])
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

/// The account's revoked keys that are kept (KEY_RETENTION), the latest to end first; `limit`
/// at most.
pub fn ended_keys_of(c: &Connection, account_id: i64, limit: usize) -> rusqlite::Result<Vec<Key>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {KEY_COLUMNS} FROM ctl_keys k WHERE k.account_id = ?1 AND k.revoked_at IS NOT NULL
         ORDER BY k.revoked_at DESC, k.ckid DESC LIMIT ?2"
    ))?;
    let rows = stmt.query_map(params![account_id, limit as i64], key_row)?;
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
/// goes with it (design v3 §10-1): its refusals (cascade), its key use and its wake report. The reasons: `logout`,
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
    // its wake report goes at once (design v3 §10-1); its wake jobs stay as the record
    c.execute("DELETE FROM wol_info WHERE device_id = ?1", [device_id])?;
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

/// Keeps the client address an accepted sync of a bound PC came from (`ip`, server.rs's
/// `client_ip`) as the address of its last sync, in place of the one before; one that is not an
/// address ("invalid", "unknown") is kept as none (schema 6, overview.rs).
pub fn store_sync_address(c: &Connection, device_id: &str, ip: &str) -> rusqlite::Result<()> {
    let ip = ip
        .trim()
        .parse::<std::net::IpAddr>()
        .ok()
        .map(|ip| ip.to_string());
    c.execute(
        "UPDATE devices SET last_sync_ip = ?2 WHERE device_id = ?1",
        params![device_id, ip],
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
    /// Wake-on-LAN (design v3 §8): pending relays past their 24 hours, pre-approvals past their
    /// 7 days, relay nonces, commands past their lifetime, wake jobs past the record's six months
    pub wake: usize,
    /// allowlist entries without an account, not blocked, put on the list more than the account
    /// time ago (schema 5); an account's own entry goes with the account
    pub entries: usize,
}

impl Purged {
    pub fn any(&self) -> bool {
        self.tokens
            + self.devices
            + self.accounts
            + self.entries
            + self.events
            + self.other
            + self.keys_revoked
            + self.keys_deleted
            + self.tombstones
            + self.sync_reports
            + self.wake
            > 0
    }
}

/// How long a revoked account key is kept (design v3 §5-4, §10-1).
pub const KEY_RETENTION: i64 = 30 * DAY;

/// Deletes what is past its keeping time, then truncates the WAL if anything went.
///
/// A PC not seen for the device time (by hbbs, a login or list request from it, or its sync)
/// leaves its account through [`release_binding`]. An account unused for the account time goes
/// with its devices, tokens, account keys and allowlist entry ([`delete_account`]), unless one
/// of its PCs was seen within the device time and the account is on the allowlist, not blocked
/// (in open mode every account that logged in is): a lab PC logged in once stays listed while it
/// is in use, whether or not its owner opens the app (v2 §5 lists the ways a PC leaves the list;
/// the account's own expiry is not one of them), while a blocked account goes after its year as
/// one taken off the list did. An entry without an account (imported and never used, or its
/// account deleted) goes after the account time from when it was put on the list; a blocked
/// entry stays until it is unblocked (schema 5). The account keys of expired tokens are revoked
/// with them (design v3 §14 question 6), and revoked keys are deleted after KEY_RETENTION;
/// tombstones after TOMBSTONE_RETENTION, key use and refusals after USE_DAYS.
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
                 EXISTS (SELECT 1 FROM allowlist l
                         WHERE l.login_key = accounts.login_key AND l.blocked_at IS NULL)
                 AND EXISTS (SELECT 1 FROM devices d
                             WHERE d.account_id = accounts.id AND d.last_seen_at > ?2))",
            [at - r.account, at - r.device_unseen],
        )?;
        for id in unused {
            accounts += usize::from(delete_account(c, id, at)?);
        }
        let entries = c.execute(
            "DELETE FROM allowlist WHERE blocked_at IS NULL AND added_at <= ?1
                 AND NOT EXISTS (SELECT 1 FROM accounts a WHERE a.login_key = allowlist.login_key)",
            [at - r.account],
        )?;
        let since = use_since(at);
        Ok(Purged {
            tokens,
            devices,
            accounts,
            entries,
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
            wake: drop_expired_pending(c, at - crate::relay::PENDING_LIFETIME)?
                + c.execute(
                    "DELETE FROM relay_preapprovals WHERE expires_at <= ?1",
                    [at],
                )?
                + c.execute("DELETE FROM relay_nonces WHERE expires_at < ?1", [at])?
                + c.execute(
                    "DELETE FROM wake_commands WHERE exp < ?1",
                    [at - crate::wake::RESULT_GRACE],
                )?
                + c.execute(
                    "DELETE FROM wake_jobs WHERE created_at <= ?1",
                    [at - r.events],
                )?,
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
    /// allowlist entries that are not blocked, of them the ones a login put there, of those the
    /// ones of the last day and week, and the blocked entries (schema 5)
    pub allowlist: i64,
    pub allowlist_by_login: i64,
    pub registered_day: i64,
    pub registered_week: i64,
    pub blocked: i64,
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
    /// NAS relays by state, the pre-approvals waiting, and each active relay's last poll
    /// (design v3 §5-6)
    pub relays: Vec<(String, i64)>,
    pub preapprovals: i64,
    pub relay_polls: Vec<(String, Option<i64>)>,
    /// the PCs that reported how to wake them
    pub wol_reports: i64,
    /// wake jobs over the last day and the last week: (result, day, week), a failure with its
    /// error ("failed no_iface")
    pub wakes: Vec<(String, i64, i64)>,
}

/// The entries a login put on the allowlist after `since` that are not blocked.
fn registered_since(c: &Connection, since: i64) -> rusqlite::Result<i64> {
    c.query_row(
        "SELECT COUNT(*) FROM allowlist
         WHERE blocked_at IS NULL AND source = 'login' AND added_at > ?1",
        [since],
        |r| r.get(0),
    )
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
        allowlist: one("SELECT COUNT(*) FROM allowlist WHERE blocked_at IS NULL")?,
        allowlist_by_login: one(
            "SELECT COUNT(*) FROM allowlist WHERE blocked_at IS NULL AND source = 'login'",
        )?,
        registered_day: registered_since(c, at - DAY)?,
        registered_week: registered_since(c, at - 7 * DAY)?,
        blocked: one("SELECT COUNT(*) FROM allowlist WHERE blocked_at IS NOT NULL")?,
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
        relays: pairs(
            "SELECT state, COUNT(*) FROM relays GROUP BY state ORDER BY state",
            &[],
        )?,
        preapprovals: c.query_row(
            "SELECT COUNT(*) FROM relay_preapprovals WHERE expires_at > ?1",
            [at],
            |r| r.get(0),
        )?,
        relay_polls: {
            let mut stmt = c.prepare(
                "SELECT relay_id, last_poll_at FROM relays WHERE state = 'active' ORDER BY relay_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        },
        wol_reports: one("SELECT COUNT(*) FROM wol_info")?,
        wakes: {
            let mut stmt = c.prepare(
                "SELECT state || COALESCE(' ' || error, ''), SUM(created_at > ?1), COUNT(*)
                 FROM wake_jobs WHERE created_at > ?2 GROUP BY 1 ORDER BY 1",
            )?;
            let rows = stmt.query_map(params![at - DAY, at - 7 * DAY], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        },
    })
}

// ---- health (check.rs, design v3 §5-9) ----

/// The PCs holding their binding at `at` (as in [`counts`]: their last sync, within a list's
/// lifetime `list_ttl`, carried it), with the time of that sync and the public key stored for
/// them, for the five-minute check.
pub fn holding_pcs(
    c: &Connection,
    at: i64,
    list_ttl: i64,
) -> rusqlite::Result<Vec<(String, i64, Vec<u8>)>> {
    let mut stmt = c.prepare(
        "SELECT device_id, last_sync_at, public_key FROM devices
         WHERE sync_state = 'present' AND last_sync_at > ?1 ORDER BY device_id",
    )?;
    let rows = stmt.query_map([at - list_ttl], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
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

// ---- Wake-on-LAN (schema 3, design v3 §8: wolinfo.rs, relay.rs, wake.rs) ----

/// A bound PC's wake report as kept (design v3 §8-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WolInfo {
    pub device_id: String,
    /// the server's time of the sync that carried the report
    pub reported_at: i64,
    pub report: Report,
    /// the PC's `wol_hash` with it
    pub info_hash: String,
    pub nets: String,
    /// where the PC's syncs come from, the latest
    pub egress_ip: String,
}

const WOL_COLUMNS: &str =
    "w.device_id, w.reported_at, w.info_json, w.info_hash, w.nets, w.egress_ip";

/// A row; `None` when its report does not read (it was written by hjv-api: none should).
fn wol_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Option<WolInfo>> {
    let json: String = r.get(2)?;
    let Ok(report) = serde_json::from_str::<Report>(&json) else {
        return Ok(None);
    };
    Ok(Some(WolInfo {
        device_id: r.get(0)?,
        reported_at: r.get(1)?,
        report,
        info_hash: r.get(3)?,
        nets: r.get(4)?,
        egress_ip: r.get(5)?,
    }))
}

pub fn wol_info(c: &Connection, device_id: &str) -> rusqlite::Result<Option<WolInfo>> {
    Ok(c.query_row(
        &format!("SELECT {WOL_COLUMNS} FROM wol_info w WHERE w.device_id = ?1"),
        [device_id],
        wol_row,
    )
    .optional()?
    .flatten())
}

/// The wake reports of an account's PCs.
pub fn wol_infos_of(c: &Connection, account_id: i64) -> rusqlite::Result<Vec<WolInfo>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {WOL_COLUMNS} FROM wol_info w JOIN devices d ON d.device_id = w.device_id
         WHERE d.account_id = ?1 ORDER BY w.device_id"
    ))?;
    let rows = stmt.query_map([account_id], wol_row)?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect())
}

/// Keeps the wake report a bound PC's sync carried, with the summary that came with it and the
/// address the sync came from.
pub fn store_wol_info(
    c: &Connection,
    device_id: &str,
    report: &Report,
    hash: &str,
    egress_ip: &str,
    at: i64,
) -> rusqlite::Result<()> {
    let json = serde_json::to_string(report)
        .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
    c.execute(
        "INSERT INTO wol_info (device_id, reported_at, info_json, info_hash, nets, egress_ip)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT (device_id) DO UPDATE SET reported_at = excluded.reported_at,
             info_json = excluded.info_json, info_hash = excluded.info_hash,
             nets = excluded.nets, egress_ip = excluded.egress_ip",
        params![
            device_id,
            at,
            json,
            hash,
            report.nets().join(","),
            egress_ip
        ],
    )?;
    Ok(())
}

/// A sync without a report: the address it came from becomes the PC's egress address; returns
/// the summary of the report kept, if one is.
pub fn wol_egress(
    c: &Connection,
    device_id: &str,
    egress_ip: &str,
) -> rusqlite::Result<Option<String>> {
    c.query_row(
        "UPDATE wol_info SET egress_ip = ?2 WHERE device_id = ?1 RETURNING info_hash",
        params![device_id, egress_ip],
        |r| r.get(0),
    )
    .optional()
}

/// A relay's registration state (relay/PROTOCOL.md §11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayState {
    Pending,
    Active,
    Revoked,
}

impl RelayState {
    pub fn as_str(self) -> &'static str {
        match self {
            RelayState::Pending => "pending",
            RelayState::Active => "active",
            RelayState::Revoked => "revoked",
        }
    }

    pub fn parse(text: &str) -> Option<RelayState> {
        Some(match text {
            "pending" => RelayState::Pending,
            "active" => RelayState::Active,
            "revoked" => RelayState::Revoked,
            _ => return None,
        })
    }
}

/// A relay as kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayRow {
    pub relay_id: String,
    pub pk: [u8; 32],
    pub state: RelayState,
    /// the account it wakes PCs of, once approved
    pub owner_account: Option<i64>,
    pub scope: String,
    pub name: String,
    pub created_at: i64,
    /// the address group of its first hello
    pub source: Option<String>,
    pub approved_at: Option<i64>,
    pub revoked_at: Option<i64>,
    pub last_hello_at: Option<i64>,
    pub last_poll_at: Option<i64>,
    pub version: Option<String>,
    pub arch: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub ifaces: Vec<RelayIface>,
    pub ifaces_hash: Option<String>,
    pub egress_ip: Option<String>,
}

const RELAY_COLUMNS: &str = "relay_id, pk, state, owner_account, scope, name, created_at, source,
     approved_at, revoked_at, last_hello_at, last_poll_at, version, arch, os, kernel, ifaces_json,
     ifaces_hash, egress_ip";

fn relay_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RelayRow> {
    let bad = |column: usize, what: &str| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            what.to_owned().into(),
        )
    };
    let pk: Vec<u8> = r.get(1)?;
    let state: String = r.get(2)?;
    let ifaces: Option<String> = r.get(16)?;
    Ok(RelayRow {
        relay_id: r.get(0)?,
        pk: <[u8; 32]>::try_from(pk).map_err(|_| bad(1, "a relay key is 32 bytes"))?,
        state: RelayState::parse(&state).ok_or_else(|| bad(2, "not a relay state"))?,
        owner_account: r.get(3)?,
        scope: r.get(4)?,
        name: r.get(5)?,
        created_at: r.get(6)?,
        source: r.get(7)?,
        approved_at: r.get(8)?,
        revoked_at: r.get(9)?,
        last_hello_at: r.get(10)?,
        last_poll_at: r.get(11)?,
        version: r.get(12)?,
        arch: r.get(13)?,
        os: r.get(14)?,
        kernel: r.get(15)?,
        ifaces: ifaces
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default(),
        ifaces_hash: r.get(17)?,
        egress_ip: r.get(18)?,
    })
}

pub fn relay(c: &Connection, relay_id: &str) -> rusqlite::Result<Option<RelayRow>> {
    c.query_row(
        &format!("SELECT {RELAY_COLUMNS} FROM relays WHERE relay_id = ?1"),
        [relay_id],
        relay_row,
    )
    .optional()
}

/// Every relay kept, the active ones first, then pending, then revoked.
pub fn all_relays(c: &Connection) -> rusqlite::Result<Vec<RelayRow>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {RELAY_COLUMNS} FROM relays
         ORDER BY CASE state WHEN 'active' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END, created_at, relay_id"
    ))?;
    let rows = stmt.query_map([], relay_row)?;
    rows.collect()
}

/// The active relays of an account.
pub fn active_relays_of(c: &Connection, account_id: i64) -> rusqlite::Result<Vec<RelayRow>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {RELAY_COLUMNS} FROM relays WHERE owner_account = ?1 AND state = 'active'
         ORDER BY relay_id"
    ))?;
    let rows = stmt.query_map([account_id], relay_row)?;
    rows.collect()
}

/// A relay seen for the first time: pending, or active for the account of its pre-approval.
pub struct NewRelay<'a> {
    pub relay_id: &'a str,
    pub pk: &'a [u8; 32],
    pub state: RelayState,
    pub owner_account: Option<i64>,
    pub name: &'a str,
    /// the address group a pending entry came from, which its source's limit counts; none for a
    /// relay approved ahead, which no limit holds back
    pub source: Option<&'a str>,
    pub at: i64,
}

pub fn insert_relay(c: &Connection, r: &NewRelay<'_>) -> rusqlite::Result<()> {
    let approved_at = (r.state == RelayState::Active).then_some(r.at);
    c.execute(
        "INSERT INTO relays (relay_id, pk, state, owner_account, scope, name, created_at, source,
             approved_at)
         VALUES (?1, ?2, ?3, ?4, 'own', ?5, ?6, ?7, ?8)",
        params![
            r.relay_id,
            r.pk.as_slice(),
            r.state.as_str(),
            r.owner_account,
            r.name,
            r.at,
            r.source,
            approved_at
        ],
    )?;
    Ok(())
}

/// What a relay's accepted request says of it (PROTOCOL.md §5).
pub struct RelaySeen<'a> {
    pub version: &'a str,
    pub arch: &'a str,
    pub os: &'a str,
    pub kernel: &'a str,
    pub egress_ip: &'a str,
    /// its interfaces as kept (JSON) with the summary that came with them, when it sent them
    pub ifaces: Option<(&'a str, &'a str)>,
    /// a poll (its time is the relay's liveness), or a hello
    pub poll: bool,
}

/// Stores what a pending or active relay's request said; a revoked relay keeps nothing new
/// (PROTOCOL.md §11).
pub fn relay_seen(
    c: &Connection,
    relay_id: &str,
    seen: &RelaySeen<'_>,
    at: i64,
) -> rusqlite::Result<()> {
    let (json, hash) = match seen.ifaces {
        Some((json, hash)) => (Some(json), Some(hash)),
        None => (None, None),
    };
    c.execute(
        "UPDATE relays SET version = ?2, arch = ?3, os = ?4, kernel = ?5, egress_ip = ?6,
             last_hello_at = CASE WHEN ?7 THEN last_hello_at ELSE ?8 END,
             last_poll_at = CASE WHEN ?7 THEN ?8 ELSE last_poll_at END,
             ifaces_json = COALESCE(?9, ifaces_json),
             ifaces_hash = CASE WHEN ?9 IS NULL THEN ifaces_hash ELSE ?10 END
         WHERE relay_id = ?1 AND state <> 'revoked'",
        params![
            relay_id,
            seen.version,
            seen.arch,
            seen.os,
            seen.kernel,
            seen.egress_ip,
            seen.poll,
            at,
            json,
            hash
        ],
    )?;
    Ok(())
}

/// A pending relay becomes active for an account; false when it is not pending.
pub fn approve_relay(
    c: &Connection,
    relay_id: &str,
    account_id: i64,
    name: &str,
    at: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE relays SET state = 'active', owner_account = ?2, name = ?3, approved_at = ?4
         WHERE relay_id = ?1 AND state = 'pending'",
        params![relay_id, account_id, name, at],
    )? > 0)
}

/// What [`revoke_relay`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revoked {
    NoSuchRelay,
    Already,
    /// revoked now; the number of its wake jobs that were waiting and failed with it
    Now(usize),
}

/// Revokes a relay (PROTOCOL.md §11): it keeps its ID, key and times, and loses its owner,
/// interfaces and addresses; its queued commands go and their jobs fail (`not_active`).
pub fn revoke_relay(c: &Connection, relay_id: &str, at: i64) -> rusqlite::Result<Revoked> {
    let Some(state) = c
        .query_row(
            "SELECT state FROM relays WHERE relay_id = ?1",
            [relay_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(Revoked::NoSuchRelay);
    };
    if state == RelayState::Revoked.as_str() {
        return Ok(Revoked::Already);
    }
    c.execute(
        "UPDATE relays SET state = 'revoked', revoked_at = ?2, owner_account = NULL, scope = 'own',
             name = '', source = NULL, approved_at = NULL, last_hello_at = NULL,
             last_poll_at = NULL, version = NULL, arch = NULL, os = NULL, kernel = NULL,
             ifaces_json = NULL, ifaces_hash = NULL, egress_ip = NULL
         WHERE relay_id = ?1",
        params![relay_id, at],
    )?;
    c.execute("DELETE FROM wake_commands WHERE relay_id = ?1", [relay_id])?;
    let failed = c.execute(
        "UPDATE wake_jobs SET state = 'failed', error = 'not_active', finished_at = ?2
         WHERE relay_id = ?1 AND state = 'queued'",
        params![relay_id, at],
    )?;
    Ok(Revoked::Now(failed))
}

/// `relay share on|off` (kept for later; W2 wakes the owner's PCs only); false when the relay
/// is not active.
pub fn set_relay_scope(c: &Connection, relay_id: &str, shared: bool) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE relays SET scope = ?2 WHERE relay_id = ?1 AND state = 'active'",
        params![relay_id, if shared { "shared" } else { "own" }],
    )? > 0)
}

/// A new name for a relay that is active for `account_id` (what the apps call it); false when it
/// is not.
pub fn rename_relay(
    c: &Connection,
    relay_id: &str,
    account_id: i64,
    name: &str,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE relays SET name = ?3
         WHERE relay_id = ?1 AND state = 'active' AND owner_account = ?2",
        params![relay_id, account_id, name],
    )? > 0)
}

/// Deletes the pending entries made at or before `before` (a pending entry lives 24 hours); the
/// relay is unknown again.
pub fn drop_expired_pending(c: &Connection, before: i64) -> rusqlite::Result<usize> {
    c.execute(
        "DELETE FROM relays WHERE state = 'pending' AND created_at <= ?1",
        [before],
    )
}

/// The pending entries now, and the relays first seen from `source` after `since`, with the
/// time of the first of those.
pub fn pending_counts(
    c: &Connection,
    source: &str,
    since: i64,
) -> rusqlite::Result<(i64, i64, Option<i64>)> {
    let total = c.query_row(
        "SELECT COUNT(*) FROM relays WHERE state = 'pending'",
        [],
        |r| r.get(0),
    )?;
    let (from_source, first) = c.query_row(
        "SELECT COUNT(*), MIN(created_at) FROM relays WHERE source = ?1 AND created_at > ?2",
        params![source, since],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok((total, from_source, first))
}

/// The oldest pending entry's time.
pub fn oldest_pending(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT MIN(created_at) FROM relays WHERE state = 'pending'",
        [],
        |r| r.get(0),
    )
}

/// A code approved before its relay's first hello (PROTOCOL.md §11).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preapproval {
    pub relay_id: String,
    pub account_id: i64,
    pub name: String,
    pub created_at: i64,
    pub expires_at: i64,
}

fn preapproval_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Preapproval> {
    Ok(Preapproval {
        relay_id: r.get(0)?,
        account_id: r.get(1)?,
        name: r.get(2)?,
        created_at: r.get(3)?,
        expires_at: r.get(4)?,
    })
}

/// The pre-approval of a relay ID that has not expired at `at`.
pub fn preapproval(
    c: &Connection,
    relay_id: &str,
    at: i64,
) -> rusqlite::Result<Option<Preapproval>> {
    c.query_row(
        "SELECT relay_id, account_id, name, created_at, expires_at FROM relay_preapprovals
         WHERE relay_id = ?1 AND expires_at > ?2",
        params![relay_id, at],
        preapproval_row,
    )
    .optional()
}

pub fn preapprovals(c: &Connection, at: i64) -> rusqlite::Result<Vec<Preapproval>> {
    let mut stmt = c.prepare(
        "SELECT relay_id, account_id, name, created_at, expires_at FROM relay_preapprovals
         WHERE expires_at > ?1 ORDER BY created_at, relay_id",
    )?;
    let rows = stmt.query_map([at], preapproval_row)?;
    rows.collect()
}

/// Approves a code ahead of its relay (again: the later approval counts).
pub fn preapprove(c: &Connection, p: &Preapproval) -> rusqlite::Result<()> {
    c.execute(
        "INSERT INTO relay_preapprovals (relay_id, account_id, name, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT (relay_id) DO UPDATE SET account_id = excluded.account_id,
             name = excluded.name, created_at = excluded.created_at,
             expires_at = excluded.expires_at",
        params![p.relay_id, p.account_id, p.name, p.created_at, p.expires_at],
    )?;
    Ok(())
}

pub fn delete_preapproval(c: &Connection, relay_id: &str) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "DELETE FROM relay_preapprovals WHERE relay_id = ?1",
        [relay_id],
    )? > 0)
}

/// What became of a relay request's (`id`, `n`) (PROTOCOL.md §4.3 step 6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceUse {
    /// new: recorded until it expires
    Fresh,
    /// accepted before, not expired
    Replayed,
    /// the store holds `max` live entries: nothing recorded (503, rather than forget one early)
    Full,
}

pub fn use_relay_nonce(
    c: &Connection,
    relay_id: &str,
    nonce: &[u8],
    at: i64,
    expires_at: i64,
    max: i64,
) -> rusqlite::Result<NonceUse> {
    let seen: Option<i64> = c
        .query_row(
            "SELECT expires_at FROM relay_nonces WHERE relay_id = ?1 AND nonce = ?2",
            params![relay_id, nonce],
            |r| r.get(0),
        )
        .optional()?;
    if seen.is_some_and(|e| e >= at) {
        return Ok(NonceUse::Replayed);
    }
    let live: i64 = c.query_row(
        "SELECT COUNT(*) FROM relay_nonces WHERE expires_at >= ?1",
        [at],
        |r| r.get(0),
    )?;
    if live >= max {
        return Ok(NonceUse::Full);
    }
    c.execute(
        "INSERT INTO relay_nonces (relay_id, nonce, expires_at) VALUES (?1, ?2, ?3)
         ON CONFLICT (relay_id, nonce) DO UPDATE SET expires_at = excluded.expires_at",
        params![relay_id, nonce, expires_at],
    )?;
    Ok(NonceUse::Fresh)
}

/// A wake job (design v3 §8-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub job: String,
    pub account_id: i64,
    pub account_tag: String,
    pub device_id: String,
    pub relay_id: String,
    pub grade: String,
    pub net: String,
    pub cmd: String,
    pub created_at: i64,
    pub exp: i64,
    /// queued, sent, confirmed, not_confirmed, failed
    pub state: String,
    pub sent_at: Option<i64>,
    pub packets: Option<i64>,
    pub confirmed_at: Option<i64>,
    pub error: Option<String>,
    pub finished_at: Option<i64>,
}

const JOB_COLUMNS: &str = "job, account_id, account_tag, device_id, relay_id, grade, net, cmd,
     created_at, exp, state, sent_at, packets, confirmed_at, error, finished_at";

fn job_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    Ok(Job {
        job: r.get(0)?,
        account_id: r.get(1)?,
        account_tag: r.get(2)?,
        device_id: r.get(3)?,
        relay_id: r.get(4)?,
        grade: r.get(5)?,
        net: r.get(6)?,
        cmd: r.get(7)?,
        created_at: r.get(8)?,
        exp: r.get(9)?,
        state: r.get(10)?,
        sent_at: r.get(11)?,
        packets: r.get(12)?,
        confirmed_at: r.get(13)?,
        error: r.get(14)?,
        finished_at: r.get(15)?,
    })
}

/// A signed command for a relay's queue (PROTOCOL.md §7.2).
pub struct NewCommand<'a> {
    pub cmd: &'a str,
    pub text: &'a str,
    pub sig: &'a str,
    pub kid: u32,
    pub iat: i64,
}

/// A new job with its command, queued for the job's relay.
pub fn insert_job(c: &Connection, job: &Job, command: &NewCommand<'_>) -> rusqlite::Result<()> {
    c.execute(
        &format!(
            "INSERT INTO wake_jobs ({JOB_COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)"
        ),
        params![
            job.job,
            job.account_id,
            job.account_tag,
            job.device_id,
            job.relay_id,
            job.grade,
            job.net,
            job.cmd,
            job.created_at,
            job.exp,
            job.state,
            job.sent_at,
            job.packets,
            job.confirmed_at,
            job.error,
            job.finished_at
        ],
    )?;
    c.execute(
        "INSERT INTO wake_commands (cmd_sha256, relay_id, job, text, sig, kid, iat, exp)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            command.cmd,
            job.relay_id,
            job.job,
            command.text,
            command.sig,
            command.kid,
            command.iat,
            job.exp
        ],
    )?;
    Ok(())
}

pub fn job(c: &Connection, job: &str) -> rusqlite::Result<Option<Job>> {
    c.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM wake_jobs WHERE job = ?1"),
        [job],
        job_row,
    )
    .optional()
}

/// The latest job an account made for a PC. Only its own: the jobs another account made for the
/// same ID (a PC that changed hands) are not this account's to be answered with or to wait for.
pub fn latest_job_of(
    c: &Connection,
    device_id: &str,
    account_id: i64,
) -> rusqlite::Result<Option<Job>> {
    c.query_row(
        &format!(
            "SELECT {JOB_COLUMNS} FROM wake_jobs WHERE device_id = ?1 AND account_id = ?2
             ORDER BY created_at DESC, rowid DESC LIMIT 1"
        ),
        params![device_id, account_id],
        job_row,
    )
    .optional()
}

/// The jobs not finished yet (queued or sent), oldest first.
pub fn open_jobs(c: &Connection) -> rusqlite::Result<Vec<Job>> {
    let mut stmt = c.prepare(&format!(
        "SELECT {JOB_COLUMNS} FROM wake_jobs WHERE state IN ('queued', 'sent')
         ORDER BY created_at, rowid"
    ))?;
    let rows = stmt.query_map([], job_row)?;
    rows.collect()
}

/// The jobs an account made since `since`, and those of a relay.
pub fn jobs_of_account_since(c: &Connection, account_id: i64, since: i64) -> rusqlite::Result<i64> {
    c.query_row(
        "SELECT COUNT(*) FROM wake_jobs WHERE account_id = ?1 AND created_at > ?2",
        params![account_id, since],
        |r| r.get(0),
    )
}

pub fn jobs_of_relay_since(c: &Connection, relay_id: &str, since: i64) -> rusqlite::Result<i64> {
    c.query_row(
        "SELECT COUNT(*) FROM wake_jobs WHERE relay_id = ?1 AND created_at > ?2",
        params![relay_id, since],
        |r| r.get(0),
    )
}

/// The networks on which a relay woke a PC before (its confirmed jobs), for the grade `history`.
pub fn woken_on(c: &Connection, relay_id: &str, device_id: &str) -> rusqlite::Result<Vec<String>> {
    column(
        c,
        "SELECT DISTINCT net FROM wake_jobs WHERE relay_id = ?1 AND device_id = ?2
         AND state = 'confirmed' ORDER BY net",
        [relay_id, device_id],
    )
}

/// The queued commands of a relay that have not expired at `at`, oldest first, at most `limit`:
/// (text, signature, key number), the very bytes queued (PROTOCOL.md §7.4).
pub fn queued_commands(
    c: &Connection,
    relay_id: &str,
    at: i64,
    limit: usize,
) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut stmt = c.prepare(
        "SELECT text, sig, kid FROM wake_commands WHERE relay_id = ?1 AND exp > ?2
         ORDER BY iat, rowid LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![relay_id, at, limit as i64], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    rows.collect()
}

/// The job of a relay's command, by the command's hash.
pub fn job_by_cmd(c: &Connection, relay_id: &str, cmd: &str) -> rusqlite::Result<Option<Job>> {
    c.query_row(
        &format!("SELECT {JOB_COLUMNS} FROM wake_jobs WHERE cmd = ?1 AND relay_id = ?2"),
        [cmd, relay_id],
        job_row,
    )
    .optional()
}

/// The command is answered: it leaves the queue.
pub fn delete_command(c: &Connection, cmd: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM wake_commands WHERE cmd_sha256 = ?1", [cmd])?;
    Ok(())
}

/// The job is over: its command leaves the queue.
pub fn delete_commands_of_job(c: &Connection, job: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM wake_commands WHERE job = ?1", [job])?;
    Ok(())
}

/// The relay sent the job's packets: `sent`, from `queued` or from a `no_result` that the result
/// came after; false when the job had moved on.
pub fn job_sent(c: &Connection, job: &str, sent_at: i64, packets: i64) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE wake_jobs SET state = 'sent', sent_at = ?2, packets = ?3, error = NULL,
             finished_at = NULL
         WHERE job = ?1 AND (state = 'queued' OR (state = 'failed' AND error = 'no_result'))",
        params![job, sent_at, packets],
    )? > 0)
}

/// The job failed with `error`: a relay's refusal (from `queued`, or a `no_result` it came after),
/// or the server's `no_result` (from `queued` only).
pub fn job_failed(c: &Connection, job: &str, error: &str, at: i64) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE wake_jobs SET state = 'failed', error = ?2, packets = COALESCE(packets, 0),
             finished_at = ?3
         WHERE job = ?1 AND (state = 'queued'
             OR (?2 <> 'no_result' AND state = 'failed' AND error = 'no_result'))",
        params![job, error, at],
    )? > 0)
}

/// The PC is on after the packets went: `confirmed` at `confirmed_at`.
pub fn job_confirmed(
    c: &Connection,
    job: &str,
    confirmed_at: i64,
    at: i64,
) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE wake_jobs SET state = 'confirmed', confirmed_at = ?2, finished_at = ?3
         WHERE job = ?1 AND state = 'sent'",
        params![job, confirmed_at, at],
    )? > 0)
}

/// Five minutes passed after the packets without a sign of the PC.
pub fn job_not_confirmed(c: &Connection, job: &str, at: i64) -> rusqlite::Result<bool> {
    Ok(c.execute(
        "UPDATE wake_jobs SET state = 'not_confirmed', finished_at = ?2
         WHERE job = ?1 AND state = 'sent'",
        params![job, at],
    )? > 0)
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
        // on to the latest schema (3: Wake-on-LAN)
        assert_eq!(v, MIGRATIONS.len() as i64);
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
        // blocked: no session, tokens revoked, the entry kept blocked
        insert_token(&mut c, &token(b"h3", id, ""), 100, 20).unwrap();
        assert!(session(&c, b"h3", 200, 1000, 5000).unwrap().is_some());
        assert_eq!(
            allow_block(&mut c, &["a1".to_owned()], 300).unwrap(),
            Blocked {
                blocked: 1,
                tokens: 1,
                ..Blocked::default()
            }
        );
        assert!(session(&c, b"h3", 200, 1000, 5000).unwrap().is_none());
        assert!(!allowed(&c, "a1").unwrap());
        assert_eq!(listing(&c, "a1").unwrap(), Listing::Blocked);
        assert!(allowed(&c, "b2").unwrap());
        // a token of a blocked account opens nothing, however it came
        insert_token(&mut c, &token(b"h4", id, ""), 300, 20).unwrap();
        assert!(session(&c, b"h4", 301, 1000, 5000).unwrap().is_none());
        // unblocked, it does again
        assert_eq!(
            allow_unblock(&mut c, &["a1".to_owned()], 302).unwrap(),
            (1, 0)
        );
        assert!(session(&c, b"h4", 303, 1000, 5000).unwrap().is_some());
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
        let application_id = |c: &Connection| -> i32 {
            c.query_row("PRAGMA application_id", [], |r| r.get(0))
                .unwrap()
        };
        // the switches on, globally and for one account, one account switched off on its own
        let (a, b, serial) = {
            let c = open(&path).unwrap();
            let a = upsert_account(&c, "a", "A", "", 1).unwrap();
            let b = upsert_account(&c, "b", "B", "", 1).unwrap();
            set_passwordless_global(&c, true).unwrap();
            set_account_passwordless(&c, a, Some(true)).unwrap();
            set_account_passwordless(&c, b, Some(false)).unwrap();
            c.execute("UPDATE meta SET value = '0' WHERE key = 'switched_at'", [])
                .unwrap();
            (a, b, change_serial(&c).unwrap())
        };
        // a database that is no backup keeps its switches
        let c = open(&path).unwrap();
        assert!(passwordless_global(&c).unwrap() && passwordless_on(&c, a).unwrap());
        c.pragma_update(None, "application_id", crate::backup::APPLICATION_ID)
            .unwrap();
        // (a relay of the backup's day, active)
        insert_relay(
            &c,
            &NewRelay {
                relay_id: "r-restored",
                pk: &[7; 32],
                state: RelayState::Active,
                owner_account: Some(a),
                name: "NAS",
                source: None,
                at: 1,
            },
        )
        .unwrap();
        drop(c);
        // served as a restored backup: off for every account before anything reads it, the
        // change serial up and the switch's time now, and no backup's ID; the journal names the
        // relays that came back with it
        crate::log::capture();
        let c = open(&path).unwrap();
        let said = |what: &str| crate::log::captured().iter().any(|l| l.contains(what));
        assert!(said(
            "the database is a restored backup: password-less connections turned off"
        ));
        assert!(said("the restored backup has 1 active NAS relay:"));
        assert_eq!(application_id(&c), 0);
        assert!(!passwordless_global(&c).unwrap());
        assert!(!passwordless_on(&c, a).unwrap() && !passwordless_on(&c, b).unwrap());
        assert_eq!(account_passwordless(&c, a).unwrap(), None);
        assert_eq!(account_passwordless(&c, b).unwrap(), Some(false));
        assert_eq!(change_serial(&c).unwrap(), serial + 1);
        assert!(switched_at(&c).unwrap().unwrap() > 0);
        drop(c);
        // once: opened again it is the database served, switched as the operator leaves it
        let c = open(&path).unwrap();
        set_passwordless_global(&c, true).unwrap();
        drop(c);
        let c = open(&path).unwrap();
        assert!(passwordless_global(&c).unwrap());
        // a backup made while everything was off stays as it is, but for its ID
        set_passwordless_global(&c, false).unwrap();
        let serial = change_serial(&c).unwrap();
        c.pragma_update(None, "application_id", crate::backup::APPLICATION_ID)
            .unwrap();
        drop(c);
        let c = open(&path).unwrap();
        assert_eq!(application_id(&c), 0);
        assert_eq!(change_serial(&c).unwrap(), serial);
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
        // with the key stored for them, which the check compares with the one hbbs holds now
        bind(&c, a, "2", &[2; 32], "windows", "PC", 6).unwrap();
        assert_eq!(
            holding_pcs(&c, 1100, 600).unwrap(),
            vec![("1".to_owned(), 1000, vec![1; 32])]
        );
        assert_eq!(
            holding_pcs(&c, 1100, 1000).unwrap(),
            vec![
                ("1".to_owned(), 1000, vec![1; 32]),
                ("2".to_owned(), 400, vec![2; 32])
            ]
        );
        assert_eq!(counts(&c, 1100, 1000).unwrap().pcs_holding, 2);
        // the relays (phase 2's schema 3): the active ones, and of them those silent for long;
        // a database without the table has nothing to say
        let bare = Connection::open_in_memory().unwrap();
        assert_eq!(relay_health(&bare, 5000, 3600).unwrap(), None);
        assert_eq!(relay_health(&c, 5000, 3600).unwrap(), Some((0, 0)));
        for (i, (id, state, last)) in [
            ("r-1", "active", Some(4900)),
            ("r-2", "active", Some(1000)),
            ("r-3", "active", None),
            ("r-4", "pending", None),
            ("r-5", "revoked", Some(10)),
        ]
        .into_iter()
        .enumerate()
        {
            c.execute(
                "INSERT INTO relays (relay_id, pk, state, scope, name, created_at, last_poll_at)
                 VALUES (?1, ?2, ?3, 'own', '', 0, ?4)",
                params![id, vec![i as u8; 32], state, last],
            )
            .unwrap();
        }
        assert_eq!(relay_health(&c, 5000, 3600).unwrap(), Some((3, 2)));
        assert_eq!(relay_health(&c, 5000, 4500).unwrap(), Some((3, 1)));
    }

    /// The upgrades to Wake-on-LAN's schema and to the answers of schema 4 keep what schema 2
    /// holds and add their tables empty.
    #[test]
    fn schema_two_moves_to_three() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(MIGRATIONS[0]).unwrap();
        c.execute_batch(MIGRATIONS[1]).unwrap();
        c.pragma_update(None, "user_version", 2).unwrap();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        bind(&c, a, "1", &[1; 32], "windows", "PC", 5).unwrap();
        set_passwordless_global(&c, true).unwrap();
        let serial = change_serial(&c).unwrap();
        let c = prepare(c).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        assert_eq!(device(&c, "1").unwrap().unwrap().account_id, a);
        assert!(passwordless_global(&c).unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial);
        for table in [
            "wol_info",
            "relays",
            "relay_preapprovals",
            "relay_nonces",
            "wake_jobs",
            "wake_commands",
            "key_answers",
        ] {
            let n: i64 = c
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
        assert_eq!(relay_health(&c, 5000, 3600).unwrap(), Some((0, 0)));
    }

    /// Schema 5 keeps what schema 4 holds, and the address of each PC's last sync is none until
    /// its next sync; an address that is not one is kept as none; it goes with the binding.
    #[test]
    fn schema_six_keeps_the_sync_address() {
        let c = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..4] {
            c.execute_batch(sql).unwrap();
        }
        c.pragma_update(None, "user_version", 4).unwrap();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        bind(&c, a, "1", &[1; 32], "windows", "PC", 5).unwrap();
        let c = prepare(c).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        assert_eq!(device(&c, "1").unwrap().unwrap().account_id, a);
        let ip = |c: &Connection| -> Option<String> {
            c.query_row(
                "SELECT last_sync_ip FROM devices WHERE device_id = '1'",
                [],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(ip(&c), None);
        for (seen, kept) in [
            ("192.0.2.7", Some("192.0.2.7")),
            (" 2001:DB8::1 ", Some("2001:db8::1")),
            ("invalid", None),
            ("unknown", None),
            ("198.51.100.4", Some("198.51.100.4")),
        ] {
            store_sync_address(&c, "1", seen).unwrap();
            assert_eq!(ip(&c).as_deref(), kept, "{seen}");
        }
        // a PC that no account has gets no row
        store_sync_address(&c, "2", "192.0.2.8").unwrap();
        assert!(device(&c, "2").unwrap().is_none());
        assert!(release_binding(&c, "1", "admin", 9).unwrap());
        let kept: i64 = c
            .query_row(
                "SELECT COUNT(*) FROM devices WHERE last_sync_ip IS NOT NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(kept, 0);
    }

    fn new_job(id: &str, device: &str, relay: &str, at: i64) -> Job {
        Job {
            job: id.to_owned(),
            account_id: 1,
            account_tag: "tag".into(),
            device_id: device.to_owned(),
            relay_id: relay.to_owned(),
            grade: "exact".into(),
            net: "192.0.2.0/24".into(),
            cmd: format!("cmd-{id}"),
            created_at: at,
            exp: at + 60,
            state: "queued".into(),
            sent_at: None,
            packets: None,
            confirmed_at: None,
            error: None,
            finished_at: None,
        }
    }

    fn queue(c: &Connection, j: &Job) {
        let command = NewCommand {
            cmd: &j.cmd,
            text: "{}",
            sig: "sig",
            kid: 1,
            iat: j.created_at,
        };
        insert_job(c, j, &command).unwrap();
    }

    /// Schema 3 (design v3 §5-4, §8): the relays' states, their nonces, the jobs' states.
    #[test]
    fn wake_on_lan_rows() {
        let c = conn();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        bind(&c, a, "1", &[1; 32], "windows", "PC", 5).unwrap();
        // the nonces of the relays' requests: once, again after their time, and no more than max
        let n = |nonce: u8, at: i64, max: i64| {
            use_relay_nonce(&c, "r-1", &[nonce; 16], at, at + 600, max).unwrap()
        };
        assert_eq!(n(1, 100, 2), NonceUse::Fresh);
        assert_eq!(n(1, 700, 2), NonceUse::Replayed);
        assert_eq!(n(2, 100, 2), NonceUse::Fresh);
        assert_eq!(n(3, 100, 2), NonceUse::Full);
        assert_eq!(
            n(1, 100, 2),
            NonceUse::Replayed,
            "a replay is a replay, also when full"
        );
        assert_eq!(n(1, 701, 5), NonceUse::Fresh, "expired: free again");
        assert_eq!(
            n(3, 701, 2),
            NonceUse::Fresh,
            "the expired ones do not count"
        );
        // a relay: pending, seen, approved, revoked to its ID, key and times
        let new = |id: &str, pk: u8, state: RelayState| {
            insert_relay(
                &c,
                &NewRelay {
                    relay_id: id,
                    pk: &[pk; 32],
                    state,
                    owner_account: None,
                    name: "",
                    source: Some("192.0.2.9"),
                    at: 1000,
                },
            )
            .unwrap();
        };
        new("r-1", 1, RelayState::Pending);
        let seen = |poll: bool, ifaces: Option<(&str, &str)>, at: i64| {
            relay_seen(
                &c,
                "r-1",
                &RelaySeen {
                    version: "1.6.0",
                    arch: "x86_64",
                    os: "DSM",
                    kernel: "5",
                    egress_ip: "192.0.2.9",
                    ifaces,
                    poll,
                },
                at,
            )
            .unwrap();
        };
        seen(
            false,
            Some((
                r#"[{"name":"eth0","mac":null,"ipv4":["192.0.2.10/24"],"up":true,"running":true,"gateway":null,"gateway_mac":null}]"#,
                "h1",
            )),
            1001,
        );
        seen(true, None, 1002);
        let r = relay(&c, "r-1").unwrap().unwrap();
        assert_eq!((r.last_hello_at, r.last_poll_at), (Some(1001), Some(1002)));
        assert_eq!((r.ifaces.len(), r.ifaces_hash.as_deref()), (1, Some("h1")));
        assert_eq!(
            pending_counts(&c, "192.0.2.9", 0).unwrap(),
            (1, 1, Some(1000))
        );
        assert!(!approve_relay(&c, "r-0", a, "NAS", 1003).unwrap());
        assert!(approve_relay(&c, "r-1", a, "NAS", 1003).unwrap());
        assert!(
            !approve_relay(&c, "r-1", a, "NAS", 1004).unwrap(),
            "pending only"
        );
        assert_eq!(active_relays_of(&c, a).unwrap().len(), 1);
        assert!(set_relay_scope(&c, "r-1", true).unwrap());
        // a new name, for the account it is active for only
        assert!(!rename_relay(&c, "r-1", a + 1, "Lab NAS").unwrap());
        assert!(rename_relay(&c, "r-1", a, "Lab NAS").unwrap());
        assert_eq!(relay(&c, "r-1").unwrap().unwrap().name, "Lab NAS");
        // its jobs: queued, sent by its result, confirmed; a refusal; no_result and a late result
        queue(&c, &new_job("j1", "1", "r-1", 2000));
        assert_eq!(queued_commands(&c, "r-1", 2059, 16).unwrap().len(), 1);
        assert_eq!(
            queued_commands(&c, "r-1", 2060, 16).unwrap().len(),
            0,
            "expired"
        );
        assert!(
            !job_confirmed(&c, "j1", 2010, 2011).unwrap(),
            "not before it was sent"
        );
        assert!(job_sent(&c, "j1", 2005, 12).unwrap());
        assert!(
            !job_sent(&c, "j1", 2006, 6).unwrap(),
            "the first result counts"
        );
        assert!(!job_failed(&c, "j1", "no_iface", 2007).unwrap());
        assert!(job_confirmed(&c, "j1", 2010, 2011).unwrap());
        assert!(!job_not_confirmed(&c, "j1", 2400).unwrap());
        assert_eq!(woken_on(&c, "r-1", "1").unwrap(), vec!["192.0.2.0/24"]);
        queue(&c, &new_job("j2", "1", "r-1", 3000));
        assert!(job_failed(&c, "j2", "no_result", 3200).unwrap());
        assert!(!job_failed(&c, "j2", "no_result", 3201).unwrap());
        assert!(
            job_sent(&c, "j2", 3050, 12).unwrap(),
            "a result after no_result counts"
        );
        assert!(job_not_confirmed(&c, "j2", 3400).unwrap());
        queue(&c, &new_job("j3", "1", "r-1", 4000));
        assert!(job_failed(&c, "j3", "no_result", 4200).unwrap());
        assert!(
            job_failed(&c, "j3", "no_iface", 4201).unwrap(),
            "the relay's word after it"
        );
        assert_eq!(
            job_by_cmd(&c, "r-1", "cmd-j3")
                .unwrap()
                .unwrap()
                .error
                .as_deref(),
            Some("no_iface")
        );
        assert_eq!(
            job_by_cmd(&c, "r-2", "cmd-j3").unwrap(),
            None,
            "another relay's command"
        );
        queue(&c, &new_job("j4", "1", "r-1", 5000));
        assert_eq!(open_jobs(&c).unwrap().len(), 1);
        assert_eq!(latest_job_of(&c, "1", a).unwrap().unwrap().job, "j4");
        // the latest job of a PC is an account's own: a later one that another account made for
        // the same ID is not
        let b = upsert_account(&c, "b", "B", "", 1).unwrap();
        let mut theirs = new_job("j5", "1", "r-9", 5001);
        theirs.account_id = b;
        queue(&c, &theirs);
        assert_eq!(latest_job_of(&c, "1", a).unwrap().unwrap().job, "j4");
        assert_eq!(latest_job_of(&c, "1", b).unwrap().unwrap().job, "j5");
        assert_eq!(latest_job_of(&c, "2", a).unwrap(), None);
        delete_account(&c, b, 5002).unwrap();
        assert_eq!(job(&c, "j5").unwrap(), None, "it went with its account");
        assert_eq!(jobs_of_account_since(&c, a, 3999).unwrap(), 2);
        assert_eq!(jobs_of_relay_since(&c, "r-1", 0).unwrap(), 4);
        // revoked: its waiting job fails, its commands go, the row keeps what PROTOCOL.md §11 allows
        assert_eq!(revoke_relay(&c, "r-1", 6000).unwrap(), Revoked::Now(1));
        assert_eq!(revoke_relay(&c, "r-1", 6001).unwrap(), Revoked::Already);
        assert_eq!(revoke_relay(&c, "r-9", 6001).unwrap(), Revoked::NoSuchRelay);
        let r = relay(&c, "r-1").unwrap().unwrap();
        assert_eq!(
            (r.state, r.pk, r.revoked_at),
            (RelayState::Revoked, [1; 32], Some(6000))
        );
        assert_eq!(
            (
                r.owner_account,
                r.source,
                r.egress_ip,
                r.version,
                r.ifaces.len(),
                r.last_poll_at,
                r.scope.as_str()
            ),
            (None, None, None, None, 0, None, "own")
        );
        assert_eq!(
            job(&c, "j4").unwrap().unwrap().error.as_deref(),
            Some("not_active")
        );
        seen(true, Some(("[]", "h2")), 6002);
        assert_eq!(
            relay(&c, "r-1").unwrap().unwrap().last_poll_at,
            None,
            "a revoked relay keeps nothing new"
        );
        // approvals ahead, until they expire
        preapprove(
            &c,
            &Preapproval {
                relay_id: "r-2".into(),
                account_id: a,
                name: "NAS".into(),
                created_at: 7000,
                expires_at: 7600,
            },
        )
        .unwrap();
        assert_eq!(preapproval(&c, "r-2", 7599).unwrap().unwrap().account_id, a);
        assert_eq!(preapproval(&c, "r-2", 7600).unwrap(), None);
        assert_eq!(preapprovals(&c, 7000).unwrap().len(), 1);
        // the PC's wake report goes with its binding; the account's relays, approvals and jobs
        // with the account
        let report = Report::default();
        store_wol_info(&c, "1", &report, "h", "192.0.2.40", 8000).unwrap();
        assert_eq!(
            wol_egress(&c, "1", "192.0.2.41").unwrap().as_deref(),
            Some("h")
        );
        assert_eq!(wol_infos_of(&c, a).unwrap()[0].egress_ip, "192.0.2.41");
        assert_eq!(wol_egress(&c, "2", "192.0.2.41").unwrap(), None);
        new("r-3", 3, RelayState::Pending);
        approve_relay(&c, "r-3", a, "NAS", 8001).unwrap();
        release_binding(&c, "1", "admin", 8002).unwrap();
        assert_eq!(wol_info(&c, "1").unwrap(), None);
        assert_eq!(
            job(&c, "j1").unwrap().unwrap().state,
            "confirmed",
            "the record stays"
        );
        delete_account(&c, a, 8003).unwrap();
        assert_eq!(
            relay(&c, "r-3").unwrap().unwrap().state,
            RelayState::Revoked
        );
        for table in [
            "wake_jobs",
            "wake_commands",
            "relay_preapprovals",
            "wol_info",
        ] {
            let n: i64 = c
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 0, "{table}");
        }
        // pending entries past their day
        new("r-4", 4, RelayState::Pending);
        assert_eq!(drop_expired_pending(&c, 999).unwrap(), 0);
        assert_eq!(drop_expired_pending(&c, 1000).unwrap(), 1);
        assert_eq!(oldest_pending(&c).unwrap(), None);
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
            evicted: 0,
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
        // at most three active keys here: a device that has one replaces it rather than adding,
        // and past three the least recently used key goes (its token's last use, else its login)
        assert!(matches!(
            login_with_key(&mut c, a, 3, "dev-3"),
            KeyAdmission::Added { evicted: 0, .. }
        ));
        assert!(matches!(
            login_with_key(&mut c, a, 4, ""),
            KeyAdmission::Added { evicted: 0, .. }
        ));
        c.execute(
            "UPDATE tokens SET last_used_at = 900 WHERE hash = ?1",
            [[2u8; 32].as_slice()],
        )
        .unwrap();
        assert!(matches!(
            login_with_key(&mut c, a, 5, "dev-5"),
            KeyAdmission::Added { evicted: 1, .. }
        ));
        assert_eq!(active(&c, a), vec![vec![5; 32], vec![4; 32], vec![2; 32]]);
        assert!(matches!(
            login_with_key(&mut c, a, 7, "dev-5"),
            KeyAdmission::Added {
                replaced: 1,
                evicted: 0,
                ..
            }
        ));
        assert_eq!(active(&c, a).len(), 3);
        assert!(session(&c, &[3; 32], 200, 1000, 5000).unwrap().is_none());
        // another account counts apart
        assert!(matches!(
            login_with_key(&mut c, b, 8, "dev-3"),
            KeyAdmission::Added { .. }
        ));
        // revoking: one key, the others, all; each takes the tokens along and raises the serial
        let serial = change_serial(&c).unwrap();
        assert!(!revoke_key(&c, b, k2, "user", None, 300).unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial);
        assert!(revoke_key(&c, a, k2, "user", Some(k2), 300).unwrap());
        assert!(!revoke_key(&c, a, k2, "user", None, 300).unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial + 1);
        assert!(session(&c, &[2; 32], 400, 1000, 5000).unwrap().is_none());
        let keep = keys_of(&c, a).unwrap()[0].ckid;
        assert_eq!(revoke_other_keys(&c, a, keep, "others", 300).unwrap(), 1);
        assert_eq!(keys_of(&c, a).unwrap().len(), 1);
        assert!(!revoke_key(&c, a, k1, "user", None, 300).unwrap());
        assert_eq!(revoke_all_keys(&c, "admin", 330).unwrap(), 2);
        let tokens: i64 = c
            .query_row("SELECT COUNT(*) FROM tokens", [], |r| r.get(0))
            .unwrap();
        assert_eq!(tokens, 0);
        let reasons: Vec<(Option<String>, Option<i64>)> = all_keys(&c)
            .unwrap()
            .into_iter()
            .map(|k| (k.revoke_reason, k.revoked_by))
            .collect();
        assert_eq!(
            reasons,
            [
                ("same-device", None),
                ("user", Some(k2)),
                ("evicted", None),
                ("others", Some(keep)),
                ("same-device", None),
                ("admin", None),
                ("admin", None),
            ]
            .map(|(r, by)| (Some(r.to_owned()), by))
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
        // a key answers for the other keys of its own account
        assert!(!ack_key(&c, a, k3, k3, 10).unwrap(), "itself");
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
        // for an older key the answer is the answering key's own
        assert!(ack_key(&c, a, k1, k2, 10).unwrap(), "an older key");
        assert_eq!(confirmed(&c, k1), Some((None, None)));
        assert_eq!(answered_by(&c, k2).unwrap(), [k1]);
        assert!(answered_by(&c, k3).unwrap().is_empty());
        // for a later key the first time stays, and so does the oldest key that confirmed it
        assert!(ack_key(&c, a, k3, k2, 10).unwrap());
        assert!(ack_key(&c, a, k3, k1, 20).unwrap());
        assert!(ack_key(&c, a, k3, k2, 30).unwrap());
        assert_eq!(confirmed(&c, k3), Some((Some(10), Some(k1))));
        // a revoked key answers no more, but it may still be answered (the notice waits)
        assert!(revoke_key(&c, a, k1, "user", Some(k2), 40).unwrap());
        assert!(!ack_key(&c, a, k2, k1, 50).unwrap());
        assert!(revoke_key(&c, a, k3, "user", Some(k2), 40).unwrap());
        assert!(ack_key(&c, a, k3, k2, 50).unwrap());
        // confirming changes no trust list: the serial stays
        let k4 = ckid(login_with_key(&mut c, a, 4, "phone"));
        let serial = change_serial(&c).unwrap();
        assert!(ack_key(&c, a, k4, k2, 60).unwrap());
        assert_eq!(change_serial(&c).unwrap(), serial);
        // [아님]: the key is marked and logged out, and its own confirmations count no more
        let k5 = ckid(login_with_key(&mut c, a, 5, "pad"));
        assert!(ack_key(&c, a, k5, k4, 70).unwrap());
        assert_eq!(confirmed(&c, k5), Some((Some(70), Some(k4))));
        assert_eq!(mark_not_mine(&c, a, k4, k4, 80).unwrap(), None, "itself");
        assert_eq!(
            mark_not_mine(&c, b, k4, kb, 80).unwrap(),
            None,
            "another account"
        );
        assert_eq!(mark_not_mine(&c, a, k4, k2, 80).unwrap(), Some(true));
        assert_eq!(confirmed(&c, k5), Some((None, None)));
        assert_eq!(change_serial(&c).unwrap(), serial + 2);
        let marked = all_keys(&c)
            .unwrap()
            .into_iter()
            .find(|k| k.ckid == k4)
            .unwrap();
        assert_eq!(
            (marked.not_mine_at, marked.not_mine_by, marked.revoked_by),
            (Some(80), Some(k2), Some(k2))
        );
        // a key that ended before the answer is only marked
        assert_eq!(mark_not_mine(&c, a, k3, k2, 90).unwrap(), Some(false));
        assert_eq!(change_serial(&c).unwrap(), serial + 2);
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
        // blocked (an import does not lift it, unblock does), or its switch off
        allow_block(&mut c, &["a".to_owned()], 2).unwrap();
        assert!(!list_on(&c, a).unwrap());
        allow_add(&mut c, &["a".to_owned()], 3).unwrap();
        assert!(!list_on(&c, a).unwrap());
        allow_unblock(&mut c, &["a".to_owned()], 3).unwrap();
        assert!(list_on(&c, a).unwrap());
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
    fn blocking_revokes_keys() {
        let mut c = conn();
        allow_add(&mut c, &["a".to_owned()], 1).unwrap();
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        login_with_key(&mut c, a, 1, "");
        login_with_key(&mut c, a, 2, "");
        // an ID not on the list is put on it blocked
        assert_eq!(
            allow_block(&mut c, &["a".to_owned(), "zz".to_owned()], 500).unwrap(),
            Blocked {
                blocked: 2,
                new: 1,
                already: 0,
                tokens: 2,
                keys: 2,
                person: 0
            }
        );
        assert!(keys_of(&c, a).unwrap().is_empty());
        let counts = counts(&c, 600, DAY).unwrap();
        assert_eq!(counts.keys_active, 0);
        assert_eq!(counts.keys_revoked, vec![("blocked".to_owned(), 2)]);
        assert_eq!((counts.allowlist, counts.blocked), (0, 2));
        let list = allowlist(&c).unwrap();
        assert_eq!(
            list,
            vec![
                ListEntry {
                    login_key: "a".into(),
                    added_at: 1,
                    source: "import".into(),
                    blocked_at: Some(500)
                },
                ListEntry {
                    login_key: "zz".into(),
                    added_at: 500,
                    source: "import".into(),
                    blocked_at: Some(500)
                },
            ]
        );
    }

    #[test]
    fn a_login_registers_its_id_unless_blocked_or_in_list_mode() {
        let mut c = conn();
        // open mode: an ID not on the list is put on it, as a login's
        assert_eq!(
            admit_login(&c, "new", "new", true, 5).unwrap(),
            Admission::Registered
        );
        assert_eq!(
            admit_login(&c, "new", "new", true, 6).unwrap(),
            Admission::Listed
        );
        assert_eq!(
            allowlist(&c).unwrap(),
            vec![ListEntry {
                login_key: "new".into(),
                added_at: 5,
                source: "login".into(),
                blocked_at: None
            }]
        );
        assert!(allowed(&c, "new").unwrap());
        // list mode: no login puts an ID on the list
        assert_eq!(
            admit_login(&c, "other", "other", false, 7).unwrap(),
            Admission::Refused
        );
        assert_eq!(listing(&c, "other").unwrap(), Listing::Absent);
        // a block holds in either mode, and an import does not lift it
        allow_block(&mut c, &["new".to_owned()], 8).unwrap();
        allow_add(&mut c, &["new".to_owned()], 8).unwrap();
        for register in [true, false] {
            assert_eq!(
                admit_login(&c, "new", "new", register, 9).unwrap(),
                Admission::Blocked
            );
        }
        // the ID the SSO names for the person counts too: typed in another form, still blocked
        assert_eq!(
            admit_login(&c, "other-form", "new", true, 9).unwrap(),
            Admission::Blocked
        );
        assert_eq!(listing(&c, "other-form").unwrap(), Listing::Absent);
        assert_eq!(
            login_listing(&c, "other-form", "new").unwrap(),
            Listing::Blocked
        );
        assert_eq!(
            login_listing(&c, "other", "unknown").unwrap(),
            Listing::Absent
        );
        // no account had those IDs: no PC's list changed
        assert_eq!(change_serial(&c).unwrap(), 0);
        // an account without an entry comes back on the list: its PCs' lists change
        upsert_account(&c, "acct", "ACCT", "", 1).unwrap();
        assert_eq!(
            admit_login(&c, "acct", "acct", true, 10).unwrap(),
            Admission::Registered
        );
        assert_eq!(change_serial(&c).unwrap(), 1);
        let n = counts(&c, 10, DAY).unwrap();
        assert_eq!(
            (
                n.allowlist,
                n.allowlist_by_login,
                n.registered_day,
                n.registered_week,
                n.blocked
            ),
            (1, 1, 1, 1, 1)
        );
        let n = counts(&c, 10 + 2 * DAY, DAY).unwrap();
        assert_eq!((n.registered_day, n.registered_week), (0, 1));
    }

    /// The SSO named one person's perslNo for two portal IDs (whether it does is not known, design
    /// v2 §4-1): blocking either blocks both and ends both accounts' keys, a login with a third
    /// form is refused, the block stays when one account is deleted, and unblock lifts them
    /// together; the person's other accounts only, not someone else's.
    #[test]
    fn a_block_takes_every_id_of_the_person() {
        let mut c = conn();
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let alias = upsert_account(&c, "alias", "P1", "", 1).unwrap();
        let main = upsert_account(&c, "p1", "P1", "", 1).unwrap();
        let other = upsert_account(&c, "p2", "P2", "", 1).unwrap();
        for (key, persl) in [("alias", "p1"), ("p1", "p1"), ("p2", "p2")] {
            admit_login(&c, key, persl, true, 1).unwrap();
        }
        login_with_key(&mut c, alias, 1, "");
        login_with_key(&mut c, main, 2, "");
        login_with_key(&mut c, other, 3, "");
        assert_eq!(
            allow_block(&mut c, &ids(&["alias"]), 5).unwrap(),
            Blocked {
                blocked: 2,
                new: 0,
                already: 0,
                tokens: 2,
                keys: 2,
                person: 1
            }
        );
        assert!(!allowed(&c, "alias").unwrap() && !allowed(&c, "p1").unwrap());
        assert!(keys_of(&c, alias).unwrap().is_empty() && keys_of(&c, main).unwrap().is_empty());
        assert!(allowed(&c, "p2").unwrap());
        assert_eq!(keys_of(&c, other).unwrap().len(), 1);
        // a third form with the same perslNo: refused, and put on no list
        assert_eq!(
            admit_login(&c, "alias2", "p1", true, 6).unwrap(),
            Admission::Blocked
        );
        assert_eq!(listing(&c, "alias2").unwrap(), Listing::Absent);
        // the account typed with the alias deleted: the perslNo form stays blocked
        let tx = c.transaction().unwrap();
        delete_account(&tx, alias, 7).unwrap();
        tx.commit().unwrap();
        assert_eq!(login_listing(&c, "p1", "p1").unwrap(), Listing::Blocked);
        // unblock through the perslNo form lifts what the server still knows of the person
        assert_eq!(allow_unblock(&mut c, &ids(&["p1"]), 8).unwrap(), (1, 0));
        assert_eq!(login_listing(&c, "p1", "p1").unwrap(), Listing::Registered);
        // the deleted account's entry is not linked to the person any more: named, it is lifted
        assert_eq!(listing(&c, "alias").unwrap(), Listing::Blocked);
        assert_eq!(allow_unblock(&mut c, &ids(&["alias"]), 9).unwrap(), (1, 0));
        // blocking the perslNo form takes the person's accounts under other IDs with it
        let alias = upsert_account(&c, "alias", "P1", "", 10).unwrap();
        login_with_key(&mut c, alias, 4, "");
        let b = allow_block(&mut c, &ids(&["p1"]), 11).unwrap();
        assert_eq!((b.blocked, b.person, b.keys), (2, 1, 1));
        assert!(!allowed(&c, "alias").unwrap());
        assert_eq!(allow_unblock(&mut c, &ids(&["alias"]), 12).unwrap(), (2, 0));
        assert!(allowed(&c, "alias").unwrap() && allowed(&c, "p1").unwrap());
        // an account blocked alone under another ID (as a block made before the person's IDs were
        // taken together left it): a login whose perslNo is that account's is refused
        c.execute(
            "UPDATE allowlist SET blocked_at = 13 WHERE login_key = 'alias'",
            [],
        )
        .unwrap();
        assert_eq!(listing(&c, "p1").unwrap(), Listing::Registered);
        assert_eq!(login_listing(&c, "p1", "p1").unwrap(), Listing::Blocked);
        assert_eq!(login_listing(&c, "p2", "p2").unwrap(), Listing::Registered);
    }

    /// Schema 5 keeps the entries as imported ones, and an account whose ID was taken off the list
    /// before stays out: its entry comes back blocked.
    #[test]
    fn the_allowlist_of_schema_four_moves_on() {
        let c = Connection::open_in_memory().unwrap();
        for sql in &MIGRATIONS[..4] {
            c.execute_batch(sql).unwrap();
        }
        c.pragma_update(None, "user_version", 4).unwrap();
        c.execute_batch(
            "INSERT INTO allowlist VALUES ('kept', 7), ('unused', 8);
             INSERT INTO accounts (login_key, portal_id, display_name, created_at, last_login_at,
                                   last_active_at)
                 VALUES ('kept', 'KEPT', '', 1, 1, 1), ('removed', 'REMOVED', '', 1, 1, 1);",
        )
        .unwrap();
        let c = prepare(c).unwrap();
        let v: i64 = c
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, MIGRATIONS.len() as i64);
        let list = allowlist(&c).unwrap();
        let state: Vec<(&str, &str, bool)> = list
            .iter()
            .map(|e| {
                (
                    e.login_key.as_str(),
                    e.source.as_str(),
                    e.blocked_at.is_some(),
                )
            })
            .collect();
        assert_eq!(
            state,
            vec![
                ("kept", "import", false),
                ("removed", "import", true),
                ("unused", "import", false)
            ]
        );
        assert_eq!((list[0].added_at, list[2].added_at), (7, 8));
        assert!(list[1].blocked_at.unwrap() > 1_700_000_000);
        assert!(allowed(&c, "kept").unwrap() && !allowed(&c, "removed").unwrap());
        // nothing a PC's list says changed
        assert_eq!(change_serial(&c).unwrap(), 0);
    }

    #[test]
    fn the_allowlist_moves_the_change_serial() {
        let mut c = conn();
        let ids = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // IDs nobody logged in with: no PC's list changes
        allow_add(&mut c, &ids(&["a", "b"]), 1).unwrap();
        allow_block(&mut c, &ids(&["b"]), 1).unwrap();
        allow_unblock(&mut c, &ids(&["b"]), 1).unwrap();
        assert_eq!(change_serial(&c).unwrap(), 0);
        // an account without active keys is blocked: its PCs' lists change all the same
        let a = upsert_account(&c, "a", "A", "", 1).unwrap();
        assert_eq!(allow_block(&mut c, &ids(&["a"]), 2).unwrap().blocked, 1);
        assert_eq!(change_serial(&c).unwrap(), 1);
        assert_eq!(
            allow_block(&mut c, &ids(&["a"]), 3).unwrap(),
            Blocked {
                already: 1,
                ..Blocked::default()
            }
        );
        assert_eq!(change_serial(&c).unwrap(), 1);
        // an import lifts no block; unblock does, once
        assert_eq!(allow_add(&mut c, &ids(&["a", "b"]), 4).unwrap(), (0, 2));
        assert_eq!(change_serial(&c).unwrap(), 1);
        assert_eq!(allow_unblock(&mut c, &ids(&["a", "b"]), 4).unwrap(), (1, 1));
        assert_eq!(change_serial(&c).unwrap(), 2);
        assert_eq!(allow_unblock(&mut c, &ids(&["a"]), 5).unwrap(), (0, 1));
        assert_eq!(change_serial(&c).unwrap(), 2);
        // with keys: once, as they are revoked
        login_with_key(&mut c, a, 1, "");
        let serial = change_serial(&c).unwrap();
        assert_eq!(
            allow_block(&mut c, &ids(&["a", "b"]), 6).unwrap(),
            Blocked {
                blocked: 2,
                new: 0,
                already: 0,
                tokens: 1,
                keys: 1,
                person: 0
            }
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
        // the same, but blocked since: deleted with its PC, its entry stays blocked
        let gone = upsert_account(&c, "gone", "GONE", "", old).unwrap();
        bind(&c, gone, "2", &[1; 32], "windows", "PC", old).unwrap();
        assert!(seen_by_hbbs(&c, "2", &[1; 32], t - DAY).unwrap());
        allow_block(&mut c, &["gone".to_owned()], t).unwrap();
        // on the list, its PC not seen for 200 days: the PC goes, then the account with its entry
        let idle = upsert_account(&c, "idle", "IDLE", "", old).unwrap();
        admit_login(&c, "idle", "idle", true, old).unwrap();
        bind(&c, idle, "3", &[1; 32], "windows", "PC", t - 200 * DAY).unwrap();
        // entries without an account: imported over a year ago and never used (goes), imported
        // yesterday (stays), blocked (stays until unblocked)
        allow_add(&mut c, &["never".to_owned()], old).unwrap();
        allow_add(&mut c, &["recent".to_owned()], t - DAY).unwrap();
        allow_block(&mut c, &["banned".to_owned()], old).unwrap();
        let p = purge(&mut c, t, &retention()).unwrap();
        assert_eq!((p.accounts, p.devices, p.entries), (2, 1, 1));
        assert!(account_by_key(&c, "lab").unwrap().is_some());
        assert!(device(&c, "1").unwrap().is_some());
        assert!(account_by_key(&c, "gone").unwrap().is_none());
        assert!(device(&c, "2").unwrap().is_none());
        assert!(account_by_key(&c, "idle").unwrap().is_none());
        let kept: Vec<(String, bool)> = allowlist(&c)
            .unwrap()
            .into_iter()
            .map(|e| (e.login_key, e.blocked_at.is_some()))
            .collect();
        assert_eq!(
            kept,
            vec![
                ("banned".to_owned(), true),
                ("gone".to_owned(), true),
                ("lab".to_owned(), false),
                ("recent".to_owned(), false)
            ]
        );
        // an unblocked entry without an account keeps from its unblocking
        allow_unblock(&mut c, &["banned".to_owned()], t).unwrap();
        assert_eq!(purge(&mut c, t + DAY, &retention()).unwrap().entries, 0);
        assert!(allowed(&c, "banned").unwrap());
    }
}
