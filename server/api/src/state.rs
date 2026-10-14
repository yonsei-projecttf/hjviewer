// SPDX-License-Identifier: AGPL-3.0-only

//! What the request handlers share.

use crate::{
    api::Reply,
    check::OnlineSince,
    config::{Config, Secrets},
    db::{self, Db, Record},
    keys::CallLimiter,
    limits::{Admit, Breaker, GlobalLimiter, Latency, RecordBudget, FAILURE_RECORDS_PER_MINUTE},
    log::Tagger,
    login::{LoginAnswer, RetryCache},
    oplog::OpLog,
    pcsync::{KeyChecks, ListCache},
    relay::RelayHub,
    seal::SealKeys,
    sso::SsoClient,
    tfa::{PendingStore, SecondFactor},
    trust::ListKey,
    util::DAY,
    wake::CommandKey,
};
use rusqlite::Connection;
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::Semaphore;

const MAX_NOTICES: usize = 10_000;

pub struct AppState {
    pub config: Config,
    pub db: Db,
    pub seal: SealKeys,
    pub tags: Tagger,
    pub sso: SsoClient,
    pub second_factor: Arc<dyn SecondFactor>,
    /// SSO calls of all accounts (v2 §9-2 step 7)
    pub sso_calls: GlobalLimiter,
    /// the pause of SSO calls while the SSO refuses this server or keeps failing
    pub sso_breaker: Breaker,
    /// how long SSO calls took lately (the failure delay follows it)
    pub sso_latency: Latency,
    /// failure records written per minute
    pub failure_records: RecordBudget,
    /// answers kept for a retry of the same login body (step 3)
    pub retries: RetryCache<LoginAnswer>,
    /// answers kept for a retry of the very same signed account request that changes something
    /// (api.rs `authenticate`)
    pub request_retries: RetryCache<Reply>,
    /// logins waiting for a second-factor code (step 8)
    pub pending: PendingStore,
    /// logins handled at once
    pub login_slots: Arc<Semaphore>,
    /// SSO messages already written to the journal
    pub sso_messages: Mutex<HashSet<String>>,
    /// calls of the account-key paths per account (keys.rs)
    pub key_calls: CallLimiter,
    /// the list key that signs the PCs' trust lists (credential hjviewer-api-list-key, number
    /// HJV_API_LIST_KID)
    pub list_key: ListKey,
    /// the last trust list of each PC, which a repeat within ten seconds gets again (pcsync.rs)
    pub lists: ListCache,
    /// when hbbs was last asked about a PC's key after a sync its stored key did not sign
    pub key_checks: KeyChecks,
    /// since when hbbs has seen each PC holding its binding on, for the five-minute check
    /// (check.rs)
    pub online: OnlineSince,
    /// the command key that signs wake commands (credential hjviewer-api-command-key, number
    /// HJV_API_COMMAND_KID), when there is one (wake.rs)
    pub command_key: Option<CommandKey>,
    /// the relays' held polls and per-relay limits (relay.rs)
    pub relays: RelayHub,
    /// the operator record and the service's own user ID, from when the admin socket is served
    /// (admin.rs): where overview.rs writes each look of a super operator
    pub operator_record: OnceLock<(Arc<OpLog>, u32)>,
    /// limits already reported, so that hammering a full limit writes no more records or lines
    notices: Mutex<HashMap<String, Instant>>,
}

impl AppState {
    /// Builds the state; `second_factor` replaces the one the settings name (tests).
    pub fn new(
        config: Config,
        secrets: Secrets,
        conn: Connection,
        second_factor: Option<Arc<dyn SecondFactor>>,
    ) -> Result<Arc<AppState>, String> {
        let second_factor = match second_factor {
            Some(f) => f,
            None => crate::tfa::from_setting(&config.second_factor)?,
        };
        let unfinished = crate::limits::drop_unfinished(&conn).map_err(|e| e.to_string())?;
        if unfinished > 0 {
            log_info!("{unfinished} login attempts were still running at the last stop");
        }
        let Secrets {
            sso_basic,
            sso_api_uuid,
            seal_keys,
            log_key,
            list_key,
            command_key,
        } = secrets;
        Ok(Arc::new(AppState {
            list_key: ListKey::new(config.list_kid, &list_key),
            command_key: command_key.map(|seed| CommandKey::new(config.command_kid, &seed)),
            relays: RelayHub::default(),
            operator_record: OnceLock::new(),
            lists: ListCache::default(),
            key_checks: KeyChecks::default(),
            online: OnlineSince::default(),
            sso: SsoClient::new(&config.sso, sso_basic, sso_api_uuid)?,
            seal: SealKeys::new(&seal_keys),
            tags: Tagger::new(&log_key),
            db: Db::new(conn),
            second_factor,
            sso_calls: GlobalLimiter::new(config.limit_sso),
            sso_breaker: Breaker::default(),
            sso_latency: Latency::default(),
            failure_records: RecordBudget::default(),
            retries: RetryCache::new(config.retry_window_secs),
            request_retries: RetryCache::new(config.retry_window_secs),
            pending: PendingStore::default(),
            login_slots: Arc::new(Semaphore::new(config.max_logins_in_flight)),
            sso_messages: Mutex::new(HashSet::new()),
            key_calls: CallLimiter::default(),
            notices: Mutex::new(HashMap::new()),
            config,
        }))
    }

    /// Whether `key` was not reported within `window`: true once per window and key.
    pub fn first_notice(&self, key: &str, window: Duration) -> bool {
        let mut notices = self.notices.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        if notices.len() >= MAX_NOTICES {
            notices.retain(|_, at| now.duration_since(*at) < window);
            if notices.len() >= MAX_NOTICES {
                return false;
            }
        }
        match notices.get(key) {
            Some(at) if now.duration_since(*at) < window => false,
            _ => {
                notices.insert(key.to_owned(), now);
                true
            }
        }
    }

    /// Forgets notices older than a day.
    pub fn purge_notices(&self) {
        let mut notices = self.notices.lock().unwrap_or_else(|e| e.into_inner());
        notices.retain(|_, at| at.elapsed() < Duration::from_secs(86_400));
    }

    /// The keeping times of the settings.
    pub fn retention(&self) -> db::Retention {
        db::Retention {
            token_idle: self.config.token_idle_secs,
            token_max: self.config.token_max_secs,
            account: self.config.account_retention_secs,
            device_unseen: self.config.device_unseen_secs,
            events: self.config.event_retention_secs,
            attempts: self.config.longest_window().max(DAY),
        }
    }

    /// Whether the record of a failed login may be written (limits.rs: at most
    /// FAILURE_RECORDS_PER_MINUTE a minute); the journal hears once a minute of those that are not.
    pub fn admit_failure_record(&self) -> bool {
        match self.failure_records.admit() {
            Admit::Write => true,
            Admit::Skip { first } => {
                if first {
                    log_warn!(
                        "login: more than {FAILURE_RECORDS_PER_MINUTE} failed logins this minute; the rest are counted in the daily summary, not recorded"
                    );
                }
                false
            }
        }
    }

    /// Writes a record; a failure to write is logged, not passed on.
    pub async fn record(
        &self,
        kind: &'static str,
        account_tag: Option<String>,
        device_tag: Option<String>,
        ip: Option<String>,
        result: &'static str,
    ) {
        let record = Record {
            at: crate::util::now(),
            kind,
            account_tag,
            device_tag,
            ip,
            result,
        };
        if let Err(e) = self.db.call(move |c| db::insert_record(c, &record)).await {
            log_err!("cannot write a record: {e}");
        }
    }
}
