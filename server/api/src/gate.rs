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
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

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
    let kept = *state.gate.0.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, mode)) = kept {
        if at.elapsed() < CACHE_FOR {
            return mode;
        }
    }
    let mode = hbbs::access_mode(state.config.hbbs_console)
        .await
        .unwrap_or(UNKNOWN);
    *state.gate.0.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), mode));
    mode
}
