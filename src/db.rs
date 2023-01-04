//! Event persistence and querying
//use crate::config::SETTINGS;
use crate::config::Settings;
use crate::error::{Error, Result};
use crate::event::{single_char_tagname, Event};
use crate::hexrange::hex_range;
use crate::hexrange::HexSearch;
use crate::nip05;
use crate::notice::Notice;
use crate::schema::{upgrade_db, STARTUP_SQL};
use crate::subscription::ReqFilter;
use crate::subscription::Subscription;
use crate::utils::{is_hex, is_lower_hex};
use governor::clock::Clock;
use governor::{Quota, RateLimiter};
use hex;
use r2d2;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use rusqlite::types::ToSql;
use rusqlite::OpenFlags;
use tokio::sync::{Mutex, MutexGuard};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tokio::task;
use tracing::{debug, info, trace, warn};

pub type SqlitePool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;
pub type PooledConnection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

/// Events submitted from a client, with a return channel for notices
pub struct SubmittedEvent {
    pub event: Event,
    pub notice_tx: tokio::sync::mpsc::Sender<Notice>,
}

/// Database file
pub const DB_FILE: &str = "nostr.db";

/// How frequently to attempt checkpointing
pub const CHECKPOINT_FREQ_SEC: u64 = 60;

/// Build a database connection pool.
/// # Panics
///
/// Will panic if the pool could not be created.
#[must_use]
pub fn build_pool(
    name: &str,
    settings: &Settings,
    flags: OpenFlags,
    min_size: u32,
    max_size: u32,
    wait_for_db: bool,
) -> SqlitePool {
    let db_dir = &settings.database.data_directory;
    let full_path = Path::new(db_dir).join(DB_FILE);
    // small hack; if the database doesn't exist yet, that means the
    // writer thread hasn't finished.  Give it a chance to work.  This
    // is only an issue with the first time we run.
    if !settings.database.in_memory {
        while !full_path.exists() && wait_for_db {
            debug!("Database reader pool is waiting on the database to be created...");
            thread::sleep(Duration::from_millis(500));
        }
    }
    let manager = if settings.database.in_memory {
        SqliteConnectionManager::memory()
            .with_flags(flags)
            .with_init(|c| c.execute_batch(STARTUP_SQL))
    } else {
        SqliteConnectionManager::file(&full_path)
            .with_flags(flags)
            .with_init(|c| c.execute_batch(STARTUP_SQL))
    };
    let pool: SqlitePool = r2d2::Pool::builder()
        .test_on_check_out(true) // no noticeable performance hit
        .min_idle(Some(min_size))
        .max_size(max_size)
        .max_lifetime(Some(Duration::from_secs(30)))
        .build(manager)
        .unwrap();
    info!(
        "Built a connection pool {:?} (min={}, max={})",
        name, min_size, max_size
    );
    pool
}

/// Display database pool stats every 1 minute
pub async fn monitor_pool(name: &str, pool: SqlitePool) {
    let sleep_dur = Duration::from_secs(60);
    loop {
	log_pool_stats(name, &pool);
	tokio::time::sleep(sleep_dur).await;
    }
}


/// Perform normal maintenance
pub fn optimize_db(conn: &mut PooledConnection) -> Result<()> {
    let start = Instant::now();
    conn.execute_batch("PRAGMA optimize;")?;
    info!("optimize ran in {:?}", start.elapsed());
    Ok(())
}
#[derive(Debug)]
enum SqliteStatus {
    Ok,
    Busy,
    Error,
    Other(u64),
}

/// Checkpoint/Truncate WAL.  Returns the number of WAL pages remaining.
pub fn checkpoint_db(conn: &mut PooledConnection) -> Result<usize> {
    let query = "PRAGMA wal_checkpoint(TRUNCATE);";
    let start = Instant::now();
    let (cp_result, wal_size, _frames_checkpointed) = conn.query_row(query, [], |row| {
        let checkpoint_result: u64 = row.get(0)?;
        let wal_size: u64 = row.get(1)?;
        let frames_checkpointed: u64 = row.get(2)?;
        Ok((checkpoint_result, wal_size, frames_checkpointed))
    })?;
    let result = match cp_result {
        0 => SqliteStatus::Ok,
        1 => SqliteStatus::Busy,
        2 => SqliteStatus::Error,
        x => SqliteStatus::Other(x),
    };
    info!(
        "checkpoint ran in {:?} (result: {:?}, WAL size: {})",
        start.elapsed(),
        result,
        wal_size
    );
    Ok(wal_size as usize)
}

/// Spawn a database writer that persists events to the SQLite store.
pub async fn db_writer(
    settings: Settings,
    mut event_rx: tokio::sync::mpsc::Receiver<SubmittedEvent>,
    bcast_tx: tokio::sync::broadcast::Sender<Event>,
    metadata_tx: tokio::sync::broadcast::Sender<Event>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<Result<()>> {
    // are we performing NIP-05 checking?
    let nip05_active = settings.verified_users.is_active();
    // are we requriing NIP-05 user verification?
    let nip05_enabled = settings.verified_users.is_enabled();

    task::spawn_blocking(move || {
        let db_dir = &settings.database.data_directory;
        let full_path = Path::new(db_dir).join(DB_FILE);
        // create a connection pool
        let pool = build_pool(
            "event writer",
            &settings,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            1,
            2,
            false,
        );
        if settings.database.in_memory {
            info!("using in-memory database, this will not persist a restart!");
        } else {
            info!("opened database {:?} for writing", full_path);
        }
        upgrade_db(&mut pool.get()?)?;

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

            // Check that event kind isn't blacklisted
            let kinds_blacklist = &settings.limits.event_kind_blacklist.clone();
            if let Some(event_kind_blacklist) = kinds_blacklist {
                if event_kind_blacklist.contains(&event.kind) {
                    info!(
                        "Rejecting event {}, blacklisted kind",
                        &event.get_event_id_prefix()
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

            // check for  NIP-05 verification
            if nip05_enabled {
                match nip05::query_latest_user_verification(pool.get()?, event.pubkey.to_owned()) {
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
                log_pool_stats("writer", &pool);
                match write_event(&mut pool.get()?, &event) {
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
    })
}

/// Persist an event to the database, returning rows added.
pub fn write_event(conn: &mut PooledConnection, e: &Event) -> Result<usize> {
    // enable auto vacuum
    conn.execute_batch("pragma auto_vacuum = FULL")?;

    // start transaction
    let tx = conn.transaction()?;
    // get relevant fields from event and convert to blobs.
    let id_blob = hex::decode(&e.id).ok();
    let pubkey_blob: Option<Vec<u8>> = hex::decode(&e.pubkey).ok();
    let delegator_blob: Option<Vec<u8>> = e.delegated_by.as_ref().and_then(|d| hex::decode(d).ok());
    let event_str = serde_json::to_string(&e).ok();
    // ignore if the event hash is a duplicate.
    let mut ins_count = tx.execute(
        "INSERT OR IGNORE INTO event (event_hash, created_at, kind, author, delegated_by, content, first_seen, hidden) VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'), FALSE);",
        params![id_blob, e.created_at, e.kind, pubkey_blob, delegator_blob, event_str]
    )?;
    if ins_count == 0 {
        // if the event was a duplicate, no need to insert event or
        // pubkey references.
        tx.rollback().ok();
        return Ok(ins_count);
    }
    // remember primary key of the event most recently inserted.
    let ev_id = tx.last_insert_rowid();
    // add all tags to the tag table
    for tag in e.tags.iter() {
        // ensure we have 2 values.
        if tag.len() >= 2 {
            let tagname = &tag[0];
            let tagval = &tag[1];
            // only single-char tags are searchable
            let tagchar_opt = single_char_tagname(tagname);
            match &tagchar_opt {
                Some(_) => {
                    // if tagvalue is lowercase hex;
                    if is_lower_hex(tagval) && (tagval.len() % 2 == 0) {
                        tx.execute(
                            "INSERT OR IGNORE INTO tag (event_id, name, value_hex) VALUES (?1, ?2, ?3)",
                            params![ev_id, &tagname, hex::decode(tagval).ok()],
                        )?;
                    } else {
                        tx.execute(
                            "INSERT OR IGNORE INTO tag (event_id, name, value) VALUES (?1, ?2, ?3)",
                            params![ev_id, &tagname, &tagval],
                        )?;
                    }
                }
                None => {}
            }
        }
    }
    // if this event is replaceable update, hide every other replaceable
    // event with the same kind from the same author that was issued
    // earlier than this.
    if e.kind == 0 || e.kind == 3 || e.kind == 41 || (e.kind >= 10000 && e.kind < 20000) {
	let author = hex::decode(&e.pubkey).ok();
	// this is a backwards check - hide any events that were older.
        let update_count = tx.execute(
            "UPDATE event SET hidden=TRUE WHERE hidden!=TRUE and kind=? and author=? and id NOT IN (SELECT id FROM event WHERE kind=? AND author=? ORDER BY created_at DESC LIMIT 1)",
            params![e.kind, author, e.kind, author],
        )?;
        if update_count > 0 {
            info!(
                "hid {} older replaceable kind {} events for author: {:?}",
                update_count,
                e.kind,
                e.get_author_prefix()
            );
        }
    }
    // if this event is a deletion, hide the referenced events from the same author.
    if e.kind == 5 {
        let event_candidates = e.tag_values_by_name("e");
        // first parameter will be author
        let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(hex::decode(&e.pubkey)?)];
        event_candidates
            .iter()
            .filter(|x| is_hex(x) && x.len() == 64)
            .filter_map(|x| hex::decode(x).ok())
            .for_each(|x| params.push(Box::new(x)));
        let query = format!(
            "UPDATE event SET hidden=TRUE WHERE kind!=5 AND author=? AND event_hash IN ({})",
            repeat_vars(params.len() - 1)
        );
        let mut stmt = tx.prepare(&query)?;
        let update_count = stmt.execute(rusqlite::params_from_iter(params))?;
        info!(
            "hid {} deleted events for author {:?}",
            update_count,
            e.get_author_prefix()
        );
    } else {
        // check if a deletion has already been recorded for this event.
        // Only relevant for non-deletion events
        let del_count = tx.query_row(
	    "SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.author=? AND t.name='e' AND e.kind=5 AND t.value_hex=? LIMIT 1;",
	    params![pubkey_blob, id_blob], |row| row.get::<usize, usize>(0));
        // check if a the query returned a result, meaning we should
        // hid the current event
        if del_count.ok().is_some() {
            // a deletion already existed, mark original event as hidden.
            info!(
                "hid event: {:?} due to existing deletion by author: {:?}",
                e.get_event_id_prefix(),
                e.get_author_prefix()
            );
            let _update_count =
                tx.execute("UPDATE event SET hidden=TRUE WHERE id=?", params![ev_id])?;
            // event was deleted, so let caller know nothing new
            // arrived, preventing this from being sent to active
            // subscriptions
            ins_count = 0;
        }
    }
    tx.commit()?;
    Ok(ins_count)
}

/// Serialized event associated with a specific subscription request.
#[derive(PartialEq, Eq, Debug, Clone)]
pub struct QueryResult {
    /// Subscription identifier
    pub sub_id: String,
    /// Serialized event
    pub event: String,
}

/// Produce a arbitrary list of '?' parameters.
fn repeat_vars(count: usize) -> String {
    if count == 0 {
        return "".to_owned();
    }
    let mut s = "?,".repeat(count);
    // Remove trailing comma
    s.pop();
    s
}

/// Create a dynamic SQL subquery and params from a subscription filter.
fn query_from_filter(f: &ReqFilter) -> (String, Vec<Box<dyn ToSql>>) {
    // build a dynamic SQL query.  all user-input is either an integer
    // (sqli-safe), or a string that is filtered to only contain
    // hexadecimal characters.  Strings that require escaping (tag
    // names/values) use parameters.

    // if the filter is malformed, don't return anything.
    if f.force_no_match {
        let empty_query = "SELECT e.content, e.created_at FROM event e WHERE 1=0".to_owned();
        // query parameters for SQLite
        let empty_params: Vec<Box<dyn ToSql>> = vec![];
        return (empty_query, empty_params);
    }

    let mut query = "SELECT e.content, e.created_at FROM event e".to_owned();
    // query parameters for SQLite
    let mut params: Vec<Box<dyn ToSql>> = vec![];

    // individual filter components (single conditions such as an author or event ID)
    let mut filter_components: Vec<String> = Vec::new();
    // Query for "authors", allowing prefix matches
    if let Some(authvec) = &f.authors {
        // take each author and convert to a hexsearch
        let mut auth_searches: Vec<String> = vec![];
        for auth in authvec {
            match hex_range(auth) {
                Some(HexSearch::Exact(ex)) => {
                    auth_searches.push("author=? OR delegated_by=?".to_owned());
                    params.push(Box::new(ex.clone()));
                    params.push(Box::new(ex));
                }
                Some(HexSearch::Range(lower, upper)) => {
                    auth_searches.push(
                        "(author>? AND author<?) OR (delegated_by>? AND delegated_by<?)".to_owned(),
                    );
                    params.push(Box::new(lower.clone()));
                    params.push(Box::new(upper.clone()));
                    params.push(Box::new(lower));
                    params.push(Box::new(upper));
                }
                Some(HexSearch::LowerOnly(lower)) => {
                    auth_searches.push("author>? OR delegated_by>?".to_owned());
                    params.push(Box::new(lower.clone()));
                    params.push(Box::new(lower));
                }
                None => {
                    info!("Could not parse hex range from author {:?}", auth);
                }
            }
        }
        if !authvec.is_empty() {
            let authors_clause = format!("({})", auth_searches.join(" OR "));
            filter_components.push(authors_clause);
        } else {
            // if the authors list was empty, we should never return
            // any results.
            filter_components.push("false".to_owned());
        }
    }
    // Query for Kind
    if let Some(ks) = &f.kinds {
        // kind is number, no escaping needed
        let str_kinds: Vec<String> = ks.iter().map(|x| x.to_string()).collect();
        let kind_clause = format!("kind IN ({})", str_kinds.join(", "));
        filter_components.push(kind_clause);
    }
    // Query for event, allowing prefix matches
    if let Some(idvec) = &f.ids {
        // take each author and convert to a hexsearch
        let mut id_searches: Vec<String> = vec![];
        for id in idvec {
            match hex_range(id) {
                Some(HexSearch::Exact(ex)) => {
                    id_searches.push("event_hash=?".to_owned());
                    params.push(Box::new(ex));
                }
                Some(HexSearch::Range(lower, upper)) => {
                    id_searches.push("(event_hash>? AND event_hash<?)".to_owned());
                    params.push(Box::new(lower));
                    params.push(Box::new(upper));
                }
                Some(HexSearch::LowerOnly(lower)) => {
                    id_searches.push("event_hash>?".to_owned());
                    params.push(Box::new(lower));
                }
                None => {
                    info!("Could not parse hex range from id {:?}", id);
                }
            }
        }
        if !idvec.is_empty() {
            let id_clause = format!("({})", id_searches.join(" OR "));
            filter_components.push(id_clause);
        } else {
            // if the ids list was empty, we should never return
            // any results.
            filter_components.push("false".to_owned());
        }
    }
    // Query for tags
    if let Some(map) = &f.tags {
        for (key, val) in map.iter() {
            let mut str_vals: Vec<Box<dyn ToSql>> = vec![];
            let mut blob_vals: Vec<Box<dyn ToSql>> = vec![];
            for v in val {
                if (v.len() % 2 == 0) && is_lower_hex(v) {
                    if let Ok(h) = hex::decode(v) {
                        blob_vals.push(Box::new(h));
                    }
                } else {
                    str_vals.push(Box::new(v.to_owned()));
                }
            }
            // create clauses with "?" params for each tag value being searched
            let str_clause = format!("value IN ({})", repeat_vars(str_vals.len()));
            let blob_clause = format!("value_hex IN ({})", repeat_vars(blob_vals.len()));
            // find evidence of the target tag name/value existing for this event.
            let tag_clause = format!(
                "e.id IN (SELECT e.id FROM event e LEFT JOIN tag t on e.id=t.event_id WHERE hidden!=TRUE and (name=? AND ({} OR {})))",
                str_clause, blob_clause
            );
            // add the tag name as the first parameter
            params.push(Box::new(key.to_string()));
            // add all tag values that are plain strings as params
            params.append(&mut str_vals);
            // add all tag values that are blobs as params
            params.append(&mut blob_vals);
            filter_components.push(tag_clause);
        }
    }
    // Query for timestamp
    if f.since.is_some() {
        let created_clause = format!("created_at > {}", f.since.unwrap());
        filter_components.push(created_clause);
    }
    // Query for timestamp
    if f.until.is_some() {
        let until_clause = format!("created_at < {}", f.until.unwrap());
        filter_components.push(until_clause);
    }
    // never display hidden events
    query.push_str(" WHERE hidden!=TRUE");
    // build filter component conditions
    if !filter_components.is_empty() {
        query.push_str(" AND ");
        query.push_str(&filter_components.join(" AND "));
    }
    // Apply per-filter limit to this subquery.
    // The use of a LIMIT implies a DESC order, to capture only the most recent events.
    if let Some(lim) = f.limit {
        let _ = write!(query, " ORDER BY e.created_at DESC LIMIT {}", lim);
    } else {
        query.push_str(" ORDER BY e.created_at ASC")
    }
    (query, params)
}

/// Create a dynamic SQL query string and params from a subscription.
fn query_from_sub(sub: &Subscription) -> (String, Vec<Box<dyn ToSql>>) {
    // build a dynamic SQL query for an entire subscription, based on
    // SQL subqueries for filters.
    let mut subqueries: Vec<String> = Vec::new();
    // subquery params
    let mut params: Vec<Box<dyn ToSql>> = vec![];
    // for every filter in the subscription, generate a subquery
    for f in sub.filters.iter() {
        let (f_subquery, mut f_params) = query_from_filter(f);
        subqueries.push(f_subquery);
        params.append(&mut f_params);
    }
    // encapsulate subqueries into select statements
    let subqueries_selects: Vec<String> = subqueries
        .iter()
        .map(|s| format!("SELECT distinct content, created_at FROM ({})", s))
        .collect();
    let query: String = subqueries_selects.join(" UNION ");
    (query, params)
}

/// Check if the pool is fully utilized
fn _pool_at_capacity(pool: &SqlitePool) -> bool {
    let state: r2d2::State = pool.state();
    state.idle_connections == 0
}

/// Log pool stats
fn log_pool_stats(name: &str, pool: &SqlitePool) {
    let state: r2d2::State = pool.state();
    let in_use_cxns = state.connections - state.idle_connections;
    debug!(
        "DB pool {:?} usage (in_use: {}, available: {}, max: {})",
        name,
        in_use_cxns,
        state.connections,
	pool.max_size()
    );
}


/// Perform database maintenance on a regular basis
pub async fn db_optimize_task(pool: SqlitePool) {
    tokio::task::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(60*60)) => {
                    if let Ok(mut conn) = pool.get() {
			// the busy timer will block writers, so don't set
			// this any higher than you want max latency for event
			// writes.
                        info!("running database optimizer");
                        optimize_db(&mut conn).ok();
                    }
		}
            };
        }
    });
}

/// Perform database WAL checkpoint on a regular basis
pub async fn db_checkpoint_task(pool: SqlitePool, safe_to_read: Arc<Mutex<u64>>) {
    tokio::task::spawn(async move {
	// WAL size in pages.
	let mut current_wal_size = 0;
	// WAL threshold for more aggressive checkpointing (10,000 pages, or about 40MB)
	let wal_threshold = 1000*10;
	// default threshold for the busy timer
	let busy_wait_default = Duration::from_secs(1);
	// if the WAL file is getting too big, switch to this
	let busy_wait_default_long = Duration::from_secs(10);
        loop {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(CHECKPOINT_FREQ_SEC)) => {
                    if let Ok(mut conn) = pool.get() {
                        let mut _guard:Option<MutexGuard<u64>> = None;
			// the busy timer will block writers, so don't set
			// this any higher than you want max latency for event
			// writes.
			if current_wal_size <= wal_threshold {
			    conn.busy_timeout(busy_wait_default).ok();
			} else {
			    // if the wal size has exceeded a threshold, increase the busy timeout.
			    conn.busy_timeout(busy_wait_default_long).ok();
			    // take a lock that will prevent new readers.
			    info!("blocking new readers to perform wal_checkpoint");
			    _guard = Some(safe_to_read.lock().await);
			}
                        debug!("running wal_checkpoint(TRUNCATE)");
                        if let Ok(new_size) = checkpoint_db(&mut conn) {
			    current_wal_size = new_size;
			}
                    }
		}
            };
        }
    });
}

/// Perform a database query using a subscription.
///
/// The [`Subscription`] is converted into a SQL query.  Each result
/// is published on the `query_tx` channel as it is returned.  If a
/// message becomes available on the `abandon_query_rx` channel, the
/// query is immediately aborted.
pub async fn db_query(
    sub: Subscription,
    client_id: String,
    pool: SqlitePool,
    query_tx: tokio::sync::mpsc::Sender<QueryResult>,
    mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
    safe_to_read: Arc<Mutex<u64>>,
) {
    let pre_spawn_start = Instant::now();
    task::spawn_blocking(move || {
	{
	    // if we are waiting on a checkpoint, stop until it is complete
	    let _ = safe_to_read.blocking_lock();
	}
        let db_queue_time = pre_spawn_start.elapsed();
        // if the queue time was very long (>5 seconds), spare the DB and abort.
        if db_queue_time > Duration::from_secs(5) {
            info!(
                "shedding DB query load queued for {:?} (cid: {}, sub: {:?})",
                db_queue_time, client_id, sub.id
            );
            return Ok(());
        }
        // otherwise, report queuing time if it is slow
        else if db_queue_time > Duration::from_secs(1) {
            debug!(
                "(slow) DB query queued for {:?} (cid: {}, sub: {:?})",
                db_queue_time, client_id, sub.id
            );
        }
        let start = Instant::now();
        let mut row_count: usize = 0;
        // generate SQL query
        let (q, p) = query_from_sub(&sub);
        let sql_gen_elapsed = start.elapsed();
        if sql_gen_elapsed > Duration::from_millis(10) {
            debug!("SQL (slow) generated in {:?}", start.elapsed());
        }
        // cutoff for displaying slow queries
        let slow_cutoff = Duration::from_millis(2000);
        // any client that doesn't cause us to generate new rows in 5
        // seconds gets dropped.
        let abort_cutoff = Duration::from_secs(5);
        let start = Instant::now();
        let mut slow_first_event;
        let mut last_successful_send = Instant::now();
        if let Ok(mut conn) = pool.get() {
            // execute the query.
            // make the actual SQL query (with parameters inserted) available
            conn.trace(Some(|x| {trace!("SQL trace: {:?}", x)}));
            let mut stmt = conn.prepare_cached(&q)?;
            let mut event_rows = stmt.query(rusqlite::params_from_iter(p))?;

            let mut first_result = true;
            while let Some(row) = event_rows.next()? {
                let first_event_elapsed = start.elapsed();
                slow_first_event = first_event_elapsed >= slow_cutoff;
                if first_result {
                    debug!(
                        "first result in {:?} (cid: {}, sub: {:?})",
                        first_event_elapsed, client_id, sub.id
                    );
                    first_result = false;
                }
                // logging for slow queries; show sub and SQL.
                // to reduce logging; only show 1/16th of clients (leading 0)
                if row_count == 0 && slow_first_event && client_id.starts_with('0') {
                    debug!(
                        "query req (slow): {:?} (cid: {}, sub: {:?})",
                        sub, client_id, sub.id
                    );
		}
		// check if a checkpoint is trying to run, and abort
		if row_count % 100 == 0 {
		    {
			if let Err(_) = safe_to_read.try_lock() {
			    // lock was held, abort this query
			    debug!("query aborted due to checkpoint (cid: {}, sub: {:?})", client_id, sub.id);
			    return Ok(());
			}
		    }
		}

                // check if this is still active; every 100 rows
                if row_count % 100 == 0 && abandon_query_rx.try_recv().is_ok() {
                    debug!("query aborted (cid: {}, sub: {:?})", client_id, sub.id);
                    return Ok(());
                }
                row_count += 1;
                let event_json = row.get(0)?;
                loop {
                    if query_tx.capacity() != 0 {
                        // we have capacity to add another item
                        break;
                    } else {
                        // the queue is full
                        trace!("db reader thread is stalled");
                        if last_successful_send + abort_cutoff < Instant::now() {
                            // the queue has been full for too long, abort
                            info!("aborting database query due to slow client (cid: {}, sub: {:?})",
				  client_id, sub.id);
                            let ok: Result<()> = Ok(());
                            return ok;
                        }
			// check if a checkpoint is trying to run, and abort
			if let Err(_) = safe_to_read.try_lock() {
			    // lock was held, abort this query
			    debug!("query aborted due to checkpoint (cid: {}, sub: {:?})", client_id, sub.id);
			    return Ok(());
			}
                        // give the queue a chance to clear before trying again
                        thread::sleep(Duration::from_millis(100));
                    }
                }
                // TODO: we could use try_send, but we'd have to juggle
                // getting the query result back as part of the error
                // result.
                query_tx
                    .blocking_send(QueryResult {
                        sub_id: sub.get_id(),
                        event: event_json,
                    })
                    .ok();
                last_successful_send = Instant::now();
            }
            query_tx
                .blocking_send(QueryResult {
                    sub_id: sub.get_id(),
                    event: "EOSE".to_string(),
                })
                .ok();
            debug!(
                "query completed in {:?} (cid: {}, sub: {:?}, db_time: {:?}, rows: {})",
                pre_spawn_start.elapsed(),
                client_id,
                sub.id,
                start.elapsed(),
                row_count
            );
        } else {
            warn!("Could not get a database connection for querying");
        }
        let ok: Result<()> = Ok(());
        ok
    });
}
