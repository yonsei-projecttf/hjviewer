// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-api, the HJViewer account API (README.md).
//!
//!     hjv-api serve [--db <file>] [--env-file <file>] [--credentials <dir>]
//!                   [--listen 127.0.0.1:<port>] [--admin-socket <path>] [--operator-log <file>]
//!     hjv-api admin [--socket <path>] [--reveal] <command>…
//!     hjv-api keygen seal|log-key|list --kid N|command --kid N [--encrypt-to <credential file> | --out <file>]
//!     hjv-api selfcheck [--env-file <file>] [--credentials <dir>] [--log-only]
//!     hjv-api check [--api-socket <path>] [--admin-socket <path>] [--findings <file>]
//!     hjv-api alert <unit>|test [--findings <file>] [--state-dir <dir>]
//!     hjv-api oplog-verify [<file>]

use hjv_api::{
    admin::{self, AdminRequest},
    alert, backup, check,
    config::{load_command_key, load_list_key, read_env_file, Config, Secrets, ENV_PREFIX},
    db, jobs,
    keygen::{self, Target},
    log_crit, log_err, log_notice, log_warn, oplog,
    selfcheck::{self, CommandKeyCheck, ListKeyCheck},
    server::{self, Listener},
    state::AppState,
    trust,
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::ExitCode,
    sync::Arc,
};
use tokio::{
    net::{TcpListener, UnixListener},
    sync::watch,
};

const DEFAULT_DB: &str = "/var/lib/hjviewer-api/hjv-api.sqlite3";
const DEFAULT_OPLOG: &str = "/var/log/hjviewer-api/operator.log";

const USAGE: &str = "\
hjv-api serve [--db <file>] [--env-file <file>] [--credentials <dir>]
              [--listen 127.0.0.1:<port>] [--admin-socket <path>] [--operator-log <file>]
hjv-api admin [--socket <path>] [--reveal] <command> (hjv-api admin help)
hjv-api keygen seal|log-key|list --kid N|command --kid N [--encrypt-to <credential file> | --out <file>]
hjv-api selfcheck [--env-file <file>] [--credentials <dir>] [--log-only]
hjv-api check [--api-socket <path>] [--admin-socket <path>] [--findings <file>]
hjv-api alert <unit>|test [--findings <file>] [--state-dir <dir>]
hjv-api oplog-verify [<file>]
hjv-api version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("serve") => {
            // before any thread starts: taking the socket clears systemd's environment variables
            let socket = server::systemd_socket();
            runtime(true).and_then(|rt| rt.block_on(serve(&args[1..], socket)))
        }
        // keygen makes a credential locally, never through the service
        Some("admin") if args.get(1).is_some_and(|a| a == "keygen") => keygen_cli(&args[2..]),
        Some("admin") => runtime(false).and_then(|rt| rt.block_on(admin_cli(&args[1..]))),
        Some("keygen") => keygen_cli(&args[1..]),
        Some("selfcheck") => runtime(false).and_then(|rt| rt.block_on(selfcheck_cli(&args[1..]))),
        // the five-minute check and the alert of its units (design v3 §5-9)
        Some("check") => runtime(false).and_then(|rt| rt.block_on(check::cli(&args[1..]))),
        Some("alert") => runtime(false).and_then(|rt| rt.block_on(alert::cli(&args[1..]))),
        Some("oplog-verify") => {
            let path = args.get(1).map_or(DEFAULT_OPLOG, String::as_str);
            oplog::verify(Path::new(path)).map(|n| println!("{n} lines, chain intact"))
        }
        Some("version" | "--version") => {
            println!(
                "hjv-api {} ({})",
                env!("CARGO_PKG_VERSION"),
                option_env!("HJV_API_COMMIT").unwrap_or("commit unknown")
            );
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hjv-api: {e}");
            ExitCode::FAILURE
        }
    }
}

fn runtime(multi: bool) -> Result<tokio::runtime::Runtime, String> {
    let mut builder = if multi {
        // a few threads are plenty, and the unit allows 64 tasks (TasksMax=)
        let mut b = tokio::runtime::Builder::new_multi_thread();
        b.worker_threads(4).max_blocking_threads(16);
        b
    } else {
        tokio::runtime::Builder::new_current_thread()
    };
    builder
        .enable_all()
        .build()
        .map_err(|e| format!("runtime: {e}"))
}

/// Options of the form `--name value`; `flags` take no value.
fn options(
    args: &[String],
    names: &[&str],
    flags: &[&str],
) -> Result<(HashMap<String, String>, Vec<String>), String> {
    let mut opts = HashMap::new();
    let mut rest = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if flags.contains(&arg.as_str()) {
            opts.insert(arg.clone(), String::new());
        } else if names.contains(&arg.as_str()) {
            let value = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
            opts.insert(arg.clone(), value.clone());
        } else if arg.starts_with("--") && rest.is_empty() {
            return Err(format!("unknown option {arg}\n{USAGE}"));
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((opts, rest))
}

/// The settings: the environment, over an env file if one is given.
fn load_config(env_file: Option<&String>) -> Result<Config, String> {
    let mut vars = match env_file {
        Some(path) => read_env_file(Path::new(path))?,
        None => HashMap::new(),
    };
    vars.extend(std::env::vars().filter(|(k, _)| k.starts_with(ENV_PREFIX)));
    Config::from_vars(&vars)
}

/// The first directory of a systemd *_DIRECTORY variable.
fn systemd_dir(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .and_then(|v| v.split(':').next().map(PathBuf::from))
        .filter(|p| !p.as_os_str().is_empty())
}

async fn serve(
    args: &[String],
    systemd_socket: Result<Option<std::os::unix::net::UnixListener>, String>,
) -> Result<(), String> {
    let (opts, rest) = options(
        args,
        &[
            "--db",
            "--env-file",
            "--credentials",
            "--listen",
            "--admin-socket",
            "--operator-log",
        ],
        &[],
    )?;
    if !rest.is_empty() {
        return Err(format!("unexpected argument {}\n{USAGE}", rest[0]));
    }
    let config = load_config(opts.get("--env-file"))?;
    let credentials = opts
        .get("--credentials")
        .map(PathBuf::from)
        .or_else(|| systemd_dir("CREDENTIALS_DIRECTORY"))
        .ok_or("no credentials: run under systemd (LoadCredentialEncrypted=) or give --credentials <dir>")?;
    let secrets = Secrets::load(&credentials)?;
    let db_path = opts.get("--db").map_or(DEFAULT_DB, String::as_str);
    let conn = db::open(Path::new(db_path))?;

    let listener = match opts.get("--listen") {
        Some(addr) => {
            let addr: SocketAddr = addr
                .parse()
                .map_err(|_| "--listen takes an address such as 127.0.0.1:18700")?;
            if !addr.ip().is_loopback() {
                return Err("--listen is for tests and takes a loopback address only".into());
            }
            Listener::Tcp(
                TcpListener::bind(addr)
                    .await
                    .map_err(|e| format!("{addr}: {e}"))?,
            )
        }
        None => match systemd_socket? {
            Some(socket) => Listener::Unix(
                UnixListener::from_std(socket).map_err(|e| format!("socket from systemd: {e}"))?,
            ),
            None => {
                return Err(
                    "no socket: start hjv-api through hjviewer-api.socket, or give --listen 127.0.0.1:<port> for tests"
                        .into(),
                )
            }
        },
    };
    let second_factor = config.second_factor.clone();
    let state = AppState::new(config, secrets, conn, None)?;

    let (stop_tx, stop_rx) = watch::channel(false);
    let stopped = move || {
        let mut rx = stop_rx.clone();
        async move {
            let _ = rx.wait_for(|stop| *stop).await;
        }
    };
    tokio::spawn(async move {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        let _ = stop_tx.send(true);
    });

    let admin_socket = opts
        .get("--admin-socket")
        .map(PathBuf::from)
        .or_else(|| systemd_dir("RUNTIME_DIRECTORY").map(|d| d.join("admin.sock")));
    let oplog_path = opts
        .get("--operator-log")
        .map(PathBuf::from)
        .or_else(|| systemd_dir("LOGS_DIRECTORY").map(|d| d.join("operator.log")));
    match (admin_socket, oplog_path) {
        (Some(socket), Some(log)) => {
            // a problem with the record or the socket takes the admin commands out, not the logins
            let admin =
                oplog::OpLog::open(&log).and_then(|oplog| Ok((oplog, admin::bind(&socket)?)));
            match admin {
                Ok((oplog, listener)) => {
                    tokio::spawn(admin::serve(
                        state.clone(),
                        listener,
                        socket,
                        Arc::new(oplog),
                        stopped(),
                    ));
                }
                Err(e) => log_err!(
                    "{e}; admin commands are off until this is fixed and hjviewer-api restarted (README.md, Admin)"
                ),
            }
        }
        (Some(_), None) => {
            return Err(
                "an admin socket needs the operator log (--operator-log or LogsDirectory=)".into(),
            )
        }
        (None, _) => log_notice!("no admin socket (RuntimeDirectory= or --admin-socket)"),
    }
    if state.config.hbbs_pubkey.is_none() {
        log_warn!("HJV_API_HBBS_PUBKEY is not set: logins work, but no PC is added to an account");
    }
    check_list_key(&state);
    check_command_key(&state);
    tokio::spawn(jobs::run(state.clone(), stopped()));
    log_notice!(
        "hjv-api {} ready: seal keys {}, list key {}, command key {}, second factor {second_factor}, login {}",
        env!("CARGO_PKG_VERSION"),
        state.seal.key_ids().join(" "),
        &trust::fingerprint(state.list_key.kid(), &state.list_key.public_key())[..18],
        state.command_key.as_ref().map_or_else(
            || "none".to_owned(),
            |k| trust::fingerprint(k.kid(), &k.public_key())[..18].to_owned()
        ),
        state.config.login.as_str()
    );
    server::serve(state, listener, stopped()).await;
    log_notice!("hjv-api stopped");
    Ok(())
}

async fn admin_cli(args: &[String]) -> Result<(), String> {
    let (opts, argv) = options(args, &["--socket"], &["--reveal"])?;
    if argv.is_empty() || argv[0] == "help" {
        println!("{}", admin::USAGE);
        return Ok(());
    }
    let socket = opts
        .get("--socket")
        .map_or_else(|| PathBuf::from(admin::DEFAULT_SOCKET), PathBuf::from);
    let sudo_user = std::env::var("SUDO_USER").unwrap_or_default();
    // the copy of the database comes back through the socket and is written here, as root
    if argv[0] == "backup" {
        let output = backup::cli(&socket, &argv[1..], sudo_user).await?;
        print!("{output}");
        return Ok(());
    }
    let mut argv = argv;
    // the allowlist file is read here, as root, and sent; only counts come back
    let input = match argv.as_slice() {
        [a, b, file] if a == "allow" && (b == "import" || b == "remove") => {
            let text = std::fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
            argv.truncate(2);
            Some(text)
        }
        [a, b] if a == "allow" && (b == "import" || b == "remove") => {
            return Err(format!("hjv-api admin allow {b} <file>"))
        }
        _ => None,
    };
    let request = AdminRequest {
        argv,
        reveal: opts.contains_key("--reveal"),
        sudo_user,
        input,
    };
    let output = admin::send(&socket, &request).await?;
    print!("{output}");
    if !output.ends_with('\n') {
        println!();
    }
    Ok(())
}

/// The list key in use against the keys the apps pin (design v3 §5-3): a key they do not pin
/// gets every trust list refused, which `selfcheck` also reports.
fn check_list_key(state: &AppState) {
    let (kid, mine) = (state.list_key.kid(), state.list_key.public_key());
    match state.config.pinned(kid) {
        Some(pinned) if pinned == mine => {}
        Some(_) => log_crit!(
            "the list key (credential hjviewer-api-list-key) is not key {kid} of HJV_API_TRUST_PUBKEYS: the PCs refuse every trust list it signs (hjv-api selfcheck)"
        ),
        None if state.config.trust_pubkeys.is_empty() => log_warn!(
            "HJV_API_TRUST_PUBKEYS is not set: the list key is not compared with the keys the apps pin (hjv-api selfcheck)"
        ),
        None => log_crit!(
            "HJV_API_LIST_KID {kid} is not among HJV_API_TRUST_PUBKEYS: the PCs refuse every trust list (hjv-api selfcheck)"
        ),
    }
}

/// The command key in use against the keys the relays pin (relay/PROTOCOL.md §3.2): a key they
/// do not pin gets every wake command refused, which `selfcheck` also reports.
fn check_command_key(state: &AppState) {
    let config = &state.config;
    let Some(key) = &state.command_key else {
        if !config.command_pubkeys.is_empty() {
            log_crit!(
                "no command key (credential hjviewer-api-command-key) while HJV_API_COMMAND_PUBKEYS is set: no PC can be woken (hjv-api selfcheck)"
            );
        }
        return;
    };
    let (kid, mine) = (key.kid(), key.public_key());
    match config.pinned_command_key(kid) {
        Some(pinned) if pinned == mine => {}
        Some(_) => log_crit!(
            "the command key (credential hjviewer-api-command-key) is not key {kid} of HJV_API_COMMAND_PUBKEYS: the relays refuse every wake command it signs (hjv-api selfcheck)"
        ),
        None if config.command_pubkeys.is_empty() => log_warn!(
            "HJV_API_COMMAND_PUBKEYS is not set: the command key is not compared with the keys the relays pin (hjv-api selfcheck)"
        ),
        None => log_crit!(
            "HJV_API_COMMAND_KID {kid} is not among HJV_API_COMMAND_PUBKEYS: the relays refuse every wake command (hjv-api selfcheck)"
        ),
    }
}

fn keygen_cli(args: &[String]) -> Result<(), String> {
    let (opts, rest) = options(args, &["--encrypt-to", "--out", "--kid"], &[])?;
    let [kind] = rest.as_slice() else {
        return Err(format!("hjv-api {}", keygen::USAGE));
    };
    let kid = match opts.get("--kid") {
        Some(n) => Some(
            n.parse::<u32>()
                .map_err(|_| "--kid takes a key number, 1 to 65535".to_owned())?,
        ),
        None => None,
    };
    let target = match (opts.get("--encrypt-to"), opts.get("--out")) {
        (Some(_), Some(_)) => return Err("--encrypt-to or --out, not both".into()),
        (Some(path), None) => Target::EncryptTo(Path::new(path)),
        (None, Some(path)) => Target::Out(Path::new(path)),
        (None, None) => Target::Stdout,
    };
    keygen::run(kind, kid, target)
}

async fn selfcheck_cli(args: &[String]) -> Result<(), String> {
    let (opts, rest) = options(args, &["--env-file", "--credentials"], &["--log-only"])?;
    if !rest.is_empty() {
        return Err(
            "hjv-api selfcheck [--env-file <file>] [--credentials <dir>] [--log-only]".into(),
        );
    }
    let config = load_config(opts.get("--env-file"))?;
    // the list key and the command key in use, where the credentials are: in the unit
    // (ExecStartPost=) or a test's directory
    let dir = opts
        .get("--credentials")
        .map(PathBuf::from)
        .or_else(|| systemd_dir("CREDENTIALS_DIRECTORY"));
    let list_key = match &dir {
        Some(dir) => match load_list_key(dir) {
            Ok(seed) => ListKeyCheck::Held {
                kid: config.list_kid,
                public_key: trust::ListKey::new(config.list_kid, &seed).public_key(),
            },
            Err(e) => ListKeyCheck::NotRead(e),
        },
        None => ListKeyCheck::Unavailable,
    };
    let command_key = match &dir {
        Some(dir) => match load_command_key(dir) {
            Ok(Some(seed)) => CommandKeyCheck::Held {
                kid: config.command_kid,
                public_key: hjv_api::wake::CommandKey::new(config.command_kid, &seed).public_key(),
            },
            Ok(None) => CommandKeyCheck::Absent,
            Err(e) => CommandKeyCheck::NotRead(e),
        },
        None => CommandKeyCheck::Unavailable,
    };
    let (report, ok) = selfcheck::run(&config, &list_key, &command_key).await;
    if opts.contains_key("--log-only") {
        // after each start (ExecStartPost=-): report through the journal, never fail the unit
        for line in report.lines() {
            if line.starts_with("[FAIL]") {
                log_err!("selfcheck {line}");
            } else {
                log_notice!("selfcheck {line}");
            }
        }
        return Ok(());
    }
    print!("{report}");
    if ok {
        Ok(())
    } else {
        Err("selfcheck failed".into())
    }
}
