//! Event persistence and querying
use crate::config::SETTINGS;
use crate::error::Error;
use crate::error::Result;
use crate::event::Event;
use crate::hexrange::hex_range;
use crate::hexrange::HexSearch;
use crate::nip05;
use crate::schema::{upgrade_db, STARTUP_SQL};
use crate::subscription::ReqFilter;
use crate::subscription::Subscription;
use crate::utils::is_hex;
use governor::clock::Clock;
use governor::{Quota, RateLimiter};
use hex;
use log::*;
use r2d2;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use rusqlite::types::ToSql;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use std::path::Path;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tokio::task;

pub type SqlitePool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;
pub type PooledConnection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

/// Events submitted from a client, with a return channel for notices
pub struct SubmittedEvent {
    pub event: Event,
    pub notice_tx: tokio::sync::mpsc::Sender<String>,
}

/// Database file
pub const DB_FILE: &str = "nostr.db";

/// Build a database connection pool.
pub fn build_pool(
    name: &str,
    flags: OpenFlags,
    min_size: u32,
    max_size: u32,
    wait_for_db: bool,
) -> SqlitePool {
    let settings = SETTINGS.read().unwrap();

    let db_dir = &settings.database.data_directory;
    let full_path = Path::new(db_dir).join(DB_FILE);
    // small hack; if the database doesn't exist yet, that means the
    // writer thread hasn't finished.  Give it a chance to work.  This
    // is only an issue with the first time we run.
    while !full_path.exists() && wait_for_db {
        debug!("Database reader pool is waiting on the database to be created...");
        thread::sleep(Duration::from_millis(500));
    }
    let manager = SqliteConnectionManager::file(&full_path)
        .with_flags(flags)
        .with_init(|c| c.execute_batch(STARTUP_SQL));
    let pool: SqlitePool = r2d2::Pool::builder()
        .test_on_check_out(true) // no noticeable performance hit
        .min_idle(Some(min_size))
        .max_size(max_size)
        .build(manager)
        .unwrap();
    info!(
        "Built a connection pool {:?} (min={}, max={})",
        name, min_size, max_size
    );
    pool
}

/// Build a single database connection, with provided flags
pub fn build_conn(flags: OpenFlags) -> Result<Connection> {
    let settings = SETTINGS.read().unwrap();
    let db_dir = &settings.database.data_directory;
    let full_path = Path::new(db_dir).join(DB_FILE);
    // create a connection
    Ok(Connection::open_with_flags(&full_path, flags)?)
}

/// Spawn a database writer that persists events to the SQLite store.
pub async fn db_writer(
    mut event_rx: tokio::sync::mpsc::Receiver<SubmittedEvent>,
    bcast_tx: tokio::sync::broadcast::Sender<Event>,
    metadata_tx: tokio::sync::broadcast::Sender<Event>,
    mut shutdown: tokio::sync::broadcast::Receiver<()>,
) -> tokio::task::JoinHandle<Result<()>> {
    let settings = SETTINGS.read().unwrap();

    // are we performing NIP-05 checking?
    let nip05_active = settings.verified_users.is_active();
    // are we requriing NIP-05 user verification?
    let nip05_enabled = settings.verified_users.is_enabled();

    task::spawn_blocking(move || {
        // get database configuration settings
        let settings = SETTINGS.read().unwrap();
        let db_dir = &settings.database.data_directory;
        let full_path = Path::new(db_dir).join(DB_FILE);
        // create a connection pool
        let pool = build_pool(
            "event writer",
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            1,
            4,
            false,
        );
        info!("opened database {:?} for writing", full_path);
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
            let mut event_write = false;
            let subm_event = next_event.unwrap();
            let event = subm_event.event;
            let notice_tx = subm_event.notice_tx;
            // check if this event is authorized.
            if let Some(allowed_addrs) = whitelist {
                // if the event address is not in allowed_addrs.
                if !allowed_addrs.contains(&event.pubkey) {
                    info!(
                        "Rejecting event {}, unauthorized author",
                        event.get_event_id_prefix()
                    );
                    notice_tx
                        .try_send("pubkey is not allowed to publish to this relay".to_owned())
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
                        if uv.is_valid() {
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
                                .try_send(
                                    "NIP-05 verification is no longer valid (expired/wrong domain)"
                                        .to_owned(),
                                )
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
                            .try_send("NIP-05 verification needed to publish events".to_owned())
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
                info!(
                    "published ephemeral event {:?} from {:?} in {:?}",
                    event.get_event_id_prefix(),
                    event.get_author_prefix(),
                    start.elapsed()
                );
                bcast_tx.send(event.clone()).ok();
                event_write = true
            } else {
                match write_event(&mut pool.get()?, &event) {
                    Ok(updated) => {
                        if updated == 0 {
                            trace!("ignoring duplicate event");
                        } else {
                            info!(
                                "persisted event {:?} from {:?} in {:?}",
                                event.get_event_id_prefix(),
                                event.get_author_prefix(),
                                start.elapsed()
                            );
                            event_write = true;
                            // send this out to all clients
                            bcast_tx.send(event.clone()).ok();
                        }
                    }
                    Err(err) => {
                        warn!("event insert failed: {:?}", err);
                        notice_tx
                            .try_send(
                                "relay experienced an error trying to publish the latest event"
                                    .to_owned(),
                            )
                            .ok();
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
    // start transaction
    let tx = conn.transaction()?;
    // get relevant fields from event and convert to blobs.
    let id_blob = hex::decode(&e.id).ok();
    let pubkey_blob = hex::decode(&e.pubkey).ok();
    let event_str = serde_json::to_string(&e).ok();
    // ignore if the event hash is a duplicate.
    let ins_count = tx.execute(
        "INSERT OR IGNORE INTO event (event_hash, created_at, kind, author, content, first_seen, hidden) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%s','now'), FALSE);",
        params![id_blob, e.created_at, e.kind, pubkey_blob, event_str]
    )?;
    if ins_count == 0 {
        // if the event was a duplicate, no need to insert event or
        // pubkey references.
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
            // if tagvalue is hex;
            if is_hex(tagval) {
                tx.execute(
                    "INSERT OR IGNORE INTO tag (event_id, name, value_hex) VALUES (?1, ?2, ?3)",
                    params![ev_id, &tagname, hex::decode(&tagval).ok()],
                )?;
            } else {
                tx.execute(
                    "INSERT OR IGNORE INTO tag (event_id, name, value) VALUES (?1, ?2, ?3)",
                    params![ev_id, &tagname, &tagval],
                )?;
            }
        }
    }
    // if this event is replaceable update, hide every other replaceable
    // event with the same kind from the same author that was issued
    // earlier than this.
    if e.kind == 0 || e.kind == 3 || (e.kind >= 10000 && e.kind < 20000) {
        let update_count = tx.execute(
            "UPDATE event SET hidden=TRUE WHERE id!=? AND kind=? AND author=? AND created_at <= ? and hidden!=TRUE",
            params![ev_id, e.kind, hex::decode(&e.pubkey).ok(), e.created_at],
        )?;
        if update_count > 0 {
            info!(
                "hid {} older replaceable kind {} events for author {:?}",
                update_count,
                e.kind,
                e.get_author_prefix()
            );
        }
    }
    // if this event is a deletion, hide the referenced events from the same author.
    if e.kind == 5 {
        let event_candidates = e.tag_values_by_name("e");
        let mut params: Vec<Box<dyn ToSql>> = vec![];
        // first parameter will be author
        params.push(Box::new(hex::decode(&e.pubkey)?));
        event_candidates
            .iter()
            .filter(|x| is_hex(x) && x.len() == 64)
            .filter_map(|x| hex::decode(x).ok())
            .for_each(|x| params.push(Box::new(x)));
        let query = format!(
            "UPDATE event SET hidden=TRUE WHERE author=? AND event_hash IN ({})",
            repeat_vars(params.len() - 1)
        );
        let mut stmt = tx.prepare(&query)?;
        let update_count = stmt.execute(rusqlite::params_from_iter(params))?;
        info!(
            "hid {} deleted events for author {:?}",
            update_count,
            e.get_author_prefix()
        );
    }
    tx.commit()?;
    Ok(ins_count)
}

/// Serialized event associated with a specific subscription request.
#[derive(PartialEq, Debug, Clone)]
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
        let empty_query =
            "SELECT DISTINCT(e.content), e.created_at FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE 1=0"
            .to_owned();
        // query parameters for SQLite
        let empty_params: Vec<Box<dyn ToSql>> = vec![];
        return (empty_query, empty_params);
    }

    let mut query =
        "SELECT DISTINCT(e.content), e.created_at FROM event e LEFT JOIN tag t ON e.id=t.event_id "
            .to_owned();
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
                    auth_searches.push("author=?".to_owned());
                    params.push(Box::new(ex));
                }
                Some(HexSearch::Range(lower, upper)) => {
                    auth_searches.push("(author>? AND author<?)".to_owned());
                    params.push(Box::new(lower));
                    params.push(Box::new(upper));
                }
                Some(HexSearch::LowerOnly(lower)) => {
                    auth_searches.push("author>?".to_owned());
                    params.push(Box::new(lower));
                }
                None => {
                    info!("Could not parse hex range from author {:?}", auth);
                }
            }
        }
        let authors_clause = format!("({})", auth_searches.join(" OR "));
        filter_components.push(authors_clause);
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
        let id_clause = format!("({})", id_searches.join(" OR "));
        filter_components.push(id_clause);
    }
    // Query for tags
    if let Some(map) = &f.tags {
        for (key, val) in map.iter() {
            let mut str_vals: Vec<Box<dyn ToSql>> = vec![];
            let mut blob_vals: Vec<Box<dyn ToSql>> = vec![];
            for v in val {
                if is_hex(v) {
                    if let Ok(h) = hex::decode(&v) {
                        blob_vals.push(Box::new(h));
                    }
                } else {
                    str_vals.push(Box::new(v.to_owned()));
                }
            }
            // create clauses with "?" params for each tag value being searched
            let str_clause = format!("value IN ({})", repeat_vars(str_vals.len()));
            let blob_clause = format!("value_hex IN ({})", repeat_vars(blob_vals.len()));
            let tag_clause = format!("(name=? AND ({} OR {}))", str_clause, blob_clause);
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
        query.push_str(&format!(" ORDER BY e.created_at DESC LIMIT {}", lim))
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
        let (f_subquery, mut f_params) = query_from_filter(&f);
        subqueries.push(f_subquery);
        params.append(&mut f_params);
    }
    // encapsulate subqueries into select statements
    let subqueries_selects: Vec<String> = subqueries
        .iter()
        .map(|s| {
            return format!("SELECT content, created_at FROM ({})", s);
        })
        .collect();
    let query: String = subqueries_selects.join(" UNION ");
    info!("final query string: {}", query);
    (query, params)
}

/// Perform a database query using a subscription.
///
/// The [`Subscription`] is converted into a SQL query.  Each result
/// is published on the `query_tx` channel as it is returned.  If a
/// message becomes available on the `abandon_query_rx` channel, the
/// query is immediately aborted.
pub async fn db_query(
    sub: Subscription,
    pool: SqlitePool,
    query_tx: tokio::sync::mpsc::Sender<QueryResult>,
    mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
) {
    task::spawn_blocking(move || {
        debug!("going to query for: {:?}", sub);
        let mut row_count: usize = 0;
        let start = Instant::now();
        // generate SQL query
        let (q, p) = query_from_sub(&sub);
        debug!("SQL generated in {:?}", start.elapsed());
        // show pool stats
        debug!("DB pool stats: {:?}", pool.state());
        let start = Instant::now();
        if let Ok(conn) = pool.get() {
            // execute the query. Don't cache, since queries vary so much.
            let mut stmt = conn.prepare(&q)?;
            let mut event_rows = stmt.query(rusqlite::params_from_iter(p))?;
            let mut first_result = true;
            while let Some(row) = event_rows.next()? {
                if first_result {
                    debug!("time to first result: {:?}", start.elapsed());
                    first_result = false;
                }
                // check if this is still active
                // TODO:  check every N rows
                if abandon_query_rx.try_recv().is_ok() {
                    debug!("query aborted");
                    return Ok(());
                }
                row_count += 1;
                let event_json = row.get(0)?;
                query_tx
                    .blocking_send(QueryResult {
                        sub_id: sub.get_id(),
                        event: event_json,
                    })
                    .ok();
            }
            query_tx
                .blocking_send(QueryResult {
                    sub_id: sub.get_id(),
                    event: "EOSE".to_string(),
                })
                .ok();
            debug!(
                "query completed ({} rows) in {:?}",
                row_count,
                start.elapsed()
            );
        } else {
            warn!("Could not get a database connection for querying");
        }
        let ok: Result<()> = Ok(());
        ok
    });
}
