// SPDX-License-Identifier: AGPL-3.0-only

//! The relay's log: one line per event, with the local date and time, on stderr (the DSM boot-up
//! task appends it to hjv-relay.log). It logs state changes, command results and refusals, and
//! connection errors at most once per ten minutes, never every poll, so an idle relay does not
//! keep the NAS's disks awake. MAC addresses appear masked (`**:**:**:**:12:34`).
//!
//! Every line goes through [`line`], which writes characters that are not printable (newlines,
//! terminal escapes, bidirectional and other format characters) as escapes such as `\n` and
//! `\u{1b}` and cuts the line at [`MAX_LINE_CHARS`], so no text, whoever supplied it, can start a
//! line of its own or reach a terminal raw. The relay also never logs text the server chose
//! beyond short codes (see `relay.rs`).
//!
//! The log file is rotated only at boot, so whatever happens (a server that misbehaves, a flood
//! of refused commands) at most [`MAX_LINES_PER_HOUR`] lines are written per hour, and nothing
//! once the file has [`MAX_LOG_BYTES`].

use std::{
    cell::RefCell,
    io::Write,
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
/// A log file of this size or more is not written to (the boot-up task rotates it at boot).
pub const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;
const HOUR: Duration = Duration::from_secs(3600);

type Sink = Box<dyn FnMut(&str)>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
}

static BUDGET: Mutex<Budget> = Mutex::new(Budget::new(MAX_LINES_PER_HOUR, HOUR));
static FULL: AtomicBool = AtomicBool::new(false);

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
    let stamp = crate::system::local_timestamp(crate::clock::system_now());
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
        FULL.store(true, Ordering::Relaxed);
        out.push_str(&format!(
            "{stamp} 로그 파일이 {}MB를 넘어 더 적지 않습니다. 부팅할 때 돌립니다.\n",
            MAX_LOG_BYTES / (1024 * 1024)
        ));
    } else {
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
    }
    // a full disk or a closed log must not stop the relay
    let _ = std::io::stderr().lock().write_all(out.as_bytes());
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
}
