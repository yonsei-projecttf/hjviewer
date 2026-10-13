// SPDX-License-Identifier: AGPL-3.0-only

//! The terminal of the person at the keyboard. Passphrases and the word `yes` are read from the
//! controlling terminal (`/dev/tty`) and from nowhere else: not from standard input, not from the
//! environment, not from an argument (design 6-3). A process without a terminal (a script, a CI
//! job, an agent's shell) cannot open it, so signing and key making stop there.

use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    sync::{
        atomic::{AtomicBool, Ordering},
        OnceLock,
    },
};
use zeroize::Zeroizing;

/// The longest line read, passphrases included.
const MAX_LINE: usize = 1024;

pub struct Tty {
    file: File,
}

/// The terminal and its settings from before the first passphrase prompt, for [`put_back`].
static ORIGINAL: OnceLock<(libc::c_int, libc::termios)> = OnceLock::new();
/// Whether the echo is off right now.
static ECHO_IS_OFF: AtomicBool = AtomicBool::new(false);

/// Ctrl-C (or another ending signal) at a passphrase prompt: the echo comes back before the
/// program ends, or the person would be left with a terminal that shows nothing they type.
extern "C" fn put_back(signal: libc::c_int) {
    if ECHO_IS_OFF.load(Ordering::SeqCst) {
        if let Some((fd, saved)) = ORIGINAL.get() {
            // SAFETY: tcsetattr is async-signal-safe; the descriptor is open while the echo is
            // off, and `saved` was read from it.
            unsafe { libc::tcsetattr(*fd, libc::TCSANOW, saved) };
        }
    }
    // SAFETY: signal and raise are async-signal-safe; the signal then ends the program as it
    // would have without this handler.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// Echo off while a secret is typed; the terminal gets its settings back when this drops.
struct NoEcho<'a> {
    file: &'a File,
    saved: libc::termios,
}

impl<'a> NoEcho<'a> {
    fn new(file: &'a File) -> Result<NoEcho<'a>, String> {
        // SAFETY: an all-zero termios is a valid value for tcgetattr to overwrite, and the file
        // descriptor is open for as long as `file` is borrowed.
        let mut saved: libc::termios = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(file.as_raw_fd(), &mut saved) } != 0 {
            return Err("터미널 설정을 읽지 못했습니다".to_owned());
        }
        if ORIGINAL.set((file.as_raw_fd(), saved)).is_ok() {
            let handler = put_back as extern "C" fn(libc::c_int) as libc::sighandler_t;
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                // SAFETY: installs a handler that calls async-signal-safe functions only.
                unsafe { libc::signal(signal, handler) };
            }
        }
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        quiet.c_lflag |= libc::ECHONL | libc::ICANON;
        ECHO_IS_OFF.store(true, Ordering::SeqCst);
        // SAFETY: `quiet` is a termios read from this descriptor with two flags changed.
        if unsafe { libc::tcsetattr(file.as_raw_fd(), libc::TCSAFLUSH, &quiet) } != 0 {
            ECHO_IS_OFF.store(false, Ordering::SeqCst);
            return Err("터미널의 입력 표시를 끄지 못했습니다".to_owned());
        }
        Ok(NoEcho { file, saved })
    }
}

impl Drop for NoEcho<'_> {
    fn drop(&mut self) {
        // SAFETY: restores the settings read from this descriptor above.
        unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSAFLUSH, &self.saved) };
        ECHO_IS_OFF.store(false, Ordering::SeqCst);
    }
}

impl Tty {
    /// Opens the controlling terminal, or says that there is none.
    pub fn open() -> Result<Tty, String> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC)
            .open("/dev/tty")
            .map_err(|_| "터미널(TTY)이 없습니다".to_owned())?;
        // SAFETY: isatty only inspects the open descriptor.
        if unsafe { libc::isatty(file.as_raw_fd()) } != 1 {
            return Err("터미널(TTY)이 없습니다".to_owned());
        }
        Ok(Tty { file })
    }

    /// Writes to the terminal, where the person reads even if the output goes to a file.
    pub fn say(&mut self, text: &str) {
        show(&self.file, text);
    }

    /// Asks a question whose answer may be seen, such as `yes`.
    pub fn ask(&mut self, prompt: &str) -> Result<String, String> {
        self.say(prompt);
        Ok(read_line(&self.file)?.trim().to_owned())
    }

    /// Asks for a secret: nothing typed is shown. The echo goes off before the prompt appears,
    /// so that whoever answers the prompt at once is not shown either, and anything typed
    /// before the prompt is thrown away.
    pub fn ask_secret(&mut self, prompt: &str) -> Result<Zeroizing<String>, String> {
        let _quiet = NoEcho::new(&self.file)?;
        show(&self.file, prompt);
        read_line(&self.file)
    }
}

fn show(mut file: &File, text: &str) {
    let _ = file.write_all(text.as_bytes());
    let _ = file.flush();
}

/// Reads one line into a buffer that never grows, so that a secret is not copied around.
fn read_line(mut file: &File) -> Result<Zeroizing<String>, String> {
    let mut line = Zeroizing::new(Vec::with_capacity(MAX_LINE + 1));
    let mut byte = [0u8; 1];
    loop {
        match file.read(&mut byte) {
            Ok(1) if byte[0] == b'\n' => break,
            Ok(1) if line.len() < MAX_LINE => line.push(byte[0]),
            Ok(1) => return Err("입력이 너무 깁니다".to_owned()),
            // the terminal went away or the person pressed Ctrl-D
            Ok(_) => return Err("입력이 끝났습니다".to_owned()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return Err("터미널에서 읽지 못했습니다".to_owned()),
        }
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    let mut text = Zeroizing::new(String::with_capacity(line.len()));
    match std::str::from_utf8(&line) {
        Ok(s) => text.push_str(s),
        Err(_) => return Err("입력이 UTF-8이 아닙니다".to_owned()),
    }
    Ok(text)
}

/// Keeps other programs of the same user away from this process's memory while it holds a
/// private key: no core dump, and no reading through /proc or ptrace by a process that is not
/// root. Best effort.
pub fn harden_process() {
    #[cfg(target_os = "linux")]
    // SAFETY: both calls take plain integers and a struct by reference and change only limits
    // and flags of this process.
    unsafe {
        libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
        let none = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::setrlimit(libc::RLIMIT_CORE, &none);
    }
}
