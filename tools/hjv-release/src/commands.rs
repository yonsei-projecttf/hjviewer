// SPDX-License-Identifier: AGPL-3.0-only

//! The commands (README.md). Results go to standard output, problems to standard error;
//! passphrases and the word `yes` come from the terminal only (tty.rs). No command prints a
//! private key, a passphrase or the door 2 code, and what `sign` shows of a draft, like every
//! error message, goes out without anything that could steer a terminal.

use crate::{
    age,
    archive::{position_of, Archive},
    args::{self, Args},
    check,
    draft::Draft,
    envelope::{Envelope, Part, POLICY_DOMAIN, ROOT_DOMAIN},
    hbbs::{self, Decision, Door2},
    keyfile::{self, SecretKey},
    keys::{self, parse_key_id, KeySet, Role},
    make::{self, Request},
    policy::{Policy, RootDecl},
    provenance::{Settings, SystemTools},
    publish, release, signing, time,
    trust::{self, check_envelope, hex, peek_server, sha256},
    tty::{self, Tty},
    version::Version,
};
use ed25519_dalek::SigningKey;
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
};

pub const USAGE: &str = "\
hjv-release: HJViewer의 서명된 버전 정책(최소 버전과 강제 업데이트)을 만들고 확인합니다.

  키(사용자가 터미널에서 직접)
    keygen --role signing|root --id N --out FILE     새 키(서명 키 1-100, 루트 키 101-199)를 비밀번호로 암호화해 씀
    pubkey --key FILE                                키 파일의 공개 줄을 다시 출력

  정책
    policy --prev FILE|none --out DRAFT [--keys FILE] [--archive DIR] [--server NAME] [--release-dir DIR]...
           [--min P=x.y.z]... [--latest P=x.y.z]... [--hold P]... [--drop P]...
           [--grace-days [P=]N]... [--grace-until [P=]T]... [--grace-min P=x.y.z]...
           [--auto-after windows=+2d|T|none] [--withdraw-package P]...
           [--os-min P=V]... [--contact TEXT] [--notes [P=]FILE]...
           [--ios-valid BUILD] [--appstore-live x.y.z|--no-appstore] [--play-live VERSIONCODE] [--no-play]
           [--move-to-play|--no-move-to-play]
           [--relax] [--revoke-key N]... [--new-epoch] [--live FILE] [--root-signs-policy] [--seq N]
           [--repo DIR] [--build-repo O/N] [--public-repo O/N] [--allow-input NAME]... [--attest]
                                                     다음 정책의 초안을 만듦(출처 확인 포함, 서명하지 않음)
    sign --key FILE [--key FILE] [--keys FILE] [--archive DIR] [--live FILE] --out FILE DRAFT
                                                     초안을 서명(터미널에서 비밀번호, 터미널 없으면 거부)
    verify [--keys FILE] --server NAME [--prev FILE] [--live FILE] [--archive DIR] FILE
                                                     서명된 정책을 앱처럼 확인(X4, --live면 X10과 서버 확인 4-5)
    publish-dir --policy FILE --release-dir DIR... --out DIR [--keys FILE]
                                                     서버로 보낼 pkg/, 알림 파일, SHA256SUMS를 모음
    void --live FILE [--keys FILE] [--archive DIR] FILE
                                                     서명했지만 내지 않은 정책을 무효로 함(런북 5-13)
    archive adopt [--keys FILE] [--archive DIR] FILE  직접 서명한 정책 파일을 보관소에 넣음(보관소를 잃었을 때)
    archive adopt --keys FILE --version V --server NAME [--archive DIR]
                                                     릴리스 태그의 키 파일을 기록된 키 집합으로 넣음

  서버 설정과 점검
    hbbs-conf --policy FILE --current FILE|none [--prev-policy FILE] [--keys FILE]
              [--text s1|s2|--text-file FILE] [--out FILE]
              [--door2-out DIR [--door2-until T] [--door2-rotate] [--door2-only]]
                                                     hbbs의 MIN·FROM, min-controller.conf와 door2.conf
    floor --live FILE [--keys FILE] [--archive DIR]  HJV_POLICY_FLOOR=<세대>:<seq> 줄(서버의 정책, 보관소의 파일만)
    check [--policy FILE] [--keys FILE] [--archive DIR] [--door2 FILE] [--live FILE]
                                                     주 1회 점검(TestFlight 75일, 문 2, 서버의 정책과 보관소)
    keys [--keys FILE] [--server NAME] [--production [--policy FILE]]
                                                     키 파일을 읽어 보임(--production: 시험 키나 폐기된 키면 실패)

  시험(이 저장소의 공개된 시험 키만 씀)
    test-keys [--env]                                시험 키 목록(--env: 시험 빌드용 update-keys.env)
    test-sign --policy FILE [--root-decl FILE] [--key NAME] [--root-key NAME] [--raw] [--out FILE]

P는 windows, android, android_play, ios(--min은 all도). --keys의 기본값은 client/update-keys.env, --archive의
기본값은 ~/hjviewer-policy-archive(저장소 밖). 키 파일에 시험 키가 있으면 policy, sign, verify, publish-dir,
hbbs-conf, floor, void, archive는 --allow-test-keys가 있어야 하고(보관소도 --archive로 직접), check는 경고로
셉니다. 자세한 설명은 README.md.";

const DEFAULT_KEYS: &str = "client/update-keys.env";
const MAX_INPUT: u64 = 1024 * 1024;
const TEST_KEY_WARNING: &str =
    "키 파일에 이 저장소의 시험 키가 있습니다. 시험 키의 비밀키는 공개되어 있어 누구나 서명할 수 있습니다";

fn read_bytes(path: &str, what: &str) -> Result<Vec<u8>, String> {
    let meta = fs::metadata(path).map_err(|e| format!("{what} {path}: 읽을 수 없습니다({e})"))?;
    if !meta.is_file() || meta.len() > MAX_INPUT {
        return Err(format!("{what} {path}: 파일이 아니거나 너무 큽니다"));
    }
    fs::read(path).map_err(|e| format!("{what} {path}: 읽을 수 없습니다({e})"))
}

fn read_text(path: &str, what: &str) -> Result<String, String> {
    String::from_utf8(read_bytes(path, what)?)
        .map_err(|_| format!("{what} {path}: UTF-8 텍스트가 아닙니다"))
}

/// Creates a file that must not exist yet.
fn write_new(path: &Path, content: &[u8], mode: u32) -> Result<(), String> {
    let at = |e: std::io::Error| format!("{}: 쓸 수 없습니다({e})", path.display());
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists => {
                format!("이미 있습니다. 덮어쓰지 않습니다: {}", path.display())
            }
            _ => at(e),
        })?;
    file.write_all(content).map_err(at)?;
    file.sync_all().map_err(at)
}

/// Writes a file only its owner may read, replacing an older one in one step.
fn write_private(path: &Path, content: &[u8]) -> Result<(), String> {
    let at = |e: std::io::Error| format!("{}: 쓸 수 없습니다({e})", path.display());
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".new-{}", std::process::id()));
    let temporary = PathBuf::from(temporary);
    let _ = fs::remove_file(&temporary);
    write_new(&temporary, content, 0o600)?;
    fs::rename(&temporary, path).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        at(e)
    })
}

/// The key file of `--keys` (or the default). A file with a test key is refused where a real
/// policy could be judged or made by mistake, unless the command line says it is a test.
fn load_keys(args: &Args, gate: bool) -> Result<KeySet, String> {
    let path = match args.one("--keys")? {
        Some(path) => path,
        None if Path::new(DEFAULT_KEYS).is_file() => DEFAULT_KEYS,
        None => {
            return Err(format!(
                "--keys <키 파일>이 필요합니다(여기에는 기본 파일 {DEFAULT_KEYS}도 없습니다)"
            ))
        }
    };
    let keys =
        KeySet::parse(&read_text(path, "키 파일")?).map_err(|e| format!("키 파일 {path}: {e}"))?;
    if keys.has_test_key() {
        if gate && !args.flag("--allow-test-keys") {
            return Err(format!(
                "{TEST_KEY_WARNING}. 시험이면 --allow-test-keys를 주세요({path})"
            ));
        }
        eprintln!("경고: {TEST_KEY_WARNING}({path})");
    }
    Ok(keys)
}

/// The archive of `--archive`, or the default one. A key file with test keys never uses the
/// default archive: a command that writes to it is refused (a test must not leave files among
/// the operator's), one that only reads it reads an empty archive (a test must not depend on
/// them). No archive lies in the repository's `build/`, where agents make and delete worktrees.
fn open_archive(args: &Args, keys: &KeySet, create: bool) -> Result<Archive, String> {
    let dir = match args.one("--archive")? {
        Some(dir) => PathBuf::from(dir),
        None if keys.has_test_key() && !create => return Ok(Archive::none()),
        None if keys.has_test_key() => {
            return Err(
                "시험 키의 키 파일로는 기본 보관소(~/hjviewer-policy-archive)를 쓰지 않습니다. --archive <시험용 디렉터리>를 \
                 주세요"
                    .to_owned(),
            )
        }
        None => Archive::default_dir()?,
    };
    let absolute = |p: &Path| {
        fs::canonicalize(p).unwrap_or_else(|_| {
            std::env::current_dir()
                .map(|d| d.join(p))
                .unwrap_or_else(|_| p.to_path_buf())
        })
    };
    if let Ok(cwd) = std::env::current_dir() {
        if absolute(&dir).starts_with(absolute(&cwd.join("build"))) {
            return Err(format!(
                "보관소를 저장소의 build/ 안에 둘 수 없습니다(에이전트가 worktree를 만들고 지우는 곳): {}",
                dir.display()
            ));
        }
    }
    Archive::open(&dir, create)
}

/// The app version next to the key file (`client/version.env`'s `HJV_VERSION`), for the key set
/// records.
fn version_next_to(keys_path: &str) -> Option<String> {
    let path = Path::new(keys_path).with_file_name("version.env");
    let text = fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|l| l.trim().strip_prefix("HJV_VERSION="))
        .map(|v| v.trim().to_owned())
        .filter(|v| Version::parse(v).is_ok())
}

/// The key file path of this command line.
fn keys_path(args: &Args) -> Result<String, String> {
    Ok(args.one("--keys")?.unwrap_or(DEFAULT_KEYS).to_owned())
}

/// The key options of this command line, for the command line it tells the person to run next:
/// the same key file when it is not the default one, and the same word about test keys.
fn same_keys(args: &Args) -> Result<String, String> {
    let mut options = String::new();
    if let Some(path) = args.one("--keys")? {
        options.push_str(&format!(" --keys {path}"));
    }
    if let Some(dir) = args.one("--archive")? {
        options.push_str(&format!(" --archive {dir}"));
    }
    if args.flag("--allow-test-keys") {
        options.push_str(" --allow-test-keys");
    }
    Ok(options)
}

/// A verified envelope from a file.
fn load_envelope(
    path: &str,
    what: &str,
    keys: &KeySet,
) -> Result<(trust::Checked, String), String> {
    let file = read_text(path, what)?;
    let server = peek_server(&file).map_err(|e| format!("{what} {path}: {e}"))?;
    let checked =
        check_envelope(&file, keys, &server).map_err(|e| format!("{what} {path}: {e}"))?;
    Ok((checked, file))
}

fn no_tty(what: &str) -> impl Fn(String) -> String + '_ {
    move |_| {
        format!(
            "터미널(TTY)이 없어 {what}. 이 명령은 사용자가 터미널에서 직접 실행하고, 비밀번호는 터미널에서만 \
             받습니다(표준 입력, 환경 변수, 인자로는 받지 않습니다)"
        )
    }
}

/// How often the passphrase of a key file is asked for.
const PASSPHRASE_TRIES: usize = 3;

/// Asks for the passphrase of a key file, up to three times.
fn open_key(tty: &mut Tty, path: &str) -> Result<SecretKey, String> {
    let bytes = read_bytes(path, "키 파일")?;
    let mut tries_left = PASSPHRASE_TRIES;
    loop {
        let passphrase = tty.ask_secret(&format!("{path}의 비밀번호: "))?;
        tries_left = tries_left.saturating_sub(1);
        match SecretKey::decrypt(&bytes, &passphrase) {
            Ok(key) => return Ok(key),
            Err(keyfile::Error::WrongPassphrase) if tries_left > 0 => {
                tty.say("비밀번호가 맞지 않습니다. 다시 입력하세요.\n");
            }
            Err(e) => return Err(format!("키 파일 {path}: {e}")),
        }
    }
}

fn keygen(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(argv, &["--role", "--id", "--out"], &[])?;
    args.no_positional()?;
    let role = Role::parse(args.need("--role")?).ok_or("--role은 signing 또는 root입니다")?;
    let id = parse_key_id(args.need("--id")?)?;
    keys::check_id(role, id)?;
    let out = Path::new(args.need("--out")?);
    if out.exists() {
        return Err(format!(
            "이미 있습니다. 키 파일은 덮어쓰지 않습니다: {}",
            out.display()
        ));
    }
    let mut tty = Tty::open().map_err(no_tty("키를 만들지 않습니다"))?;
    tty::harden_process();
    tty.say(&format!(
        "새 {}(번호 {id})를 만듭니다. 키 파일을 지킬 비밀번호를 정하세요({}자 이상).\n\
         비밀번호를 잊으면 이 키는 다시 쓸 수 없습니다.\n",
        role.korean(),
        keyfile::MIN_PASSPHRASE_CHARS
    ));
    let passphrase = tty.ask_secret("비밀번호: ")?;
    keyfile::check_new_passphrase(&passphrase)?;
    let again = tty.ask_secret("비밀번호 다시: ")?;
    if *passphrase != *again {
        return Err("두 비밀번호가 다릅니다. 키를 만들지 않았습니다".to_owned());
    }
    let key = SecretKey::generate(role, id)?;
    let file = key.encrypt(&passphrase, age::DEFAULT_WORK_FACTOR)?;
    // the file must open again before the only copy of the key is trusted to it
    let reopened = SecretKey::decrypt(&file, &passphrase)
        .map_err(|_| "만든 키 파일을 다시 열지 못했습니다. 키를 만들지 않았습니다".to_owned())?;
    if reopened.public() != key.public() || reopened.role != role {
        return Err("만든 키 파일이 키와 다릅니다. 키를 만들지 않았습니다".to_owned());
    }
    write_new(out, &file, 0o600)?;
    println!("{}", key.public());
    let variable = match role {
        Role::Signing => keys::SIGNING_VAR,
        Role::Root => keys::ROOT_VAR,
    };
    tty.say(&format!(
        "키 파일을 썼습니다: {}(0600, 비밀번호로 암호화).\n\
         위의 공개 줄을 client/update-keys.env의 {variable}에 넣으세요.\n\
         키 파일의 오프라인 사본과 비밀번호를 따로 보관하세요.{}\n",
        out.display(),
        if role == Role::Root {
            " 루트 키는 오프라인 매체에만 둡니다."
        } else {
            ""
        }
    ));
    Ok(0)
}

fn pubkey(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(argv, &["--key"], &[])?;
    args.no_positional()?;
    let path = args.need("--key")?;
    let mut tty = Tty::open().map_err(no_tty("키 파일을 열지 않습니다"))?;
    tty::harden_process();
    let key = open_key(&mut tty, path)?;
    println!("{}", key.public());
    tty.say(&format!(
        "{} {}의 공개 줄입니다.\n",
        key.role.korean(),
        key.id
    ));
    Ok(0)
}

fn policy(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &[
            "--prev",
            "--out",
            "--keys",
            "--server",
            "--release-dir",
            "--min",
            "--latest",
            "--hold",
            "--drop",
            "--grace-days",
            "--grace-until",
            "--auto-after",
            "--os-min",
            "--contact",
            "--notes",
            "--ios-valid",
            "--appstore-live",
            "--play-live",
            "--revoke-key",
            "--seq",
            "--live",
            "--archive",
            "--grace-min",
            "--withdraw-package",
            "--repo",
            "--build-repo",
            "--public-repo",
            "--allow-input",
        ],
        &[
            "--move-to-play",
            "--no-move-to-play",
            "--no-appstore",
            "--no-play",
            "--relax",
            "--new-epoch",
            "--root-signs-policy",
            "--attest",
            "--allow-test-keys",
        ],
    )?;
    args.no_positional()?;
    let keys = load_keys(&args, true)?;
    let archive = open_archive(&args, &keys, false)?;
    let out = args.need("--out")?;
    if Path::new(out).exists() {
        return Err(format!("이미 있습니다. 덮어쓰지 않습니다: {out}"));
    }
    let prev = match args.need("--prev")? {
        "none" => None,
        path => Some(read_text(path, "이전 정책")?),
    };
    let dirs: Vec<PathBuf> = args
        .all("--release-dir")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let mut request = Request {
        prev,
        server: args.one("--server")?.map(str::to_owned),
        artifacts: if dirs.is_empty() {
            Vec::new()
        } else {
            release::scan(&dirs)?
        },
        now: time::now(),
        ..Request::default()
    };
    for value in args.all("--min") {
        request.min.push(args::min(value)?);
    }
    for value in args.all("--latest") {
        request.latest.push(args::latest(value)?);
    }
    for value in args.all("--hold") {
        request.hold.push(args::platform(value)?);
    }
    for value in args.all("--drop") {
        request.drop.push(args::platform(value)?);
    }
    for value in args.all("--grace-days") {
        request.grace.push(args::grace_days(value)?);
    }
    for value in args.all("--grace-until") {
        request.grace.push(args::grace_until(value)?);
    }
    request.auto_after = args
        .one("--auto-after")?
        .map(args::auto_after)
        .transpose()?;
    for value in args.all("--os-min") {
        let (platform, os_min) = args::platform_value("--os-min", value)?;
        request.os_min.push((platform, os_min.to_owned()));
    }
    request.contact = args.one("--contact")?.map(|text| text.trim().to_owned());
    for value in args.all("--notes") {
        let (target, path) = args::target_value("--notes", value, true)?;
        request
            .notes
            .push((target, read_text(path, "알림 글")?.trim().to_owned()));
    }
    request.ios_valid = args.one("--ios-valid")?.map(str::to_owned);
    request.appstore_live = args
        .one("--appstore-live")?
        .map(|v| Version::parse(v).map_err(|e| format!("--appstore-live: {e}")))
        .transpose()?;
    request.no_appstore = args.flag("--no-appstore");
    request.no_play = args.flag("--no-play");
    request.play_live = args
        .one("--play-live")?
        .map(|code| args::number("--play-live", code))
        .transpose()?;
    request.move_to_play = match (args.flag("--move-to-play"), args.flag("--no-move-to-play")) {
        (true, true) => {
            return Err("--move-to-play와 --no-move-to-play를 함께 줄 수 없습니다".to_owned())
        }
        (true, false) => Some(true),
        (false, true) => Some(false),
        (false, false) => None,
    };
    request.relax = args.flag("--relax");
    for value in args.all("--revoke-key") {
        request.revoke.push(parse_key_id(value)?);
    }
    request.new_epoch = args.flag("--new-epoch");
    request.seq = args
        .one("--seq")?
        .map(|seq| args::number("--seq", seq))
        .transpose()?;
    request.root_signs_policy = args.flag("--root-signs-policy");
    request.live = args
        .one("--live")?
        .map(|path| read_text(path, "서버의 정책(--live)"))
        .transpose()?;
    for value in args.all("--grace-min") {
        let (platform, version) = args::platform_value("--grace-min", value)?;
        request.grace_min.push((platform, Version::parse(version)?));
    }
    for value in args.all("--withdraw-package") {
        request.withdraw.push(args::platform(value)?);
    }

    let mut settings = Settings::new(PathBuf::from(args.one("--repo")?.unwrap_or(".")));
    if let Some(repo) = args.one("--build-repo")? {
        settings.build_repo = repo.to_owned();
    }
    if let Some(repo) = args.one("--public-repo")? {
        settings.public_repo = repo.to_owned();
    }
    settings.extra_inputs = args
        .all("--allow-input")
        .into_iter()
        .map(str::to_owned)
        .collect();
    settings.attest = args.flag("--attest");

    let made = make::make(&request, &keys, &archive, &settings, &SystemTools)?;
    write_new(Path::new(out), made.draft.to_text().as_bytes(), 0o644)?;
    for line in trust::describe(&made.policy) {
        println!("{line}");
    }
    println!("바뀌는 것:");
    for line in &made.report {
        println!("  {line}");
    }
    for note in &made.draft.notes {
        println!("  확인함: {note}");
    }
    println!("초안을 썼습니다: {out}");
    let (key_files, live) = match (made.draft.signer, made.draft.root_p.is_some()) {
        (Role::Root, true) => ("--key <루트 키 파일>", " --live <방금 받은 서버의 정책>"),
        (Role::Root, false) => ("--key <루트 키 파일>", ""),
        (Role::Signing, true) => (
            "--key <루트 키 파일> --key <서명 키 파일>",
            " --live <방금 받은 서버의 정책>",
        ),
        (Role::Signing, false) => ("--key <서명 키 파일>", ""),
    };
    if made.draft.signer == Role::Root || made.draft.root_p.is_some() {
        println!(
            "루트 키는 깨끗한 컴퓨터에서만 엽니다(런북 5-0): 그 컴퓨터에서 서버의 정책을 받아 서명합니다"
        );
    }
    println!("서명은 사용자가 터미널에서 직접 합니다(에이전트는 서명하지 않습니다):");
    println!(
        "  hjv-release sign {key_files}{}{live} --out version-policy.json {out}",
        same_keys(&args)?
    );
    Ok(0)
}

fn sign(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &["--key", "--keys", "--out", "--archive", "--live"],
        &["--allow-test-keys"],
    )?;
    let draft_path = args.file("초안 파일")?;
    let key_paths = args.all("--key");
    if key_paths.is_empty() {
        return Err("--key <키 파일>이 필요합니다".to_owned());
    }
    let out = args.need("--out")?;
    // the terminal first: without a person at it, nothing is read and nothing is signed
    let mut tty = Tty::open().map_err(no_tty("서명하지 않습니다"))?;
    tty::harden_process();
    // which tool signs, before anything it asks (the owner signs with scripts/build-signer.sh's)
    tty.say(&format!("서명하는 도구: {}\n", build_name()));
    let keys = load_keys(&args, true)?;
    let archive = open_archive(&args, &keys, true)?;
    let draft = Draft::parse(&read_text(draft_path, "초안")?)?;
    if Path::new(out).exists() {
        return Err(format!("이미 있습니다. 덮어쓰지 않습니다: {out}"));
    }
    let live = args
        .one("--live")?
        .map(|path| read_text(path, "서버의 정책(--live)"))
        .transpose()?;
    let plan = signing::plan(&draft, &keys, &archive, live.as_deref(), time::now())?;

    // every line is one line, whatever the draft's texts hold (tty::printable)
    tty.say("서명할 정책:\n");
    for line in &plan.summary {
        tty.say(&format!("  {}\n", tty::printable(line)));
    }
    if !plan.dangers.is_empty() {
        tty.say(
            "\n아래는 되돌릴 수 없거나, 서버에 넣는 즉시 일어나거나, 루트 키가 있어야 하는 변경입니다:\n",
        );
        for danger in &plan.dangers {
            tty.say(&format!("  - {}\n", tty::printable(danger)));
        }
        if tty.ask("그래도 서명하려면 yes를 입력하세요: ")? != "yes" {
            return Err("yes가 아니어서 서명하지 않았습니다".to_owned());
        }
    }
    // which keys the draft needs is known before any key file is opened
    let needed: Vec<&str> = plan.needed(&draft).iter().map(|r| r.korean()).collect();
    tty.say(&format!("\n필요한 키: {}\n", needed.join(", ")));
    if key_paths.len() != needed.len() {
        return Err(format!(
            "이 초안에는 키 파일이 {}개 필요합니다({}). --key로 준 파일은 {}개입니다",
            needed.len(),
            needed.join(", "),
            key_paths.len()
        ));
    }
    let mut secrets = Vec::new();
    for path in key_paths {
        let key = open_key(&mut tty, path)?;
        tty.say(&format!(
            "  {}(번호 {})를 열었습니다.\n",
            key.role.korean(),
            key.id
        ));
        secrets.push(key);
    }
    let version = version_next_to(&keys_path(&args)?);
    let signed = signing::sign(&draft, &plan, &secrets, &keys, &archive, version.as_deref())?;
    drop(secrets);
    write_new(Path::new(out), signed.file.as_bytes(), 0o644)?;
    println!("서명했습니다: {out}");
    println!("sha256 {}", hex(&sha256(signed.file.as_bytes())));
    if !signed.archived.is_empty() {
        println!("보관소에 넣었습니다(서명 키의 오프라인 매체에도 복사하세요):");
        for path in &signed.archived {
            println!("  {}", path.display());
        }
    }
    if plan.new_root.is_some() {
        println!("  {}", make::relaxes_line(&plan.relaxed));
    }
    println!(
        "확인: hjv-release verify{} --server {} --prev <이전 정책> {out}",
        same_keys(&args)?,
        plan.policy.server
    );
    Ok(0)
}

fn verify(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &["--keys", "--server", "--prev", "--live", "--archive"],
        &["--allow-test-keys"],
    )?;
    let path = args.file("정책 파일")?;
    let server = args.need("--server")?;
    let keys = load_keys(&args, true)?;
    let file = read_text(path, "정책")?;
    let previous = args
        .one("--prev")?
        .map(|p| read_text(p, "이전 정책"))
        .transpose()?;
    let checked = check_envelope(&file, &keys, server)?;
    for line in trust::describe(&checked.policy) {
        println!("{line}");
    }
    println!(
        "서명: {} {}",
        checked.role.korean(),
        checked.envelope.policy.k
    );
    if let Some(root) = &checked.root {
        for line in trust::describe_declaration(root) {
            println!("{line}");
        }
    }
    println!("sha256 {}", hex(&sha256(file.as_bytes())));
    // X4: a device holding --prev and a fresh install take it
    make::x4(&file, previous.as_deref(), &keys, server, true)?;
    match &previous {
        Some(_) => {
            println!("검증됨: 이전 정책을 가진 앱과 새로 설치한 앱이 이 정책을 받습니다(X4)")
        }
        None => println!(
            "검증됨: 이 키 파일(하한 {})로 새로 설치한 앱이 이 정책을 받습니다(X4)",
            keys.floor
        ),
    }
    if let Some(live_path) = args.one("--live")? {
        let live = read_text(live_path, "서버의 정책(--live)")?;
        // the server check's items 4 and 5 against the live file
        let state = trust::holding(&keys, server, &live);
        if state.held.is_none() {
            return Err(
                "서버의 정책을 이 키 파일로 읽지 못합니다(서버 확인의 LiveUnreadable)".to_owned(),
            );
        }
        let (_, outcome) = trust::accept(&keys, server, &state, file.as_bytes())?;
        if !outcome.is_taken() {
            let reason = outcome.reason();
            return Err(format!(
                "서버의 정책을 가진 앱이 이 정책을 받지 않습니다({}: {})",
                reason.map(|r| r.code()).unwrap_or_default(),
                reason.map(|r| r.korean()).unwrap_or_default()
            ));
        }
        let live_root = Envelope::read_form(&live)?.1.root;
        let differs =
            checked.envelope.root.as_ref().map(|r| &r.p) != live_root.as_ref().map(|r| &r.p);
        if differs && checked.root.is_some() {
            // X10: a declaration only from the live file, which is --prev and a file of the archive
            let archive = open_archive(&args, &keys, false)?;
            if previous.as_deref() != Some(live.as_str()) {
                return Err(
                    "X10: 새 루트 선언은 서버의 정책(--live)을 --prev로 해서 만듭니다".to_owned(),
                );
            }
            if archive.find(&live)?.is_none() {
                return Err(
                    "X10: 서버의 정책이 보관소에 없습니다(다른 곳에서 서명됨: 런북 5-8)".to_owned(),
                );
            }
            let live_policy = check_envelope(&live, &keys, server)?.policy;
            if checked.policy.issued < live_policy.issued {
                return Err(
                    "서버 확인 5: 새 정책이 서버의 정책보다 일찍 발행되었습니다(LiveIssued)"
                        .to_owned(),
                );
            }
            let relaxed =
                trust::relaxed(&live_policy.requirements(), &checked.policy.requirements());
            println!("새 루트 선언: {}", make::relaxes_line(&relaxed));
        }
        println!(
            "서버의 정책을 가진 앱도 받습니다: {}",
            outcome_word(&outcome)
        );
    }
    Ok(0)
}

fn outcome_word(outcome: &trust::Outcome) -> &'static str {
    match outcome {
        trust::Outcome::Taken => "받음",
        trust::Outcome::Unchanged => "같은 정책(바뀌는 것 없음)",
        trust::Outcome::Refused(_) => "거부",
    }
}

fn publish_dir(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &["--policy", "--release-dir", "--out", "--keys"],
        &["--allow-test-keys"],
    )?;
    args.no_positional()?;
    let path = args.need("--policy")?;
    let out = Path::new(args.need("--out")?);
    let dirs: Vec<PathBuf> = args
        .all("--release-dir")
        .into_iter()
        .map(PathBuf::from)
        .collect();
    if dirs.is_empty() {
        return Err("--release-dir <디렉터리>가 필요합니다".to_owned());
    }
    let text = read_text(path, "정책")?;
    // a draft (before signing) or the signed envelope
    let (policy, envelope) = match Draft::parse(&text) {
        Ok(draft) => (Policy::parse(&draft.p)?, None),
        // a draft that breaks a rule of drafts is not an envelope either: say what is wrong
        Err(e) if text.contains("\"hjv_release_draft\"") => return Err(e),
        Err(_) => {
            let keys = load_keys(&args, true)?;
            let server = peek_server(&text)?;
            (check_envelope(&text, &keys, &server)?.policy, Some(text))
        }
    };
    let artifacts = release::scan(&dirs)?;
    for line in publish::publish(&policy, envelope.as_deref(), &artifacts, out)? {
        println!("{line}");
    }
    println!("모았습니다: {}", out.display());
    println!(
        "서버에서는 서명과 해시를 다시 확인한 뒤(deploy/hjv-update-verify.sh) 패키지를 먼저, 정책을 나중에 넣습니다"
    );
    Ok(0)
}

/// A directory only its owner can enter, for files with the door 2 code.
fn private_dir(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: 만들 수 없습니다({e})", dir.display()))?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("{}: 권한을 바꿀 수 없습니다({e})", dir.display()))?;
    }
    let mode = fs::metadata(dir)
        .map_err(|e| format!("{}: 읽을 수 없습니다({e})", dir.display()))?
        .permissions()
        .mode();
    if mode & 0o077 != 0 {
        return Err(format!(
            "{}: 다른 사용자가 읽을 수 있는 디렉터리입니다. 문 2 코드가 든 파일을 두기 전에 chmod 700 하세요",
            dir.display()
        ));
    }
    Ok(())
}

/// Door 2's file in `dir`: the one there, or a new one. Returns it and whether the code is new.
fn door2(args: &Args, dir: &Path, now: i64) -> Result<(Door2, bool), String> {
    let path = dir.join("door2.conf");
    let until = args
        .one("--door2-until")?
        .map(time::parse_operator_time)
        .transpose()?;
    if until.is_some_and(|t| t <= now) {
        return Err("--door2-until이 이미 지났습니다".to_owned());
    }
    // a file that is there but cannot be read must not be taken for none: a new code would
    // replace the one the server has
    let existing = match fs::read_to_string(&path) {
        Ok(text) => Some(Door2::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: 읽을 수 없습니다({e})", path.display())),
    };
    let door = match (existing, args.flag("--door2-rotate")) {
        (Some(old), false) => (
            Door2 {
                code: old.code,
                until: until.unwrap_or(old.until),
            },
            false,
        ),
        (old, _) => {
            let until = until.or(old.map(|o| o.until)).ok_or(
                "새 door2.conf에는 --door2-until <문을 닫을 시각>이 필요합니다(예: 유예 끝 + 30일)",
            )?;
            (
                Door2 {
                    code: hbbs::new_code()?,
                    until,
                },
                true,
            )
        }
    };
    write_private(&path, door.0.to_text().as_bytes())?;
    Ok(door)
}

fn hbbs_conf(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &[
            "--policy",
            "--prev-policy",
            "--current",
            "--keys",
            "--text",
            "--text-file",
            "--out",
            "--door2-out",
            "--door2-until",
        ],
        &["--door2-rotate", "--door2-only", "--allow-test-keys"],
    )?;
    args.no_positional()?;
    let now = time::now();
    let door_dir = args.one("--door2-out")?.map(Path::new);
    for option in ["--door2-until", "--door2-rotate", "--door2-only"] {
        let given = args.flag(option) || args.one(option).ok().flatten().is_some();
        if given && door_dir.is_none() {
            return Err(format!("{option}: --door2-out과 함께 씁니다"));
        }
    }
    if let (Some(dir), true) = (door_dir, args.flag("--door2-only")) {
        // M5: door 2 opens before hbbs enforces anything
        private_dir(dir)?;
        let (door, new) = door2(&args, dir, now)?;
        println!(
            "{}: {}, {}까지 열림(코드는 화면에 쓰지 않습니다)",
            dir.join("door2.conf").display(),
            if new {
                "새 코드"
            } else {
                "코드 그대로"
            },
            time::format_kst(door.until)
        );
        println!("서버의 /etc/hjviewer/door2.conf로 넣습니다(root:hjviewer-api 0640)");
        return Ok(0);
    }

    let keys = load_keys(&args, true)?;
    let (checked, _) = load_envelope(args.need("--policy")?, "정책", &keys)?;
    let previous = args
        .one("--prev-policy")?
        .map(|path| load_envelope(path, "이전 정책", &keys))
        .transpose()?;
    let current = match args.need("--current")? {
        "none" => None,
        path => Some(
            hbbs::parse_conf(&read_text(path, "지금 설정")?).map_err(|e| format!("{path}: {e}"))?,
        ),
    };
    let decision = hbbs::decide(
        &checked.policy,
        previous.as_ref().map(|(c, _)| &c.policy),
        current.as_ref(),
        now,
    )?;
    // with MIN and FROM as they are, a file written below differs from the server's in its text
    // at most (a new door 2 code, or the text without the link)
    let same_values = "MIN과 FROM은 지금 파일과 같습니다. 문구(문 2 코드, S1·S2)가 바뀔 때만 서버 파일을 바꿉니다";
    let unchanged = matches!(decision, Decision::Unchanged { .. });
    if let Decision::Later { min, from, notes } = &decision {
        println!(
            "hbbs 파일을 아직 바꾸지 않음: 지금 파일을 그대로 두세요. {} 뒤에 hbbs-conf를 다시 실행하세요(MIN={min})",
            time::format_kst(*from)
        );
        for note in notes {
            println!("  {note}");
        }
        return Ok(0);
    }
    let (min, from): (Version, Option<i64>) = match &decision {
        Decision::Unchanged { min, from, notes } => {
            println!("hbbs 파일은 바꾸지 않음(MIN={min} 그대로, FROM도 그대로)");
            for note in notes {
                println!("  {note}");
            }
            (*min, *from)
        }
        Decision::Change {
            min,
            from,
            basis,
            notes,
        } => {
            let names: Vec<&str> = basis.iter().map(|p| p.name()).collect();
            println!("hbbs 파일을 바꿈:");
            println!("MIN={min}");
            println!("FROM={}", time::format_utc(*from));
            println!(
                "  {} · 기준 항목: {}",
                time::format_kst(*from),
                names.join(", ")
            );
            for note in notes {
                println!("  {note}");
            }
            (*min, Some(*from))
        }
        Decision::Later { .. } => return Ok(0),
    };

    let custom = args
        .one("--text-file")?
        .map(|p| read_text(p, "문구"))
        .transpose()?;
    let text_choice = args.one("--text")?;
    if custom.is_some() && text_choice.is_some() {
        return Err("--text와 --text-file을 함께 줄 수 없습니다".to_owned());
    }
    if let Some(dir) = door_dir {
        if args.one("--out")?.is_some()
            || custom.is_some()
            || text_choice.is_some_and(|t| t != "s1")
        {
            return Err("--door2-out은 문 2 링크가 든 문구(S1)로 두 파일을 씁니다(--out, --text s2, --text-file과 함께 못 씀)".to_owned());
        }
        private_dir(dir)?;
        let (door, new) = door2(&args, dir, now)?;
        let text = hbbs::s1(&checked.policy.server, &door.code);
        let conf = dir.join("min-controller.conf");
        write_private(&conf, hbbs::conf_text(min, from, &text)?.as_bytes())?;
        println!(
            "{}: {}, {}까지 열림",
            dir.join("door2.conf").display(),
            if new {
                "새 코드"
            } else {
                "코드 그대로"
            },
            time::format_kst(door.until)
        );
        println!("{}: MIN·FROM과 문 2 링크가 든 문구(S1)", conf.display());
        if unchanged {
            println!("  {same_values}");
        }
        println!("두 파일에는 문 2 코드가 들어 있습니다. 화면, 문서, 메모에 옮기지 마세요");
        println!("서버: /etc/hjviewer/door2.conf(root:hjviewer-api 0640), /etc/hjviewer/min-controller.conf(root:hjviewer 0640)");
    } else if let Some(out) = args.one("--out")? {
        let text = match (custom, text_choice) {
            (Some(text), _) => text.trim().to_owned(),
            (None, Some("s2")) => hbbs::S2.to_owned(),
            (None, Some("s1")) => {
                return Err(
                    "문구(S1)에는 문 2 링크가 들어갑니다: --door2-out <디렉터리>를 쓰세요"
                        .to_owned(),
                )
            }
            (None, Some(other)) => return Err(format!("--text는 s1 또는 s2입니다: {other:?}")),
            (None, None) => {
                return Err("--out에는 --text s2 또는 --text-file <파일>이 필요합니다".to_owned())
            }
        };
        write_private(
            Path::new(out),
            hbbs::conf_text(min, from, &text)?.as_bytes(),
        )?;
        println!("{out}: MIN·FROM과 문구를 썼습니다");
        if unchanged {
            println!("  {same_values}");
        }
        println!("서버: /etc/hjviewer/min-controller.conf(root:hjviewer 0640). 재시작은 필요 없습니다(10초 안에 반영)");
    } else if custom.is_some() || text_choice.is_some() {
        return Err("--text와 --text-file은 --out과 함께 씁니다".to_owned());
    }
    Ok(0)
}

/// X6: a key file that still pins a signing key the live declaration revokes.
fn x6(keys: &KeySet, live: &str) -> Result<(), String> {
    let root = Envelope::read_form(live)?.1.root;
    let Some(part) = root else { return Ok(()) };
    let decl = RootDecl::read(&part.p)?;
    if let Some(key) = keys.signing.iter().find(|k| decl.revoked.contains(&k.id)) {
        return Err(format!(
            "X6: 키 파일이 서버의 루트 선언(세대 {})이 폐기한 서명 키 {}를 아직 박고 있습니다. 그 키를 뺀 키 파일로 \
             빌드하세요",
            decl.epoch, key.id
        ));
    }
    Ok(())
}

fn floor(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &["--live", "--keys", "--archive"],
        &["--allow-test-keys"],
    )?;
    args.no_positional()?;
    let keys = load_keys(&args, true)?;
    let path = args.need("--live")?;
    let live = read_text(path, "서버의 정책(--live)")?;
    let archive = open_archive(&args, &keys, false)?;
    // the live file, and one the operator signed: S's file on the server never becomes a floor
    if archive.find(&live)?.is_none() {
        return Err(format!(
            "{path}이(가) 보관소({})의 파일이 아닙니다. 하한은 직접 서명해서 낸 서버의 정책에서만 정합니다. 다른 곳에서 \
             서명된 정책이면 서명 키를 도둑맞았을 수 있습니다(런북 5-8)",
            archive.dir.display()
        ));
    }
    let server = peek_server(&live)?;
    // verified with the key file: LOAD keeps its policy part (its declaration's root key too)
    if trust::holding(&keys, &server, &live).held.is_none() {
        return Err(
            "서버의 정책을 이 키 파일로 확인하지 못합니다(정책의 서명 키나 루트 선언의 루트 키를 이 키 파일이 박지 않음). \
             루트 키를 잃었고 서버의 선언이 그 키의 것이면, 남은 루트 키로 세대를 먼저 올리세요(런북 5-10)"
                .to_owned(),
        );
    }
    x6(&keys, &live)?;
    let (epoch, seq) = position_of(&live)?;
    // the build being made: its key set is recorded (X12)
    let archive = open_archive(&args, &keys, true)?;
    let version = version_next_to(&keys_path(&args)?);
    if let Some(written) = archive.record_keyset(&keys, &server, version.as_deref())? {
        eprintln!("기록한 키 집합: {}", written.display());
    }
    println!("{}={epoch}:{seq}", keys::FLOOR_VAR);
    eprintln!(
        "지금 키 파일의 하한은 {}입니다. 다음 빌드를 내기 전에 client/update-keys.env의 줄을 위 값으로 바꿉니다",
        keys.floor
    );
    Ok(0)
}

fn check(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &["--policy", "--keys", "--door2", "--live", "--archive"],
        &[],
    )?;
    args.no_positional()?;
    let keys = load_keys(&args, false)?;
    let archive = match open_archive(&args, &keys, false) {
        Ok(archive) => Some(archive),
        Err(e) if args.one("--archive")?.is_some() => return Err(e),
        Err(_) => None,
    };
    let (checked, file) = match args.one("--policy")? {
        Some(path) => load_envelope(path, "정책", &keys)?,
        None => {
            let last = archive
                .as_ref()
                .map(|a| a.highest())
                .transpose()?
                .flatten()
                .ok_or("--policy <정책 파일>이 필요합니다(보관소에 서명한 정책이 없습니다)")?;
            let server = peek_server(&last.file)?;
            (check_envelope(&last.file, &keys, &server)?, last.file)
        }
    };
    let door = args
        .one("--door2")?
        .map(|path| {
            Door2::parse(&read_text(path, "문 2 설정")?).map_err(|e| format!("{path}: {e}"))
        })
        .transpose()?;
    let live = args
        .one("--live")?
        .map(|path| read_text(path, "서버의 정책"))
        .transpose()?;
    let report = check::check(
        &checked,
        &file,
        door.as_ref(),
        live.as_deref(),
        &keys,
        archive.as_ref(),
        time::now(),
    );
    for line in &report.lines {
        println!("{line}");
    }
    if report.warnings > 0 {
        println!("경고 {}건", report.warnings);
        return Ok(3);
    }
    println!("경고 없음");
    Ok(0)
}

fn keys_command(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(argv, &["--keys", "--server", "--policy"], &["--production"])?;
    args.no_positional()?;
    let path = match args.one("--keys")? {
        Some(path) => path,
        None => DEFAULT_KEYS,
    };
    // the number ranges (signing keys 1-100, root keys 101-199) are part of reading the file
    let keys =
        KeySet::parse(&read_text(path, "키 파일")?).map_err(|e| format!("키 파일 {path}: {e}"))?;
    for (role, key) in keys.all() {
        let mark = if keys::is_test_key(&key.key) {
            "  (시험 키)"
        } else {
            ""
        };
        println!("{} {key}{mark}", role.name());
    }
    println!("floor {}", keys.floor);
    if let Some(server) = args.one("--server")? {
        println!("fingerprint {}", hex(&keys.fingerprint(server)));
    }
    if keys.has_test_key() {
        if args.flag("--production") {
            return Err(format!(
                "{TEST_KEY_WARNING}. 실서버 빌드에 쓸 수 없습니다({path})"
            ));
        }
        eprintln!("경고: {TEST_KEY_WARNING}({path})");
    }
    if let Some(policy) = args.one("--policy")? {
        if !args.flag("--production") {
            return Err("--policy는 --production과 함께 씁니다".to_owned());
        }
        x6(&keys, &read_text(policy, "서버의 정책")?)?;
        println!("서버의 정책이 폐기한 키를 박지 않았습니다(X6)");
    }
    Ok(0)
}

fn void(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &["--live", "--keys", "--archive"],
        &["--allow-test-keys"],
    )?;
    let path = args.file("무효로 할 정책 파일")?;
    let keys = load_keys(&args, true)?;
    let archive = open_archive(&args, &keys, false)?;
    let file = read_text(path, "정책")?;
    let live = read_text(args.need("--live")?, "서버의 정책(--live)")?;
    let highest = archive
        .highest()?
        .ok_or("보관소에 무효로 할 정책이 없습니다")?;
    if highest.file != file {
        return Err(format!(
            "무효로 할 수 있는 것은 보관소에서 무효로 하지 않은 가장 높은 정책({}:{})뿐입니다",
            highest.epoch, highest.seq
        ));
    }
    let live_at = archive.find(&live)?.ok_or(
        "서버의 정책(--live)이 보관소의 파일이 아닙니다. 다른 곳에서 서명된 것입니다(런북 5-8)",
    )?;
    if (live_at.epoch, live_at.seq) >= (highest.epoch, highest.seq) {
        return Err(
            "무효로 할 정책이 이미 서버에 있습니다(서버의 정책보다 높은 것만 무효로 합니다)"
                .to_owned(),
        );
    }
    // a policy that opens no epoch: it carries the declaration of the file before it unchanged
    let before = archive
        .policies()?
        .into_iter()
        .rfind(|a| (a.epoch, a.seq) < (highest.epoch, highest.seq))
        .ok_or("무효로 할 정책 앞에 보관소의 파일이 없습니다")?;
    let root_of = |f: &str| -> Result<Option<String>, String> {
        Ok(Envelope::read_form(f)?.1.root.map(|r| r.p))
    };
    if highest.epoch != before.epoch || root_of(&highest.file)? != root_of(&before.file)? {
        return Err(
            "새 세대를 연 정책(새 루트 선언)은 무효로 하지 않습니다. 그 위의 선언을 곧 내세요(런북 5-13)".to_owned(),
        );
    }
    let mut tty = Tty::open().map_err(no_tty("무효로 하지 않습니다"))?;
    tty.say(&format!(
        "{}:{} 정책을 무효로 합니다. 이 파일은 컴퓨터 밖으로 나가지 않았습니까(서버에 보내지 않았고, 다른 곳에 복사하지 \
         않았습니까)? 이 정책을 받은 기기는 다음 루트 선언까지 이 세대의 다음 정책들을 받지 않습니다.\n",
        highest.epoch, highest.seq
    ));
    if tty.ask("맞으면 yes를 입력하세요: ")? != "yes" {
        return Err("yes가 아니어서 무효로 하지 않았습니다".to_owned());
    }
    let archive = open_archive(&args, &keys, true)?;
    let written = archive.add_void(highest.epoch, highest.seq, time::now())?;
    println!("무효로 했습니다: {}", written.display());
    println!(
        "다음 정책은 보관소의 마지막 파일(무효가 아닌 것)에서 seq {}보다 높게 만듭니다",
        highest.seq
    );
    Ok(0)
}

fn archive_command(argv: &[String]) -> Result<i32, String> {
    let Some(("adopt", rest)) = argv.split_first().map(|(c, r)| (c.as_str(), r)) else {
        return Err(
            "archive adopt FILE 또는 archive adopt --keys FILE --version V --server NAME"
                .to_owned(),
        );
    };
    let args = Args::parse(
        rest,
        &["--keys", "--archive", "--version", "--server"],
        &["--allow-test-keys"],
    )?;
    let keys = load_keys(&args, true)?;
    if let Some(version) = args.one("--version")? {
        // the key set of a release tag's update-keys.env
        args.no_positional()?;
        Version::parse(version)?;
        let server = args.need("--server")?;
        let archive = open_archive(&args, &keys, true)?;
        match archive.record_keyset(&keys, server, Some(version))? {
            Some(path) => println!("기록한 키 집합: {}", path.display()),
            None => println!("이미 기록된 키 집합입니다"),
        }
        return Ok(0);
    }
    let path = args.file("보관소에 넣을 정책 파일")?;
    let file = read_text(path, "정책")?;
    let server = peek_server(&file)?;
    let checked = check_envelope(&file, &keys, &server)?;
    if trust::holding(&keys, &server, &file).held.is_none() {
        return Err("이 키 파일로 확인하지 못하는 정책입니다".to_owned());
    }
    let archive = open_archive(&args, &keys, false)?;
    let (epoch, seq) = (checked.policy.epoch, checked.policy.seq);
    if let Some(other) = archive.at(epoch, seq)?.filter(|a| a.file != file) {
        return Err(format!(
            "보관소에 같은 위치({epoch}:{seq})의 다른 정책이 있습니다: {}",
            other.path.display()
        ));
    }
    if let (Some(part), Some(ours)) = (&checked.envelope.root, archive.declaration(epoch)?) {
        if ours.p != part.p {
            return Err(format!("보관소에 세대 {epoch}의 다른 루트 선언이 있습니다"));
        }
    }
    let mut tty = Tty::open().map_err(no_tty("보관소에 넣지 않습니다"))?;
    tty.say(&format!(
        "서명: {} {} · 위치 {epoch}:{seq}\n",
        checked.role.korean(),
        checked.envelope.policy.k
    ));
    if let Some(root) = &checked.root {
        for line in trust::describe_declaration(root) {
            tty.say(&format!("{}\n", tty::printable(&line)));
        }
    }
    for (platform, entry) in &checked.policy.platforms {
        if let Some(p) = &entry.package {
            tty.say(&format!(
                "패키지 {platform} {}: {} · {}바이트 · sha256 {}\n",
                entry.latest, p.file, p.size, p.sha256
            ));
        }
    }
    tty.say(
        "이 파일을 직접 서명한 파일로 보관소에 넣습니까? 서명하지 않은 서버의 파일은 도둑맞은 키의 흔적입니다(런북 5-8).\n",
    );
    if tty.ask("맞으면 yes를 입력하세요: ")? != "yes" {
        return Err("yes가 아니어서 넣지 않았습니다".to_owned());
    }
    let archive = open_archive(&args, &keys, true)?;
    if let Some(written) = archive.add_policy(epoch, seq, &file)? {
        println!("넣었습니다: {}", written.display());
    }
    if let Some(part) = &checked.envelope.root {
        if let Some(written) = archive.add_declaration(epoch, part)? {
            println!("넣었습니다: {}", written.display());
        }
    }
    Ok(0)
}

fn test_keys(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(argv, &[], &["--env"])?;
    args.no_positional()?;
    if args.flag("--env") {
        print!("{}", keys::test_key_set().to_text());
        return Ok(0);
    }
    for (key, about) in keys::named_test_keys() {
        println!("{key}  # {about}");
    }
    println!(
        "# and every key whose seed is 32 equal bytes or 32 consecutive bytes (the test seeds"
    );
    println!("# of this repository are of these kinds) or an example of RFC 8032 7.1:");
    println!("# `hjv-release keys --production` refuses them all");
    Ok(0)
}

fn test_sign(argv: &[String]) -> Result<i32, String> {
    let args = Args::parse(
        argv,
        &["--policy", "--root-decl", "--key", "--root-key", "--out"],
        &["--raw"],
    )?;
    args.no_positional()?;
    let raw = args.flag("--raw");
    let test_key = |name: &str| {
        keys::test_key(name).ok_or_else(|| {
            let names: Vec<&str> = keys::TEST_KEYS.iter().map(|k| k.name).collect();
            format!("시험 키는 {} 중 하나입니다: {name:?}", names.join(", "))
        })
    };
    let text = |path: &str, what: &str| -> Result<String, String> {
        let text = read_text(path, what)?;
        Ok(text.strip_suffix('\n').map(str::to_owned).unwrap_or(text))
    };
    let mut p = text(args.need("--policy")?, "정책 본문")?;
    if !raw {
        p = Policy::read(&p)?.to_text();
    }
    let key = test_key(args.one("--key")?.unwrap_or("signing-1"))?;
    let root = match args.one("--root-decl")? {
        Some(path) => {
            let mut root_p = text(path, "루트 선언")?;
            if !raw {
                root_p = RootDecl::read(&root_p)?.to_text();
            }
            let root_key = test_key(args.one("--root-key")?.unwrap_or("root-101"))?;
            Some(Part::sign(
                &SigningKey::from_bytes(&root_key.seed()),
                root_key.id,
                ROOT_DOMAIN,
                &root_p,
            ))
        }
        None => None,
    };
    let file = Envelope {
        policy: Part::sign(
            &SigningKey::from_bytes(&key.seed()),
            key.id,
            POLICY_DOMAIN,
            &p,
        ),
        root,
    }
    .to_file();
    if !raw {
        // what is not meant to be refused must pass for a build with the test keys
        let server = Policy::read(&p)?.server;
        check_envelope(&file, &keys::test_key_set(), &server)
            .map_err(|e| format!("{e}(거절될 봉투를 일부러 만들려면 --raw)"))?;
    }
    match args.one("--out")? {
        Some(out) => {
            fs::write(out, &file).map_err(|e| format!("{out}: 쓸 수 없습니다({e})"))?;
        }
        None => print!("{file}"),
    }
    eprintln!(
        "시험 키({})로 서명했습니다. 시험 키를 박은 시험 빌드만 받습니다",
        key.name
    );
    Ok(0)
}

/// The tool by its version and the commit it was built from: scripts/build-signer.sh bakes in
/// HJV_BUILD_COMMIT, the commit of the clean checkout it builds, so that `version` and `sign` show
/// which commit's tool signs. A build without it (CI, tests, cargo by hand) says it does not know.
pub fn build_name() -> String {
    build_name_of(option_env!("HJV_BUILD_COMMIT"))
}

fn build_name_of(commit: Option<&str>) -> String {
    match commit {
        Some(commit) if !commit.is_empty() => {
            format!(
                "hjv-release {} (commit {commit})",
                env!("CARGO_PKG_VERSION")
            )
        }
        _ => format!(
            "hjv-release {} (commit unknown: not built by scripts/build-signer.sh)",
            env!("CARGO_PKG_VERSION")
        ),
    }
}

/// Runs a command line; returns the exit code.
pub fn run(argv: &[String]) -> i32 {
    let Some((command, rest)) = argv.split_first() else {
        eprintln!("{USAGE}");
        return 2;
    };
    let result = match command.as_str() {
        "keygen" => keygen(rest),
        "pubkey" => pubkey(rest),
        "policy" => policy(rest),
        "sign" => sign(rest),
        "verify" => verify(rest),
        "publish-dir" => publish_dir(rest),
        "hbbs-conf" => hbbs_conf(rest),
        "floor" => floor(rest),
        "check" => check(rest),
        "void" => void(rest),
        "archive" => archive_command(rest),
        "keys" => keys_command(rest),
        "test-keys" => test_keys(rest),
        "test-sign" => test_sign(rest),
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            return 0;
        }
        "version" | "--version" => {
            println!("{}", build_name());
            return 0;
        }
        other => {
            eprintln!("hjv-release: 모르는 명령 {other:?}\n\n{USAGE}");
            return 2;
        }
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            // a message may repeat part of a file or of another program's answer: on one line,
            // with nothing that could steer the terminal it is read on
            eprintln!("hjv-release {command}: {}", tty::printable(&message));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::tests::temp_dir;

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn the_build_name_says_the_commit_or_that_it_is_unknown() {
        let version = env!("CARGO_PKG_VERSION");
        let commit = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            build_name_of(Some(commit)),
            format!("hjv-release {version} (commit {commit})")
        );
        for unknown in [None, Some("")] {
            assert_eq!(
                build_name_of(unknown),
                format!(
                    "hjv-release {version} (commit unknown: not built by scripts/build-signer.sh)"
                )
            );
        }
    }

    #[test]
    fn files_are_never_overwritten_by_accident() {
        let dir = temp_dir("write-new");
        let path = dir.join("key");
        write_new(&path, b"first", 0o600).unwrap();
        assert_eq!(mode(&path), 0o600);
        // a second write does not touch the file, whatever checked for it before
        let err = write_new(&path, b"second", 0o600).unwrap_err();
        assert!(err.contains("이미 있습니다"), "{err}");
        assert_eq!(fs::read(&path).unwrap(), b"first");
        // not through a link that points somewhere else either
        std::os::unix::fs::symlink(dir.join("elsewhere"), dir.join("link")).unwrap();
        assert!(write_new(&dir.join("link"), b"x", 0o600).is_err());
        assert!(!dir.join("elsewhere").exists());
        // a public file gets the mode asked for (the umask may take bits away, never add)
        write_new(&dir.join("policy"), b"{}", 0o644).unwrap();
        assert_eq!(mode(&dir.join("policy")) & 0o133, 0);
        assert!(write_new(&dir.join("missing/file"), b"x", 0o600).is_err());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn private_files_are_replaced_in_one_step() {
        let dir = temp_dir("write-private");
        let path = dir.join("door2.conf");
        write_private(&path, b"CODE=old\n").unwrap();
        assert_eq!(mode(&path), 0o600);
        // a file left readable by others becomes private again when it is written anew
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"CODE=new\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"CODE=new\n");
        assert_eq!(mode(&path), 0o600);
        // nothing else is left in the directory
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        // a directory for the code: made for its owner only, refused when others can read it
        let private = dir.join("private");
        private_dir(&private).unwrap();
        assert_eq!(mode(&private), 0o700);
        private_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(private_dir(&private).unwrap_err().contains("chmod 700"));
        fs::remove_dir_all(dir).unwrap();
    }
}
