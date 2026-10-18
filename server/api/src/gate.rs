// SPDX-License-Identifier: AGPL-3.0-only

//! The access check's mode, for the apps (docs/access-tickets.md 3-6): `GET /api/hjv/access`
//! answers `{"mode":"<mode>"}` with the mode hbbs's console names for `hjv-access` (server patch
//! 0006: `off`, `count`, `enforce` or `closed`), or `unknown` when hbbs cannot be asked or does
//! not know the command. hbbs's counts stay out. No token: an app asks before any login, to say
//! ahead of a refusal that the server lets in only staff, and to word what a logout or taking a
//! PC out of the list does there. hbbs is asked at most once every CACHE_FOR; the answers within
//! it are the same.

use crate::{api::Reply, hbbs, state::AppState};
use serde_json::json;
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Mutex;

pub const PATH: &str = "/api/hjv/access";
/// How long one answer of hbbs serves.
const CACHE_FOR: Duration = Duration::from_secs(30);
/// The mode when hbbs cannot tell.
pub const UNKNOWN: &str = "unknown";

/// hbbs's last answer, and when it came.
#[derive(Default)]
pub struct GateMode(Mutex<Option<(Instant, &'static str)>>);

pub async fn handle(state: &Arc<AppState>) -> Reply {
    Reply::ok(&json!({ "mode": mode(state).await }))
}

/// The mode, asked of hbbs at most once every CACHE_FOR.
pub async fn mode(state: &AppState) -> &'static str {
    state.gate.read(state.config.hbbs_console).await
}

impl GateMode {
    async fn read(&self, console: SocketAddr) -> &'static str {
        // Keep one refresh in flight. Other requests wait without occupying a runtime thread
        // and reuse its answer, including `unknown`. Cancellation drops the guard, so the next
        // request can retry; it never leaves a permanent refresh marker behind.
        let mut kept = self.0.lock().await;
        if let Some((at, mode)) = *kept {
            if at.elapsed() < CACHE_FOR {
                return mode;
            }
        }
        let mode = hbbs::access_mode(console).await.unwrap_or(UNKNOWN);
        *kept = Some((Instant::now(), mode));
        mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Semaphore,
        task::JoinSet,
        time::timeout,
    };

    async fn console(reply: &'static str) -> (SocketAddr, Arc<AtomicUsize>, Arc<Semaphore>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let answers = Arc::new(Semaphore::new(0));
        let (count, permits) = (calls.clone(), answers.clone());
        tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let (count, permits) = (count.clone(), permits.clone());
                tokio::spawn(async move {
                    let mut command = [0; 10];
                    stream.read_exact(&mut command).await.unwrap();
                    assert_eq!(&command, hbbs::ACCESS_COMMAND.as_bytes());
                    count.fetch_add(1, Ordering::SeqCst);
                    permits.acquire().await.unwrap().forget();
                    let _ = stream.write_all(reply.as_bytes()).await;
                });
            }
        });
        (addr, calls, answers)
    }

    async fn reached(counter: &AtomicUsize, n: usize) {
        timeout(Duration::from_secs(1), async {
            while counter.load(Ordering::SeqCst) < n {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("calls did not reach the console");
    }

    async fn burst(cache: &Arc<GateMode>, addr: SocketAddr) -> JoinSet<&'static str> {
        let mut requests = JoinSet::new();
        let entered = Arc::new(AtomicUsize::new(0));
        for _ in 0..32 {
            let (cache, entered) = (cache.clone(), entered.clone());
            requests.spawn(async move {
                entered.fetch_add(1, Ordering::SeqCst);
                cache.read(addr).await
            });
        }
        reached(&entered, 32).await;
        requests
    }

    #[tokio::test]
    async fn concurrent_cache_misses_share_one_lookup_and_refresh_after_expiry() {
        let (addr, calls, answers) = console("HJVACCESS1 mode=enforce\n").await;
        let cache = Arc::new(GateMode::default());
        for round in 1..=2 {
            let mut requests = burst(&cache, addr).await;
            reached(&calls, round).await;
            answers.add_permits(32);
            while let Some(result) = requests.join_next().await {
                assert_eq!(result.unwrap(), "enforce");
            }
            assert_eq!(calls.load(Ordering::SeqCst), round);
            assert_eq!(cache.read(addr).await, "enforce");
            assert_eq!(calls.load(Ordering::SeqCst), round);
            // The stale answer must be replaced, even if its text differs from hbbs's mode.
            *cache.0.lock().await = Some((Instant::now() - CACHE_FOR, "off"));
        }
    }

    #[tokio::test]
    async fn concurrent_lookup_failures_share_the_unknown_cache() {
        let (addr, calls, answers) = console("").await;
        let cache = Arc::new(GateMode::default());
        let mut requests = burst(&cache, addr).await;
        reached(&calls, 1).await;
        answers.add_permits(32);
        while let Some(result) = requests.join_next().await {
            assert_eq!(result.unwrap(), UNKNOWN);
        }
        assert_eq!(cache.read(addr).await, UNKNOWN);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_refresh_does_not_block_the_next_request() {
        let (addr, calls, answers) = console("HJVACCESS1 mode=closed\n").await;
        let cache = Arc::new(GateMode::default());
        let first = {
            let cache = cache.clone();
            tokio::spawn(async move { cache.read(addr).await })
        };
        reached(&calls, 1).await;
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        answers.add_permits(2);
        assert_eq!(
            timeout(Duration::from_secs(1), cache.read(addr))
                .await
                .unwrap(),
            "closed"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(cache.read(addr).await, "closed");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
