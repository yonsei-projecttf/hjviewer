// SPDX-License-Identifier: AGPL-3.0-only

//! Upkeep (design v2 §6, §9-3, §13-1). Every hour: delete what is past its keeping time (tokens
//! idle 90 days or a year old, devices not seen for 180 days, accounts unused for a year unless
//! still allowlisted with a PC in use, records older than six months, spent nonces, and limit
//! counters past the longest window), revoke the account keys no valid token is left for (their
//! logins expired, or were logged out without the key's signature: design v3 §4-7, §14 question
//! 6), delete account keys 30 days after they were revoked, and truncate the WAL. Every day: ask
//! hbbs when each device in an account last registered (a device counts as seen when hbbs has it
//! with the stored key; logins and list requests from it count as well), and write a summary of
//! the day's logins to the journal, with counts only.

use crate::{
    db,
    hbbs::{self, HbbsError, Lookup},
    state::AppState,
    util::{now, DAY},
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::{interval_at, Instant, MissedTickBehavior};

const FIRST_PURGE: Duration = Duration::from_secs(60);
const PURGE_EVERY: Duration = Duration::from_secs(3600);
const FIRST_DAILY: Duration = Duration::from_secs(300);
const DAILY_EVERY: Duration = Duration::from_secs(24 * 3600);

/// Runs the upkeep until `shutdown` completes.
pub async fn run(state: Arc<AppState>, shutdown: impl Future<Output = ()>) {
    let mut purge = interval_at(Instant::now() + FIRST_PURGE, PURGE_EVERY);
    let mut daily = interval_at(Instant::now() + FIRST_DAILY, DAILY_EVERY);
    purge.set_missed_tick_behavior(MissedTickBehavior::Delay);
    daily.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            _ = purge.tick() => {
                purge_now(&state).await;
            }
            _ = daily.tick() => {
                refresh_seen(&state).await;
                summary(&state).await;
            }
        }
    }
}

/// Deletes what is past its keeping time.
pub async fn purge_now(state: &Arc<AppState>) -> Option<db::Purged> {
    state.retries.purge();
    state.pending.purge();
    state.purge_notices();
    state.key_calls.purge();
    let retention = state.retention();
    let at = now();
    match state.db.call(move |c| db::purge(c, at, &retention)).await {
        Ok(p) => {
            if p.devices + p.accounts > 0 {
                log_notice!(
                    "upkeep: removed {} devices not seen for long and {} unused accounts",
                    p.devices,
                    p.accounts
                );
            }
            if p.keys_revoked > 0 {
                log_notice!(
                    "upkeep: revoked {} account keys whose logins ended",
                    p.keys_revoked
                );
            }
            Some(p)
        }
        Err(e) => {
            log_err!("upkeep: {e}");
            None
        }
    }
}

/// Counts of the daily hbbs round.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SeenRound {
    pub asked: usize,
    pub seen: usize,
    pub key_changed: usize,
}

/// Asks hbbs about every device in an account and records the ones it has seen.
pub async fn refresh_seen(state: &Arc<AppState>) -> SeenRound {
    let mut round = SeenRound::default();
    let Some(server_key) = state.config.hbbs_pubkey else {
        log_warn!("upkeep: HJV_API_HBBS_PUBKEY is not set; devices not checked with hbbs");
        return round;
    };
    let devices = match state.db.call(|c| db::all_devices(c)).await {
        Ok(d) => d,
        Err(e) => {
            log_err!("upkeep: {e}");
            return round;
        }
    };
    for device in devices {
        round.asked += 1;
        match hbbs::lookup(
            state.config.hbbs_console,
            Some(&server_key),
            &device.device_id,
        )
        .await
        {
            Ok(Lookup::Known {
                public_key,
                seen_secs_ago,
            }) => {
                if public_key.as_slice() != device.public_key {
                    round.key_changed += 1;
                } else if let Some(age) = seen_secs_ago {
                    // hbbs.rs bounds the age to ten years; saturating all the same
                    let age = i64::try_from(age).unwrap_or(i64::MAX);
                    let (id, seen_at) = (device.device_id.clone(), now().saturating_sub(age));
                    let written = state
                        .db
                        .call(move |c| db::seen_by_hbbs(c, &id, &public_key, seen_at))
                        .await;
                    match written {
                        Ok(_) => round.seen += 1,
                        Err(e) => log_err!("upkeep: {e}"),
                    }
                }
            }
            Ok(Lookup::Unknown) | Err(HbbsError::BadId) => {}
            Err(e @ HbbsError::BadSignature) => {
                log_err!("upkeep: {e}; devices not checked today");
                break;
            }
            Err(e) => {
                log_warn!("upkeep: {e}; devices not checked today");
                break;
            }
        }
    }
    if round.key_changed > 0 {
        log_notice!(
            "upkeep: {} devices registered with hbbs under a new key; they count as seen after their next login",
            round.key_changed
        );
    }
    round
}

/// The day's logins in the journal, counts only (design v2 §9-2 step 5: "하루 요약").
async fn summary(state: &Arc<AppState>) {
    let at = now();
    let rows = state
        .db
        .call(move |c| {
            let mut stmt = c.prepare(
                "SELECT kind || ' ' || result, COUNT(*) FROM events WHERE at > ?1
                 GROUP BY kind, result ORDER BY kind, result",
            )?;
            let rows = stmt
                .query_map([at - DAY], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .await;
    let skipped = state.failure_records.take_skipped();
    match rows {
        Ok(rows) if rows.is_empty() && skipped == 0 => {
            log_info!("daily: no logins in the last day")
        }
        Ok(rows) => {
            let mut parts: Vec<String> = rows.iter().map(|(k, n)| format!("{k}={n}")).collect();
            if skipped > 0 {
                parts.push(format!("failed logins not recorded={skipped}"));
            }
            log_notice!("daily: {}", parts.join(", "));
        }
        Err(e) => log_err!("daily: {e}"),
    }
}
