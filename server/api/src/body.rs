// SPDX-License-Identifier: AGPL-3.0-only

//! What an answer's body is made of, and the guard on a connection's writes (the design of the
//! minimum version and forced update, §4-6).
//!
//! Every answer but a file is a few bytes sent at once ([`Body::Full`]). A file (update.rs: a
//! package of tens of megabytes, or one of its notice files) is read [`PIECE`] bytes at a time,
//! each piece only when hyper has room for it: a download takes the memory of a piece or two
//! however large the file is and however slowly it is read. A file sent holds one of the service's
//! download slots ([`Slots`], HJV_API_DOWNLOADS_MAX) from before its first byte until its last
//! one went or its connection ended.
//!
//! A file goes out no faster than [`RATE`], piece after piece: the server's line is shared. nginx
//! cannot do that for hjv-api: it passes a package on as it comes (`X-Accel-Buffering: no`, so
//! that it never writes one to a file of its own), and an answer it passes on so it does not
//! slow down, whatever `X-Accel-Limit-Rate` says (nginx 1.24, measured).
//!
//! A download ends with an error, which closes its connection, when the file turns out shorter
//! than it was when it was opened, and when it takes longer than [`DOWNLOAD_TIME`]. The time limit
//! of a request (server.rs) ends when the answer's head is ready, so [`StallGuard`] is around
//! every connection: a write that the other side does not take for [`WRITE_STALL`] closes the
//! connection, and so does a write after the time of the download it sends ([`Deadline`]). The
//! body alone cannot keep that time: hyper asks it for the next piece only once the last one has
//! gone, which for a reader that takes a few bytes at a time is hours later. Without the two, a
//! reader that stops in the middle of a package, or takes it a few bytes at a time, would hold a
//! download slot for as long as it likes.

use http_body_util::Full;
use hyper::body::{Bytes, Frame, SizeHint};
use std::{
    fs::File,
    future::Future,
    io,
    os::unix::fs::FileExt,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::{OwnedSemaphorePermit, Semaphore},
    task::JoinHandle,
    time::{Instant, Sleep},
};

/// A file is read and sent this many bytes at a time.
pub const PIECE: usize = 64 * 1024;
/// A file goes out no faster than this, in bytes a second: about 30 Mb/s, the relay's limit for
/// a session on the same line.
pub const RATE: u64 = 3_840_000;
/// How long a download may take from its first byte: the apps give a package 30 minutes.
pub const DOWNLOAD_TIME: Duration = Duration::from_secs(3600);
/// How long a write may wait for the other side to take anything.
pub const WRITE_STALL: Duration = Duration::from_secs(60);

/// The downloads that may run at once.
pub struct Slots(Arc<Semaphore>);

/// One of the [`Slots`], free again when it is dropped.
pub struct Slot {
    _permit: OwnedSemaphorePermit,
}

impl Slots {
    pub fn new(count: usize) -> Slots {
        Slots(Arc::new(Semaphore::new(count)))
    }

    /// A slot, if one is free.
    pub fn take(&self) -> Option<Slot> {
        let permit = self.0.clone().try_acquire_owned().ok()?;
        Some(Slot { _permit: permit })
    }

    /// How many are free now.
    pub fn free(&self) -> usize {
        self.0.available_permits()
    }
}

/// A file open for sending: its first `size` bytes, and the download slot it holds, if it holds
/// one, until the last body made of it is gone.
pub struct Source {
    file: File,
    size: u64,
    _slot: Option<Slot>,
}

impl Source {
    pub fn new(file: File, size: u64, slot: Option<Slot>) -> Arc<Source> {
        Arc::new(Source {
            file,
            size,
            _slot: slot,
        })
    }

    pub fn size(&self) -> u64 {
        self.size
    }
}

/// The body of an answer.
pub enum Body {
    /// bytes that are there already
    Full(Full<Bytes>),
    /// a file, read as it is sent
    File(FileBody),
}

/// A [`Source`] on its way out.
pub struct FileBody {
    source: Arc<Source>,
    sent: u64,
    deadline: Instant,
    /// when the next piece may go, by [`RATE`]
    next_at: Instant,
    /// the wait for it, while it is not time yet
    pause: Option<Pin<Box<Sleep>>>,
    reading: Option<JoinHandle<io::Result<Vec<u8>>>>,
}

impl Body {
    pub fn empty() -> Body {
        Body::Full(Full::new(Bytes::new()))
    }

    /// The whole of `source`, from now on: no faster than [`RATE`] and within [`DOWNLOAD_TIME`].
    pub fn file(source: Arc<Source>) -> Body {
        let now = Instant::now();
        Body::File(FileBody {
            source,
            sent: 0,
            deadline: now + DOWNLOAD_TIME,
            next_at: now,
            pause: None,
            reading: None,
        })
    }
}

impl From<Bytes> for Body {
    fn from(bytes: Bytes) -> Body {
        Body::Full(Full::new(bytes))
    }
}

/// The `want` bytes of `file` from `at` on; an error when the file ends before.
fn read_piece(file: &File, at: u64, want: usize) -> io::Result<Vec<u8>> {
    let mut piece = vec![0u8; want];
    let mut have = 0;
    while have < want {
        match file.read_at(&mut piece[have..], at + have as u64) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the file is shorter than it was when it was opened",
                ))
            }
            Ok(n) => have += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(piece)
}

impl FileBody {
    fn left(&self) -> u64 {
        self.source.size - self.sent
    }

    fn poll_piece(&mut self, cx: &mut Context<'_>) -> Poll<Option<io::Result<Frame<Bytes>>>> {
        if self.left() == 0 {
            return Poll::Ready(None);
        }
        let now = Instant::now();
        if now >= self.deadline {
            self.reading = None;
            return Poll::Ready(Some(Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the download took too long",
            ))));
        }
        // the next piece is read when it may go
        if self.reading.is_none() && now < self.next_at {
            let next_at = self.next_at;
            let pause = self
                .pause
                .get_or_insert_with(|| Box::pin(tokio::time::sleep_until(next_at)));
            if pause.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
        }
        self.pause = None;
        let (source, at) = (&self.source, self.sent);
        let want = self.left().min(PIECE as u64) as usize;
        let reading = self.reading.get_or_insert_with(|| {
            let source = source.clone();
            tokio::task::spawn_blocking(move || read_piece(&source.file, at, want))
        });
        let read = match Pin::new(reading).poll(cx) {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(read) => read,
        };
        self.reading = None;
        Poll::Ready(Some(match read {
            Ok(Ok(piece)) => {
                self.sent += piece.len() as u64;
                // The pace is kept through what a timer is late by (tokio's wake to the
                // millisecond). A reader slower than the rate is not slowed down, and saves
                // nothing up: the pace starts over from when it took this piece.
                let took = Duration::from_nanos(piece.len() as u64 * 1_000_000_000 / RATE);
                let now = Instant::now();
                let from = if now < self.next_at + took {
                    self.next_at
                } else {
                    now
                };
                self.next_at = from + took;
                Ok(Frame::data(Bytes::from(piece)))
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::other("reading the file did not end")),
        }))
    }
}

impl hyper::body::Body for Body {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, io::Error>>> {
        match self.get_mut() {
            Body::Full(full) => Pin::new(full)
                .poll_frame(cx)
                .map(|frame| frame.map(|f| f.map_err(|never| match never {}))),
            Body::File(file) => file.poll_piece(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        match self {
            Body::Full(full) => full.is_end_stream(),
            Body::File(file) => file.left() == 0,
        }
    }

    fn size_hint(&self) -> SizeHint {
        match self {
            Body::Full(full) => full.size_hint(),
            Body::File(file) => SizeHint::with_exact(file.left()),
        }
    }
}

/// The time by which the answer a connection sends has to be out: while that answer is a file,
/// the end of its [`DOWNLOAD_TIME`]. What makes the connection's answers sets it (server.rs) and
/// the connection's [`StallGuard`] keeps to it.
#[derive(Clone, Default)]
pub struct Deadline(Arc<Mutex<Option<Instant>>>);

impl Deadline {
    /// The time of the answer `body` from now on: its download's end for a file, none for bytes
    /// that are there already.
    pub fn follow(&self, body: &Body) {
        let at = match body {
            Body::File(file) => Some(file.deadline),
            Body::Full(_) => None,
        };
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = at;
    }

    /// No time from now on, as when a new request comes on the connection.
    pub fn clear(&self) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    fn passed(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some_and(|at| Instant::now() >= at)
    }
}

/// A connection whose writes may not wait longer than [`WRITE_STALL`] for the other side: a
/// write that is not taken for that long ends with an error, which makes hyper close the
/// connection and drop what it was sending. So does a write once the [`Deadline`] of the answer
/// has passed, however fast it would go. A connection that has nothing to write (an idle one, a
/// held poll) is not timed.
pub struct StallGuard<S> {
    stream: S,
    limit: Duration,
    deadline: Deadline,
    /// set while a write waits
    waiting: Option<Pin<Box<Sleep>>>,
}

impl<S> StallGuard<S> {
    pub fn new(stream: S, deadline: Deadline) -> StallGuard<S> {
        StallGuard {
            stream,
            limit: WRITE_STALL,
            deadline,
            waiting: None,
        }
    }

    /// The error for a write after the answer's time.
    fn late(&mut self) -> Option<io::Error> {
        if !self.deadline.passed() {
            return None;
        }
        self.waiting = None;
        Some(io::Error::new(
            io::ErrorKind::TimedOut,
            "the download took too long",
        ))
    }

    /// What a write came to: one that went on ends the wait, one that still waits is given up
    /// once it has waited the limit.
    fn watch<T>(
        &mut self,
        cx: &mut Context<'_>,
        polled: Poll<io::Result<T>>,
    ) -> Poll<io::Result<T>> {
        if polled.is_ready() {
            self.waiting = None;
            return polled;
        }
        let limit = self.limit;
        let waiting = self
            .waiting
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(limit)));
        match waiting.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(()) => {
                self.waiting = None;
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the other side took nothing for too long",
                )))
            }
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for StallGuard<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for StallGuard<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Some(late) = this.late() {
            return Poll::Ready(Err(late));
        }
        let polled = Pin::new(&mut this.stream).poll_write(cx, buf);
        this.watch(cx, polled)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if let Some(late) = this.late() {
            return Poll::Ready(Err(late));
        }
        let polled = Pin::new(&mut this.stream).poll_write_vectored(cx, bufs);
        this.watch(cx, polled)
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // no look at the deadline here: bytes go out through the two writes, which look at it,
        // and hyper flushes also while it reads the next request's head, after the answer ended
        let this = self.get_mut();
        let polled = Pin::new(&mut this.stream).poll_flush(cx);
        this.watch(cx, polled)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::BodyExt;
    use hyper::body::Body as _;
    use std::io::Write;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A file of `len` bytes that tell their place, and its content.
    fn file_of(len: usize) -> (File, Vec<u8>) {
        let content: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let path = std::env::temp_dir().join(format!(
            "hjv-api-body-{}-{}",
            std::process::id(),
            crate::util::hex(&crate::util::random_bytes::<6>())
        ));
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        file.write_all(&content).unwrap();
        std::fs::remove_file(&path).unwrap();
        (file, content)
    }

    async fn pieces(body: &mut Body) -> Vec<io::Result<Bytes>> {
        let mut out = Vec::new();
        while let Some(frame) = body.frame().await {
            let failed = frame.is_err();
            out.push(frame.map(|f| f.into_data().unwrap()));
            if failed {
                break;
            }
        }
        out
    }

    #[tokio::test]
    async fn a_file_goes_out_in_pieces() {
        // the design's 64 KiB at a time
        assert_eq!(PIECE, 65_536);
        let (file, content) = file_of(2 * PIECE + 4321);
        let mut body = Body::file(Source::new(file, content.len() as u64, None));
        assert_eq!(body.size_hint().exact(), Some(content.len() as u64));
        assert!(!body.is_end_stream());
        let got: Vec<Bytes> = pieces(&mut body)
            .await
            .into_iter()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            got.iter().map(Bytes::len).collect::<Vec<_>>(),
            [PIECE, PIECE, 4321]
        );
        assert_eq!(got.concat(), content);
        assert!(body.is_end_stream());
        assert_eq!(body.size_hint().exact(), Some(0));
        // only the first bytes it was opened for, also of a file that grew
        let (file, content) = file_of(PIECE + 10);
        let mut body = Body::file(Source::new(file, PIECE as u64 + 1, None));
        let sent: Vec<u8> = pieces(&mut body)
            .await
            .into_iter()
            .flat_map(Result::unwrap)
            .collect();
        assert_eq!(sent, content[..PIECE + 1]);
        // and nothing of an empty one
        let (file, _) = file_of(0);
        let mut body = Body::file(Source::new(file, 0, None));
        assert!(body.is_end_stream());
        assert!(body.frame().await.is_none());
    }

    #[tokio::test]
    async fn a_file_that_got_shorter_ends_the_download() {
        let (file, content) = file_of(PIECE + 100);
        let mut body = Body::file(Source::new(file, 2 * PIECE as u64, None));
        let got = pieces(&mut body).await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].as_ref().unwrap()[..], content[..PIECE]);
        assert_eq!(
            got[1].as_ref().unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_download_ends_after_its_time() {
        let (file, content) = file_of(3 * PIECE);
        let mut body = Body::file(Source::new(file, content.len() as u64, None));
        assert_eq!(
            body.frame()
                .await
                .unwrap()
                .unwrap()
                .into_data()
                .unwrap()
                .len(),
            PIECE
        );
        tokio::time::advance(DOWNLOAD_TIME - Duration::from_secs(1)).await;
        assert_eq!(
            body.frame()
                .await
                .unwrap()
                .unwrap()
                .into_data()
                .unwrap()
                .len(),
            PIECE
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        let late = body.frame().await.unwrap().unwrap_err();
        assert_eq!(late.kind(), io::ErrorKind::TimedOut);
    }

    #[tokio::test(start_paused = true)]
    async fn a_file_goes_out_no_faster_than_the_rate() {
        // 60 pieces: a second's worth and a little
        let (file, content) = file_of(60 * PIECE);
        let piece = Duration::from_nanos(PIECE as u64 * 1_000_000_000 / RATE);
        // (tokio's timers wake at the next whole millisecond)
        let woken =
            |due: Duration| Duration::from_millis(due.as_nanos().div_ceil(1_000_000) as u64);
        let mut body = Body::file(Source::new(file, content.len() as u64, None));
        let started = Instant::now();
        // the first piece at once, the i-th when i pieces' time has passed
        let mut got = Vec::new();
        for i in 0..60u32 {
            let frame = body.frame().await.unwrap().unwrap().into_data().unwrap();
            assert_eq!(started.elapsed(), woken(piece * i), "piece {i}");
            got.extend_from_slice(&frame);
        }
        assert!(body.frame().await.is_none());
        assert_eq!(got, content);
        let took = started.elapsed().as_secs_f64();
        assert!(
            content.len() as f64 / took <= RATE as f64 * 60.0 / 59.0,
            "{took}"
        );
        assert!((1.0..1.01).contains(&took), "{took}");

        // a reader slower than the rate is not slowed down further
        let (file, content) = file_of(5 * PIECE);
        let mut body = Body::file(Source::new(file, content.len() as u64, None));
        let started = Instant::now();
        for _ in 0..5 {
            tokio::time::sleep(Duration::from_secs(1)).await;
            body.frame().await.unwrap().unwrap();
        }
        assert_eq!(started.elapsed(), Duration::from_secs(5));
        // and it saves nothing up: after a long wait the pieces come at the rate again
        let (file, content) = file_of(12 * PIECE);
        let mut body = Body::file(Source::new(file, content.len() as u64, None));
        body.frame().await.unwrap().unwrap();
        tokio::time::sleep(Duration::from_secs(600)).await;
        let started = Instant::now();
        for _ in 0..11 {
            body.frame().await.unwrap().unwrap();
        }
        assert_eq!(started.elapsed(), woken(piece * 10));
    }

    #[tokio::test]
    async fn a_slot_is_held_until_the_download_is_gone() {
        let slots = Slots::new(2);
        assert_eq!(slots.free(), 2);
        let (file, _) = file_of(10);
        let source = Source::new(file, 10, slots.take());
        let other = slots.take().unwrap();
        assert_eq!(slots.free(), 0);
        assert!(slots.take().is_none());
        // a copy of the source keeps the slot, as the answer it is part of may be copied
        let mut body = Body::file(source.clone());
        drop(source);
        assert_eq!(slots.free(), 0);
        assert_eq!(pieces(&mut body).await.len(), 1);
        assert_eq!(slots.free(), 0);
        drop(body);
        assert_eq!(slots.free(), 1);
        drop(other);
        assert_eq!(slots.free(), 2);
        // bytes at once take none
        let full = Body::from(Bytes::from_static(b"{}"));
        assert_eq!(full.size_hint().exact(), Some(2));
        assert_eq!(slots.free(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_write_nobody_takes_is_given_up() {
        // a minute, as nginx gives a reader of its own (send_timeout)
        assert_eq!(WRITE_STALL, Duration::from_secs(60));
        let (near, mut far) = tokio::io::duplex(16);
        let mut guarded = StallGuard::new(near, Deadline::default());
        let started = Instant::now();
        // the first 16 bytes fit; the rest waits for a reader that never reads (a write that
        // waited for ever would end the test here, not hang it)
        let failed = tokio::time::timeout(2 * WRITE_STALL, guarded.write_all(&[7u8; 64]))
            .await
            .expect("the write was given up")
            .unwrap_err();
        assert_eq!(failed.kind(), io::ErrorKind::TimedOut);
        assert_eq!(started.elapsed(), WRITE_STALL);
        let mut got = [0u8; 16];
        far.read_exact(&mut got).await.unwrap();
        assert_eq!(got, [7u8; 16]);
        // the wait starts over with the next write
        let started = Instant::now();
        guarded.write_all(&[8u8; 16]).await.unwrap();
        let again = tokio::time::timeout(2 * WRITE_STALL, guarded.write_all(&[9u8; 1])).await;
        assert!(again.expect("the write was given up").is_err());
        assert_eq!(started.elapsed(), WRITE_STALL);
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_reader_is_not_given_up() {
        let (near, mut far) = tokio::io::duplex(16);
        let mut guarded = StallGuard::new(near, Deadline::default());
        let reader = tokio::spawn(async move {
            let mut got = Vec::new();
            let mut buf = [0u8; 8];
            // eight bytes every 50 seconds: slow, but never a minute without taking any
            loop {
                tokio::time::sleep(WRITE_STALL - Duration::from_secs(10)).await;
                match far.read(&mut buf).await.unwrap() {
                    0 => return got,
                    n => got.extend_from_slice(&buf[..n]),
                }
            }
        });
        let started = Instant::now();
        let sent: Vec<u8> = (0..200u8).collect();
        guarded.write_all(&sent).await.unwrap();
        guarded.flush().await.unwrap();
        assert!(started.elapsed() > 10 * WRITE_STALL);
        guarded.shutdown().await.unwrap();
        drop(guarded);
        assert_eq!(reader.await.unwrap(), sent);
    }

    #[tokio::test(start_paused = true)]
    async fn a_download_past_its_time_ends_however_its_reader_reads() {
        let (near, mut far) = tokio::io::duplex(16);
        let deadline = Deadline::default();
        let mut guarded = StallGuard::new(near, deadline.clone());
        // the answer is a file: its hour runs from when its body was made
        let (file, _) = file_of(10);
        let started = Instant::now();
        deadline.follow(&Body::file(Source::new(file, 10, None)));
        let reader = tokio::spawn(async move {
            // eight bytes every 50 seconds: never a minute without taking any, and so slow that
            // hyper would not ask a body for its next piece within the hour
            let mut got = 0;
            let mut buf = [0u8; 8];
            loop {
                tokio::time::sleep(WRITE_STALL - Duration::from_secs(10)).await;
                match far.read(&mut buf).await {
                    Ok(0) | Err(_) => return got,
                    Ok(n) => got += n,
                }
            }
        });
        let failed = guarded.write_all(&[7u8; 4096]).await.unwrap_err();
        assert_eq!(failed.kind(), io::ErrorKind::TimedOut);
        let took = started.elapsed();
        assert!(took >= DOWNLOAD_TIME, "{took:?}");
        assert!(took < DOWNLOAD_TIME + WRITE_STALL, "{took:?}");
        // nothing more goes: the next write fails too
        assert!(guarded.write_all(&[7u8; 16]).await.is_err());
        drop(guarded);
        let got = reader.await.unwrap();
        assert!(got < 2048, "{got}");

        // an answer of bytes that are there has no time, and nor has a new request
        let (near, mut far) = tokio::io::duplex(16);
        let mut guarded = StallGuard::new(near, deadline.clone());
        assert!(deadline.passed());
        deadline.follow(&Body::empty());
        assert!(!deadline.passed());
        guarded.write_all(b"ok").await.unwrap();
        let (file, _) = file_of(10);
        deadline.follow(&Body::file(Source::new(file, 10, None)));
        tokio::time::sleep(DOWNLOAD_TIME).await;
        assert!(deadline.passed());
        deadline.clear();
        guarded.write_all(b"ok").await.unwrap();
        let mut got = [0u8; 4];
        far.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"okok");
    }

    #[tokio::test(start_paused = true)]
    async fn reads_and_idle_connections_are_not_timed() {
        let (near, mut far) = tokio::io::duplex(16);
        let mut guarded = StallGuard::new(near, Deadline::default());
        let writer = tokio::spawn(async move {
            tokio::time::sleep(10 * WRITE_STALL).await;
            far.write_all(b"late").await.unwrap();
            far
        });
        let mut got = [0u8; 4];
        guarded.read_exact(&mut got).await.unwrap();
        assert_eq!(&got, b"late");
        let mut far = writer.await.unwrap();
        guarded.write_all(b"ok").await.unwrap();
        far.read_exact(&mut got[..2]).await.unwrap();
        assert_eq!(&got[..2], b"ok");
    }
}
