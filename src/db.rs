//! Event persistence and querying
use crate::config::Settings;
use crate::error::{Error, Result};
use crate::event::Event;
use crate::nip05;
use crate::notice::Notice;
use crate::repo::sqlite::SqliteRepo;
use crate::repo::{Nip05Repo, NostrRepo};
use governor::clock::Clock;
use governor::{Quota, RateLimiter};
use sqlx::pool::PoolOptions;
use sqlx::SqlitePool;
use std::path::Path;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tokio::task;
use tracing::{debug, info, trace, warn};

/// Events submitted from a client, with a return channel for notices
pub struct SubmittedEvent {
    pub event: Event,
    pub notice_tx: tokio::sync::mpsc::Sender<Notice>,
}

/// Database file
pub const DB_FILE: &str = "nostr.db";
/// How many persisted events before optimization is triggered
pub const EVENT_COUNT_OPTIMIZE_TRIGGER: usize = 500;

/// Build a database connection pool.
/// # Panics
///
/// Will panic if the pool could not be created.
pub async fn build_pool(
    name: &str,
    settings: &Settings,
    min_size: u32,
    max_size: u32,
    wait_for_db: bool,
) -> Result<SqlitePool> {
    let db_dir = &settings.database.data_directory;
    let full_path = Path::new(db_dir).join(DB_FILE);

    // small hack; if the database doesn't exist yet, that means the
    // writer thread hasn't finished.  Give it a chance to work.  This
    // is only an issue with the first time we run.
    if !settings.database.in_memory {
        while !full_path.exists() && wait_for_db {
            debug!("Database reader pool is waiting on the database to be created...");
            async_std::task::sleep(Duration::from_millis(500)).await;
        }
    }

    let db_str = if settings.database.in_memory {
        "sqlite::memory".to_owned()
    } else {
        format!("sqlite:{}?mode=rwc", full_path.display())
    };
    let pool: SqlitePool = PoolOptions::new()
        .max_connections(max_size)
        .min_connections(min_size)
        .idle_timeout(Duration::from_secs(60))
        .connect(db_str.as_str())
        .await?;

    // todo!("Init DB");

    info!(
        "Built a connection pool {:?} (min={}, max={})",
        name, min_size, max_size
    );
    Ok(pool)
}

/// Spawn a database writer that persists events to the SQLite store.
pub async fn db_writer(
    mut repo: impl NostrRepo,
    mut nip05_repo: impl Nip05Repo,
    settings: Settings,
    mut event_rx: tokio::sync::mpsc::Receiver<SubmittedEvent>,
    bcast_tx: tokio::sync::broadcast::Sender<Event>,
    metadata_tx: tokio::sync::broadcast::Sender<Event>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    // are we performing NIP-05 checking?
    let nip05_active = settings.verified_users.is_active();
    // are we requriing NIP-05 user verification?
    let nip05_enabled = settings.verified_users.is_enabled();

    // Make a copy of the whitelist
    let whitelist = &settings.authorization.pubkey_whitelist.clone();

    // get rate limit settings
    let rps_setting = settings.limits.messages_per_sec;
    let mut most_recent_rate_limit = Instant::now();
    let mut lim_opt = None;
    // Keep rough track of events so we can run optimize eventually.
    let mut optimize_counter: usize = 0;
    let clock = governor::clock::QuantaClock::default();
    if let Some(rps) = rps_setting {
        if rps > 0 {
            info!("Enabling rate limits for event creation ({}/sec)", rps);
            let quota = core::num::NonZeroU32::new(rps * 60).unwrap();
            lim_opt = Some(RateLimiter::direct(Quota::per_minute(quota)));
        }
    }

    loop {
        if shutdown.try_recv().is_ok() {
            info!("shutting down database writer");
            break;
        }
        // call blocking read on channel
        let next_event = event_rx.blocking_recv();
        // if the channel has closed, we will never get work
        if next_event.is_none() {
            break;
        }
        // track if an event write occurred; this is used to
        // update the rate limiter
        let mut event_write = false;
        let subm_event = next_event.unwrap();
        let event = subm_event.event;
        let notice_tx = subm_event.notice_tx;
        // check if this event is authorized.
        if let Some(allowed_addrs) = whitelist {
            // TODO: incorporate delegated pubkeys
            // if the event address is not in allowed_addrs.
            if !allowed_addrs.contains(&event.pubkey) {
                info!(
                    "Rejecting event {}, unauthorized author",
                    event.get_event_id_prefix()
                );
                notice_tx
                    .try_send(Notice::blocked(
                        event.id,
                        "pubkey is not allowed to publish to this relay",
                    ))
                    .ok();
                continue;
            }
        }

        // send any metadata events to the NIP-05 verifier
        if nip05_active && event.is_kind_metadata() {
            // we are sending this prior to even deciding if we
            // persist it.  this allows the nip05 module to
            // inspect it, update if necessary, or persist a new
            // event and broadcast it itself.
            metadata_tx.send(event.clone()).ok();
        }

        // check for  NIP-05 verification
        if nip05_enabled {
            match nip05_repo.get_latest_user_verification(&event.pubkey).await {
                Ok(uv) => {
                    if uv.is_valid(&settings.verified_users) {
                        info!(
                            "new event from verified author ({:?},{:?})",
                            uv.name.to_string(),
                            event.get_author_prefix()
                        );
                    } else {
                        info!("rejecting event, author ({:?} / {:?}) verification invalid (expired/wrong domain)",
                                  uv.name.to_string(),
                                  event.get_author_prefix()
                            );
                        notice_tx
                            .try_send(Notice::blocked(
                                event.id,
                                "NIP-05 verification is no longer valid (expired/wrong domain)",
                            ))
                            .ok();
                        continue;
                    }
                }
                Err(_RowNotFound) => {
                    debug!(
                        "no verification records found for pubkey: {:?}",
                        event.get_author_prefix()
                    );
                    notice_tx
                        .try_send(Notice::blocked(
                            event.id,
                            "NIP-05 verification needed to publish events",
                        ))
                        .ok();
                    continue;
                }
                Err(e) => {
                    warn!("checking nip05 verification status failed: {:?}", e);
                    continue;
                }
            }
        }

        // TODO: cache recent list of authors to remove a DB call.
        let start = Instant::now();
        if event.kind >= 20000 && event.kind < 30000 {
            bcast_tx.send(event.clone()).ok();
            info!(
                "published ephemeral event: {:?} from: {:?} in: {:?}",
                event.get_event_id_prefix(),
                event.get_author_prefix(),
                start.elapsed()
            );
            event_write = true
        } else {
            match repo.write_event(&event).await {
                Ok(updated) => {
                    if updated == 0 {
                        trace!("ignoring duplicate or deleted event");
                        notice_tx.try_send(Notice::duplicate(event.id)).ok();
                    } else {
                        info!(
                            "persisted event: {:?} from: {:?} in: {:?}",
                            event.get_event_id_prefix(),
                            event.get_author_prefix(),
                            start.elapsed()
                        );
                        event_write = true;
                        // send this out to all clients
                        bcast_tx.send(event.clone()).ok();
                        notice_tx.try_send(Notice::saved(event.id)).ok();
                    }
                }
                Err(err) => {
                    warn!("event insert failed: {:?}", err);
                    let msg = "relay experienced an error trying to publish the latest event";
                    notice_tx.try_send(Notice::error(event.id, msg)).ok();
                }
            }
            // Use this as a trigger to do optimization
            optimize_counter += 1;
            if optimize_counter > EVENT_COUNT_OPTIMIZE_TRIGGER {
                info!("running database optimizer");
                optimize_counter = 0;
                repo.optimize_db().await?;
            }
        }

        // use rate limit, if defined, and if an event was actually written.
        if event_write {
            if let Some(ref lim) = lim_opt {
                if let Err(n) = lim.check() {
                    let wait_for = n.wait_time_from(clock.now());
                    // check if we have recently logged rate
                    // limits, but print out a message only once
                    // per second.
                    if most_recent_rate_limit.elapsed().as_secs() > 10 {
                        warn!(
                                "rate limit reached for event creation (sleep for {:?}) (suppressing future messages for 10 seconds)",
                                wait_for
                            );
                        // reset last rate limit message
                        most_recent_rate_limit = Instant::now();
                    }
                    // block event writes, allowing them to queue up
                    thread::sleep(wait_for);
                    continue;
                }
            }
        }
    }
    info!("database connection closed");
    Ok(())
}

/// Serialized event associated with a specific subscription request.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct QueryResult {
    /// Subscription identifier
    pub sub_id: String,
    /// Serialized event
    pub event: String,
}
