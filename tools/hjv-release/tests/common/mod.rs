// SPDX-License-Identifier: AGPL-3.0-only

//! Runs the hjv-release binary for the tests: without any terminal (as a script, a CI job or an
//! agent's shell would), or with a pseudo terminal as its controlling terminal and a person's
//! answers typed into it.

#![allow(dead_code)]

use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

pub const BIN: &str = env!("CARGO_BIN_EXE_hjv-release");
/// How long a step of a terminal session may take (a key file with age's work factor takes a
/// second or two to open).
const PATIENCE: Duration = Duration::from_secs(60);
/// The exit code reported for a tool that was still waiting at the terminal when the script had
/// nothing more to type, and was ended.
pub const ASKED_FOR_MORE: i32 = -99;

pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
    /// What appeared on the terminal, prompts and echoes included.
    pub tty: String,
    /// Whether the terminal echoes what is typed after the tool has ended.
    pub echo: bool,
}

/// Typed as an answer in a [`run_tty`] script: Ctrl-C, without Enter.
pub const CTRL_C: &str = "\u{3}";

impl Output {
    /// Everything the run showed anywhere.
    pub fn all(&self) -> String {
        format!("{}\n{}\n{}", self.stdout, self.stderr, self.tty)
    }
}

/// A new directory under the system's temporary directory.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("hjv-release-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn command(args: &[&str], dir: &Path, env: &[(&str, &str)]) -> Command {
    let mut command = Command::new(BIN);
    command.args(args).current_dir(dir);
    for (name, value) in env {
        command.env(name, value);
    }
    command
}

/// Runs the tool in a session of its own without a controlling terminal, whatever terminal the
/// tests themselves run in. `stdin` is what a script would pipe in.
pub fn run_with(args: &[&str], dir: &Path, env: &[(&str, &str)], stdin: &str) -> Output {
    let mut command = command(args, dir, env);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: setsid is async-signal-safe and touches only the child.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    // the tool may end before it reads anything: a closed pipe is not the test's failure
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    let output = child.wait_with_output().unwrap();
    Output {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        tty: String::new(),
        echo: true,
    }
}

pub fn run(args: &[&str], dir: &Path) -> Output {
    run_with(args, dir, &[], "")
}

/// A pseudo terminal: the master side and the path of the slave side.
fn open_pty() -> (OwnedFd, CString) {
    // SAFETY: plain libc calls on a descriptor this function owns; the buffer is large enough
    // for a pts name and is NUL-terminated by ptsname_r.
    unsafe {
        let master = libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC);
        assert!(master >= 0, "posix_openpt");
        assert_eq!(libc::grantpt(master), 0, "grantpt");
        assert_eq!(libc::unlockpt(master), 0, "unlockpt");
        let mut name = [0 as libc::c_char; 128];
        assert_eq!(
            libc::ptsname_r(master, name.as_mut_ptr(), name.len()),
            0,
            "ptsname_r"
        );
        let name = std::ffi::CStr::from_ptr(name.as_ptr()).to_owned();
        (OwnedFd::from_raw_fd(master), name)
    }
}

/// Runs the tool with a pseudo terminal as its controlling terminal. `script` is what the person
/// does: when the text of a step has appeared on the terminal, its answer is typed with Enter.
/// Standard input is not the terminal: it is empty, as for a command started from a script.
pub fn run_tty(args: &[&str], dir: &Path, env: &[(&str, &str)], script: &[(&str, &str)]) -> Output {
    let (master, slave) = open_pty();
    let mut command = command(args, dir, env);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // SAFETY: setsid, open and ioctl are async-signal-safe; the slave's path was made before
    // the fork. The descriptor stays open in the child on purpose: it keeps the terminal alive.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            let fd = libc::open(slave.as_ptr(), libc::O_RDWR);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(fd, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let pipes = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let drain = |mut pipe: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut all = Vec::new();
            let _ = pipe.read_to_end(&mut all);
            all
        })
    };
    let (stdout, stderr) = (drain(Box::new(pipes.0)), drain(Box::new(pipes.1)));

    // everything the terminal shows, as it comes
    let (send, receive) = mpsc::channel::<Vec<u8>>();
    let mut reader = File::from(master.try_clone().unwrap());
    let reading = std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        // the read fails with EIO once the child and its terminal are gone
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 || send.send(buffer[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut writer = File::from(master.try_clone().unwrap());
    let mut shown = Vec::new();
    let mut seen_to = 0;
    for (wait_for, answer) in script {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let text = String::from_utf8_lossy(&shown[seen_to..]).into_owned();
            if let Some(at) = text.find(wait_for) {
                // later steps look only at what comes after this prompt
                seen_to += text[..at + wait_for.len()].len();
                break;
            }
            match receive.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(bytes) => shown.extend_from_slice(&bytes),
                // the tool ended, or never asked: stop typing and report what happened
                Err(_) => break,
            }
        }
        let typed = match *answer {
            CTRL_C => CTRL_C.to_owned(),
            answer => format!("{answer}\n"),
        };
        let _ = writer.write_all(typed.as_bytes());
    }
    // a tool that asks for more than the script answers must not hang the tests: it is ended,
    // and the exit code says so
    let deadline = Instant::now() + PATIENCE;
    let code = loop {
        match child.try_wait().unwrap() {
            Some(status) => break status.code().unwrap_or(-1),
            None if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break ASKED_FOR_MORE;
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    // the terminal's settings as the tool left them
    // SAFETY: an all-zero termios is a valid value for tcgetattr to overwrite; the descriptor is
    // the open master side.
    let echo = unsafe {
        let mut settings: libc::termios = std::mem::zeroed();
        libc::tcgetattr(master.as_raw_fd(), &mut settings) == 0
            && settings.c_lflag & libc::ECHO != 0
    };
    drop(writer);
    drop(master);
    while let Ok(bytes) = receive.recv_timeout(Duration::from_secs(10)) {
        shown.extend_from_slice(&bytes);
    }
    let _ = reading.join();
    Output {
        code,
        stdout: String::from_utf8_lossy(&stdout.join().unwrap()).into_owned(),
        stderr: String::from_utf8_lossy(&stderr.join().unwrap()).into_owned(),
        tty: String::from_utf8_lossy(&shown).replace("\r\n", "\n"),
        echo,
    }
}

/// Whether `text` shows the bytes of `secret` in a form a careless print could give them: as
/// they are, in base64, in hex however it is spaced or prefixed (`4a0f`, `[4a, 0f]`, `0x4a:0x0f`)
/// or as a list of decimal numbers (`{:?}` of the bytes).
pub fn shows_bytes(text: &str, secret: &[u8]) -> bool {
    use base64::{
        engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD},
        Engine,
    };
    let base64 = [
        STANDARD.encode(secret),
        STANDARD_NO_PAD.encode(secret),
        URL_SAFE.encode(secret),
        URL_SAFE_NO_PAD.encode(secret),
    ];
    if base64.iter().any(|form| text.contains(form)) {
        return true;
    }
    if std::str::from_utf8(secret).is_ok_and(|plain| text.contains(plain)) {
        return true;
    }
    let hex: String = secret.iter().map(|b| format!("{b:02x}")).collect();
    let lower = text.to_lowercase().replace("0x", "");
    let squeezed: String = lower.chars().filter(char::is_ascii_hexdigit).collect();
    if squeezed.contains(&hex) {
        return true;
    }
    let numbers: Vec<u32> = text
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect();
    let wanted: Vec<u32> = secret.iter().map(|b| u32::from(*b)).collect();
    numbers
        .windows(wanted.len())
        .any(|w| w == wanted.as_slice())
}

/// The permission bits of a file.
pub fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}
