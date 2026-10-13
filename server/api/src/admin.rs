// SPDX-License-Identifier: AGPL-3.0-only

//! `hjv-api admin <command>` (design v2 §9-4): commands go to the running service over its admin
//! socket, which only root (and the service's own user) may use; the service writes each command
//! to the operator record (oplog.rs) before it runs it.
//!
//! Output is masked unless `--reveal` is given (for human operators; agents do not use it): portal
//! and device IDs show their first four characters, names do not show, and accounts and devices
//! carry their tags (log.rs), which the journal also uses. Commands take an ID or `tag:<tag>`, so
//! that an operator can act on a journal warning without seeing who it is about. The allowlist is
//! read from a file the user wrote (`allow import <file>`), and only counts are printed, so that
//! portal IDs do not pass through an agent's conversation. A file with a line that is not a portal
//! ID changes nothing, and the answer names only the line numbers.
//!
//! The protocol is one JSON line each way: `{"argv":[…],"reveal":false,"sudo_user":"…","input":"…"}`
//! and `{"ok":true,"output":"…"}` or `{"ok":false,"error":"…"}`.
//!
//! Design v3 §5-6 adds the account keys (`key list` with what the PCs reported of each key's use,
//! `key revoke`, `user revoke-all`), the password-less switch (`passwordless`, off when installed;
//! `--owner-of <PC ID>` names an account by one of its PCs, so that no portal ID has to pass
//! through an agent), `strict`, the bindings (`binding list`, `binding revoke`, with `device
//! remove` as its other name: both release the PC through `db::release_binding`, so that its next
//! sync gets a signed `revoked`) and the PC sync's counts in `report`. Device names, host names,
//! the names apps give themselves and portal IDs show only with `--reveal`.
//!
//! Design v3 §5-6 and §8-3 add the NAS relays (relay.rs): `relay list`, `relay approve <code>
//! --owner-of <PC ID>` (the account by one of its PCs; it shows the verdicts of the relay against
//! that PC, never the addresses unless `--reveal`, and approves only a relay on the PC's network,
//! as the relay itself reports its interfaces, unless `--force`; a code no relay has said hello
//! with is approved ahead for 7 days), `relay revoke` and `relay share on|off` (kept for later).
//! Their records name the relay ID and the PC's tag.
//!
//! Design v3 §5-9 adds `backup` (backup.rs): the service takes a copy of its database and sends
//! its bytes after the answer's line (`"backup":{size, sha256, schema, counts}` says how many),
//! and the command line writes and checks the file; and `health` (check.rs), the counts the
//! five-minute check reads: it changes nothing, shows no person or device, and is the one command
//! the operator record leaves out, so that the check's 288 calls a day do not bury what people
//! did.

use crate::{
    backup::{self, BackupInfo, Snapshot},
    check, db, limits,
    oplog::{OpEntry, OpLog},
    pcsync, relay,
    selfcheck::{CommandKeyCheck, ListKeyCheck},
    state::AppState,
    util::{
        date, datetime, hex, login_key, mask, now, sha256, valid_device_id, valid_portal_id, DAY,
    },
};
use serde::{Deserialize, Serialize};
use std::{
    fmt::Write as _,
    future::Future,
    net::IpAddr,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

pub const DEFAULT_SOCKET: &str = "/run/hjviewer-api-admin/admin.sock";
const MAX_REQUEST: u64 = 4 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub const USAGE: &str = "\
hjv-api admin [--socket <path>] [--reveal] <command>
  allow import <file>       add the portal IDs in <file> (one per line, # comments) to the allowlist
  allow remove <file>       remove the IDs in <file>; their tokens and account keys stop working
                            at once (their PCs stay in the accounts)
  allow list                the allowlist
  users                     accounts: tag, ID, devices, account keys, tokens, last login
  devices                   devices in accounts: tag, ID, account tag, OS, last seen
  key list [<id|tag:T>]     account keys: number, account, platform, device, version, network,
                            times, and per PC the last password-less use and the uses of 30 days
  key revoke <ckid>         revoke an account key with its token (that device must log in again)
  unlock <id|tag:T|ip:A>    clear the login counters of a portal ID (with its PCs' and successes'),
                            of the tag from a journal warning, or of a client address
  user revoke-all <id|tag:T> | --all
                            revoke every token and account key of an account, or of every account
                            (each device must log in again; PCs stay in the accounts)
  revoke <id|tag:T>         the same as user revoke-all <id|tag:T>
  binding list              the PCs bound to accounts: password-less state, last sync, version,
                            and the released bindings kept, by how they were released
  binding revoke <PC ID|tag:T>
                            release a PC from its account; its next sync gets a signed revoked
  device remove <PC ID|tag:T>
                            the same as binding revoke
  user delete <id|tag:T>    delete an account with its tokens and account keys, after releasing
                            its PCs
  passwordless status | on | off [--account <id|tag:T> | --owner-of <PC ID>]
                            connections without the PC's password between an account's devices:
                            for everyone (off when installed; off also ends the accounts turned on
                            one by one), or for one account
  strict on | off           PCs ask the server at each password-less connection (incidents)
  report                    counts: accounts, devices, tokens, keys, switches, logins of the last
                            day and week, the PCs' syncs, needs-relogin, refusals, released
                            bindings, password-less connections of 30 days
  selfcheck                 the service checks that it can reach the SSO and hbbs, and that its
                            list key is the one the apps pin (with the fingerprints)
  backup [--keep-days N] <file>.sqlite3 | <directory>
                            a checked copy of the database (0600) into a directory only you may
                            enter (/var/lib/hjviewer-api/backup); a directory gets a name of the
                            time; --keep-days N then deletes the backups there older than N days
  health                    the counts the five-minute check reads, as JSON (not recorded)
  relay list                the NAS relays: state, account, last poll; for an active relay whether
                            it is on the network of each of its account's PCs (values with --reveal)
  relay approve <code> --owner-of <PC ID> [--name <name>] [--force]
                            approve a relay's enrollment code for the account that has the PC,
                            after the PC's owner confirmed the code: when the verdict says the
                            relay is on the PC's network (by the relay's own report), or with
                            --force; a code no relay has said hello with is approved ahead for
                            7 days; --name for a relay already active for that account renames it
  relay revoke <relay ID|code>
                            the relay takes no commands any more (its key is kept and never
                            approved again); withdraws a code approved ahead
  relay share on|off <relay ID>
                            kept for later: in this version a relay wakes its account's PCs only
  keygen seal|log-key|list --kid N|command --kid N [--encrypt-to <file> | --out <file>]
                            make a secret for a credential (runs here, not in the service)";

#[derive(Serialize, Deserialize)]
pub struct AdminRequest {
    pub argv: Vec<String>,
    #[serde(default)]
    pub reveal: bool,
    #[serde(default)]
    pub sudo_user: String,
    #[serde(default)]
    pub input: Option<String>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct AdminResponse {
    pub ok: bool,
    #[serde(default)]
    pub output: String,
    #[serde(default)]
    pub error: String,
    /// `backup`: the copy whose bytes follow the answer's line
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<BackupInfo>,
}

/// Creates the admin socket at `path` (a stale one is replaced), readable by its owner only.
pub fn bind(path: &Path) -> Result<UnixListener, String> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(listener)
}

/// Answers admin commands until `shutdown` completes.
pub async fn serve(
    state: Arc<AppState>,
    listener: UnixListener,
    path: PathBuf,
    oplog: Arc<OpLog>,
    shutdown: impl Future<Output = ()>,
) {
    // the socket file belongs to the service's user; that user and root may connect
    let own_uid = std::fs::metadata(&path).map(|m| m.uid()).unwrap_or(0);
    tokio::pin!(shutdown);
    loop {
        let stream = tokio::select! {
            () = &mut shutdown => break,
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => stream,
                Err(e) => {
                    log_err!("admin socket: {e}");
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    continue;
                }
            },
        };
        let (state, oplog) = (state.clone(), oplog.clone());
        tokio::spawn(async move {
            if let Err(e) = connection(&state, &oplog, stream, own_uid).await {
                log_warn!("admin: {e}");
            }
        });
    }
    let _ = std::fs::remove_file(&path);
}

async fn connection(
    state: &Arc<AppState>,
    oplog: &OpLog,
    stream: UnixStream,
    own_uid: u32,
) -> Result<(), String> {
    let uid = stream
        .peer_cred()
        .map_err(|e| format!("peer credentials: {e}"))?
        .uid();
    let (read, mut write) = stream.into_split();
    let (response, snapshot) = if uid != 0 && uid != own_uid {
        log_warn!("admin: refused a connection from uid {uid}");
        (
            AdminResponse {
                error: "admin commands are for root only".into(),
                ..AdminResponse::default()
            },
            None,
        )
    } else {
        let mut line = String::new();
        let read = tokio::time::timeout(
            IO_TIMEOUT,
            BufReader::new(read.take(MAX_REQUEST)).read_line(&mut line),
        )
        .await;
        match read {
            Ok(Ok(_)) => match serde_json::from_str::<AdminRequest>(&line) {
                Ok(request) => run(state, oplog, uid, request).await,
                Err(_) => (
                    AdminResponse {
                        error: "not an admin request".into(),
                        ..AdminResponse::default()
                    },
                    None,
                ),
            },
            _ => return Err("no request".into()),
        }
    };
    let mut text = serde_json::to_string(&response).unwrap_or_default();
    text.push('\n');
    tokio::time::timeout(IO_TIMEOUT, write.write_all(text.as_bytes()))
        .await
        .map_err(|_| "answer timed out".to_owned())?
        .map_err(|e| e.to_string())?;
    // a backup's bytes follow its line; the copy is deleted when `snapshot` goes
    if let Some(snapshot) = snapshot {
        backup::send_file(&snapshot, &mut write)
            .await
            .map_err(|e| format!("backup: {e}"))?;
    }
    Ok(())
}

/// What a command line points at: an account or a device, by value or by tag.
enum Target {
    Value(String),
    Tag(String),
}

fn target(arg: &str) -> Result<Target, String> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Err("which one? give an ID or tag:<tag>".into());
    }
    match arg.strip_prefix("tag:") {
        Some(tag) if tag.len() == 16 && tag.bytes().all(|b| b.is_ascii_hexdigit()) => {
            Ok(Target::Tag(tag.to_ascii_lowercase()))
        }
        Some(_) => Err("a tag is 16 hex digits".into()),
        None => Ok(Target::Value(arg.to_owned())),
    }
}

async fn run(
    state: &Arc<AppState>,
    oplog: &OpLog,
    uid: u32,
    req: AdminRequest,
) -> (AdminResponse, Option<Snapshot>) {
    let words: Vec<&str> = req.argv.iter().map(String::as_str).collect();
    // the five-minute check's counts: not recorded (see the module's notes)
    if words == ["health"] {
        let response = match check::health(state).await {
            Ok(health) => AdminResponse {
                ok: true,
                output: serde_json::to_string(&health).unwrap_or_default(),
                ..AdminResponse::default()
            },
            Err(error) => {
                log_err!("health: {error}");
                AdminResponse {
                    error,
                    ..AdminResponse::default()
                }
            }
        };
        return (response, None);
    }
    // known command names only: a mistyped command line may hold an ID
    let command = match words.as_slice() {
        ["allow", sub @ ("import" | "remove" | "list"), ..]
        | ["device", sub @ "remove", ..]
        | ["binding", sub @ ("list" | "revoke"), ..]
        | ["user", sub @ ("delete" | "revoke-all"), ..]
        | ["key", sub @ ("list" | "revoke"), ..]
        | ["passwordless", sub @ ("status" | "on" | "off"), ..]
        | ["strict", sub @ ("on" | "off"), ..]
        | ["relay", sub @ ("list" | "approve" | "revoke"), ..] => format!("{} {sub}", words[0]),
        ["relay", "share", turn @ ("on" | "off"), ..] => format!("relay share {turn}"),
        [one @ ("users" | "devices" | "unlock" | "revoke" | "report" | "selfcheck" | "backup"), ..] => {
            (*one).to_owned()
        }
        _ => "unknown".to_owned(),
    };
    // the record shows tags, never the IDs or addresses the operator typed
    let targets: Vec<String> = match words.as_slice() {
        ["allow", "import" | "remove", ..] => match read_ids(req.input.as_deref()) {
            Ok(ids) => id_targets(state, &ids),
            Err(_) => vec![],
        },
        ["unlock", arg] if arg.trim().starts_with("ip:") => match address(arg) {
            Ok(ip) => vec![state.tags.ip(&ip)],
            Err(_) => vec![],
        },
        ["user", "revoke-all", "--all"] => vec!["all".to_owned()],
        ["unlock" | "revoke", arg]
        | ["user", "delete" | "revoke-all", arg]
        | ["key", "list", arg]
        | ["passwordless", _, "--account", arg] => match target(arg) {
            Ok(Target::Tag(t)) => vec![t],
            Ok(Target::Value(v)) => vec![state.tags.account(&login_key(&v))],
            Err(_) => vec![],
        },
        ["device", "remove", arg]
        | ["binding", "revoke", arg]
        | ["passwordless", _, "--owner-of", arg] => match target(arg) {
            Ok(Target::Tag(t)) => vec![t],
            Ok(Target::Value(v)) => vec![state.tags.device(&v)],
            Err(_) => vec![],
        },
        ["key", "revoke", n] if key_number(n).is_some() => vec![format!("ckid:{}", n.trim())],
        // a relay by its ID (not a person's); the PC of --owner-of by its tag
        ["relay", "approve", code, rest @ ..] => {
            let mut targets: Vec<String> = relay::parse_code(code).into_iter().collect();
            if let Ok(options) = approve_options(rest) {
                match target(options.pc) {
                    Ok(Target::Tag(t)) => targets.push(t),
                    Ok(Target::Value(v)) => targets.push(state.tags.device(&v)),
                    Err(_) => {}
                }
            }
            targets
        }
        ["relay", "revoke", arg] | ["relay", "share", _, arg] => {
            relay::parse_code(arg).into_iter().collect()
        }
        // the backup's file name, which the command line sends for the record only
        ["backup", name] if backup_name(name) => vec![(*name).to_owned()],
        _ => vec![],
    };
    let entry = OpEntry {
        uid,
        sudo_user: &req.sudo_user,
        command: &command,
        targets: &targets,
        reveal: req.reveal,
    };
    if let Err(e) = oplog.append(&entry) {
        log_err!("admin: {e}; the command did not run");
        return (
            AdminResponse {
                error: format!("{e}; the command did not run"),
                ..AdminResponse::default()
            },
            None,
        );
    }
    let (result, snapshot) = match words.as_slice() {
        ["backup"] | ["backup", _] => match backup::snapshot(state).await {
            Ok(snapshot) => (Ok(snapshot.info.summary()), Some(snapshot)),
            Err(e) => {
                log_err!("admin: {e}");
                (Err(e), None)
            }
        },
        _ => (execute(state, &req).await, None),
    };
    // what changed (or, for a backup, what was copied), in counts; the output of a listing stays
    // out of the record
    let done = match &result {
        Ok(output) if CHANGES.contains(&command.as_str()) || command == "backup" => {
            format!("ok: {}", output.lines().next().unwrap_or(""))
        }
        Ok(_) => "ok".to_owned(),
        Err(_) => "failed".to_owned(),
    };
    if let Err(e) = oplog.append_done(&entry, &done) {
        log_err!("admin: {e}");
    }
    let response = match result {
        Ok(output) => AdminResponse {
            ok: true,
            output,
            backup: snapshot.as_ref().map(|s| s.info.clone()),
            ..AdminResponse::default()
        },
        Err(error) => AdminResponse {
            error,
            ..AdminResponse::default()
        },
    };
    (response, snapshot)
}

/// A backup's file name as the command line sends it for the record: `<name>.sqlite3`, short and
/// plain.
fn backup_name(name: &str) -> bool {
    name.len() <= 64
        && name.ends_with(".sqlite3")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

fn show(value: &str, reveal: bool) -> String {
    if reveal {
        value.to_owned()
    } else {
        mask(value)
    }
}

/// The commands that change something; their outcome line in the record carries their output,
/// which is counts only.
const CHANGES: &[&str] = &[
    "allow import",
    "allow remove",
    "unlock",
    "revoke",
    "device remove",
    "binding revoke",
    "user delete",
    "user revoke-all",
    "key revoke",
    "passwordless on",
    "passwordless off",
    "strict on",
    "strict off",
    "relay approve",
    "relay revoke",
    "relay share on",
    "relay share off",
];

/// The options of `relay approve <code>`: `--owner-of <PC ID>` (required), `--name <name>`,
/// `--force`.
struct ApproveOptions<'a> {
    pc: &'a str,
    name: Option<&'a str>,
    force: bool,
}

fn approve_options<'a>(rest: &[&'a str]) -> Result<ApproveOptions<'a>, String> {
    let (mut pc, mut name, mut force) = (None, None, false);
    let mut it = rest.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--owner-of" => pc = Some(*it.next().ok_or("--owner-of takes a PC ID")?),
            "--name" => name = Some(*it.next().ok_or("--name takes a name")?),
            "--force" => force = true,
            _ => return Err(format!("relay approve: unknown option\n{USAGE}")),
        }
    }
    Ok(ApproveOptions {
        pc: pc.ok_or("relay approve <code> --owner-of <PC ID>: which PC's account?")?,
        name,
        force,
    })
}

/// An account key's number.
fn key_number(text: &str) -> Option<i64> {
    let text = text.trim();
    (!text.is_empty() && text.len() <= 18 && text.bytes().all(|b| b.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
        .filter(|n| *n > 0)
}

/// Characters editors put into text files unseen: a byte-order mark, zero-width spaces and joiners.
fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{feff}' | '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{2060}'
    )
}

/// The case-folded IDs of an allowlist file: one per line, '#' starts a comment. A line that is
/// anything else refuses the whole file, so that a change never happens in part; the error names
/// line numbers only.
fn read_ids(input: Option<&str>) -> Result<Vec<String>, String> {
    let input = input.ok_or("no file content: hjv-api admin allow import <file>")?;
    let mut ids = Vec::new();
    let mut bad = Vec::new();
    for (n, line) in input.lines().enumerate() {
        let line: String = line.chars().filter(|c| !invisible(*c)).collect();
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if valid_portal_id(line) {
            ids.push(login_key(line));
        } else {
            bad.push((n + 1).to_string());
        }
    }
    if !bad.is_empty() {
        let more = if bad.len() > 10 { ", …" } else { "" };
        bad.truncate(10);
        return Err(format!(
            "nothing changed: line {}{more} of the file is not a portal ID (one ID per line, # starts a comment)",
            bad.join(", ")
        ));
    }
    ids.sort();
    ids.dedup();
    Ok(ids)
}

/// How the record names the IDs of an allowlist file: their account tags, or past 32 of them,
/// how many there are and a hash of their sorted tags (the same file, the same hash).
fn id_targets(state: &AppState, ids: &[String]) -> Vec<String> {
    let mut tags: Vec<String> = ids.iter().map(|id| state.tags.account(id)).collect();
    tags.sort();
    if tags.len() <= 32 {
        return tags;
    }
    let set = hex(&sha256(tags.join(",").as_bytes())[..8]);
    vec![format!("{}-ids", tags.len()), format!("set:{set}")]
}

/// The address of `ip:<address>`.
fn address(arg: &str) -> Result<String, String> {
    arg.trim()
        .strip_prefix("ip:")
        .and_then(|a| a.trim().parse::<IpAddr>().ok())
        .map(|ip| ip.to_string())
        .ok_or_else(|| "ip: takes an address, such as ip:192.0.2.1 or ip:2001:db8::1".to_owned())
}

/// The counters `unlock` clears: an address's (by its tag, an IPv6 address by its /64), or an
/// ID's with those of its account's PCs and successes. A tag from a journal warning may stand for
/// an account or an address.
async fn unlock_keys(state: &Arc<AppState>, arg: &str) -> Result<Vec<String>, String> {
    if arg.trim().starts_with("ip:") {
        return Ok(vec![state.tags.ip(&address(arg)?)]);
    }
    let key = match target(arg)? {
        Target::Value(v) => login_key(&v),
        Target::Tag(tag) => match find_account(state, Target::Tag(tag.clone())).await? {
            Some(account) => account.login_key,
            None => return Ok(vec![tag]),
        },
    };
    Ok(vec![state.tags.account(&key), state.tags.bound(&key)])
}

async fn find_account(state: &Arc<AppState>, t: Target) -> Result<Option<db::Account>, String> {
    let accounts = state
        .db
        .call(|c| db::accounts(c))
        .await
        .map_err(|e| e.to_string())?;
    Ok(match t {
        Target::Value(v) => {
            let key = login_key(&v);
            accounts.into_iter().find(|a| a.login_key == key)
        }
        Target::Tag(tag) => accounts
            .into_iter()
            .find(|a| state.tags.account(&a.login_key) == tag),
    })
}

fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// The account that has the PC with this ID (`--owner-of`), by ID or `tag:`.
async fn owner_of(state: &Arc<AppState>, arg: &str) -> Result<db::Account, String> {
    let device_id = match target(arg)? {
        Target::Value(v) if valid_device_id(&v) => v,
        Target::Value(_) => return Err("not a device ID".into()),
        Target::Tag(tag) => state
            .db
            .call(|c| db::all_devices(c))
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|d| state.tags.device(&d.device_id) == tag)
            .map(|d| d.device_id)
            .ok_or("no account has that PC")?,
    };
    let account = state
        .db
        .call(move |c| {
            let Some(device) = db::device(c, &device_id)? else {
                return Ok(None);
            };
            db::accounts(c).map(|all| all.into_iter().find(|a| a.id == device.account_id))
        })
        .await
        .map_err(|e| e.to_string())?;
    account.ok_or_else(|| "no account has that PC".to_owned())
}

/// `passwordless status`: the global switch, the accounts switched one by one, and strict.
async fn passwordless_status(state: &Arc<AppState>, reveal: bool) -> Result<String, String> {
    let (global, strict, accounts) = state
        .db
        .call(|c| {
            let mut stmt = c.prepare(
                "SELECT login_key, portal_id, passwordless FROM accounts
                 WHERE passwordless IS NOT NULL ORDER BY login_key",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((db::passwordless_global(c)?, db::strict(c)?, rows))
        })
        .await
        .map_err(|e| e.to_string())?;
    let mut out = format!(
        "password-less connections: {} for every account; strict {}\n",
        on_off(global),
        on_off(strict)
    );
    let _ = writeln!(out, "accounts switched one by one: {}", accounts.len());
    for (key, portal_id, switch) in accounts {
        let _ = writeln!(
            out,
            "  {:<16}  {:<16}  {switch}",
            state.tags.account(&key),
            show(&portal_id, reveal)
        );
    }
    Ok(out)
}

/// `key list`: the account keys, of one account or all, the revoked ones still kept too, each
/// with a line per PC that reported its password-less use in the last 30 days (U6): the PC's tag
/// and masked ID, the last use by the PC's clock, the count (its host name with `--reveal`).
async fn key_list(
    state: &Arc<AppState>,
    account: Option<db::Account>,
    reveal: bool,
) -> Result<String, String> {
    let since = db::use_since(now());
    let (keys, accounts, uses) = state
        .db
        .call(move |c| {
            Ok((
                db::all_keys(c)?,
                db::accounts(c)?,
                db::all_key_use(c, &since)?,
            ))
        })
        .await
        .map_err(|e| e.to_string())?;
    let keys: Vec<db::Key> = keys
        .into_iter()
        .filter(|k| account.as_ref().is_none_or(|a| a.id == k.account_id))
        .collect();
    let active = keys.iter().filter(|k| k.revoked_at.is_none()).count();
    let mut out = format!(
        "account keys: {active} active, {} revoked\n",
        keys.len() - active
    );
    let _ = writeln!(
        out,
        "  {:>5}  {:<16}  {:<8}  {:<16}  {:<8}  {:<10}  {:<10}  {:<10}  state{}",
        "ckid",
        "account",
        "platform",
        "device",
        "version",
        "network",
        "created",
        "device ID",
        if reveal { "  app's name" } else { "" }
    );
    for k in keys {
        let account_tag = accounts
            .iter()
            .find(|a| a.id == k.account_id)
            .map(|a| state.tags.account(&a.login_key))
            .unwrap_or_default();
        let state_text = match (k.revoked_at, k.ack_by) {
            (Some(at), _) => format!(
                "revoked {} ({})",
                date(at),
                k.revoke_reason.as_deref().unwrap_or("-")
            ),
            (None, Some(by)) => format!("active, confirmed by {by}"),
            (None, None) => "active".to_owned(),
        };
        let _ = writeln!(
            out,
            "  {:>5}  {:<16}  {:<8}  {:<16}  {:<8}  {:<10}  {:<10}  {:<10}  {state_text}{}",
            k.ckid,
            account_tag,
            k.platform,
            k.model,
            if k.app_ver.is_empty() {
                "-"
            } else {
                &k.app_ver
            },
            if k.login_net.is_empty() {
                "-"
            } else {
                &k.login_net
            },
            date(k.created_at),
            show(&k.ctl_id, reveal),
            if reveal {
                format!("  {}", k.app_name)
            } else {
                String::new()
            }
        );
        for u in uses.iter().filter(|u| u.ckid == k.ckid) {
            let _ = writeln!(
                out,
                "         used on PC {:<16}  {:<12}  last {}  30 days {}{}",
                state.tags.device(&u.device_id),
                show(&u.device_id, reveal),
                datetime(u.last),
                u.n,
                if reveal && !u.pc.is_empty() {
                    format!("  {}", u.pc)
                } else {
                    String::new()
                }
            );
        }
    }
    Ok(out)
}

/// The PC an admin command names, by its ID or `tag:`; `None` when no account has it.
async fn bound_pc(state: &Arc<AppState>, arg: &str) -> Result<Option<String>, String> {
    Ok(match target(arg)? {
        Target::Value(v) if valid_device_id(&v) => Some(v),
        Target::Value(_) => return Err("not a device ID".into()),
        Target::Tag(tag) => state
            .db
            .call(|c| db::all_devices(c))
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|d| state.tags.device(&d.device_id) == tag)
            .map(|d| d.device_id),
    })
}

/// `binding list`: the PCs in accounts with their password-less state (as their lists say it: an
/// account off the allowlist makes them `disabled`) and last sync, and the released bindings kept.
async fn binding_list(state: &Arc<AppState>, reveal: bool) -> Result<String, String> {
    let at = now();
    let (accounts, pcs, tombstones) = state
        .db
        .call(|c| {
            let mut pcs = Vec::new();
            for d in db::all_devices(c)? {
                let on = db::list_on(c, d.account_id)?;
                pcs.push((d, on));
            }
            Ok((db::accounts(c)?, pcs, db::tombstones_by_reason(c)?))
        })
        .await
        .map_err(|e| e.to_string())?;
    let mut out = format!("bindings: {}\n", pcs.len());
    let _ = writeln!(
        out,
        "  {:<16}  {:<12}  {:<16}  {:<13}  {:<10}  {:<10}  {:<8}  bound{}",
        "tag",
        "device ID",
        "account",
        "pwless",
        "mark",
        "last sync",
        "version",
        if reveal { "       host name" } else { "" }
    );
    for (d, on) in &pcs {
        let account = accounts
            .iter()
            .find(|a| a.id == d.account_id)
            .map(|a| state.tags.account(&a.login_key))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "  {:<16}  {:<12}  {:<16}  {:<13}  {:<10}  {:<10}  {:<8}  {}{}",
            state.tags.device(&d.device_id),
            show(&d.device_id, reveal),
            account,
            pcsync::pwless_state(d, *on, at, state.config.trust_ttl_secs),
            pcsync::activation(d),
            d.last_sync_at.map_or_else(|| "-".to_owned(), date),
            d.app_ver
                .as_deref()
                .filter(|v| !v.is_empty())
                .unwrap_or("-"),
            date(d.bound_at),
            if reveal {
                format!("  {}", d.hostname)
            } else {
                String::new()
            }
        );
    }
    let _ = writeln!(
        out,
        "released bindings kept 180 days: {}",
        pairs_text(&tombstones)
    );
    Ok(out)
}

/// "a 1, b 2", or "none".
fn pairs_text(pairs: &[(String, i64)]) -> String {
    if pairs.is_empty() {
        return "none".to_owned();
    }
    pairs
        .iter()
        .map(|(name, n)| format!("{name} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

async fn execute(state: &Arc<AppState>, req: &AdminRequest) -> Result<String, String> {
    let words: Vec<&str> = req.argv.iter().map(String::as_str).collect();
    let reveal = req.reveal;
    let dberr = |e: crate::db::DbError| e.to_string();
    let at = now();
    match words.as_slice() {
        ["allow", "import", ..] => {
            let ids = read_ids(req.input.as_deref())?;
            let (added, already) = state
                .db
                .call(move |c| db::allow_add(c, &ids, at))
                .await
                .map_err(dberr)?;
            Ok(format!("allowlist: {added} added, {already} already there"))
        }
        ["allow", "remove", ..] => {
            let ids = read_ids(req.input.as_deref())?;
            let listed = ids.len();
            let (removed, tokens, keys) = state
                .db
                .call(move |c| {
                    let r = db::allow_remove(c, &ids, at)?;
                    db::checkpoint(c)?;
                    Ok(r)
                })
                .await
                .map_err(dberr)?;
            Ok(format!(
                "allowlist: {removed} removed, {} not on it, {tokens} tokens and {keys} account keys revoked",
                listed - removed
            ))
        }
        ["allow", "list"] => {
            let list = state.db.call(|c| db::allowlist(c)).await.map_err(dberr)?;
            let mut out = format!("allowlist: {} IDs\n", list.len());
            for (key, added) in list {
                let _ = writeln!(out, "  {:<24} added {}", show(&key, reveal), date(added));
            }
            Ok(out)
        }
        ["users"] => {
            let (accounts, devices, tokens, keys) = state
                .db
                .call(|c| {
                    let accounts = db::accounts(c)?;
                    let devices = db::all_devices(c)?;
                    let per_account = |sql: &str| -> rusqlite::Result<Vec<(i64, i64)>> {
                        let mut stmt = c.prepare(sql)?;
                        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                        rows.collect()
                    };
                    let tokens =
                        per_account("SELECT account_id, COUNT(*) FROM tokens GROUP BY account_id")?;
                    let keys = per_account(
                        "SELECT account_id, COUNT(*) FROM ctl_keys WHERE revoked_at IS NULL
                         GROUP BY account_id",
                    )?;
                    Ok((accounts, devices, tokens, keys))
                })
                .await
                .map_err(dberr)?;
            let count = |rows: &[(i64, i64)], id: i64| {
                rows.iter().find(|(a, _)| *a == id).map_or(0, |(_, n)| *n)
            };
            let mut out = format!("accounts: {}\n", accounts.len());
            let _ = writeln!(
                out,
                "  {:<16}  {:<16}  {:>7}  {:>4}  {:>6}  last login{}",
                "tag",
                "portal ID",
                "devices",
                "keys",
                "tokens",
                if reveal { "  name" } else { "" }
            );
            for a in accounts {
                let d = devices.iter().filter(|d| d.account_id == a.id).count();
                let _ = writeln!(
                    out,
                    "  {:<16}  {:<16}  {:>7}  {:>4}  {:>6}  {}{}",
                    state.tags.account(&a.login_key),
                    show(&a.portal_id, reveal),
                    d,
                    count(&keys, a.id),
                    count(&tokens, a.id),
                    date(a.last_login_at),
                    if reveal {
                        format!("  {}", a.display_name)
                    } else {
                        String::new()
                    }
                );
            }
            Ok(out)
        }
        ["key", "list"] => key_list(state, None, reveal).await,
        ["key", "list", arg] => {
            let account = find_account(state, target(arg)?)
                .await?
                .ok_or("no such account")?;
            key_list(state, Some(account), reveal).await
        }
        ["key", "revoke", n] => {
            let ckid =
                key_number(n).ok_or("key revoke takes an account key's number (key list)")?;
            let revoked = state
                .db
                .call(move |c| db::in_tx(c, |tx| db::revoke_any_key(tx, ckid, "admin", at)))
                .await
                .map_err(dberr)?;
            Ok(if revoked {
                format!("revoked account key {ckid} and its token")
            } else {
                "no such active account key".into()
            })
        }
        ["devices"] => {
            let (accounts, devices) = state
                .db
                .call(|c| Ok((db::accounts(c)?, db::all_devices(c)?)))
                .await
                .map_err(dberr)?;
            let mut out = format!("devices: {}\n", devices.len());
            let _ = writeln!(
                out,
                "  {:<16}  {:<12}  {:<16}  {:<8}  {:<10}  added{}",
                "tag",
                "device ID",
                "account",
                "os",
                "last seen",
                if reveal { "       host name" } else { "" }
            );
            for d in devices {
                let account = accounts
                    .iter()
                    .find(|a| a.id == d.account_id)
                    .map(|a| state.tags.account(&a.login_key))
                    .unwrap_or_default();
                let _ = writeln!(
                    out,
                    "  {:<16}  {:<12}  {:<16}  {:<8}  {:<10}  {}{}",
                    state.tags.device(&d.device_id),
                    show(&d.device_id, reveal),
                    account,
                    d.os,
                    date(d.last_seen_at),
                    date(d.bound_at),
                    if reveal {
                        format!("  {}", d.hostname)
                    } else {
                        String::new()
                    }
                );
            }
            Ok(out)
        }
        ["unlock", arg] => {
            let keys = unlock_keys(state, arg).await?;
            let cleared = state
                .db
                .call(move |c| limits::clear(c, &keys))
                .await
                .map_err(dberr)?;
            Ok(format!("cleared {cleared} counted login attempts"))
        }
        ["user", "revoke-all", "--all"] => {
            let (keys, tokens) = state
                .db
                .call(move |c| {
                    db::in_tx(c, |tx| {
                        let tokens = tx.execute("DELETE FROM tokens", [])?;
                        Ok((db::revoke_all_keys(tx, "admin", at)?, tokens))
                    })
                })
                .await
                .map_err(dberr)?;
            Ok(format!(
                "revoked {keys} account keys and {tokens} tokens of every account"
            ))
        }
        ["revoke", arg] | ["user", "revoke-all", arg] => {
            let account = find_account(state, target(arg)?)
                .await?
                .ok_or("no such account")?;
            let (keys, tokens) = state
                .db
                .call(move |c| {
                    db::in_tx(c, |tx| {
                        let tokens = db::revoke_tokens(tx, account.id)?;
                        Ok((
                            db::revoke_account_keys(tx, account.id, "admin", at)?,
                            tokens,
                        ))
                    })
                })
                .await
                .map_err(dberr)?;
            Ok(format!("revoked {keys} account keys and {tokens} tokens"))
        }
        ["binding", "list"] => binding_list(state, reveal).await,
        // design v3 §5-6: device remove is binding revoke's other name, so that either name
        // leaves the tombstone that gets the PC a signed revoked
        ["binding", "revoke", arg] | ["device", "remove", arg] => {
            let Some(id) = bound_pc(state, arg).await? else {
                return Ok("no such PC in any account".into());
            };
            let released = state
                .db
                .call(move |c| {
                    let released = db::in_tx(c, |tx| db::release_binding(tx, &id, "admin", at))?;
                    if released {
                        db::checkpoint(c)?;
                    }
                    Ok(released)
                })
                .await
                .map_err(dberr)?;
            Ok(if released {
                "released the PC from its account; its next sync gets a signed revoked".into()
            } else {
                "no such PC in any account".into()
            })
        }
        ["user", "delete", arg] => {
            let account = find_account(state, target(arg)?)
                .await?
                .ok_or("no such account")?;
            let id = account.id;
            let (devices, keys, tokens) = state
                .db
                .call(move |c| {
                    let counts = db::in_tx(c, |tx| {
                        let devices = db::devices_of(tx, id)?.len();
                        let keys = db::keys_of(tx, id)?.len();
                        let tokens = db::revoke_tokens(tx, id)?;
                        // its PCs are released first (tombstones); the keys, their nonces and
                        // their reported use go with the account (cascade)
                        db::delete_account(tx, id, at)?;
                        Ok((devices, keys, tokens))
                    })?;
                    db::checkpoint(c)?;
                    Ok(counts)
                })
                .await
                .map_err(dberr)?;
            Ok(format!(
                "deleted the account with {devices} devices, {keys} account keys and {tokens} tokens (the allowlist entry stays)"
            ))
        }
        ["passwordless", "status"] => passwordless_status(state, reveal).await,
        ["passwordless", turn @ ("on" | "off")] => {
            let on = *turn == "on";
            let changed = state
                .db
                .call(move |c| db::in_tx(c, |tx| db::set_passwordless_global(tx, on)))
                .await
                .map_err(dberr)?;
            Ok(format!(
                "password-less connections: {turn} for every account{}",
                if changed { "" } else { " (no change)" }
            ))
        }
        ["passwordless", turn @ ("on" | "off"), how @ ("--account" | "--owner-of"), arg] => {
            let on = *turn == "on";
            let account = if *how == "--account" {
                find_account(state, target(arg)?)
                    .await?
                    .ok_or("no such account")?
            } else {
                owner_of(state, arg).await?
            };
            let changed = state
                .db
                .call(move |c| {
                    db::in_tx(c, |tx| {
                        db::set_account_passwordless(tx, account.id, Some(on))
                    })
                })
                .await
                .map_err(dberr)?;
            Ok(format!(
                "password-less connections: {turn} for the account {}{}",
                state.tags.account(&account.login_key),
                if changed { "" } else { " (no change)" }
            ))
        }
        ["strict", turn @ ("on" | "off")] => {
            let on = *turn == "on";
            let changed = state
                .db
                .call(move |c| db::in_tx(c, |tx| db::set_strict(tx, on)))
                .await
                .map_err(dberr)?;
            Ok(format!(
                "strict: {turn}{}",
                if changed { "" } else { " (no change)" }
            ))
        }
        ["report"] => {
            let ttl = state.config.trust_ttl_secs;
            let (counts, counted) = state
                .db
                .call(move |c| Ok((db::counts(c, at, ttl)?, limits::counted_since(c, at - DAY)?)))
                .await
                .map_err(dberr)?;
            let mut out = format!(
                "allowlist {}, accounts {}, devices {}, tokens {}\n",
                counts.allowlist, counts.accounts, counts.devices, counts.tokens
            );
            let revoked: Vec<String> = counts
                .keys_revoked
                .iter()
                .map(|(reason, n)| format!("{reason} {n}"))
                .collect();
            let _ = writeln!(
                out,
                "account keys: {} active; revoked, kept 30 days: {}",
                counts.keys_active,
                if revoked.is_empty() {
                    "none".to_owned()
                } else {
                    revoked.join(", ")
                }
            );
            let _ = writeln!(
                out,
                "password-less: {} for every account, accounts on {}, accounts off {}; strict {}; change serial {}",
                on_off(counts.passwordless),
                counts.accounts_on,
                counts.accounts_off,
                on_off(counts.strict),
                counts.change_serial
            );
            // design v3 §5-6, §5-9: what the monitoring and the incident switch go by
            let _ = writeln!(
                out,
                "PCs holding their binding: {}, of them synced in the last 10 minutes: {}",
                counts.pcs_holding, counts.pcs_synced_recently
            );
            let _ = writeln!(
                out,
                "PCs holding their binding that have not synced since the last switch change{}: {}",
                counts
                    .switched_at
                    .map_or_else(String::new, |t| format!(" ({})", datetime(t))),
                counts.pcs_behind_switch
            );
            let _ = writeln!(
                out,
                "PCs that need a login on them (needs-relogin): {}",
                counts.needs_relogin
            );
            let _ = writeln!(out, "PCs by app version: {}", pairs_text(&counts.versions));
            let _ = writeln!(
                out,
                "refused proofs of the last 30 days: {}",
                pairs_text(&counts.refusals)
            );
            let _ = writeln!(
                out,
                "released bindings kept 180 days: {}",
                pairs_text(&counts.tombstones)
            );
            let _ = writeln!(
                out,
                "password-less connections of the last 30 days: {}",
                counts.uses
            );
            // Wake-on-LAN (design v3 §5-6): the relays, their last polls, the PCs' reports, the jobs
            let _ = writeln!(
                out,
                "relays: {}; codes approved ahead: {}",
                pairs_text(&counts.relays),
                counts.preapprovals
            );
            for (id, last) in &counts.relay_polls {
                let _ = writeln!(
                    out,
                    "  active relay {id}: last poll {}",
                    last.map_or_else(|| "never".to_owned(), |t| format!("{} s ago", at - t))
                );
            }
            let _ = writeln!(
                out,
                "PCs that reported how to wake them: {}",
                counts.wol_reports
            );
            out.push_str("wake jobs (last day / last week):\n");
            for (result, day, week) in &counts.wakes {
                let _ = writeln!(out, "  {result:<28} {day:>5} / {week}");
            }
            out.push_str("records (last day / last week):\n");
            for (result, day, week) in counts.results {
                let _ = writeln!(out, "  {result:<28} {day:>5} / {week}");
            }
            out.push_str("login-limit counters of the last day (ip, id, bound: failures; ok-*: successes):\n");
            for (scope, n) in counted {
                let _ = writeln!(out, "  {scope:<28} {n:>5}");
            }
            if let Some(reason) = state.sso_breaker.pause_reason() {
                let _ = writeln!(
                    out,
                    "SSO calls: paused ({reason}: the SSO refused this server or kept failing); logins get 503"
                );
            }
            let _ = writeln!(out, "seal key IDs: {}", state.seal.key_ids().join(", "));
            Ok(out)
        }
        ["relay", "list"] => relay::admin_list(state, reveal).await,
        ["relay", "approve", code, rest @ ..] => {
            let options = approve_options(rest)?;
            relay::admin_approve(state, code, options.pc, options.name, options.force, reveal).await
        }
        ["relay", "revoke", arg] => relay::admin_revoke(state, arg).await,
        ["relay", "share", turn @ ("on" | "off"), arg] => {
            relay::admin_share(state, arg, *turn == "on").await
        }
        ["selfcheck"] => {
            let held = ListKeyCheck::Held {
                kid: state.list_key.kid(),
                public_key: state.list_key.public_key(),
            };
            let command = match &state.command_key {
                Some(key) => CommandKeyCheck::Held {
                    kid: key.kid(),
                    public_key: key.public_key(),
                },
                None => CommandKeyCheck::Absent,
            };
            let (report, ok) = crate::selfcheck::run(&state.config, &held, &command).await;
            if ok {
                Ok(report)
            } else {
                Err(report)
            }
        }
        _ => Err(format!("unknown command\n{USAGE}")),
    }
}

/// How long the command line waits for the admin socket of a service that is starting: it binds
/// the socket after opening its database, a moment after `systemctl start` returns.
pub const START_WAIT: Duration = Duration::from_secs(10);

/// Connects to the admin socket; while it is not there yet or refuses, tries again for `wait`.
pub async fn connect(socket: &Path, wait: Duration) -> Result<UnixStream, String> {
    let until = tokio::time::Instant::now() + wait;
    loop {
        match UnixStream::connect(socket).await {
            Ok(stream) => return Ok(stream),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                ) && tokio::time::Instant::now() < until =>
            {
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => {
                return Err(format!(
                    "{}: {e} (is hjviewer-api running, and are you root?)",
                    socket.display()
                ))
            }
        }
    }
}

/// The command line side: sends one command to the service and returns its output.
pub async fn send(socket: &Path, request: &AdminRequest) -> Result<String, String> {
    send_within(socket, request, START_WAIT).await
}

/// [`send`], waiting at most `wait` for the admin socket.
pub async fn send_within(
    socket: &Path,
    request: &AdminRequest,
    wait: Duration,
) -> Result<String, String> {
    let mut stream = connect(socket, wait).await?;
    let mut text = serde_json::to_string(request).map_err(|e| e.to_string())?;
    text.push('\n');
    stream
        .write_all(text.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut answer = String::new();
    tokio::time::timeout(
        Duration::from_secs(60),
        BufReader::new(stream).read_line(&mut answer),
    )
    .await
    .map_err(|_| "no answer from the service".to_owned())?
    .map_err(|e| e.to_string())?;
    let response: AdminResponse =
        serde_json::from_str(&answer).map_err(|_| "the service answered nonsense".to_owned())?;
    if response.ok {
        Ok(response.output)
    } else {
        Err(response.error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_files() {
        let ids = read_ids(Some("# staff\n2026000001\n Y1234 # 교수\ny1234\n\n")).unwrap();
        assert_eq!(ids, vec!["2026000001".to_owned(), "y1234".to_owned()]);
        assert!(read_ids(None).is_err());
        // a byte-order mark, zero-width characters, Windows line ends
        let ids = read_ids(Some(
            "\u{feff}2026000001\r\nY12\u{200b}34\r\n\u{a0}2026000002\u{a0}\r\n",
        ))
        .unwrap();
        assert_eq!(ids, vec!["2026000001", "2026000002", "y1234"]);
        // a line that is not an ID refuses the whole file, naming line numbers only
        let err = read_ids(Some("2026000001\n홍길동 2026000002\n\nnot an id\n")).unwrap_err();
        assert!(err.starts_with("nothing changed: line 2, 4 of"), "{err}");
        assert!(
            !err.contains("2026000002") && !err.contains("홍길동"),
            "{err}"
        );
        let many: String = (0..12).map(|i| format!("bad id {i}\n")).collect();
        assert!(read_ids(Some(&many))
            .unwrap_err()
            .contains("line 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, … of"));
    }

    #[test]
    fn addresses() {
        assert_eq!(address("ip:192.0.2.1").unwrap(), "192.0.2.1");
        assert_eq!(address(" ip: 2001:DB8::1 ").unwrap(), "2001:db8::1");
        assert!(address("ip:2026000001x").is_err());
        assert!(address("ip:").is_err());
    }

    #[test]
    fn targets() {
        assert!(
            matches!(target("tag:0123456789ABCDEF"), Ok(Target::Tag(t)) if t == "0123456789abcdef")
        );
        assert!(target("tag:xyz").is_err());
        assert!(matches!(target("2026"), Ok(Target::Value(_))));
        assert!(target(" ").is_err());
    }
}
