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
    for option in ["--log FILE", "--watch FILE", "--stop FILE", "--out FILE"] {
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

    // a mistake anywhere in the command line goes to the log too (the boot-up task gives the
    // relay nothing else to write to)
    for args in [
        &["--frobnicate"][..],
        &["--state", "/a", "--state", "/b"],
        &["--log", "/elsewhere.log"],
    ] {
        fs::write(&log, "").unwrap();
        let log_arg = format!("--log={}", log.display());
        let out = relay(&[&["run", log_arg.as_str()][..], args].concat());
        assert_eq!(out.status.code(), Some(78), "{args:?}");
        assert!(text(&out).is_empty(), "{args:?}: {}", text(&out));
        let written = fs::read_to_string(&log).unwrap();
        assert!(written.contains("설정 오류: run: "), "{args:?}: {written}");
    }
    assert!(!PathBuf::from("/elsewhere.log").exists());

    // never through a symbolic link, also one that points nowhere, nor into a hard-linked file
    // or a folder (the relay rewrites its log, as root on the NAS): without its log the relay
    // does not run, and says so on stderr only
    let target = dir.join("elsewhere");
    fs::write(&target, "untouched\n").unwrap();
    let link = dir.join("link.log");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let dangling = dir.join("dangling.log");
    std::os::unix::fs::symlink(dir.join("new"), &dangling).unwrap();
    let hard = dir.join("hard.log");
    fs::hard_link(&target, &hard).unwrap();
    let folder = dir.join("folder.log");
    fs::create_dir(&folder).unwrap();
    for (path, why) in [
        (&link, "a symbolic link"),
        (&dangling, "a symbolic link"),
        (&hard, "hard links"),
        (&folder, "a folder"),
    ] {
        let out = relay(&[
            "run",
            "--log",
            path.to_str().unwrap(),
            "--state",
            "/nonexistent",
        ]);
        assert_eq!(out.status.code(), Some(78));
        let said = text(&out);
        assert!(
            said.contains("설정 오류: 로그 파일 ")
                && said.contains("쓸 수 없어 끝냅니다: ")
                && said.contains(why),
            "{said}"
        );
        assert_eq!(said.lines().count(), 1, "{said}");
    }
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched\n");
    assert!(!dir.join("new").exists());
    fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn status_and_test_wake_write_only_to_a_plain_out_file() {
    let dir = temp_dir("out-file");
    let state = dir.join("state");
    let state_arg = state.to_str().unwrap();
    let out_file = dir.join("status.txt");
    let out_arg = out_file.to_str().unwrap();
    let status_file = dir.join("no-status");
    let status = |extra: &[&str]| {
        relay(
            &[
                &[
                    "status",
                    "--state",
                    state_arg,
                    "--status-file",
                    status_file.to_str().unwrap(),
                    "--out",
                    out_arg,
                ][..],
                extra,
            ]
            .concat(),
        )
    };
    // the report goes to the file, emptied first, and nothing to stdout or stderr
    fs::write(&out_file, "x".repeat(50_000)).unwrap();
    let out = status(&[]);
    assert_eq!(out.status.code(), Some(0), "{}", text(&out));
    assert!(text(&out).is_empty(), "{}", text(&out));
    let report = fs::read_to_string(&out_file).unwrap();
    assert!(report.starts_with("hjv-relay "), "{report}");
    assert!(
        report.contains("중계기 키: 없음") && !report.contains("xxxx"),
        "{report}"
    );
    assert_eq!(
        fs::metadata(&out_file).unwrap().permissions().mode() & 0o777,
        0o644
    );
    // a mistake in the command line is written there too
    let out = status(&["--bogus"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out).is_empty(), "{}", text(&out));
    assert!(fs::read_to_string(&out_file)
        .unwrap()
        .contains("unknown option --bogus"));

    // --log: what is at the log's place, without following a link
    let log = dir.join("hjv-relay.log");
    let log_line = |extra: &[&str]| -> String {
        let out = status(&[&["--log", log.to_str().unwrap()][..], extra].concat());
        assert_eq!(out.status.code(), Some(0), "{}", text(&out));
        let report = fs::read_to_string(&out_file).unwrap();
        report
            .lines()
            .find(|l| l.starts_with("로그 파일: "))
            .unwrap_or_else(|| panic!("{report}"))
            .to_owned()
    };
    assert!(log_line(&[]).contains("(없음. "), "{}", log_line(&[]));
    fs::write(&log, "12345").unwrap();
    assert!(
        log_line(&[]).ends_with("(일반 파일, 5바이트)"),
        "{}",
        log_line(&[])
    );
    fs::remove_file(&log).unwrap();
    std::os::unix::fs::symlink(&out_file, &log).unwrap();
    assert!(log_line(&[]).contains("(바로 가기(심볼릭 링크)라 "));
    fs::remove_file(&log).unwrap();
    fs::create_dir(&log).unwrap();
    assert!(log_line(&[]).contains("(폴더라 "));
    fs::remove_dir(&log).unwrap();
    fs::write(&log, "").unwrap();
    fs::hard_link(&log, dir.join("other-name")).unwrap();
    assert!(log_line(&[]).contains("(다른 이름(하드 링크)이 있어 "));

    // never through a symbolic link (also one that points nowhere), into a hard-linked file, a
    // folder or a FIFO: a configuration error on stderr, and the target stays as it was
    let target = dir.join("elsewhere");
    fs::write(&target, "untouched\n").unwrap();
    let place = dir.join("place");
    let fifo = std::ffi::CString::new(dir.join("fifo").to_str().unwrap()).unwrap();
    // SAFETY: a NUL-terminated path.
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o644) }, 0);
    let wake = |out: &std::path::Path| {
        relay(&[
            "test-wake",
            "00:00:5e:00:53:01",
            "--iface",
            "hjv-none0",
            "--out",
            out.to_str().unwrap(),
        ])
    };
    for (make, why) in [
        (0, "a symbolic link"),
        (1, "a symbolic link"),
        (2, "hard links"),
        (3, "a folder"),
        (4, "not a regular file"),
    ] {
        let _ = fs::remove_file(&place);
        let _ = fs::remove_dir(&place);
        let path = match make {
            0 => {
                std::os::unix::fs::symlink(&target, &place).unwrap();
                place.clone()
            }
            1 => {
                std::os::unix::fs::symlink(dir.join("new"), &place).unwrap();
                place.clone()
            }
            2 => {
                fs::hard_link(&target, &place).unwrap();
                place.clone()
            }
            3 => {
                fs::create_dir(&place).unwrap();
                place.clone()
            }
            _ => dir.join("fifo"),
        };
        for out in [
            relay(&[
                "status",
                "--state",
                state_arg,
                "--out",
                path.to_str().unwrap(),
            ]),
            wake(&path),
        ] {
            assert_eq!(out.status.code(), Some(78), "{why}: {}", text(&out));
            let said = text(&out);
            assert!(said.contains(why) && said.contains("--out"), "{said}");
            assert_eq!(said.lines().count(), 1, "{said}");
        }
    }
    assert_eq!(fs::read_to_string(&target).unwrap(), "untouched\n");
    assert!(!dir.join("new").exists());

    // test-wake appends to its record, mistakes included
    let record = dir.join("test.log");
    for _ in 0..2 {
        let out = wake(&record);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out));
        assert!(text(&out).is_empty(), "{}", text(&out));
    }
    let out = relay(&["test-wake", "zz", "--out", record.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
    assert!(text(&out).is_empty(), "{}", text(&out));
    let written = fs::read_to_string(&record).unwrap();
    assert_eq!(
        written.matches("보낼 랜카드가 없습니다").count(),
        2,
        "{written}"
    );
    assert!(
        written.contains("\"zz\" is not a PC's MAC address"),
        "{written}"
    );
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
