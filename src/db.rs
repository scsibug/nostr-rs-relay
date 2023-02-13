//! Event persistence and querying
use crate::config::Settings;
use crate::error::{Error, Result};
use crate::event::Event;
use crate::notice::Notice;
use crate::server::NostrMetrics;
use crate::nauthz;
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

/// Events submitted from a client, with a return channel for notices
pub struct SubmittedEvent {
    pub event: Event,
    pub notice_tx: tokio::sync::mpsc::Sender<Notice>,
    pub source_ip: String,
    pub origin: Option<String>,
    pub user_agent: Option<String>,
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
    mut event_rx: tokio::sync::mpsc::Receiver<SubmittedEvent>,
    bcast_tx: tokio::sync::broadcast::Sender<Event>,
    metadata_tx: tokio::sync::broadcast::Sender<Event>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> Result<()> {
    // are we performing NIP-05 checking?
    let nip05_active = settings.verified_users.is_active();
    // are we requriing NIP-05 user verification?
    let nip05_enabled = settings.verified_users.is_enabled();

    //upgrade_db(&mut pool.get()?)?;

    // Make a copy of the whitelist
    let whitelist = &settings.authorization.pubkey_whitelist.clone();

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
    // create a client if GRPC is enabled.
    // Check with externalized event admitter service, if one is defined.
    let mut grpc_client = if let Some(svr) = settings.grpc.event_admission_server {
        Some(nauthz::EventAuthzService::connect(&svr).await)
    } else {
        None
    };

    //let gprc_client = settings.grpc.event_admission_server.map(|s| {
//        event_admitter_connect(&s);
//    });

    loop {
        if shutdown.try_recv().is_ok() {
            info!("shutting down database writer");
            break;
        }
        // call blocking read on channel
        let next_event = event_rx.recv().await;
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
                debug!(
                    "rejecting event: {}, unauthorized author",
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

        // Check that event kind isn't blacklisted
        let kinds_blacklist = &settings.limits.event_kind_blacklist.clone();
        if let Some(event_kind_blacklist) = kinds_blacklist {
            if event_kind_blacklist.contains(&event.kind) {
                debug!(
                    "rejecting event: {}, blacklisted kind: {}",
                    &event.get_event_id_prefix(),
                    &event.kind
                );
                notice_tx
                    .try_send(Notice::blocked(
                        event.id,
                        "event kind is blocked by relay"
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

        // get a validation result for use in verification and GPRC
        let validation = if nip05_active {
            Some(repo.get_latest_user_verification(&event.pubkey).await)
        } else {
            None
        };

        // check for  NIP-05 verification
        if nip05_enabled && validation.is_some() {
            match validation.as_ref().unwrap() {
                Ok(uv) => {
                    if uv.is_valid(&settings.verified_users) {
                        info!(
                            "new event from verified author ({:?},{:?})",
                            uv.name.to_string(),
                            event.get_author_prefix()
                        );

                    } else {
                        info!(
                            "rejecting event, author ({:?} / {:?}) verification invalid (expired/wrong domain)",
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
                Err(Error::SqlError(rusqlite::Error::QueryReturnedNoRows)) => {
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

        // nip05 address
        let nip05_address : Option<crate::nip05::Nip05Name> = validation.and_then(|x| x.ok().map(|y| y.name));

        // GRPC check
        if let Some(ref mut c) = grpc_client {
            trace!("checking if grpc permits");
            let grpc_start = Instant::now();
            let decision_res = c.admit_event(&event, &subm_event.source_ip, subm_event.origin, subm_event.user_agent, nip05_address).await;
            match decision_res {
                Ok(decision) => {
                    if !decision.permitted() {
                        // GPRC returned a decision to reject this event
                        info!("GRPC rejected event: {:?} (kind: {}) from: {:?} in: {:?} (IP: {:?})",
                              event.get_event_id_prefix(),
                              event.kind,
                              event.get_author_prefix(),
                              grpc_start.elapsed(),
                              subm_event.source_ip);
                        notice_tx.try_send(Notice::blocked(event.id, &decision.message().unwrap_or_else(|| "".to_string()))).ok();
                        continue;
                    }
                },
                Err(e) => {
                    warn!("GRPC server error: {:?}", e);
                }
            }
        }

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
                            "persisted event: {:?} (kind: {}) from: {:?} in: {:?} (IP: {:?})",
                            event.get_event_id_prefix(),
                            event.kind,
                            event.get_author_prefix(),
                            start.elapsed(),
                            subm_event.source_ip,
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
