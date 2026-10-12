// SPDX-License-Identifier: AGPL-3.0-only

//! hjv-relay: the HJViewer Wake-on-LAN relay for an always-on Linux machine in the PCs' LAN
//! (docs/relay-dsm.md installs it on a Synology NAS; relay/PROTOCOL.md is the server contract).
//!
//!     hjv-relay run --state DIR [--log FILE] [--watch FILE] [--stop FILE] [--user UID:GID]
//!                   [--server HOST[:PORT]] [--ca FILE] [--status-file FILE]
//!     hjv-relay init --state DIR
//!     hjv-relay status --state DIR [--status-file FILE]
//!     hjv-relay test-wake MAC [--net CIDR | --iface NAME] [--count N] [--user UID:GID]
//!     hjv-relay version
//!
//! Exit codes: 0 done (for `run`: the executable was replaced, start it again, or the stop marker
//! appeared), 1 a failure worth retrying, 2 usage, 75 another relay already runs with this state
//! folder, 78 configuration (the boot-up task's loop stops on 2 and 78, and on 75 after a few
//! tries). `run` reports its usage errors in its log as configuration errors (78): its command line
//! is the boot-up task's.

use hjv_relay::{
    config, https, identity,
    log::clean,
    logln,
    netif::{self, Ipv4Net, Target},
    relay::{Exit, Relay, RelayConfig, SystemEnv, Timing},
    system::{self, LockError, StateDirError},
    wake::{self, SendPlan},
};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

const EX_FAIL: u8 = 1;
const EX_USAGE: u8 = 2;
const EX_TEMPFAIL: u8 = 75;
const EX_CONFIG: u8 = 78;

const USAGE: &str = "\
usage: hjv-relay <command>

  run --state DIR [--log FILE] [--watch FILE] [--stop FILE] [--user UID:GID]
      [--server HOST[:PORT]] [--ca FILE] [--status-file FILE]
        register with the HJViewer server and send magic packets on its signed commands;
        started as root it keeps DIR (mode 0700) and its key, then runs as UID:GID
        (default 65534:65534; not the owner of this program or its folder);
        --log: write the log to FILE and keep it to the last 180 days and 2 MB;
        --watch: end (exit 0) when FILE changes, instead of when this program does (the
        boot-up task runs a checked copy and watches the uploaded file);
        --stop: end (exit 0) when FILE appears (the stop marker);
        --server and --ca (the only trusted CA) are for tests
  init --state DIR
        make the relay key if there is none and print the relay ID and enrollment code
  status --state DIR [--status-file FILE]
        registration, last contact with the server, and the network interfaces
  test-wake MAC [--net CIDR | --iface NAME] [--count N] [--user UID:GID]
        send magic packets now, without the server (field test): to 255.255.255.255 and the
        interface's own broadcast, ports 9 and 7, N rounds (1-5, default 3) 200 ms apart, on
        the interface matching CIDR, on NAME, or on every LAN interface
  version
        version and what the build is configured with
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    let code = match args.first().map(String::as_str) {
        Some("run") => run(rest),
        Some("init") => init(rest),
        Some("status") => status(rest),
        Some("test-wake") => test_wake(rest),
        Some("version" | "--version" | "-V") => version(),
        Some("help" | "--help" | "-h") => {
            print!("{USAGE}");
            0
        }
        _ => {
            eprint!("{USAGE}");
            EX_USAGE
        }
    };
    ExitCode::from(code)
}

struct Opts {
    values: HashMap<&'static str, String>,
    positional: Vec<String>,
}

impl Opts {
    fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }
}

fn parse_opts(args: &[String], known: &[&'static str], positional: usize) -> Result<Opts, String> {
    let mut opts = Opts {
        values: HashMap::new(),
        positional: Vec::new(),
    };
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_owned())),
            _ => (arg.as_str(), None),
        };
        if let Some(known) = known.iter().find(|k| **k == name) {
            let value = match inline {
                Some(v) => v,
                None => it.next().cloned().ok_or(format!("{name} needs a value"))?,
            };
            if opts.values.insert(known, value).is_some() {
                return Err(format!("{name} given twice"));
            }
        } else if arg.starts_with('-') && arg.len() > 1 {
            return Err(format!("unknown option {arg}"));
        } else if opts.positional.len() < positional {
            opts.positional.push(arg.clone());
        } else {
            return Err(format!("unexpected argument {arg}"));
        }
    }
    Ok(opts)
}

fn usage_error(e: impl std::fmt::Display) -> u8 {
    eprintln!("hjv-relay: {e}");
    eprint!("{USAGE}");
    EX_USAGE
}

fn config_error(e: impl std::fmt::Display) -> u8 {
    logln!("설정 오류: {e}");
    EX_CONFIG
}

/// A usage error of `run`: logged, and a configuration error, so the boot-up task's loop stops.
fn run_usage_error(e: impl std::fmt::Display) -> u8 {
    config_error(format_args!(
        "run: {e} (명령줄은 hjv-relay help로 확인합니다)"
    ))
}

/// Refuses a `--user` (when running as root) that owns the executable or its folder: the
/// unprivileged relay could then replace the program the boot-up task starts as root.
fn check_user(uid: u32) -> Result<(), String> {
    if system::euid() != 0 {
        return Ok(());
    }
    let Ok(exe) = std::env::current_exe() else {
        return Ok(());
    };
    match system::owned_by(uid, &exe) {
        Some(path) => Err(format!(
            "--user: uid {uid} owns {}, so it could change the program root starts; use a user that owns nothing there (the default is 65534:65534)",
            path.display()
        )),
        None => Ok(()),
    }
}

/// Prints a line with the local time to stdout (test-wake's record of what it sent); a log that
/// cannot be written (a full disk) does not stop the sending.
fn say(line: impl std::fmt::Display) {
    use std::io::Write;
    let stamp = system::local_timestamp(hjv_relay::clock::system_now());
    let line = clean(&line.to_string(), hjv_relay::log::MAX_LINE_CHARS);
    let _ = writeln!(std::io::stdout(), "{stamp} {line}");
}

fn run(args: &[String]) -> u8 {
    // before anything else, and above all before the key is read
    let _ = system::set_non_dumpable();
    system::tidy_descriptors();
    system::detach_terminal();
    let opts = match parse_opts(
        args,
        &[
            "--state",
            "--user",
            "--server",
            "--ca",
            "--status-file",
            "--log",
            "--watch",
            "--stop",
        ],
        0,
    ) {
        Ok(o) => o,
        Err(e) => return run_usage_error(e),
    };
    // the log file first, opened as root, so that everything below goes there
    if let Some(path) = opts.get("--log") {
        if let Err(e) = hjv_relay::log::open_file(Path::new(path)) {
            logln!("로그 파일 {path}을(를) 쓸 수 없어 표준 오류에 씁니다: {e}");
        }
    }
    let Some(state) = opts.get("--state").map(PathBuf::from) else {
        return run_usage_error("run needs --state DIR");
    };
    let user = match system::parse_user(opts.get("--user").unwrap_or("65534:65534")) {
        Ok(u) => u,
        Err(e) => return run_usage_error(e),
    };
    if let Err(e) = check_user(user.0) {
        return config_error(e);
    }
    let server = match opts.get("--server").map(config::Server::parse).or_else(config::server) {
        Some(Ok(s)) => s,
        Some(Err(e)) => return config_error(format!("서버 주소: {e}")),
        None => {
            return config_error(
                "이 빌드에는 서버 주소(HJV_SERVER)가 없어 run을 할 수 없습니다(현장 시험용 묶음이면 정상입니다). test-wake, init, status, version은 됩니다.",
            )
        }
    };
    let keys = match config::command_keys() {
        Some(Ok(k)) => k,
        Some(Err(e)) => return config_error(format!("명령 공개키(HJV_COMMAND_PUBKEY): {e}")),
        None => {
            return config_error(
                "이 빌드에는 명령 공개키(HJV_COMMAND_PUBKEY)가 없어 run을 할 수 없습니다. test-wake, init, status, version은 됩니다.",
            )
        }
    };
    let roots = match opts.get("--ca") {
        Some(path) => match std::fs::read(path)
            .map_err(|e| e.to_string())
            .and_then(|pem| https::roots_from_pem(&pem))
        {
            Ok(r) => r,
            Err(e) => return config_error(format!("--ca {path}: {e}")),
        },
        None => https::builtin_roots(),
    };
    let client = match https::Client::new(server.clone(), roots) {
        Ok(c) => c,
        Err(e) => return config_error(format!("서버 {server}: {e}")),
    };

    match system::prepare_state_dir(&state, true) {
        Ok(()) => {}
        Err(StateDirError::Unavailable(e)) => {
            logln!("상태 폴더를 아직 쓸 수 없습니다({e}). 볼륨이 준비되면 다시 시작합니다.");
            return EX_FAIL;
        }
        Err(StateDirError::Unsafe(e)) => return config_error(e),
        Err(StateDirError::Io(e)) => {
            logln!("상태 폴더 {}: {e}", state.display());
            return EX_FAIL;
        }
    }
    let lock = match system::lock(&state) {
        Ok(l) => l,
        Err(LockError::Busy) => {
            logln!("이미 다른 hjv-relay가 이 상태 폴더로 실행 중이라 끝냅니다.");
            return EX_TEMPFAIL;
        }
        Err(LockError::Io(e)) => {
            logln!("잠금 파일: {e}");
            return EX_FAIL;
        }
    };
    // only now, with the lock: a second relay never rewrites the log this one writes
    hjv_relay::log::manage();
    // only now: a second start that ends at the lock leaves its one line above, not a restart
    logln!(
        "hjv-relay {} 시작 (서버 {server}, 상태 폴더 {})",
        config::VERSION,
        state.display()
    );
    let (identity, created) = match identity::load_or_create(&state) {
        Ok(v) => v,
        Err(identity::KeyError::Invalid(e)) => {
            drop(lock);
            return config_error(format!(
                "중계기 키를 쓸 수 없습니다: {e}. 키를 지우면 새 중계기로 다시 등록해야 합니다."
            ));
        }
        Err(identity::KeyError::Io(e)) => {
            logln!("중계기 키: {e}");
            return EX_FAIL;
        }
    };
    let os = system::os_description();
    let kernel = system::kernel_release();
    let exe = match opts.get("--watch") {
        Some(path) => match system::ExeWatch::file(PathBuf::from(path)) {
            Ok(w) => Some(w),
            Err(e) => {
                logln!("--watch {path}을(를) 열 수 없어 이 실행 파일을 지켜봅니다: {e}");
                system::ExeWatch::new()
            }
        },
        None => system::ExeWatch::new(),
    };
    let stop = match opts.get("--stop") {
        Some(path) => match system::StopMarker::open(Path::new(path)) {
            Ok(m) => Some((m, path)),
            Err(e) => {
                logln!("멈춤 표지 {path}의 폴더를 열 수 없어 멈춤 표지를 보지 않습니다: {e}");
                None
            }
        },
        None => None,
    };
    let stop_name = stop
        .as_ref()
        .map(|(_, path)| (*path).to_owned())
        .unwrap_or_default();
    let status_path = PathBuf::from(
        opts.get("--status-file")
            .unwrap_or(system::DEFAULT_STATUS_FILE),
    );
    let status_file = system::StatusFile::open(&status_path).ok();

    // nothing below needs root
    match system::drop_privileges(user.0, user.1) {
        Ok(true) => logln!("권한을 uid {} gid {}로 낮췄습니다.", user.0, user.1),
        Ok(false) => logln!(
            "root가 아니어서 uid {}로 그대로 실행합니다.",
            system::euid()
        ),
        Err(e) => return config_error(format!("권한을 낮추지 못했습니다: {e}")),
    }
    logln!(
        "중계기 ID {}, 등록 코드 {}{} ({os}, 커널 {kernel})",
        identity.id,
        identity.code,
        if created { ", 새 키" } else { "" }
    );

    let cfg = RelayConfig {
        identity,
        keys,
        client,
        os,
        kernel,
        timing: Timing::default(),
    };
    let env = SystemEnv {
        exe,
        stop: stop.map(|(m, _)| m),
        status: status_file,
    };
    let mut relay = Relay::new(cfg, env);
    match relay.run() {
        Exit::ExeChanged(why) => logln!("실행 파일이 바뀌었습니다({why}). 끝냅니다."),
        Exit::Stopped => logln!("멈춤 표지 {stop_name}이(가) 있어 끝냅니다."),
    }
    drop(lock);
    0
}

fn init(args: &[String]) -> u8 {
    let opts = match parse_opts(args, &["--state"], 0) {
        Ok(o) => o,
        Err(e) => return usage_error(e),
    };
    let Some(state) = opts.get("--state").map(PathBuf::from) else {
        return usage_error("init needs --state DIR");
    };
    match system::prepare_state_dir(&state, true) {
        Ok(()) => {}
        Err(StateDirError::Unsafe(e)) => return config_error(e),
        Err(StateDirError::Unavailable(e) | StateDirError::Io(e)) => {
            eprintln!("hjv-relay: {}: {e}", state.display());
            return EX_FAIL;
        }
    }
    match identity::load_or_create(&state) {
        Ok((id, created)) => {
            println!("중계기 ID: {}", id.id);
            println!("등록 코드: {}", id.code);
            println!("공개키: {}", id.pk_base64());
            println!(
                "키 파일: {} ({})",
                identity::key_path(&state).display(),
                if created {
                    "새로 만듦"
                } else {
                    "이미 있음"
                }
            );
            0
        }
        Err(identity::KeyError::Invalid(e)) => config_error(e),
        Err(identity::KeyError::Io(e)) => {
            eprintln!("hjv-relay: {e}");
            EX_FAIL
        }
    }
}

fn version() -> u8 {
    println!("hjv-relay {} ({})", config::VERSION, config::arch());
    println!("commit: {}", config::commit().unwrap_or("unknown"));
    match config::server() {
        Some(Ok(s)) => println!("server: {s}"),
        Some(Err(e)) => println!("server: invalid ({e})"),
        None => println!("server: none (this build cannot 'run')"),
    }
    match config::command_keys() {
        // public keys, in the form HJV_COMMAND_PUBKEY takes, so a build can be reproduced
        Some(Ok(keys)) => {
            let given: Vec<String> = keys.iter().map(config::CommandKey::to_string).collect();
            println!("command keys: {}", given.join(","));
            // what hjv-api selfcheck shows for its command keys (PROTOCOL.md, "Command keys")
            let prints: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", k.id, k.fingerprint()))
                .collect();
            println!("command key fingerprints: {}", prints.join(","));
        }
        Some(Err(e)) => println!("command keys: invalid ({e})"),
        None => println!("command keys: none (this build cannot 'run')"),
    }
    0
}

fn status(args: &[String]) -> u8 {
    let opts = match parse_opts(args, &["--state", "--status-file"], 0) {
        Ok(o) => o,
        Err(e) => return usage_error(e),
    };
    let Some(state) = opts.get("--state").map(PathBuf::from) else {
        return usage_error("status needs --state DIR");
    };
    version();
    println!("상태 폴더: {}", state.display());
    match identity::load(&state) {
        Ok(Some(id)) => {
            println!("중계기 ID: {}", id.id);
            println!("등록 코드: {}", id.code);
        }
        Ok(None) => println!("중계기 키: 없음 (run이나 init을 한 적이 없음)"),
        Err(e) => println!("중계기 키: 읽을 수 없음 ({e})"),
    }
    match system::lock_held(&state) {
        Ok(true) => println!("실행 중: 예"),
        Ok(false) => println!("실행 중: 아니요"),
        Err(e) => println!("실행 중: 알 수 없음 ({e})"),
    }
    let status_path = opts
        .get("--status-file")
        .unwrap_or(system::DEFAULT_STATUS_FILE);
    print_runtime_status(Path::new(status_path));
    println!("랜카드:");
    let ifaces = netif::list().unwrap_or_default();
    let gateways = netif::system_gateways();
    for i in &ifaces {
        let addrs: Vec<String> = i.addrs.iter().map(Ipv4Net::to_string).collect();
        let gw = gateways
            .iter()
            .find(|g| g.iface == i.name)
            .map(|g| format!(" 게이트웨이 {}", g.ip))
            .unwrap_or_default();
        let use_ = match netif::ineligible_reason(i) {
            None => "사용".to_owned(),
            Some(why) => format!("제외: {why}"),
        };
        println!(
            "  {} {} {}{}{}{} [{use_}]",
            clean(&i.name, 40),
            i.mac
                .map(|m| wake::format_mac(&m))
                .unwrap_or_else(|| "-".to_owned()),
            if addrs.is_empty() {
                "-".to_owned()
            } else {
                addrs.join(" ")
            },
            if i.up() { " up" } else { " down" },
            if i.up() && !i.running() {
                " (연결 없음)"
            } else {
                ""
            },
            gw
        );
    }
    0
}

/// Prints the runtime status file. Whatever is in it (the unprivileged relay writes it) is
/// printed as numbers or as [cleaned](clean) text, so nothing reaches the terminal raw.
fn print_runtime_status(path: &Path) {
    let text = (0..3).find_map(|_| {
        let text = std::fs::read_to_string(path).ok()?;
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(v) => Some(v),
            Err(_) => {
                std::thread::sleep(Duration::from_millis(100));
                None
            }
        }
    });
    let Some(v) = text else {
        println!("실행 상태: 없음 ({} 없음)", path.display());
        return;
    };
    let time = |v: &serde_json::Value, key: &str| {
        v.get(key)
            .and_then(serde_json::Value::as_i64)
            .map(system::local_timestamp)
            .unwrap_or_else(|| "-".to_owned())
    };
    let s = |v: &serde_json::Value, key: &str, none: &str| {
        v.get(key)
            .and_then(serde_json::Value::as_str)
            .map(|s| clean(s, 200))
            .unwrap_or_else(|| none.to_owned())
    };
    let n = |v: &serde_json::Value, key: &str| {
        v.get(key)
            .and_then(serde_json::Value::as_u64)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_owned())
    };
    println!(
        "실행 상태 (pid {}, {} 기록):",
        n(&v, "pid"),
        time(&v, "updated")
    );
    println!(
        "  등록 상태: {} ({}부터)",
        s(&v, "state", "-"),
        time(&v, "state_since")
    );
    println!(
        "  서버: {}, 마지막 정상 응답: {}",
        s(&v, "server", "-"),
        time(&v, "last_ok")
    );
    if v.get("failures")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        > 0
    {
        println!(
            "  연속 실패 {}번, 마지막 오류: {} ({})",
            n(&v, "failures"),
            s(&v, "last_error", "-"),
            time(&v, "last_error_at")
        );
    }
    if let Some(c) = v.get("last_command").filter(|c| c.is_object()) {
        println!(
            "  마지막 켜기 명령: {} job {} 랜카드 {} 패킷 {} 결과 {}",
            time(c, "at"),
            s(c, "job", "-"),
            s(c, "iface", "-"),
            n(c, "packets"),
            s(c, "error", "ok")
        );
    }
}

fn test_wake(args: &[String]) -> u8 {
    system::tidy_descriptors();
    system::detach_terminal();
    let opts = match parse_opts(args, &["--net", "--iface", "--count", "--user"], 1) {
        Ok(o) => o,
        Err(e) => return usage_error(e),
    };
    let Some(mac) = opts.positional.first() else {
        return usage_error("test-wake needs the PC's MAC address");
    };
    let Some(mac) = wake::parse_mac_lenient(mac).filter(wake::wakeable) else {
        return usage_error(format!(
            "{mac:?} is not a PC's MAC address (aa:bb:cc:dd:ee:ff)"
        ));
    };
    let count = match opts.get("--count").map(str::parse::<u32>) {
        None => 3,
        Some(Ok(n)) if (1..=5).contains(&n) => n,
        Some(_) => return usage_error("--count must be 1-5"),
    };
    let net: Option<Ipv4Net> = match opts.get("--net").map(str::parse::<Ipv4Net>) {
        None => None,
        // a PC address with its prefix is fine: use its network
        Some(Ok(n)) if n.prefix >= 8 => Ipv4Net::new(n.network(), n.prefix),
        Some(Ok(_)) => return usage_error("--net needs a prefix of 8 or more"),
        Some(Err(e)) => return usage_error(e),
    };
    if net.is_some() && opts.get("--iface").is_some() {
        return usage_error("give --net or --iface, not both");
    }
    let user = match system::parse_user(opts.get("--user").unwrap_or("65534:65534")) {
        Ok(u) => u,
        Err(e) => return usage_error(e),
    };
    if let Err(e) = check_user(user.0) {
        eprintln!("hjv-relay: {e}");
        return EX_CONFIG;
    }
    let ifaces = match netif::list() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("hjv-relay: cannot list the network interfaces: {e}");
            return EX_FAIL;
        }
    };
    if let Err(e) = system::drop_privileges(user.0, user.1) {
        eprintln!("hjv-relay: cannot drop privileges: {e}");
        return EX_CONFIG;
    }
    let targets: Vec<Target> = if let Some(name) = opts.get("--iface") {
        // named explicitly: any interface with a usable address, even one normally left out
        ifaces
            .iter()
            .filter(|i| i.name == name)
            .filter_map(|i| {
                i.addrs
                    .iter()
                    .find(|a| netif::usable_addr(a))
                    .map(|addr| Target {
                        iface: i.name.clone(),
                        addr: *addr,
                    })
            })
            .collect()
    } else if let Some(net) = &net {
        netif::select(&ifaces, net)
            .map(|(t, _)| t)
            .into_iter()
            .collect()
    } else {
        netif::all_targets(&ifaces)
    };
    if targets.is_empty() {
        let usable: Vec<String> = netif::all_targets(&ifaces)
            .iter()
            .map(|t| format!("{} {}", t.iface, t.addr))
            .collect();
        say(format_args!(
            "test-wake {}: 보낼 랜카드가 없습니다{}. 쓸 수 있는 랜카드: {}",
            wake::mask_mac(&mac),
            match (&net, opts.get("--iface")) {
                (Some(n), _) => format!(" ({n}과 겹치는 랜카드 없음)"),
                (_, Some(name)) => format!(" ({name}에 쓸 수 있는 IPv4 주소 없음)"),
                _ => String::new(),
            },
            if usable.is_empty() {
                "없음".to_owned()
            } else {
                usable.join(", ")
            }
        ));
        return EX_FAIL;
    }
    let mut all_sent = true;
    for target in &targets {
        let plan = SendPlan::new(target, &[mac], &[9, 7], count, Duration::from_millis(200));
        let carrier = ifaces.iter().any(|i| i.name == target.iface && i.running());
        let out = wake::send(&plan);
        say(format_args!(
            "test-wake {}: {}{}: {}/{}개 보냄{}{}",
            wake::mask_mac(&mac),
            plan.describe(),
            if carrier { "" } else { " (연결 없음)" },
            out.sent,
            out.planned,
            out.device_error
                .as_ref()
                .map(|e| format!(" (SO_BINDTODEVICE 안 됨: {e}, 주소 bind만)"))
                .unwrap_or_default(),
            out.errors
                .first()
                .map(|e| format!(" 오류: {e}"))
                .unwrap_or_default()
        ));
        all_sent &= out.sent > 0;
    }
    if all_sent {
        0
    } else {
        EX_FAIL
    }
}
