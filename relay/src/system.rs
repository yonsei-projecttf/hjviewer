// SPDX-License-Identifier: AGPL-3.0-only

//! The operating system side: the state folder and its lock, giving up root, noticing that the
//! executable was replaced, the DSM version, the runtime status file and local time for the log.
//!
//! On the NAS the DSM boot-up task starts the relay as root. The relay makes itself non-dumpable
//! first (no core dump, no /proc access to its memory, while it holds the key), lets go of a
//! terminal it was started from, opens what needs root (the state folder `/volume1/@hjv-relay`,
//! mode 0700, with its lock and `relay.key`; its own executable; the status file in /run), then
//! switches to uid/gid 65534 with no supplementary groups, sets PR_SET_NO_NEW_PRIVS and stays
//! non-dumpable, before it touches the network.

use std::{
    ffi::{CString, OsString},
    fs::{self, File, OpenOptions},
    io,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, FileExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
    time::Duration,
};

pub const LOCK_FILE: &str = "lock";
pub const DEFAULT_STATUS_FILE: &str = "/run/hjv-relay.status";
/// nobody/nogroup
pub const DEFAULT_USER: (u32, u32) = (65534, 65534);
/// How long [`lock`] keeps trying when the lock is taken (`status` holds it for an instant).
const LOCK_TRIES: u32 = 10;
const LOCK_RETRY: Duration = Duration::from_millis(100);

pub fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() }
}

/// No core dumps and no access to this process's memory through /proc or ptrace by processes of
/// the same user, from now on (the kernel resets this when the user changes, so
/// [`drop_privileges`] sets it again).
pub fn set_non_dumpable() -> io::Result<()> {
    // SAFETY: prctl with integer arguments.
    let rc = unsafe {
        libc::prctl(
            libc::PR_SET_DUMPABLE,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
            0 as libc::c_ulong,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn is_tty(fd: libc::c_int) -> bool {
    // SAFETY: isatty only queries the descriptor.
    unsafe { libc::isatty(fd) == 1 }
}

/// Puts `file` (opened with `flags`) on descriptor `fd`; keeps `fd` as it is if that fails.
fn reopen(fd: libc::c_int, file: &Path, flags: libc::c_int) {
    let Ok(path) = CString::new(file.as_os_str().as_bytes()) else {
        return;
    };
    // SAFETY: a NUL-terminated path; the new descriptor is closed below.
    let new = unsafe { libc::open(path.as_ptr(), flags | libc::O_NOCTTY | libc::O_CLOEXEC) };
    if new < 0 {
        return;
    }
    // SAFETY: both descriptors are open; dup2 clears close-on-exec on `fd`.
    unsafe {
        libc::dup2(new, fd);
        libc::close(new);
    }
}

/// When the relay was started from a terminal (an administrator trying it over SSH with sudo),
/// keeps the unprivileged process away from that root shell's terminal: stdin moves to
/// /dev/null, stdout and stderr are opened again write-only (the log still shows there), and the
/// process gives up the terminal as its controlling terminal, so that it can neither read what
/// is typed there nor push input into it (TIOCSTI). It stays in its process group, so Ctrl+C
/// still stops it. Without a terminal (the boot-up task) this does nothing. Call it as root,
/// before [`drop_privileges`].
pub fn detach_terminal() {
    // SAFETY: opening /dev/tty only succeeds for a process with a controlling terminal.
    let ctty = unsafe {
        libc::open(
            c"/dev/tty".as_ptr(),
            libc::O_RDONLY | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if is_tty(0) {
        reopen(0, Path::new("/dev/null"), libc::O_RDONLY);
    }
    for fd in [1, 2] {
        if is_tty(fd) {
            reopen(
                fd,
                &PathBuf::from(format!("/proc/self/fd/{fd}")),
                libc::O_WRONLY,
            );
        }
    }
    if ctty >= 0 {
        // SAFETY: plain queries and an ioctl on an open descriptor, which is closed after.
        unsafe {
            // a session leader would hang up the terminal's foreground jobs; its terminal is its
            // own session's, with nothing else to reach
            if libc::getsid(0) != libc::getpid() {
                libc::ioctl(ctty, libc::TIOCNOTTY);
            }
            libc::close(ctty);
        }
    }
}

/// What a `--user` uid owns that root runs or reads later: the executable or its folder (it
/// could replace the program the boot-up task starts as root).
pub fn owned_by(uid: u32, exe: &Path) -> Option<PathBuf> {
    [Some(exe), exe.parent()]
        .into_iter()
        .flatten()
        .find(|p| fs::metadata(p).is_ok_and(|m| m.uid() == uid))
        .map(Path::to_path_buf)
}

/// Daemon hygiene for a process a scheduler started as root: stdin, stdout and stderr are open
/// (on /dev/null if they were closed, so that no file the relay opens later becomes fd 2 and
/// receives log lines), and no other inherited descriptor survives into the unprivileged part.
pub fn tidy_descriptors() {
    for fd in 0..=2 {
        // SAFETY: F_GETFD only queries the descriptor.
        if unsafe { libc::fcntl(fd, libc::F_GETFD) } == -1 {
            // SAFETY: opening /dev/null takes the lowest free descriptor, which is `fd`.
            unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_RDWR) };
        }
    }
    let inherited: Vec<i32> = fs::read_dir("/proc/self/fd")
        .map(|dir| {
            dir.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
                .filter(|fd| *fd > 2)
                .collect()
        })
        .unwrap_or_default();
    for fd in inherited {
        // SAFETY: closes descriptors this process does not use (one was the listing's own, now
        // closed already: EBADF, ignored).
        unsafe { libc::close(fd) };
    }
}

#[derive(Debug)]
pub enum StateDirError {
    /// The folder or its parent is missing (a volume not mounted yet): retry later.
    Unavailable(io::Error),
    /// The path is there but not safe to use: a person has to look.
    Unsafe(String),
    Io(io::Error),
}

/// Makes sure `dir` is a directory owned by this user with mode 0700, creating it (not its
/// parents) when `create` is set.
pub fn prepare_state_dir(dir: &Path, create: bool) -> Result<(), StateDirError> {
    match fs::symlink_metadata(dir) {
        Err(e) if e.kind() == io::ErrorKind::NotFound && create => {
            match fs::DirBuilder::new().mode(0o700).create(dir) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StateDirError::Unavailable(e)),
                Err(e) => Err(StateDirError::Io(e)),
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StateDirError::Unavailable(e)),
        Err(e) => Err(StateDirError::Io(e)),
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(StateDirError::Unsafe(format!(
                    "{} is not a directory",
                    dir.display()
                )));
            }
            if meta.uid() != euid() {
                return Err(StateDirError::Unsafe(format!(
                    "{} belongs to uid {}, not to uid {}",
                    dir.display(),
                    meta.uid(),
                    euid()
                )));
            }
            if meta.mode() & 0o077 != 0 {
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| {
                    StateDirError::Unsafe(format!("cannot make {} mode 0700: {e}", dir.display()))
                })?;
                crate::logln!("상태 폴더 권한을 0700으로 고쳤습니다: {}", dir.display());
            }
            Ok(())
        }
    }
}

/// The state folder's lock; held until dropped (the process ends).
pub struct Lock {
    _file: File,
}

#[derive(Debug)]
pub enum LockError {
    /// Another relay holds it.
    Busy,
    Io(io::Error),
}

/// Takes the state folder's lock. The file is opened read-only (flock needs no more), so the
/// unprivileged relay holds no writable descriptor to it. Another relay holds the lock for its
/// whole life, while `status` takes it shared only for an instant: the lock is tried for about a
/// second before the answer is [`LockError::Busy`].
pub fn lock(dir: &Path) -> Result<Lock, LockError> {
    let path = CString::new(dir.join(LOCK_FILE).as_os_str().as_bytes())
        .map_err(|e| LockError::Io(io::Error::other(e)))?;
    // SAFETY: a NUL-terminated path; the descriptor is owned by the File below.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600 as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(LockError::Io(io::Error::last_os_error()));
    }
    // SAFETY: `fd` is a new descriptor that nothing else owns.
    let file = unsafe { File::from_raw_fd(fd) };
    for attempt in 1..=LOCK_TRIES {
        // SAFETY: flock on an open descriptor.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(Lock { _file: file });
        }
        let e = io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(LockError::Io(e));
        }
        if attempt < LOCK_TRIES {
            std::thread::sleep(LOCK_RETRY);
        }
    }
    Err(LockError::Busy)
}

/// Whether a relay holds the lock of `dir` (for `status`).
pub fn lock_held(dir: &Path) -> io::Result<bool> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join(LOCK_FILE))
    {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    // SAFETY: flock on an open descriptor; the shared lock goes away with the file.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
        return Ok(false);
    }
    let e = io::Error::last_os_error();
    if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
        Ok(true)
    } else {
        Err(e)
    }
}

/// Parses `UID:GID` (both non-zero).
pub fn parse_user(s: &str) -> Result<(u32, u32), String> {
    let (uid, gid) = s
        .split_once(':')
        .ok_or_else(|| format!("{s:?} is not UID:GID"))?;
    let num = |v: &str| -> Option<u32> {
        (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .then(|| v.parse().ok())
            .flatten()
            .filter(|n| *n != 0 && *n != u32::MAX)
    };
    match (num(uid), num(gid)) {
        (Some(u), Some(g)) => Ok((u, g)),
        _ => Err(format!("{s:?}: UID and GID must be numbers other than 0")),
    }
}

/// Switches a root process to `uid`/`gid` for good (no supplementary groups) and checks that it
/// cannot switch back; any process then sets PR_SET_NO_NEW_PRIVS and stays non-dumpable, so
/// other processes of the same uid cannot read its memory. Returns whether the user changed.
pub fn drop_privileges(uid: u32, gid: u32) -> io::Result<bool> {
    let check = |rc: libc::c_int, what: &str| -> io::Result<()> {
        if rc == 0 {
            Ok(())
        } else {
            let e = io::Error::last_os_error();
            Err(io::Error::new(e.kind(), format!("{what}: {e}")))
        }
    };
    let changed = euid() == 0;
    if changed {
        // SAFETY: plain system calls on this process's credentials; the process has one thread.
        unsafe {
            check(libc::setgroups(0, std::ptr::null()), "setgroups")?;
            check(libc::setresgid(gid, gid, gid), "setresgid")?;
            check(libc::setresuid(uid, uid, uid), "setresuid")?;
        }
        let (mut r, mut e, mut s) = (0, 0, 0);
        // SAFETY: the pointers are to local variables.
        check(
            unsafe { libc::getresuid(&mut r, &mut e, &mut s) },
            "getresuid",
        )?;
        let (mut gr, mut ge, mut gs) = (0, 0, 0);
        // SAFETY: as above.
        check(
            unsafe { libc::getresgid(&mut gr, &mut ge, &mut gs) },
            "getresgid",
        )?;
        if [r, e, s] != [uid; 3] || [gr, ge, gs] != [gid; 3] {
            return Err(io::Error::other("the user or group did not change"));
        }
        // SAFETY: as above; must fail now.
        if unsafe { libc::setuid(0) } == 0 {
            return Err(io::Error::other("root could be regained"));
        }
    }
    // SAFETY: prctl with integer arguments.
    unsafe {
        check(
            libc::prctl(
                libc::PR_SET_NO_NEW_PRIVS,
                1 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
                0 as libc::c_ulong,
            ),
            "PR_SET_NO_NEW_PRIVS",
        )?;
    }
    set_non_dumpable().map_err(|e| io::Error::new(e.kind(), format!("PR_SET_DUMPABLE: {e}")))?;
    Ok(changed)
}

/// Notices that the relay's own executable was replaced or changed, so that it can exit and let
/// the boot-up task's loop start the new one. It keeps the file open and remembers
/// /proc/self/exe, which keep working after the relay gave up root (the folder the executable
/// is in may not be readable then).
pub struct ExeWatch {
    path: PathBuf,
    link: Option<OsString>,
    file: Option<File>,
    ident: Option<Ident>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Ident {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}

impl Ident {
    fn of(m: &fs::Metadata) -> Ident {
        Ident {
            dev: m.dev(),
            ino: m.ino(),
            size: m.size(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
        }
    }
}

impl ExeWatch {
    /// Watches the running executable.
    pub fn new() -> Option<ExeWatch> {
        let path = std::env::current_exe().ok()?;
        Some(Self::of(path))
    }

    /// Watches `path` (tests).
    pub fn of(path: PathBuf) -> ExeWatch {
        let link = fs::read_link("/proc/self/exe")
            .ok()
            .map(PathBuf::into_os_string);
        let file = File::open(&path).ok();
        let ident = file
            .as_ref()
            .and_then(|f| f.metadata().ok())
            .map(|m| Ident::of(&m));
        ExeWatch {
            path,
            link,
            file,
            ident,
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Why the executable is not the one that was started, if it changed.
    pub fn changed(&self) -> Option<String> {
        if let (Some(link), Ok(now)) = (&self.link, fs::read_link("/proc/self/exe")) {
            if now.as_os_str() != link {
                return Some(format!("now {}", now.display()));
            }
        }
        let ident = self.ident?;
        if let Some(meta) = self.file.as_ref().and_then(|f| f.metadata().ok()) {
            if meta.nlink() == 0 {
                return Some("removed".to_owned());
            }
            if Ident::of(&meta) != ident {
                return Some("modified".to_owned());
            }
        }
        // after giving up root this may fail for lack of permission; the checks above still work
        if let Ok(meta) = fs::metadata(&self.path) {
            if Ident::of(&meta) != ident {
                return Some("replaced".to_owned());
            }
        }
        None
    }
}

/// `DSM 7.2.1-69057 Update 5` from DSM's /etc.defaults/VERSION, else the os-release name, else
/// `Linux`.
pub fn os_description() -> String {
    let text = fs::read_to_string("/etc.defaults/VERSION").unwrap_or_default();
    if let Some(v) = dsm_version(&text) {
        return v;
    }
    let text = fs::read_to_string("/etc/os-release").unwrap_or_default();
    if let Some(v) = os_release_name(&text) {
        return v;
    }
    "Linux".to_owned()
}

fn key_value<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (k, v) = line.split_once('=')?;
        (k.trim() == key)
            .then(|| v.trim().trim_matches('"'))
            .filter(|v| !v.is_empty())
    })
}

fn printable(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(100).collect()
}

pub fn dsm_version(text: &str) -> Option<String> {
    let version = key_value(text, "productversion")?;
    let mut s = format!("DSM {version}");
    if let Some(build) = key_value(text, "buildnumber") {
        s.push('-');
        s.push_str(build);
    }
    if let Some(fix) = key_value(text, "smallfixnumber").filter(|f| *f != "0") {
        s.push_str(" Update ");
        s.push_str(fix);
    }
    Some(printable(&s))
}

fn os_release_name(text: &str) -> Option<String> {
    key_value(text, "PRETTY_NAME").map(printable)
}

/// The running kernel's release (`uname -r`).
pub fn kernel_release() -> String {
    printable(
        fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim(),
    )
}

/// The runtime status `hjv-relay status` shows. Opened before the relay gives up root and
/// written through the open descriptor; written on every poll only on a memory file system
/// (tmpfs), otherwise only when something changes, so that it does not keep disks awake.
pub struct StatusFile {
    file: File,
    frequent: bool,
}

impl StatusFile {
    pub fn open(path: &Path) -> io::Result<StatusFile> {
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o644)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        // SAFETY: statfs fills the zeroed struct.
        let mut fs_info: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: fstatfs on an open descriptor.
        let ok = unsafe { libc::fstatfs(file.as_raw_fd(), &mut fs_info) } == 0;
        const TMPFS_MAGIC: i64 = 0x0102_1994;
        const RAMFS_MAGIC: i64 = 0x8584_58f6;
        #[allow(clippy::unnecessary_cast)]
        let kind = fs_info.f_type as i64;
        Ok(StatusFile {
            file,
            frequent: ok && (kind == TMPFS_MAGIC || kind == RAMFS_MAGIC),
        })
    }

    pub fn frequent(&self) -> bool {
        self.frequent
    }

    pub fn write(&mut self, text: &str) {
        let _ = self.file.write_all_at(text.as_bytes(), 0);
        let _ = self.file.set_len(text.len() as u64);
    }
}

/// `2026-10-12 17:00:00 +0900` in the system's time zone (TZ, else /etc/localtime).
pub fn local_timestamp(secs: i64) -> String {
    // time_t, inferred: naming the alias is deprecated on musl (it is 64 bits on x86_64)
    #[allow(clippy::unnecessary_cast)]
    let t = secs as _;
    // SAFETY: localtime_r fills the zeroed struct from `t`.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: both pointers are valid for the call.
    if unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
        return format!("@{secs}");
    }
    #[allow(clippy::unnecessary_cast)]
    let offset = tm.tm_gmtoff as i64;
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.abs();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {sign}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        offset / 3600,
        offset % 3600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hjv-relay-sys-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn state_dir() {
        let dir = temp_dir("state");
        assert!(matches!(
            prepare_state_dir(&dir, false),
            Err(StateDirError::Unavailable(_))
        ));
        prepare_state_dir(&dir, true).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        prepare_state_dir(&dir, true).unwrap();
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        // a missing parent is "not yet", not an error to give up on
        let deep = dir.join("missing/state");
        assert!(matches!(
            prepare_state_dir(&deep, true),
            Err(StateDirError::Unavailable(_))
        ));
        let file = dir.join("file");
        fs::write(&file, "").unwrap();
        assert!(matches!(
            prepare_state_dir(&file, true),
            Err(StateDirError::Unsafe(_))
        ));
        let link = dir.join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(matches!(
            prepare_state_dir(&link, true),
            Err(StateDirError::Unsafe(_))
        ));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn one_relay_per_state_dir() {
        let dir = temp_dir("lock");
        prepare_state_dir(&dir, true).unwrap();
        assert!(!lock_held(&dir).unwrap());
        let held = lock(&dir).unwrap();
        // held read-only
        // SAFETY: F_GETFL only queries the descriptor.
        let flags = unsafe { libc::fcntl(held._file.as_raw_fd(), libc::F_GETFL) };
        assert_eq!(flags & libc::O_ACCMODE, libc::O_RDONLY);
        let started = std::time::Instant::now();
        assert!(matches!(lock(&dir), Err(LockError::Busy)));
        assert!(started.elapsed() >= LOCK_RETRY * (LOCK_TRIES - 1));
        assert!(lock_held(&dir).unwrap());
        drop(held);
        assert!(!lock_held(&dir).unwrap());
        let again = lock(&dir).unwrap();
        drop(again);

        // `status` looking at the lock while a relay starts: the relay waits for it
        let file = File::open(dir.join(LOCK_FILE)).unwrap();
        // SAFETY: flock on an open descriptor.
        assert_eq!(
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) },
            0
        );
        let peek = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            drop(file);
        });
        let _relay = lock(&dir).unwrap();
        peek.join().unwrap();
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_user_that_owns_the_program_is_refused() {
        let dir = temp_dir("owner");
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("hjv-relay");
        fs::write(&exe, "").unwrap();
        assert_eq!(owned_by(euid(), &exe), Some(exe.clone()));
        let other = euid().wrapping_add(12345);
        assert_eq!(owned_by(other, &exe), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn users() {
        assert_eq!(parse_user("65534:65534"), Ok((65534, 65534)));
        assert_eq!(parse_user("1026:100"), Ok((1026, 100)));
        for bad in [
            "0:0",
            "0:100",
            "100:0",
            "65534",
            "a:b",
            ":1",
            "1:",
            "+1:1",
            "4294967295:1",
        ] {
            assert!(parse_user(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn exe_watch_sees_replacement_and_change() {
        let dir = temp_dir("exe");
        fs::create_dir(&dir).unwrap();
        let exe = dir.join("hjv-relay");
        fs::write(&exe, "v1").unwrap();
        let watch = ExeWatch::of(exe.clone());
        assert_eq!(watch.changed(), None);
        // replaced the way uploads do it: a new file renamed over the old one
        let new = dir.join("hjv-relay.new");
        fs::write(&new, "v2").unwrap();
        fs::rename(&new, &exe).unwrap();
        assert!(watch.changed().is_some());
        // changed in place
        let watch = ExeWatch::of(exe.clone());
        assert_eq!(watch.changed(), None);
        fs::write(&exe, "v3 longer").unwrap();
        assert!(watch.changed().is_some());
        // removed
        let watch = ExeWatch::of(exe.clone());
        fs::remove_file(&exe).unwrap();
        assert_eq!(watch.changed().as_deref(), Some("removed"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn versions() {
        let dsm = "majorversion=\"7\"\nminorversion=\"2\"\nmajor=\"7\"\nminor=\"2\"\nmicro=\"1\"\n\
                   buildphase=\"GM\"\nbuildnumber=\"69057\"\nsmallfixnumber=\"5\"\nproductversion=\"7.2.1\"\nos_name=\"DSM\"\n";
        assert_eq!(
            dsm_version(dsm).as_deref(),
            Some("DSM 7.2.1-69057 Update 5")
        );
        let no_update = dsm.replace("smallfixnumber=\"5\"", "smallfixnumber=\"0\"");
        assert_eq!(dsm_version(&no_update).as_deref(), Some("DSM 7.2.1-69057"));
        assert_eq!(dsm_version("PRETTY_NAME=\"x\""), None);
        assert_eq!(
            os_release_name("NAME=Ubuntu\nPRETTY_NAME=\"Ubuntu 24.04.3 LTS\"\n").as_deref(),
            Some("Ubuntu 24.04.3 LTS")
        );
        assert!(!kernel_release().is_empty());
        assert!(!os_description().is_empty());
    }

    #[test]
    fn status_file() {
        let dir = temp_dir("status");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("status");
        let mut status = StatusFile::open(&path).unwrap();
        status.write("{\"long\":\"content\"}");
        status.write("{}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{}");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn timestamps() {
        let s = local_timestamp(1_760_000_000);
        assert_eq!(s.len(), "2025-10-09 08:53:20 +0000".len(), "{s}");
        assert!(
            s.starts_with("2025-10-09 ") || s.starts_with("2025-10-08 "),
            "{s}"
        );
    }
}
