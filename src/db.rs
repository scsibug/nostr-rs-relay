//! Event persistence and querying
use crate::config::Settings;
use crate::error::Result;
use crate::event::Event;
use crate::notice::Notice;
use crate::server::NostrMetrics;
use governor::clock::Clock;
use governor::{Quota, RateLimiter};
use r2d2;
use std::sync::Arc;
use std::thread;
use sqlx::pool::PoolOptions;
use sqlx::postgres::PgConnectOptions;
use sqlx::ConnectOptions;
use crate::repo::sqlite::SqliteRepo;
use crate::repo::postgres::{PostgresRepo,PostgresPool};
use crate::repo::NostrRepo;
use std::time::{Instant, Duration};
use tracing::log::LevelFilter;
use tracing::{debug, info, trace, warn};

pub type SqlitePool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;
pub type PooledConnection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

#[derive(Debug)]
pub struct AdmittedEvent {
    pub event: Event,
    pub notice_tx: tokio::sync::mpsc::Sender<Notice>,
}

/// Database file
pub const DB_FILE: &str = "nostr.db";

/// Build repo
/// # Panics
///
/// Will panic if the pool could not be created.
pub async fn build_repo(settings: &Settings, metrics: NostrMetrics) -> Arc<dyn NostrRepo> {
    match settings.database.engine.as_str() {
        "sqlite" => {Arc::new(build_sqlite_pool(settings, metrics).await)},
        "postgres" => {Arc::new(build_postgres_pool(settings, metrics).await)},
        _ => panic!("Unknown database engine"),
    }
}

async fn build_sqlite_pool(settings: &Settings, metrics: NostrMetrics) -> SqliteRepo {
    let repo = SqliteRepo::new(settings, metrics);
    repo.start().await.ok();
    repo.migrate_up().await.ok();
    repo
}

async fn build_postgres_pool(settings: &Settings, metrics: NostrMetrics) -> PostgresRepo {
    let mut options: PgConnectOptions = settings.database.connection.as_str().parse().unwrap();
    options.log_statements(LevelFilter::Debug);
    options.log_slow_statements(LevelFilter::Warn, Duration::from_secs(60));

    let pool: PostgresPool = PoolOptions::new()
        .max_connections(settings.database.max_conn)
        .min_connections(settings.database.min_conn)
        .idle_timeout(Duration::from_secs(60))
        .connect_with(options)
        .await
        .unwrap();
    let repo = PostgresRepo::new(pool, metrics);
    // Panic on migration failure
    let version = repo.migrate_up().await.unwrap();
    info!("Postgres migration completed, at v{}", version);
    repo
}

/// Spawn a database writer that persists events to the `SQLite` store.
pub async fn db_writer(
    repo: Arc<dyn NostrRepo>,
    settings: Settings,
    mut admitted_event_rx: tokio::sync::mpsc::Receiver<AdmittedEvent>,
    bcast_tx: tokio::sync::broadcast::Sender<Event>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    //upgrade_db(&mut pool.get()?)?;

    // get rate limit settings
    let rps_setting = settings.limits.messages_per_sec;
    let mut most_recent_rate_limit = Instant::now();
    let mut lim_opt = None;
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
        let next_event = admitted_event_rx.recv().await;
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

        // TODO: cache recent list of authors to remove a DB call.
        let start = Instant::now();
        if event.is_ephemeral() {
            bcast_tx.send(event.clone()).ok();
            debug!(
                "published ephemeral event: {:?} from: {:?} in: {:?}",
                event.get_event_id_prefix(),
                event.get_author_prefix(),
                start.elapsed()
            );
            event_write = true;
        } else {
            match repo.write_event(&event).await {
                Ok(updated) => {
                    if updated == 0 {
                        trace!("ignoring duplicate or deleted event");
                        notice_tx.try_send(Notice::duplicate(event.id)).ok();
                    } else {
                        info!(
                            "persisted event: {:?} (kind: {}) from: {:?} in: {:?}",
                            event.get_event_id_prefix(),
                            event.kind,
                            event.get_author_prefix(),
                            start.elapsed(),
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
