// SPDX-License-Identifier: AGPL-3.0-only

//! The relay's log: one line per event, with the local date and time, on stderr. It logs state
//! changes, command results and refusals, and connection errors at most once per ten minutes,
//! never every poll, so an idle relay does not keep the NAS's disks awake. MAC addresses appear
//! masked (`**:**:**:**:12:34`).
//!
//! Every line goes through [`line`], which writes characters that are not printable (newlines,
//! terminal escapes, bidirectional and other format characters) as escapes such as `\n` and
//! `\u{1b}` and cuts the line at [`MAX_LINE_CHARS`], so no text, whoever supplied it, can start a
//! line of its own or reach a terminal raw. The relay also never logs text the server chose
//! beyond short codes (see `relay.rs`). Whatever happens (a server that misbehaves, a flood of
//! refused commands), at most [`MAX_LINES_PER_HOUR`] lines are written per hour.
//!
//! The DSM boot-up task starts `run --log FILE` with stdout and stderr on /dev/null
//! (docs/relay-dsm.md), so that the root shell never opens a file in the shared folder: the relay
//! opens the file itself while it is still root, for reading and appending, never through a
//! symbolic link and only a regular file, and puts it on stderr ([`open_file`]). Once it holds the
//! state folder's lock it keeps that file ([`manage`]): lines older than [`KEEP_DAYS`] days go
//! when it starts and then once a day, and a file that reaches [`MAX_LOG_BYTES`] is cut to its
//! newest [`TRIM_TO_BYTES`]. Only a running relay does that: a log left behind while the relay is
//! stopped keeps its lines. A log it cannot rewrite (stderr from a shell's `>>`, or that of a
//! second relay that did not get the lock) is not written to once it has [`MAX_LOG_BYTES`].

use crate::clock::system_now;
use std::{
    cell::RefCell,
    fs::File,
    io::{self, Write},
    mem::ManuallyDrop,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::FileExt,
    },
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

pub const MAX_LINES_PER_HOUR: u32 = 200;
/// The longest line after the time stamp, in characters after escaping (the longest regular
/// line, a wake command with four MACs and a send error, is about 300).
pub const MAX_LINE_CHARS: usize = 400;
/// A log file of this size or more is cut ([`manage`]) or, when the relay cannot rewrite it, no
/// longer written to.
pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
/// What a kept log is cut to when it reaches [`MAX_LOG_BYTES`]: its newest lines, this many bytes.
pub const TRIM_TO_BYTES: usize = 1024 * 1024;
/// Lines of a kept log are removed once they are this many days old (the privacy notice's
/// retention for the relay log).
pub const KEEP_DAYS: i64 = 180;
const HOUR: Duration = Duration::from_secs(3600);
/// How often a running relay removes the old lines of its kept log.
const TIDY_EVERY: Duration = Duration::from_secs(24 * 3600);
/// A log larger than this is not read whole: only its newest [`TRIM_TO_BYTES`] are kept.
const READ_LIMIT: u64 = 4 * MAX_LOG_BYTES;
/// Before 2020-01-01 the NAS's clock is not set (a dead clock battery), and no line counts as old.
const CLOCK_SET: i64 = 1_577_836_800;

type Sink = Box<dyn FnMut(&str)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

static BUDGET: Mutex<Budget> = Mutex::new(Budget::new(MAX_LINES_PER_HOUR, HOUR));
static FULL: AtomicBool = AtomicBool::new(false);
/// Whether stderr is the relay's own log file, which it may rewrite ([`manage`]).
static MANAGED: AtomicBool = AtomicBool::new(false);
static LAST_TIDY: Mutex<Option<Instant>> = Mutex::new(None);

/// Sends this thread's log lines to `sink` instead of stderr (tests); `None` restores stderr.
pub fn capture(sink: Option<Sink>) {
    SINK.with(|s| *s.borrow_mut() = sink);
}

/// Whether `c` may appear in a log line or on a terminal as it is: not a control, format, line
/// or paragraph separator, private-use, unassigned or non-ASCII space character, and not a
/// combining mark (Rust's `escape_debug` rules, without its escaping of quotes and backslashes).
fn printable(c: char) -> bool {
    if matches!(c, '"' | '\'' | '\\') {
        return true;
    }
    let mut escaped = c.escape_debug();
    escaped.next() == Some(c) && escaped.next().is_none()
}

/// `s` with every character that is not [printable] written as its escape (`\n`, `\u{1b}`,
/// `\u{202e}`), cut after `max` characters with `…`. For text from outside the relay that is
/// shown in a log line or on a terminal.
pub fn clean(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max.saturating_mul(4)));
    let mut count = 0;
    for c in s.chars() {
        let width = if printable(c) {
            1
        } else {
            c.escape_debug().count()
        };
        if count + width > max {
            out.push('…');
            break;
        }
        if width == 1 {
            out.push(c);
        } else {
            out.extend(c.escape_debug());
        }
        count += width;
    }
    out
}

/// Whether stderr is a regular file of [`MAX_LOG_BYTES`] or more.
fn log_file_full() -> bool {
    // SAFETY: fstat fills the zeroed struct for an open descriptor (or fails).
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    if unsafe { libc::fstat(2, &mut st) } != 0 {
        return false;
    }
    #[allow(clippy::unnecessary_cast)]
    let size = st.st_size as i64;
    st.st_mode & libc::S_IFMT == libc::S_IFREG && u64::try_from(size).unwrap_or(0) >= MAX_LOG_BYTES
}

/// Writes one line; use [`logln!`](crate::logln).
pub fn line(args: std::fmt::Arguments<'_>) {
    let stamp = crate::system::local_timestamp(system_now());
    let text = format!("{stamp} {}\n", clean(&args.to_string(), MAX_LINE_CHARS));
    let captured = SINK.with(|s| match s.borrow_mut().as_mut() {
        Some(sink) => {
            sink(&text);
            true
        }
        None => false,
    });
    if captured {
        return;
    }
    let lines = match BUDGET.lock() {
        Ok(mut budget) => budget.admit(Instant::now()),
        Err(_) => vec![Line::Text],
    };
    if lines.is_empty() || FULL.load(Ordering::Relaxed) {
        return;
    }
    let mut out = String::new();
    if log_file_full() {
        let mb = MAX_LOG_BYTES / (1024 * 1024);
        if MANAGED.load(Ordering::Relaxed) && tidy(&stderr_file(), system_now(), true).is_ok() {
            out.push_str(&format!(
                "{stamp} 로그 파일이 {mb}MB에 이르러 오래된 줄을 지웠습니다.\n"
            ));
        } else {
            FULL.store(true, Ordering::Relaxed);
            out.push_str(&format!(
                "{stamp} 로그 파일이 {mb}MB를 넘어 더 적지 않습니다.\n"
            ));
            // a full disk or a closed log must not stop the relay
            let _ = std::io::stderr().lock().write_all(out.as_bytes());
            return;
        }
    }
    for l in lines {
        match l {
            Line::Text => out.push_str(&text),
            Line::Dropped(n) => out.push_str(&format!(
                "{stamp} (로그가 많아 지난 한 시간 동안 {n}줄을 적지 않았습니다)\n"
            )),
            Line::Stopping => out.push_str(&format!(
                "{stamp} 로그가 한 시간에 {MAX_LINES_PER_HOUR}줄을 넘어 한 시간 동안 적지 않습니다.\n"
            )),
        }
    }
    // a full disk or a closed log must not stop the relay
    let _ = std::io::stderr().lock().write_all(out.as_bytes());
}

/// Opens `path` for the log and puts it on stderr: created when missing (mode 0644), appended
/// to, and readable, so that [`manage`] can keep it later. Called as root, before the relay gives
/// up root, since the folder may be closed to the unprivileged relay. The file is in a folder
/// others may write to, and the relay rewrites it: it refuses a symbolic link, a file with other
/// names (hard links) and anything that is not a regular file
/// ([`open_plain`](crate::system::open_plain)).
pub fn open_file(path: &Path) -> io::Result<()> {
    let file = crate::system::open_plain(path, true, true)?;
    // SAFETY: both descriptors are open; dup2 puts the file on stderr, which the relay keeps open
    // for its whole life.
    if unsafe { libc::dup2(file.as_raw_fd(), 2) } < 0 {
        return Err(io::Error::last_os_error());
    }
    FULL.store(false, Ordering::Relaxed);
    Ok(())
}

/// Starts keeping the log on stderr when it is a file that [`open_file`] opened (readable and
/// appended to): its old lines go now and then once a day ([`tidy_if_due`]), and it is cut when
/// it reaches [`MAX_LOG_BYTES`]. Called once the relay holds the state folder's lock, so that a
/// relay that ends at the lock never rewrites the file another relay writes to. Returns whether
/// the log is kept.
pub fn manage() -> bool {
    // SAFETY: fcntl F_GETFL only queries the descriptor.
    let flags = unsafe { libc::fcntl(2, libc::F_GETFL) };
    let keepable = flags >= 0
        && flags & libc::O_ACCMODE == libc::O_RDWR
        && flags & libc::O_APPEND != 0
        && stderr_file()
            .metadata()
            .is_ok_and(|m| m.file_type().is_file());
    if keepable {
        MANAGED.store(true, Ordering::Relaxed);
        tidy_now();
    }
    keepable
}

/// Removes the old lines of the kept log once a day; the relay loop calls it between requests.
pub fn tidy_if_due() {
    if !MANAGED.load(Ordering::Relaxed) {
        return;
    }
    let due = match LAST_TIDY.lock() {
        Ok(last) => last.is_none_or(|t| t.elapsed() >= TIDY_EVERY),
        Err(_) => false,
    };
    if due {
        tidy_now();
    }
}

fn tidy_now() {
    if let Ok(mut last) = LAST_TIDY.lock() {
        *last = Some(Instant::now());
    }
    // a log that cannot be rewritten now stays as it is; the next day tries again
    let _ = tidy(&stderr_file(), system_now(), false);
}

/// The log on stderr as a file that is never closed.
fn stderr_file() -> ManuallyDrop<File> {
    // SAFETY: stderr stays open for the life of the process, and ManuallyDrop never closes it.
    ManuallyDrop::new(unsafe { File::from_raw_fd(2) })
}

/// Removes from `file` the lines older than [`KEEP_DAYS`] days before `now` (Unix seconds), and,
/// with `trim` or when the file has [`MAX_LOG_BYTES`] or more, all but its newest
/// [`TRIM_TO_BYTES`]; see [`prune`]. The file is rewritten in place, since the relay may not be
/// allowed to make a new one in its folder; it must be open for reading and writing. Returns the
/// number of bytes removed.
pub fn tidy(file: &File, now: i64, trim: bool) -> io::Result<u64> {
    let size = file.metadata()?.len();
    let trim = trim || size >= MAX_LOG_BYTES;
    // a file far over the limit (written by something else) is not read whole
    let start = if size > READ_LIMIT {
        size - TRIM_TO_BYTES as u64
    } else {
        0
    };
    let mut text = vec![0u8; usize::try_from(size - start).map_err(io::Error::other)?];
    file.read_exact_at(&mut text, start)?;
    if start > 0 {
        // starts inside a line
        let next = text
            .iter()
            .position(|b| *b == b'\n')
            .map_or(text.len(), |i| i + 1);
        text.drain(..next);
    }
    let cutoff = (now >= CLOCK_SET).then(|| now - KEEP_DAYS * 86_400);
    let kept = prune(&text, cutoff, if trim { TRIM_TO_BYTES } else { usize::MAX });
    if start == 0 && kept.len() == text.len() {
        return Ok(0);
    }
    rewrite(file, &kept)?;
    Ok(size - kept.len() as u64)
}

/// Replaces the content of `file` with `data`.
fn rewrite(file: &File, data: &[u8]) -> io::Result<()> {
    let fd = file.as_raw_fd();
    let set_flags = |flags: libc::c_int| -> io::Result<()> {
        // SAFETY: fcntl on an open descriptor.
        if unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    };
    // SAFETY: as above.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // with O_APPEND Linux appends whatever offset pwrite is given, so it is off while the file is
    // written from its start
    set_flags(flags & !libc::O_APPEND)?;
    let written = file
        .write_all_at(data, 0)
        .and_then(|()| file.set_len(data.len() as u64));
    let restored = set_flags(flags);
    written.and(restored)
}

/// The lines of a log to keep: those whose time stamp is at `cutoff` or later (all of them
/// without a cutoff), then at most `max` bytes of the newest of those. A line without a time
/// stamp (output that is not the relay's, such as a shell's message) goes with the stamped line
/// before it, or with the first stamped line when it comes before all of them; when no line has a
/// stamp, all are kept. Lines dated in the future are kept: only their own age removes them.
pub fn prune(text: &[u8], cutoff: Option<i64>, max: usize) -> Vec<u8> {
    let lines: Vec<&[u8]> = text.split_inclusive(|b| *b == b'\n').collect();
    let times: Vec<Option<i64>> = lines.iter().map(|l| line_time(l)).collect();
    let mut current = times.iter().flatten().next().copied();
    let mut keep: Vec<bool> = times
        .iter()
        .map(|t| {
            if t.is_some() {
                current = *t;
            }
            match (cutoff, current) {
                (Some(cutoff), Some(t)) => t >= cutoff,
                _ => true,
            }
        })
        .collect();
    let mut total: usize = lines
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(l, _)| l.len())
        .sum();
    for (l, k) in lines.iter().zip(keep.iter_mut()) {
        if total <= max {
            break;
        }
        if *k {
            *k = false;
            total -= l.len();
        }
    }
    lines
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .flat_map(|(l, _)| l.iter().copied())
        .collect()
}

/// The time of a log line that starts with a stamp like `2026-10-12 17:31:27 +0900 ` (the form of
/// [`crate::system::local_timestamp`] and of the boot-up task's `date "+%Y-%m-%d %H:%M:%S %z"`),
/// in Unix seconds.
pub fn line_time(line: &[u8]) -> Option<i64> {
    // YYYY-MM-DD HH:MM:SS +HHMM
    // 0123456789012345678901234
    let s = line.get(..25)?;
    if line
        .get(25)
        .is_some_and(|b| !matches!(b, b' ' | b'\n' | b'\r'))
    {
        return None;
    }
    for (i, sep) in [
        (4, b'-'),
        (7, b'-'),
        (10, b' '),
        (13, b':'),
        (16, b':'),
        (19, b' '),
    ] {
        if s[i] != sep {
            return None;
        }
    }
    let sign = match s[20] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let num = |from: usize, to: usize| -> Option<i64> {
        s[from..to].iter().try_fold(0i64, |n, b| {
            b.is_ascii_digit().then(|| n * 10 + i64::from(b - b'0'))
        })
    };
    let (year, month, day) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hour, minute, second) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let (off_hours, off_minutes) = (num(21, 23)?, num(23, 25)?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
        || off_hours > 23
        || off_minutes > 59
    {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
            - sign * (off_hours * 3600 + off_minutes * 60),
    )
}

/// Days from 1970-01-01 to a date of the proleptic Gregorian calendar (H. Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let year_of_era = y - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[derive(Debug, PartialEq, Eq)]
enum Line {
    /// The line itself.
    Text,
    /// A note that `n` lines of the previous period were not written.
    Dropped(u32),
    /// A note that the budget is used up.
    Stopping,
}

/// At most `max` lines per `period`.
struct Budget {
    max: u32,
    period: Duration,
    start: Option<Instant>,
    count: u32,
    dropped: u32,
}

impl Budget {
    const fn new(max: u32, period: Duration) -> Budget {
        Budget {
            max,
            period,
            start: None,
            count: 0,
            dropped: 0,
        }
    }

    /// What to write for one more line now.
    fn admit(&mut self, now: Instant) -> Vec<Line> {
        let mut out = Vec::new();
        if self
            .start
            .is_none_or(|s| now.saturating_duration_since(s) >= self.period)
        {
            if self.dropped > 0 {
                out.push(Line::Dropped(self.dropped));
            }
            *self = Budget {
                start: Some(now),
                ..Budget::new(self.max, self.period)
            };
        }
        self.count += 1;
        if self.count <= self.max {
            out.push(Line::Text);
        } else if self.count == self.max + 1 {
            out.push(Line::Stopping);
            self.dropped += 1;
        } else {
            self.dropped += 1;
        }
        out
    }
}

/// Logs one line: `logln!("format {}", args)`.
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        $crate::log::line(format_args!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs::{self, OpenOptions},
        os::unix::fs::MetadataExt,
        path::PathBuf,
    };

    #[test]
    fn at_most_max_lines_per_period() {
        let mut b = Budget::new(3, Duration::from_secs(60));
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(b.admit(t0), [Line::Text]);
        }
        assert_eq!(b.admit(t0), [Line::Stopping]);
        assert_eq!(b.admit(t0 + Duration::from_secs(59)), []);
        // the next period says what was left out
        assert_eq!(
            b.admit(t0 + Duration::from_secs(60)),
            [Line::Dropped(2), Line::Text]
        );
        assert_eq!(b.admit(t0 + Duration::from_secs(61)), [Line::Text]);
        assert_eq!(b.admit(t0 + Duration::from_secs(200)), [Line::Text]);
    }

    #[test]
    fn text_is_escaped_and_cut() {
        // what the relay itself writes stays as it is
        let own = "켜기 01234567 (**:**:**:**:53:01, 망 10.1.4.0/22 exact): eth0 10.1.5.20/22 -> \
                   255.255.255.255, 10.1.7.255 포트 9,7 ×3: 12/12개 보냄 \"q\" 'q' \\ … ok";
        assert_eq!(clean(own, MAX_LINE_CHARS), own);
        // a forged line stays inside this one
        assert_eq!(
            clean("x\n2026-10-12 17:31:28 +0900 등록 코드: AAAA", 100),
            "x\\n2026-10-12 17:31:28 +0900 등록 코드: AAAA"
        );
        assert_eq!(clean("a\r\tb\0", 100), "a\\r\\tb\\0");
        // terminal escapes, C1 controls, bidirectional overrides, line separators, odd spaces
        assert_eq!(clean("\u{1b}[2J", 100), "\\u{1b}[2J");
        assert_eq!(clean("a\u{9b}b", 100), "a\\u{9b}b");
        assert_eq!(clean("a\u{202e}b\u{2066}", 100), "a\\u{202e}b\\u{2066}");
        assert_eq!(clean("a\u{2028}b\u{2029}", 100), "a\\u{2028}b\\u{2029}");
        assert_eq!(clean("a\u{3000}b\u{a0}", 100), "a\\u{3000}b\\u{a0}");
        assert_eq!(clean("e\u{301}", 100), "e\\u{301}");
        // cut after max characters, never inside an escape
        assert_eq!(clean("abcdef", 3), "abc…");
        assert_eq!(clean("ab\ncd", 3), "ab…");
        assert_eq!(clean("가나다라", 2), "가나…");
        assert_eq!(clean("abc", 3), "abc");
        let long = clean(&"\u{1b}".repeat(100_000), MAX_LINE_CHARS);
        assert!(long.chars().count() <= MAX_LINE_CHARS + 1);
        assert!(!long.contains('\u{1b}'));
    }

    #[test]
    fn time_stamps_of_lines() {
        assert_eq!(
            line_time(b"2026-10-12 17:31:27 +0900 hjv-relay 1.6.0 \xec\x8b\x9c\xec\x9e\x91\n"),
            Some(1_791_793_887)
        );
        assert_eq!(
            line_time(b"2025-10-09 08:53:20 +0000 x"),
            Some(1_760_000_000)
        );
        assert_eq!(line_time(b"1970-01-01 00:00:00 +0000\n"), Some(0));
        assert_eq!(
            line_time(b"2000-02-29 23:59:59 -0130 leap day"),
            Some(951_874_199)
        );
        // the relay's own stamps read back as the time they were made
        let t = 1_776_189_600;
        let stamp = crate::system::local_timestamp(t);
        assert_eq!(
            line_time(format!("{stamp} x").as_bytes()),
            Some(t),
            "{stamp}"
        );
        for bad in [
            &b""[..],
            b"2026-10-12 17:31:27",
            b"2026-10-12 17:31:27 +09",
            b"2026-10-12 17:31:27 +0900x",
            b"2026-10-12T17:31:27 +0900 x",
            b"2026-13-12 17:31:27 +0900 x",
            b"2026-10-00 17:31:27 +0900 x",
            b"2026-10-12 24:31:27 +0900 x",
            b"2026-10-12 17:31:27 0900 x",
            b"2026-1O-12 17:31:27 +0900 x",
            b"stand-in started: run --state /x",
            b"@1760000000 x",
        ] {
            assert_eq!(line_time(bad), None, "{}", String::from_utf8_lossy(bad));
        }
    }

    const DAY: i64 = 86_400;

    fn stamped(t: i64, text: &str) -> String {
        format!("{} {text}\n", crate::system::local_timestamp(t))
    }

    #[test]
    fn pruning_by_date_and_size() {
        let now = 1_791_793_887;
        let cutoff = Some(now - KEEP_DAYS * DAY);
        let old = stamped(now - 200 * DAY, "old");
        let old_more = "a shell's message without a stamp\n";
        let edge = stamped(now - KEEP_DAYS * DAY, "exactly 180 days");
        let new = stamped(now - DAY, "new");
        let new_more = "panicked at ...\n";
        let text = format!("{old}{old_more}{edge}{new}{new_more}");
        let kept = prune(text.as_bytes(), cutoff, usize::MAX);
        assert_eq!(
            String::from_utf8(kept).unwrap(),
            format!("{edge}{new}{new_more}")
        );
        // no cutoff, or nothing old: all of it
        assert_eq!(prune(text.as_bytes(), None, usize::MAX), text.as_bytes());
        let recent = format!("{edge}{new}");
        assert_eq!(
            prune(recent.as_bytes(), cutoff, usize::MAX),
            recent.as_bytes()
        );
        // unstamped lines before the first stamp go with it
        let lead = format!("{old_more}{old}{new}");
        assert_eq!(prune(lead.as_bytes(), cutoff, usize::MAX), new.as_bytes());
        let lead = format!("{old_more}{new}");
        assert_eq!(prune(lead.as_bytes(), cutoff, usize::MAX), lead.as_bytes());
        // without any stamp nothing is known to be old
        let unknown = "x\ny\n";
        assert_eq!(
            prune(unknown.as_bytes(), cutoff, usize::MAX),
            unknown.as_bytes()
        );
        // each line by its own date: a line from a clock set back stays only while it is young,
        // and a line dated in the future does not keep older lines after it
        let future = stamped(now + 400 * DAY, "clock ahead");
        let mixed = format!("{new}{old}{future}{old}{new}");
        assert_eq!(
            String::from_utf8(prune(mixed.as_bytes(), cutoff, usize::MAX)).unwrap(),
            format!("{new}{future}{new}")
        );
        // the newest lines that fit, whole lines only; a last line without newline counts
        let lines: Vec<String> = (0..10).map(|i| stamped(now - i, &format!("{i}"))).collect();
        let all = format!("{}tail", lines.concat());
        let kept = prune(all.as_bytes(), cutoff, lines[0].len() * 3 + 4);
        assert_eq!(
            String::from_utf8(kept).unwrap(),
            format!("{}{}{}tail", lines[7], lines[8], lines[9])
        );
        assert!(prune(all.as_bytes(), cutoff, 3).is_empty());
    }

    fn temp_file(name: &str, content: &[u8]) -> (PathBuf, File) {
        let dir = std::env::temp_dir().join(format!("hjv-relay-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        let path = dir.join("hjv-relay.log");
        fs::write(&path, content).unwrap();
        let file = OpenOptions::new()
            .read(true)
            .append(true)
            .open(&path)
            .unwrap();
        (path, file)
    }

    #[test]
    fn tidy_rewrites_the_file_in_place() {
        let now = 1_791_793_887;
        let old = stamped(now - 181 * DAY, "old");
        let new = stamped(now - 179 * DAY, "new");
        let (path, file) = temp_file("tidy", format!("{old}{old}{new}").as_bytes());
        let inode = fs::metadata(&path).unwrap().ino();
        assert_eq!(tidy(&file, now, false).unwrap(), 2 * old.len() as u64);
        assert_eq!(fs::read_to_string(&path).unwrap(), new);
        assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
        // still appended to, at the end
        (&file).write_all(b"next\n").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), format!("{new}next\n"));
        // nothing to do: not rewritten
        let before = fs::metadata(&path).unwrap().mtime_nsec();
        assert_eq!(tidy(&file, now, false).unwrap(), 0);
        assert_eq!(fs::metadata(&path).unwrap().mtime_nsec(), before);
        // a clock that is not set removes nothing
        let (_, file) = temp_file("clock", old.as_bytes());
        assert_eq!(tidy(&file, 1_000, false).unwrap(), 0);
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_full_log_is_cut_to_its_newest_lines() {
        let now = 1_791_793_887;
        let line = stamped(now - 60, &"x".repeat(100));
        let count = MAX_LOG_BYTES as usize / line.len() + 1;
        let (path, file) = temp_file("full", line.repeat(count).as_bytes());
        tidy(&file, now, false).unwrap();
        let left = fs::read(&path).unwrap();
        assert!(left.len() <= TRIM_TO_BYTES && left.len() > TRIM_TO_BYTES - line.len());
        assert!(left.starts_with(line.as_bytes()) && left.ends_with(line.as_bytes()));
        // a file far over the limit is read from its end only
        let huge = vec![b'y'; (READ_LIMIT + 10) as usize];
        let (path2, file) = temp_file("huge", &[&huge[..], b"\n", line.as_bytes()].concat());
        tidy(&file, now, false).unwrap();
        assert_eq!(fs::read(&path2).unwrap(), line.as_bytes());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
        fs::remove_dir_all(path2.parent().unwrap()).unwrap();
    }

    #[test]
    fn only_a_plain_file_is_taken_for_the_log() {
        let dir = std::env::temp_dir().join(format!("hjv-relay-log-open-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir(&dir).unwrap();
        // a symbolic link, a hard link and a directory are refused (before stderr is touched)
        let target = dir.join("target");
        fs::write(&target, "keep\n").unwrap();
        let link = dir.join("link.log");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(open_file(&link)
            .unwrap_err()
            .to_string()
            .contains("symbolic link"));
        let hard = dir.join("hard.log");
        fs::hard_link(&target, &hard).unwrap();
        assert!(open_file(&hard)
            .unwrap_err()
            .to_string()
            .contains("hard links"));
        let sub = dir.join("sub.log");
        fs::create_dir(&sub).unwrap();
        assert!(open_file(&sub).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep\n");
        fs::remove_dir_all(&dir).unwrap();
    }
}
