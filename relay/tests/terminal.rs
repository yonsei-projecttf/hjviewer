// SPDX-License-Identifier: AGPL-3.0-only

//! `system::detach_terminal`, which `run` and `test-wake` call before they give up root. Started
//! from a terminal (an administrator trying the relay over SSH with sudo), the process must end
//! up unable to read from that terminal or to push input into it (TIOCSTI), while its output
//! still shows there. The test runs itself on a new pseudo-terminal: once as the session leader
//! that owns the terminal (a login shell), which starts a second copy (a command the shell runs)
//! that detaches and checks.

use std::{
    fs,
    io::Read,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::process::CommandExt,
    },
    process::{Command, Stdio},
};

const ROLE: &str = "HJV_RELAY_TTY_ROLE";

#[test]
fn a_terminal_is_given_up() {
    match std::env::var(ROLE).as_deref() {
        Ok("leader") => leader(),
        Ok("command") => command(),
        _ => outer(),
    }
}

fn this_test(role: &str) -> Command {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args([
        "a_terminal_is_given_up",
        "--exact",
        "--nocapture",
        "--test-threads=1",
    ])
    .env(ROLE, role);
    cmd
}

fn outer() {
    let (mut master, mut slave) = (-1, -1);
    // SAFETY: openpty fills the two descriptors.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if rc != 0 {
        eprintln!(
            "SKIPPED: no pseudo-terminal here: {}",
            std::io::Error::last_os_error()
        );
        return;
    }
    // SAFETY: two new descriptors that nothing else owns.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    let mut cmd = this_test("leader");
    cmd.stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: only async-signal-safe system calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    // the terminal's other end stays open only in the children from here on
    drop(cmd);
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = fs::File::from(master).read_to_end(&mut out);
        out
    });
    let status = child.wait().unwrap();
    let out = reader.join().unwrap();
    let out = String::from_utf8_lossy(&out);
    assert!(status.success(), "{out}");
    // the command ran on the terminal, and its output still reached it after detaching
    assert!(out.contains("detached"), "{out}");
}

/// The session leader of the terminal: runs the command as a shell would.
fn leader() {
    let status = this_test("command").status().unwrap();
    assert!(status.success());
}

fn is_tty(fd: libc::c_int) -> bool {
    // SAFETY: isatty only queries the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// A command started from the terminal, still root's in real life: detaches and checks.
fn command() {
    assert!(is_tty(0) && is_tty(1) && is_tty(2), "not on the terminal");
    assert!(
        fs::File::open("/dev/tty").is_ok(),
        "no controlling terminal to begin with"
    );

    hjv_relay::system::detach_terminal();

    assert!(!is_tty(0), "stdin is still the terminal");
    for fd in [1, 2] {
        assert!(is_tty(fd), "fd {fd} no longer shows on the terminal");
        // SAFETY: F_GETFL only queries the descriptor.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(
            flags & libc::O_ACCMODE,
            libc::O_WRONLY,
            "fd {fd} can still read what is typed"
        );
    }
    // no controlling terminal any more: /dev/tty is gone, and so is TIOCSTI
    let e = fs::File::open("/dev/tty").unwrap_err();
    assert_eq!(e.raw_os_error(), Some(libc::ENXIO), "{e}");
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        let c = b'x' as libc::c_char;
        // SAFETY: TIOCSTI reads one byte from the pointer.
        let rc = unsafe { libc::ioctl(1, libc::TIOCSTI, &c) };
        assert_eq!(rc, -1, "TIOCSTI still works");
    }
    // and the log still shows there
    println!("detached");
}
