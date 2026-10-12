// SPDX-License-Identifier: AGPL-3.0-only

//! The command line of the binary `cargo test` builds (without HJV_SERVER and HJV_COMMAND_PUBKEY
//! unless the environment sets them). Nothing here sends a packet: test-wake only runs with
//! arguments it refuses or with an interface that does not exist (tests/netns.rs sends real ones).

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Output},
};

fn relay(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hjv-relay"))
        .args(args)
        .output()
        .unwrap()
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hjv-relay-cli-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir(&dir).unwrap();
    dir
}

#[test]
fn version_and_usage() {
    let out = relay(&["version"]);
    assert_eq!(out.status.code(), Some(0));
    let version = text(&out);
    assert!(
        version.starts_with(&format!("hjv-relay {} (", env!("CARGO_PKG_VERSION"))),
        "{version}"
    );
    assert!(
        version.contains("server: ") && version.contains("command keys: "),
        "{version}"
    );
    let out = relay(&["help"]);
    assert_eq!(out.status.code(), Some(0));
    let help = text(&out);
    assert!(help.contains("test-wake MAC"));
    for option in ["--log FILE", "--watch FILE", "--stop FILE"] {
        assert!(help.contains(option), "{help}");
    }
    for args in [&[][..], &["bogus"], &["init"], &["status"]] {
        let out = relay(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
        assert!(text(&out).contains("usage: hjv-relay"), "{args:?}");
    }
    // the boot-up task's command line: a configuration error, one log line, the loop stops
    for args in [
        &["run"][..],
        &["run", "--state"],
        &["run", "--state", "/nonexistent", "--user", "0:0"],
        &["run", "--state", "/nonexistent", "--frobnicate"],
        &["run", "--stat", "/nonexistent"],
        &["run", "--state", "/a", "--state", "/b"],
        &["run", "--state", "/a", "--log"],
        &["run", "--state", "/a", "--watch", "/x", "--watch", "/y"],
    ] {
        let out = relay(args);
        assert_eq!(out.status.code(), Some(78), "{args:?}: {}", text(&out));
        let said = text(&out);
        assert!(said.contains("설정 오류: run: "), "{args:?}: {said}");
        assert_eq!(said.lines().count(), 1, "{args:?}: {said}");
    }
}

#[test]
fn version_shows_the_command_keys_as_given() {
    let out = relay(&["version"]);
    let version = text(&out);
    let keys = version
        .lines()
        .find_map(|l| l.strip_prefix("command keys: "))
        .unwrap();
    match std::env::var("HJV_COMMAND_PUBKEY") {
        // a configured build prints the keys it was given, in the same form, and their
        // fingerprints (hex SHA-256 of the key bytes, as hjv-api selfcheck shows them)
        Ok(given) if !given.trim().is_empty() => {
            let given: Vec<String> = given.split(',').map(|k| k.replace(' ', "")).collect();
            assert_eq!(keys, given.join(","));
            let prints = version
                .lines()
                .find_map(|l| l.strip_prefix("command key fingerprints: "))
                .unwrap();
            assert_eq!(prints.split(',').count(), given.len());
        }
        _ => {
            assert!(keys.starts_with("none"), "{version}");
            assert!(!version.contains("fingerprints"), "{version}");
        }
    }
}

#[test]
fn status_shows_the_status_file_escaped() {
    let dir = temp_dir("status-escape");
    let state = dir.join("state");
    let status_file = dir.join("status");
    // what a compromised unprivileged relay could write there
    let nasty = "x\n\u{1b}[2J\u{9b}31m\u{202e}등록 코드: AAAA-BBBB-CCCC-DDDD";
    let content = serde_json::json!({
        "pid": 1, "state": nasty, "server": nasty, "failures": 1, "last_error": nasty,
        "last_command": {"job": nasty, "iface": nasty, "packets": "12\n13", "error": nasty}
    });
    fs::write(&status_file, content.to_string()).unwrap();
    let out = relay(&[
        "status",
        "--state",
        state.to_str().unwrap(),
        "--status-file",
        status_file.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0));
    let report = String::from_utf8_lossy(&out.stdout);
    for bad in ['\u{1b}', '\u{9b}', '\u{202e}'] {
        assert!(!report.contains(bad), "{report}");
    }
    assert!(
        !report.lines().any(|l| l.starts_with("등록 코드: AAAA")),
        "{report}"
    );
    assert!(report.contains("x\\n\\u{1b}[2J"), "{report}");
    assert!(report.contains("패킷 -"), "{report}");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn an_unconfigured_build_refuses_run() {
    if !text(&relay(&["version"])).contains("server: none") {
        eprintln!("skipped: this build has HJV_SERVER");
        return;
    }
    let dir = temp_dir("unconfigured");
    let state = dir.join("state");
    let out = relay(&["run", "--state", state.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(78), "{}", text(&out));
    assert!(text(&out).contains("HJV_SERVER"), "{}", text(&out));
    // refused before it touches anything
    assert!(!state.exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn init_and_status() {
    let dir = temp_dir("init");
    let state = dir.join("state");
    let state_arg = state.to_str().unwrap();
    let status_file = dir.join("no-status");
    let out = relay(&[
        "status",
        "--state",
        state_arg,
        "--status-file",
        status_file.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0));
    assert!(text(&out).contains("중계기 키: 없음"), "{}", text(&out));

    let out = relay(&["init", "--state", state_arg]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    let first = text(&out);
    let id = first
        .lines()
        .find_map(|l| l.strip_prefix("중계기 ID: "))
        .unwrap()
        .to_owned();
    let code = first
        .lines()
        .find_map(|l| l.strip_prefix("등록 코드: "))
        .unwrap()
        .to_owned();
    assert!(id.starts_with("r-") && id.len() == 18, "{id}");
    assert_eq!(code.len(), 19);
    assert_eq!(code.replace('-', "").to_ascii_lowercase(), id[2..]);
    assert!(first.contains("새로 만듦"));
    assert_eq!(
        fs::metadata(&state).unwrap().permissions().mode() & 0o777,
        0o700
    );
    let key = state.join("relay.key");
    assert_eq!(
        fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let out = relay(&["init", "--state", state_arg]);
    assert!(text(&out).contains(&format!("중계기 ID: {id}")));
    assert!(text(&out).contains("이미 있음"));

    let out = relay(&[
        "status",
        "--state",
        state_arg,
        "--status-file",
        status_file.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(0));
    let report = text(&out);
    assert!(report.contains(&format!("중계기 ID: {id}")), "{report}");
    assert!(report.contains(&format!("등록 코드: {code}")), "{report}");
    assert!(report.contains("실행 중: 아니요"), "{report}");
    assert!(report.contains("실행 상태: 없음"), "{report}");
    assert!(report.contains("랜카드:"), "{report}");

    // a damaged key is reported, never replaced
    fs::write(&key, "garbage\n").unwrap();
    let out = relay(&["init", "--state", state_arg]);
    assert_eq!(out.status.code(), Some(78), "{}", text(&out));
    assert_eq!(fs::read_to_string(&key).unwrap(), "garbage\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn test_wake_refuses_bad_arguments_and_sends_nothing() {
    for args in [
        &["test-wake"][..],
        &["test-wake", "zz"],
        &["test-wake", "01:00:5e:00:00:01"],
        &["test-wake", "ff:ff:ff:ff:ff:ff"],
        &["test-wake", "00:00:00:00:00:00"],
        &["test-wake", "00:00:5e:00:53:01", "--count", "0"],
        &["test-wake", "00:00:5e:00:53:01", "--count", "6"],
        &["test-wake", "00:00:5e:00:53:01", "--net", "10.0.0.0/4"],
        &["test-wake", "00:00:5e:00:53:01", "--net", "bogus"],
        &[
            "test-wake",
            "00:00:5e:00:53:01",
            "--net",
            "10.0.0.0/8",
            "--iface",
            "eth0",
        ],
        &["test-wake", "00:00:5e:00:53:01", "--user", "0:0"],
        &["test-wake", "00:00:5e:00:53:01", "00:00:5e:00:53:02"],
    ] {
        let out = relay(args);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {}", text(&out));
    }
    // the Windows getmac form is accepted; an interface that does not exist sends nothing
    let out = relay(&["test-wake", "00-00-5E-00-53-01", "--iface", "hjv-none0"]);
    assert_eq!(out.status.code(), Some(1), "{}", text(&out));
    let said = text(&out);
    assert!(
        said.contains("**:**:**:**:53:01") && said.contains("보낼 랜카드가 없습니다"),
        "{said}"
    );
}

#[test]
fn run_writes_to_the_log_file_it_is_given() {
    let dir = temp_dir("log-file");
    let log = dir.join("hjv-relay.log");
    fs::write(&log, "2026-10-12 17:00:00 +0900 an earlier line\n").unwrap();
    // opened by the relay itself (as root on the NAS), before anything else is said
    let out = relay(&[
        "run",
        "--log",
        log.to_str().unwrap(),
        "--state",
        "/nonexistent",
        "--user",
        "0:0",
    ]);
    assert_eq!(out.status.code(), Some(78), "{}", text(&out));
    assert!(text(&out).is_empty(), "{}", text(&out));
    let written = fs::read_to_string(&log).unwrap();
    assert!(written.starts_with("2026-10-12 17:00:00 +0900 an earlier line\n"));
    assert!(written.contains("설정 오류: run: "), "{written}");
    assert_eq!(written.lines().count(), 2, "{written}");

    // never through a symbolic link (the relay rewrites its log): it says so on stderr instead
    let target = dir.join("elsewhere");
    fs::write(&target, "untouched\n").unwrap();
    let link = dir.join("link.log");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let out = relay(&[
        "run",
        "--log",
        link.to_str().unwrap(),
        "--state",
        "/nonexistent",
        "--user",
        "0:0",
    ]);
    assert_eq!(out.status.code(), Some(78));
    let said = text(&out);
    assert!(
        said.contains("쓸 수 없어 표준 오류에 씁니다: a symbolic link"),
        "{said}"
    );
    assert!(said.contains("설정 오류: run: "), "{said}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched\n");
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_full_log_file_is_not_written_to() {
    let dir = temp_dir("full-log");
    let log = dir.join("hjv-relay.log");
    // what `run` adds to a log of `size` bytes, appended to it as the boot-up task does
    let run = |size: usize| {
        fs::write(&log, "x".repeat(size)).unwrap();
        let file = fs::OpenOptions::new().append(true).open(&log).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_hjv-relay"))
            .args(["run", "--stat", "x"])
            .stderr(file)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(78));
        fs::read_to_string(&log).unwrap()[size..].to_owned()
    };
    let written = run(10);
    assert!(written.contains("설정 오류: run: "), "{written}");
    let written = run(2 * 1024 * 1024);
    assert!(
        written.contains("로그 파일이 2MB를 넘어 더 적지 않습니다")
            && !written.contains("설정 오류"),
        "{written}"
    );
    assert_eq!(written.lines().count(), 1, "{written}");
    fs::remove_dir_all(&dir).unwrap();
}
