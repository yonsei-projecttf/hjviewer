// SPDX-License-Identifier: AGPL-3.0-only

//! Upkeep (design v2 §6, §9-3, §13-1). Every hour: delete what is past its keeping time (tokens
//! idle 90 days or a year old, accounts unused for a year unless still allowlisted with a PC in
//! use, each with its allowlist entry unless blocked, entries without an account a year after
//! they came on the list unless blocked, records older than six months, spent nonces, and limit
//! counters past the longest window),
//! release the PCs not seen for 180 days (`db::release_binding`, which leaves the tombstone that
//! gets them a signed `revoked`: design v3 §4-3), revoke the account keys no valid token is left
//! for (their logins expired, or their tokens went without them: design v3 §1-8, §14 question 6),
//! delete account keys 30 days after they were revoked, tombstones after 180 days and the PCs'
//! reports of key use and refusals after 30, and truncate the WAL. Every day: ask hbbs about each
//! device in an account (a device counts as seen when hbbs has it with the stored key; logins,
//! list requests and syncs from it count as well), mark the ones hbbs holds another key for as
//! needs-relogin (for the PCs that do not sync; a sync does it at once: design v3 §4-6), and write
//! a summary of the day's logins to the journal, with counts only. Every hour too: delete the
//! copies of the database that a backup left when the service stopped in the middle of sending
//! one (backup.rs). Every five seconds: drop the shown symbols of the support codes that are no
//! longer open and tell the waiting room of those over (support.rs `SupportCodes::sweep`), and
//! move the wake jobs along (wake.rs `tick`); every hour also
//! delete the relays' pending entries past 24 hours, pre-approvals past 7 days, spent relay
//! nonces, expired commands and wake jobs past six months.

use crate::{
    db,
    hbbs::{self, HbbsError, Lookup},
    state::AppState,
    util::{now, DAY},
};
use std::{future::Future, sync::Arc, time::Duration};
use tokio::time::{interval_at, Instant, MissedTickBehavior};

/// How often the wake jobs move along (wake.rs `tick`: design v3 §8-4 asks hbbs every five
/// seconds after the packets).
const WAKE_EVERY: Duration = Duration::from_secs(5);
const FIRST_PURGE: Duration = Duration::from_secs(60);
const PURGE_EVERY: Duration = Duration::from_secs(3600);
const FIRST_DAILY: Duration = Duration::from_secs(300);
const DAILY_EVERY: Duration = Duration::from_secs(24 * 3600);

/// Runs the upkeep until `shutdown` completes.
pub async fn run(state: Arc<AppState>, shutdown: impl Future<Output = ()>) {
    let mut purge = interval_at(Instant::now() + FIRST_PURGE, PURGE_EVERY);
    let mut daily = interval_at(Instant::now() + FIRST_DAILY, DAILY_EVERY);
    let mut wake = interval_at(Instant::now() + WAKE_EVERY, WAKE_EVERY);
    purge.set_missed_tick_behavior(MissedTickBehavior::Delay);
    daily.set_missed_tick_behavior(MissedTickBehavior::Delay);
    wake.set_missed_tick_behavior(MissedTickBehavior::Delay);
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            () = &mut shutdown => break,
            _ = wake.tick() => {
                state.support.sweep(now());
                crate::wake::tick(&state).await;
            }
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
    state.request_retries.purge();
    state.pending.purge();
    state.purge_notices();
    state.key_calls.purge();
    state.lists.purge();
    state.key_checks.purge();
    state.relays.purge();
    match crate::backup::remove_stale_copies(state).await {
        Ok(0) => {}
        Ok(n) => {
            log_notice!("upkeep: removed {n} copies of the database left by broken-off backups")
        }
        Err(e) => log_warn!("upkeep: {e}"),
    }
    let retention = state.retention();
    let at = now();
    match state.db.call(move |c| db::purge(c, at, &retention)).await {
        Ok(p) => {
            if p.devices + p.accounts > 0 {
                log_notice!(
                    "upkeep: released {} PCs not seen for long and deleted {} unused accounts",
                    p.devices,
                    p.accounts
                );
            }
            if p.entries > 0 {
                log_notice!(
                    "upkeep: deleted {} allowlist entries without an account, on the list for the account time",
                    p.entries
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
    /// devices hbbs holds another key for, and of them the ones marked needs-relogin now
    pub key_changed: usize,
    pub marked: usize,
    /// needs-relogin devices hbbs holds the stored key for again
    pub cleared: usize,
    /// devices whose answer was in another format (a key that is not 32 bytes): skipped
    pub malformed: usize,
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
                    let (id, stored, at) =
                        (device.device_id.clone(), device.public_key.clone(), now());
                    let marked = state
                        .db
                        .call(move |c| {
                            db::in_tx(c, |tx| db::mark_key_changed(tx, &id, &stored, at))
                        })
                        .await;
                    match marked {
                        Ok(true) => round.marked += 1,
                        Ok(false) => {}
                        Err(e) => log_err!("upkeep: {e}"),
                    }
                    continue;
                }
                if device.key_changed_at.is_some() {
                    let id = device.device_id.clone();
                    match state
                        .db
                        .call(move |c| db::in_tx(c, |tx| db::clear_key_changed(tx, &id)))
                        .await
                    {
                        Ok(true) => round.cleared += 1,
                        Ok(false) => {}
                        Err(e) => log_err!("upkeep: {e}"),
                    }
                }
                if let Some(age) = seen_secs_ago {
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
            // One device's answer: hbbs signs and returns whatever key a client registered,
            // also one that is not 32 bytes. The other devices are still checked, so that a
            // client that registers such a key for its own ID cannot keep hbbs's view of the
            // devices after it (their keys, when they were seen) from hjv-api.
            Err(HbbsError::Malformed) => round.malformed += 1,
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
    if round.malformed > 0 {
        log_warn!(
            "upkeep: hbbs answered hjv-pk in an unknown format for {} devices; skipped",
            round.malformed
        );
    }
    if round.key_changed > 0 {
        log_notice!(
            "upkeep: {} devices registered with hbbs under a new key ({} newly needs-relogin); they count as seen after a login on them",
            round.key_changed,
            round.marked
        );
    }
    if round.cleared > 0 {
        log_notice!(
            "upkeep: hbbs holds the stored key again for {} needs-relogin devices",
            round.cleared
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
