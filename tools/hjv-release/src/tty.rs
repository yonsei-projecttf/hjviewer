// SPDX-License-Identifier: AGPL-3.0-only

//! The terminal of the person at the keyboard. Passphrases and the word `yes` are read from the
//! controlling terminal (`/dev/tty`) and from nowhere else: not from standard input, not from the
//! environment, not from an argument (design 6-3). A process without a terminal (a script, a CI
//! job, an agent's shell) cannot open it, so signing and key making stop there.
//!
//! What is written to the terminal is text for a person to judge, and some of it comes from a
//! file an agent wrote (a draft). So nothing written here can steer the terminal: control
//! characters and characters that show nothing or turn the text around go out as visible escapes
//! ([`printable`]).

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

/// The terminal of the passphrase prompts: its descriptor, its settings from before the first
/// prompt (for [`put_back`]) and the settings of a prompt (for [`quiet_again`]).
struct Prompt {
    fd: libc::c_int,
    original: libc::termios,
    quiet: libc::termios,
}

static PROMPT: OnceLock<Prompt> = OnceLock::new();
/// Whether the echo is off right now.
static ECHO_IS_OFF: AtomicBool = AtomicBool::new(false);

/// Ctrl-C (or another ending signal) at a passphrase prompt: the echo comes back before the
/// program ends, or the person would be left with a terminal that shows nothing they type.
extern "C" fn put_back(signal: libc::c_int) {
    if ECHO_IS_OFF.load(Ordering::SeqCst) {
        if let Some(prompt) = PROMPT.get() {
            // SAFETY: tcsetattr is async-signal-safe; the descriptor is open while the echo is
            // off, and `original` was read from it.
            unsafe { libc::tcsetattr(prompt.fd, libc::TCSANOW, &prompt.original) };
        }
    }
    // SAFETY: signal and raise are async-signal-safe; the signal then ends the program as it
    // would have without this handler.
    unsafe {
        libc::signal(signal, libc::SIG_DFL);
        libc::raise(signal);
    }
}

/// The program was stopped at a passphrase prompt and goes on (`fg`). A shell puts its own
/// terminal settings back while a job is stopped, the echo with them, and bash does not restore
/// the job's settings when it continues: without this the rest of the passphrase would be shown.
extern "C" fn quiet_again(_: libc::c_int) {
    if ECHO_IS_OFF.load(Ordering::SeqCst) {
        if let Some(prompt) = PROMPT.get() {
            // SAFETY: tcsetattr is async-signal-safe; the descriptor is open while the echo is
            // off, and `quiet` is what the prompt set on it. TCSANOW, so that a handler never
            // waits. From the background the call would stop the program again (SIGTTOU) until
            // it is brought to the foreground, which is what a prompt does there anyway.
            unsafe { libc::tcsetattr(prompt.fd, libc::TCSANOW, &prompt.quiet) };
        }
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
        let mut quiet = saved;
        quiet.c_lflag &= !libc::ECHO;
        quiet.c_lflag |= libc::ECHONL | libc::ICANON;
        // No suspend key at this prompt: Ctrl-Z would stop the program, the shell would turn
        // the echo on again, and what is typed after `fg` could be shown. (A stop that comes
        // another way is what `quiet_again` is for.)
        quiet.c_cc[libc::VSUSP] = libc::_POSIX_VDISABLE;
        let prompt = Prompt {
            fd: file.as_raw_fd(),
            original: saved,
            quiet,
        };
        if PROMPT.set(prompt).is_ok() {
            let ending = put_back as extern "C" fn(libc::c_int) as libc::sighandler_t;
            for signal in [libc::SIGINT, libc::SIGTERM, libc::SIGHUP, libc::SIGQUIT] {
                // SAFETY: installs a handler that calls async-signal-safe functions only.
                unsafe { libc::signal(signal, ending) };
            }
            let continued = quiet_again as extern "C" fn(libc::c_int) as libc::sighandler_t;
            // SAFETY: as above.
            unsafe { libc::signal(libc::SIGCONT, continued) };
        }
        ECHO_IS_OFF.store(true, Ordering::SeqCst);
        // SAFETY: `quiet` is a termios read from this descriptor with flags and one key changed.
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
        // Once more: a continue signal between the two lines above would have turned the echo
        // off again. (The other order would let Ctrl-C end the program with the echo off.)
        // SAFETY: as above.
        unsafe { libc::tcsetattr(self.file.as_raw_fd(), libc::TCSANOW, &self.saved) };
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

    /// Writes to the terminal, where the person reads even if the output goes to a file. Line
    /// feeds are kept; anything else that could steer the terminal is written as an escape.
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

/// Whether a character must not reach a terminal as it is: a control character (an escape
/// sequence moves the cursor, clears the screen, hides what follows), or a character that shows
/// nothing or changes the order in which the text around it is shown (Unicode's format
/// characters, the line and paragraph separators, the Hangul fillers).
fn hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{890}'..='\u{891}'
                | '\u{8e2}'
                | '\u{115f}'..='\u{1160}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{3164}'
                | '\u{feff}'
                | '\u{ffa0}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0000}'..='\u{e0fff}'
        )
}

fn escaped(text: &str, keep_line_feeds: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if hidden(c) && !(keep_line_feeds && c == '\n') {
            // `\n`, `\t`, `\u{1b}`, `\u{202e}`
            out.extend(c.escape_default());
        } else {
            out.push(c);
        }
    }
    out
}

/// `text` as one line that is safe to show: every control character (line feeds included) and
/// every character that shows nothing or turns the text around is written as a visible escape
/// such as `\n` or `\u{1b}`. For text that did not come from this program: a draft's lines, the
/// first line of another program's error.
pub fn printable(text: &str) -> String {
    escaped(text, false)
}

fn show(mut file: &File, text: &str) {
    let _ = file.write_all(escaped(text, true).as_bytes());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_steers_the_terminal() {
        // what the tool itself writes goes out as it is
        let plain = "windows: 최소 1.6.0 · 최신 1.6.1 → 유예 끝 2026-11-23 00:00 한국 시각";
        assert_eq!(printable(plain), plain);
        // clear the screen, go home, paint another summary, hide the rest
        let evil = "\u{1b}[2J\u{1b}[H서명할 정책:\n  가짜 요약\u{1b}[8m";
        let shown = printable(evil);
        assert_eq!(
            shown,
            "\\u{1b}[2J\\u{1b}[H서명할 정책:\\n  가짜 요약\\u{1b}[8m"
        );
        assert!(!shown.chars().any(|c| c.is_control()));
        for (text, expected) in [
            ("a\rb", "a\\rb"),
            ("a\tb", "a\\tb"),
            ("a\u{8}b", "a\\u{8}b"),
            ("a\u{7f}b", "a\\u{7f}b"),
            // the 8-bit control sequence introducer
            ("a\u{9b}2Jb", "a\\u{9b}2Jb"),
            // text turned around, and text that shows nothing
            ("a\u{202e}b", "a\\u{202e}b"),
            ("a\u{2066}b", "a\\u{2066}b"),
            ("a\u{200b}b", "a\\u{200b}b"),
            ("a\u{feff}b", "a\\u{feff}b"),
            ("a\u{2028}b", "a\\u{2028}b"),
            ("a\u{3164}b", "a\\u{3164}b"),
            ("a\u{e0041}b", "a\\u{e0041}b"),
        ] {
            assert_eq!(printable(text), expected);
        }
        // the lines of the tool's own messages keep their line feeds, and nothing else
        assert_eq!(escaped("서명할 정책:\n", true), "서명할 정책:\n");
        assert_eq!(escaped("a\u{1b}[2J\nb\r", true), "a\\u{1b}[2J\nb\\r");
    }
}
