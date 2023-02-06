//! Event persistence and querying
//use crate::config::SETTINGS;
use crate::config::Settings;
use crate::error::Result;
use crate::event::{single_char_tagname, Event};
use crate::hexrange::hex_range;
use crate::hexrange::HexSearch;
use crate::repo::sqlite_migration::{STARTUP_SQL,upgrade_db};
use crate::utils::{is_hex, is_lower_hex};
use crate::nip05::{Nip05Name, VerificationRecord};
use crate::subscription::{ReqFilter, Subscription};
use crate::server::NostrMetrics;
use hex;
use r2d2;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use rusqlite::types::ToSql;
use rusqlite::OpenFlags;
use tokio::sync::{Mutex, MutexGuard, Semaphore};
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use tokio::task;
use tracing::{debug, info, trace, warn};
use async_trait::async_trait;
use crate::db::QueryResult;

use crate::repo::{now_jitter, NostrRepo};

pub type SqlitePool = r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>;
pub type PooledConnection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;
pub const DB_FILE: &str = "nostr.db";

#[derive(Clone)]
pub struct SqliteRepo {
    /// Metrics
    metrics: NostrMetrics,
    /// Pool for reading events and NIP-05 status
    read_pool: SqlitePool,
    /// Pool for writing events and NIP-05 verification
    write_pool: SqlitePool,
    /// Pool for performing checkpoints/optimization
    maint_pool: SqlitePool,
    /// Flag to indicate a checkpoint is underway
    checkpoint_in_progress: Arc<Mutex<u64>>,
    /// Flag to limit writer concurrency
    write_in_progress: Arc<Mutex<u64>>,
    /// Semaphore for readers to acquire blocking threads
    reader_threads_ready: Arc<Semaphore>,
}

impl SqliteRepo {
    // build all the pools needed
    #[must_use] pub fn new(settings: &Settings, metrics: NostrMetrics) -> SqliteRepo {
        let write_pool = build_pool(
            "writer",
            settings,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            1,
            2,
            false,
        );
        let maint_pool = build_pool(
            "maintenance",
            settings,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            1,
            2,
            true,
        );
        let read_pool = build_pool(
            "reader",
            settings,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
            settings.database.min_conn,
            settings.database.max_conn,
            true,
        );

        // this is used to block new reads during critical checkpoints
        let checkpoint_in_progress = Arc::new(Mutex::new(0));
        // SQLite can only effectively write single threaded, so don't
        // block multiple worker threads unnecessarily.
        let write_in_progress = Arc::new(Mutex::new(0));
        // configure the number of worker threads that can be spawned
        // to match the number of database reader connections.
        let max_conn = settings.database.max_conn as usize;
        let reader_threads_ready = Arc::new(Semaphore::new(max_conn));
        SqliteRepo {
            metrics,
            read_pool,
            write_pool,
            maint_pool,
            checkpoint_in_progress,
            write_in_progress,
            reader_threads_ready,
        }
    }

    /// Persist an event to the database, returning rows added.
    pub fn persist_event(conn: &mut PooledConnection, e: &Event) -> Result<u64> {
        // enable auto vacuum
        conn.execute_batch("pragma auto_vacuum = FULL")?;

        // start transaction
        let tx = conn.transaction()?;
        // get relevant fields from event and convert to blobs.
        let id_blob = hex::decode(&e.id).ok();
        let pubkey_blob: Option<Vec<u8>> = hex::decode(&e.pubkey).ok();
        let delegator_blob: Option<Vec<u8>> = e.delegated_by.as_ref().and_then(|d| hex::decode(d).ok());
        let event_str = serde_json::to_string(&e).ok();
        // check for replaceable events that would hide this one; we won't even attempt to insert these.
        if e.is_replaceable() {
            let repl_count = tx.query_row(
                "SELECT e.id FROM event e INDEXED BY author_index WHERE e.author=? AND e.kind=? AND e.created_at >= ? LIMIT 1;",
                params![pubkey_blob, e.kind, e.created_at], |row| row.get::<usize, usize>(0));
            if repl_count.ok().is_some() {
                return Ok(0);
            }
        }
        // check for parameterized replaceable events that would be hidden; don't insert these either.
        if let Some(d_tag) = e.distinct_param() {
            let repl_count = if is_lower_hex(&d_tag) && (d_tag.len() % 2 == 0) {
                tx.query_row(
                    "SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.author=? AND e.kind=? AND t.name='d' AND t.value_hex=? AND e.created_at >= ? LIMIT 1;",
                    params![pubkey_blob, e.kind, hex::decode(d_tag).ok(), e.created_at],|row| row.get::<usize, usize>(0))
            } else {
                tx.query_row(
                    "SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.author=? AND e.kind=? AND t.name='d' AND t.value=? AND e.created_at >= ? LIMIT 1;",
                    params![pubkey_blob, e.kind, d_tag, e.created_at],|row| row.get::<usize, usize>(0))
            };
            // if any rows were returned, then some newer event with
            // the same author/kind/tag value exist, and we can ignore
            // this event.
            if repl_count.ok().is_some() {
                return Ok(0)
            }
        }
        // ignore if the event hash is a duplicate.
        let mut ins_count = tx.execute(
            "INSERT OR IGNORE INTO event (event_hash, created_at, kind, author, delegated_by, content, first_seen, hidden) VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'), FALSE);",
            params![id_blob, e.created_at, e.kind, pubkey_blob, delegator_blob, event_str]
        )? as u64;
        if ins_count == 0 {
            // if the event was a duplicate, no need to insert event or
            // pubkey references.
            tx.rollback().ok();
            return Ok(ins_count);
        }
        // remember primary key of the event most recently inserted.
        let ev_id = tx.last_insert_rowid();
        // add all tags to the tag table
        for tag in &e.tags {
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
        // if this event is replaceable update, remove other replaceable
        // event with the same kind from the same author that was issued
        // earlier than this.
        if e.is_replaceable() {
            let author = hex::decode(&e.pubkey).ok();
            // this is a backwards check - hide any events that were older.
            let update_count = tx.execute(
                "DELETE FROM event WHERE kind=? and author=? and id NOT IN (SELECT id FROM event INDEXED BY author_kind_index WHERE kind=? AND author=? ORDER BY created_at DESC LIMIT 1)",
                params![e.kind, author, e.kind, author],
            )?;
            if update_count > 0 {
                info!(
                    "removed {} older replaceable kind {} events for author: {:?}",
                    update_count,
                    e.kind,
                    e.get_author_prefix()
                );
            }
        }
        // if this event is parameterized replaceable, remove other events.
        if let Some(d_tag) = e.distinct_param() {
            let update_count = if is_lower_hex(&d_tag) && (d_tag.len() % 2 == 0) {
                tx.execute(
                    "DELETE FROM event WHERE kind=? AND author=? AND id IN (SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.kind=? AND e.author=? AND t.name='d' AND t.value_hex=? ORDER BY created_at DESC LIMIT -1 OFFSET 1);",
                    params![e.kind, pubkey_blob, e.kind, pubkey_blob, hex::decode(d_tag).ok()])?
            } else {
                tx.execute(
                    "DELETE FROM event WHERE kind=? AND author=? AND id IN (SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.kind=? AND e.author=? AND t.name='d' AND t.value=? ORDER BY created_at DESC LIMIT -1 OFFSET 1);",
                    params![e.kind, pubkey_blob, e.kind, pubkey_blob, d_tag])?
            };
            if update_count > 0 {
                info!(
                    "removed {} older parameterized replaceable kind {} events for author: {:?}",
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
}

#[async_trait]
impl NostrRepo for SqliteRepo {

    async fn start(&self) -> Result<()> {
        db_checkpoint_task(self.maint_pool.clone(), Duration::from_secs(60), self.checkpoint_in_progress.clone()).await
    }

    async fn migrate_up(&self) -> Result<usize> {
        let _write_guard = self.write_in_progress.lock().await;
        let mut conn = self.write_pool.get()?;
        task::spawn_blocking(move || {
            upgrade_db(&mut conn)
        }).await?
    }
    /// Persist event to database
    async fn write_event(&self, e: &Event) -> Result<u64> {
        let start = Instant::now();
        let _write_guard = self.write_in_progress.lock().await;
        // spawn a blocking thread
        //let mut conn = self.write_pool.get()?;
        let pool = self.write_pool.clone();
        let e = e.clone();
        let event_count = task::spawn_blocking(move || {
            let mut conn = pool.get()?;
            SqliteRepo::persist_event(&mut conn, &e)
        }).await?;
        self.metrics
            .write_events
            .observe(start.elapsed().as_secs_f64());
        event_count
    }

    /// Perform a database query using a subscription.
    ///
    /// The [`Subscription`] is converted into a SQL query.  Each result
    /// is published on the `query_tx` channel as it is returned.  If a
    /// message becomes available on the `abandon_query_rx` channel, the
    /// query is immediately aborted.
    async fn query_subscription(
        &self,
        sub: Subscription,
        client_id: String,
        query_tx: tokio::sync::mpsc::Sender<QueryResult>,
        mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let pre_spawn_start = Instant::now();
        // if we let every request spawn a thread, we'll exhaust the
        // thread pool waiting for queries to finish under high load.
        // Instead, don't bother spawning threads when they will just
        // block on a database connection.
        let sem = self.reader_threads_ready.clone().acquire_owned().await.unwrap();
        let self=self.clone();
        let metrics=self.metrics.clone();
        task::spawn_blocking(move || {
            {
                // if we are waiting on a checkpoint, stop until it is complete
                let _x = self.checkpoint_in_progress.blocking_lock();
            }
            let db_queue_time = pre_spawn_start.elapsed();
            // if the queue time was very long (>5 seconds), spare the DB and abort.
            if db_queue_time > Duration::from_secs(5) {
                info!(
                    "shedding DB query load queued for {:?} (cid: {}, sub: {:?})",
                    db_queue_time, client_id, sub.id
                );
                metrics.query_aborts.with_label_values(&["loadshed"]).inc();
                return Ok(());
            }
            // otherwise, report queuing time if it is slow
            else if db_queue_time > Duration::from_secs(1) {
                debug!(
                    "(slow) DB query queued for {:?} (cid: {}, sub: {:?})",
                    db_queue_time, client_id, sub.id
                );
            }
            // check before getting a DB connection if the client still wants the results
            if abandon_query_rx.try_recv().is_ok() {
                debug!("query cancelled by client (before execution) (cid: {}, sub: {:?})", client_id, sub.id);
                return Ok(());
            }

            let start = Instant::now();
            let mut row_count: usize = 0;
            // cutoff for displaying slow queries
            let slow_cutoff = Duration::from_millis(250);
            let mut filter_count = 0;
            // remove duplicates from the filter list.
            if let Ok(mut conn) = self.read_pool.get() {
                {
                    let pool_state = self.read_pool.state();
                    metrics.db_connections.set((pool_state.connections - pool_state.idle_connections).into());
                }
                for filter in sub.filters.iter() {
                    let filter_start = Instant::now();
                    filter_count += 1;
                    let sql_gen_elapsed = start.elapsed();
                    let (q, p, idx) = query_from_filter(filter);
                    if sql_gen_elapsed > Duration::from_millis(10) {
                        debug!("SQL (slow) generated in {:?}", filter_start.elapsed());
                    }
                    // any client that doesn't cause us to generate new rows in 2
                    // seconds gets dropped.
                    let abort_cutoff = Duration::from_secs(2);
                    let mut slow_first_event;
                    let mut last_successful_send = Instant::now();
                    // execute the query.
                    // make the actual SQL query (with parameters inserted) available
                    conn.trace(Some(|x| {trace!("SQL trace: {:?}", x)}));
                    let mut stmt = conn.prepare_cached(&q)?;
                    let mut event_rows = stmt.query(rusqlite::params_from_iter(p))?;

                    let mut first_result = true;
                    while let Some(row) = event_rows.next()? {
                        let first_event_elapsed = filter_start.elapsed();
                        slow_first_event = first_event_elapsed >= slow_cutoff;
                        if first_result {
                            debug!(
                                "first result in {:?} (cid: {}, sub: {:?}, filter: {}) [used index: {:?}]",
                                first_event_elapsed, client_id, sub.id, filter_count, idx
                            );
                            // logging for slow queries; show filter and SQL.
                            // to reduce logging; only show 1/16th of clients (leading 0)
                            if slow_first_event && client_id.starts_with('0') {
                                debug!(
                                    "filter first result in {:?} (slow): {} (cid: {}, sub: {:?})",
                                    first_event_elapsed, serde_json::to_string(&filter)?, client_id, sub.id
                                );
                            }
                            first_result = false;
                        }
                        // check if a checkpoint is trying to run, and abort
                        if row_count % 100 == 0 {
                            {
                                if self.checkpoint_in_progress.try_lock().is_err() {
                                    // lock was held, abort this query
                                    debug!("query aborted due to checkpoint (cid: {}, sub: {:?})", client_id, sub.id);
                                    metrics.query_aborts.with_label_values(&["checkpoint"]).inc();
                                    return Ok(());
                                }
                            }
                        }

                        // check if this is still active; every 100 rows
                        if row_count % 100 == 0 && abandon_query_rx.try_recv().is_ok() {
                            debug!("query cancelled by client (cid: {}, sub: {:?})", client_id, sub.id);
                            return Ok(());
                        }
                        row_count += 1;
                        let event_json = row.get(0)?;
                        loop {
                            if query_tx.capacity() != 0 {
                                // we have capacity to add another item
                                break;
                            }
                            // the queue is full
                            trace!("db reader thread is stalled");
                            if last_successful_send + abort_cutoff < Instant::now() {
                                // the queue has been full for too long, abort
                                info!("aborting database query due to slow client (cid: {}, sub: {:?})",
                                      client_id, sub.id);
                                metrics.query_aborts.with_label_values(&["slowclient"]).inc();
                                let ok: Result<()> = Ok(());
                                return ok;
                            }
                            // check if a checkpoint is trying to run, and abort
                            if self.checkpoint_in_progress.try_lock().is_err() {
                                // lock was held, abort this query
                                debug!("query aborted due to checkpoint (cid: {}, sub: {:?})", client_id, sub.id);
                                metrics.query_aborts.with_label_values(&["checkpoint"]).inc();
                                return Ok(());
                            }
                            // give the queue a chance to clear before trying again
                            debug!("query thread sleeping due to full query_tx (cid: {}, sub: {:?})", client_id, sub.id);
                            thread::sleep(Duration::from_millis(500));
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
                    metrics
                        .query_db
                        .observe(filter_start.elapsed().as_secs_f64());
                    // if the filter took too much db_time, print out the JSON.
                    if filter_start.elapsed() > slow_cutoff && client_id.starts_with('0') {
                        debug!(
                            "query filter req (slow): {} (cid: {}, sub: {:?}, filter: {})",
                            serde_json::to_string(&filter)?, client_id, sub.id, filter_count
                        );
                    }

                }
            } else {
                warn!("Could not get a database connection for querying");
            }
            drop(sem); // new query can begin
            debug!(
                "query completed in {:?} (cid: {}, sub: {:?}, db_time: {:?}, rows: {})",
                pre_spawn_start.elapsed(),
                client_id,
                sub.id,
                start.elapsed(),
                row_count
            );
            query_tx
                .blocking_send(QueryResult {
                    sub_id: sub.get_id(),
                    event: "EOSE".to_string(),
                })
                .ok();
            metrics
                .query_sub
                .observe(pre_spawn_start.elapsed().as_secs_f64());
            let ok: Result<()> = Ok(());
            ok
        });
        Ok(())
    }

    /// Perform normal maintenance
    async fn optimize_db(&self) -> Result<()> {
        let conn = self.write_pool.get()?;
        task::spawn_blocking(move || {
            let start = Instant::now();
            conn.execute_batch("PRAGMA optimize;").ok();
            info!("optimize ran in {:?}", start.elapsed());
        }).await?;
        Ok(())
    }

    /// Create a new verification record connected to a specific event
    async fn create_verification_record(&self, event_id: &str, name: &str) -> Result<()> {
        let e = hex::decode(event_id).ok();
        let n = name.to_owned();
        let mut conn = self.write_pool.get()?;
        tokio::task::spawn_blocking(move || {
            let tx = conn.transaction()?;
            {
                // if we create a /new/ one, we should get rid of any old ones.  or group the new ones by name and only consider the latest.
                let query = "INSERT INTO user_verification (metadata_event, name, verified_at) VALUES ((SELECT id from event WHERE event_hash=?), ?, strftime('%s','now'));";
                let mut stmt = tx.prepare(query)?;
                stmt.execute(params![e, n])?;
                // get the row ID
                let v_id = tx.last_insert_rowid();
                // delete everything else by this name
                let del_query = "DELETE FROM user_verification WHERE name = ? AND id != ?;";
                let mut del_stmt = tx.prepare(del_query)?;
                let count = del_stmt.execute(params![n,v_id])?;
                if count > 0 {
                    info!("removed {} old verification records for ({:?})", count, n);
                }
            }
            tx.commit()?;
            info!("saved new verification record for ({:?})", n);
            let ok: Result<()> = Ok(());
            ok
        }).await?
    }

    /// Update verification timestamp
    async fn update_verification_timestamp(&self, id: u64) -> Result<()> {
        let mut conn = self.write_pool.get()?;
        tokio::task::spawn_blocking(move || {
            // add some jitter to the verification to prevent everything from stacking up together.
            let verif_time = now_jitter(600);
            let tx = conn.transaction()?;
            {
                // update verification time and reset any failure count
                let query =
                    "UPDATE user_verification SET verified_at=?, failure_count=0 WHERE id=?";
                let mut stmt = tx.prepare(query)?;
                stmt.execute(params![verif_time, id])?;
            }
            tx.commit()?;
            let ok: Result<()> = Ok(());
            ok
        })
            .await?

    }

    /// Update verification record as failed
    async fn fail_verification(&self, id: u64) -> Result<()> {
        let mut conn = self.write_pool.get()?;
        tokio::task::spawn_blocking(move || {
            // add some jitter to the verification to prevent everything from stacking up together.
            let fail_time = now_jitter(600);
            let tx = conn.transaction()?;
            {
                let query = "UPDATE user_verification SET failed_at=?, failure_count=failure_count+1 WHERE id=?";
                let mut stmt = tx.prepare(query)?;
                stmt.execute(params![fail_time, id])?;
            }
            tx.commit()?;
            let ok: Result<()> = Ok(());
            ok
        })
            .await?
    }

    /// Delete verification record
    async fn delete_verification(&self, id: u64) -> Result<()> {
        let mut conn = self.write_pool.get()?;
        tokio::task::spawn_blocking(move || {
            let tx = conn.transaction()?;
            {
                let query = "DELETE FROM user_verification WHERE id=?;";
                let mut stmt = tx.prepare(query)?;
                stmt.execute(params![id])?;
            }
            tx.commit()?;
            let ok: Result<()> = Ok(());
            ok
        })
            .await?
    }

    /// Get the latest verification record for a given pubkey.
    async fn get_latest_user_verification(&self, pub_key: &str) -> Result<VerificationRecord> {
        let mut conn = self.read_pool.get()?;
        let pub_key = pub_key.to_owned();
        tokio::task::spawn_blocking(move || {
            let tx = conn.transaction()?;
            let query = "SELECT v.id, v.name, e.event_hash, e.created_at, v.verified_at, v.failed_at, v.failure_count FROM user_verification v LEFT JOIN event e ON e.id=v.metadata_event WHERE e.author=? ORDER BY e.created_at DESC, v.verified_at DESC, v.failed_at DESC LIMIT 1;";
            let mut stmt = tx.prepare_cached(query)?;
            let fields = stmt.query_row(params![hex::decode(&pub_key).ok()], |r| {
                let rowid: u64 = r.get(0)?;
                let rowname: String = r.get(1)?;
                let eventid: Vec<u8> = r.get(2)?;
                let created_at: u64 = r.get(3)?;
                // create a tuple since we can't throw non-rusqlite errors in this closure
                Ok((
                    rowid,
                    rowname,
                    eventid,
                    created_at,
                    r.get(4).ok(),
                    r.get(5).ok(),
                    r.get(6)?,
                ))
            })?;
            Ok(VerificationRecord {
                rowid: fields.0,
                name: Nip05Name::try_from(&fields.1[..])?,
                address: pub_key,
                event: hex::encode(fields.2),
                event_created: fields.3,
                last_success: fields.4,
                last_failure: fields.5,
                failure_count: fields.6,
            })
        }).await?
    }

    /// Get oldest verification before timestamp
    async fn get_oldest_user_verification(&self, before: u64) -> Result<VerificationRecord> {
        let mut conn = self.read_pool.get()?;
        tokio::task::spawn_blocking(move || {
            let tx = conn.transaction()?;
            let query = "SELECT v.id, v.name, e.event_hash, e.author, e.created_at, v.verified_at, v.failed_at, v.failure_count FROM user_verification v INNER JOIN event e ON e.id=v.metadata_event WHERE (v.verified_at < ? OR v.verified_at IS NULL) AND (v.failed_at < ? OR v.failed_at IS NULL) ORDER BY v.verified_at ASC, v.failed_at ASC LIMIT 1;";
            let mut stmt = tx.prepare_cached(query)?;
            let fields = stmt.query_row(params![before, before], |r| {
                let rowid: u64 = r.get(0)?;
                let rowname: String = r.get(1)?;
                let eventid: Vec<u8> = r.get(2)?;
                let pubkey: Vec<u8> = r.get(3)?;
                let created_at: u64 = r.get(4)?;
                // create a tuple since we can't throw non-rusqlite errors in this closure
                Ok((
                    rowid,
                    rowname,
                    eventid,
                    pubkey,
                    created_at,
                    r.get(5).ok(),
                    r.get(6).ok(),
                    r.get(7)?,
                ))
            })?;
            let vr = VerificationRecord {
                rowid: fields.0,
                name: Nip05Name::try_from(&fields.1[..])?,
                address: hex::encode(fields.3),
                event: hex::encode(fields.2),
                event_created: fields.4,
                last_success: fields.5,
                last_failure: fields.6,
                failure_count: fields.7,
            };
            Ok(vr)
        }).await?
    }
}

/// Decide if there is an index that should be used explicitly
fn override_index(f: &ReqFilter) -> Option<String> {
    if f.ids.is_some() {
        return Some("event_hash_index".into());
    }
    // queries for multiple kinds default to kind_index, which is
    // significantly slower than kind_created_at_index.
    if let Some(ks) = &f.kinds {
        if f.ids.is_none() &&
            ks.len() > 1 &&
            f.since.is_none() &&
            f.until.is_none() &&
            f.tags.is_none() &&
            f.authors.is_none() {
                return Some("kind_created_at_index".into());
            }
    }
    // if there is an author, it is much better to force the authors index.
    if f.authors.is_some() {
        if f.since.is_none() && f.until.is_none() && f.limit.is_none() {
            if f.kinds.is_none() {
                // with no use of kinds/created_at, just author
                return Some("author_index".into());
            }
            // prefer author_kind if there are kinds
            return Some("author_kind_index".into());
        }
        // finally, prefer author_created_at if time is provided
        return Some("author_created_at_index".into());
    }
    None
}

/// Create a dynamic SQL subquery and params from a subscription filter (and optional explicit index used)
fn query_from_filter(f: &ReqFilter) -> (String, Vec<Box<dyn ToSql>>, Option<String>) {
    // build a dynamic SQL query.  all user-input is either an integer
    // (sqli-safe), or a string that is filtered to only contain
    // hexadecimal characters.  Strings that require escaping (tag
    // names/values) use parameters.

    // if the filter is malformed, don't return anything.
    if f.force_no_match {
        let empty_query = "SELECT e.content FROM event e WHERE 1=0".to_owned();
        // query parameters for SQLite
        let empty_params: Vec<Box<dyn ToSql>> = vec![];
        return (empty_query, empty_params, None);
    }

    // check if the index needs to be overriden
    let idx_name = override_index(f);
    let idx_stmt = idx_name.as_ref().map_or_else(|| "".to_owned(), |i| format!("INDEXED BY {i}"));
    let mut query = format!("SELECT e.content FROM event e {idx_stmt}");
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
                    auth_searches.push(
                        "(author>? AND author<?)".to_owned(),
                    );
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
        if !authvec.is_empty() {
            let auth_clause = format!("({})", auth_searches.join(" OR "));
            filter_components.push(auth_clause);
        } else {
            filter_components.push("false".to_owned());
        }
    }
    // Query for Kind
    if let Some(ks) = &f.kinds {
        // kind is number, no escaping needed
        let str_kinds: Vec<String> = ks.iter().map(std::string::ToString::to_string).collect();
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
        if idvec.is_empty() {
            // if the ids list was empty, we should never return
            // any results.
            filter_components.push("false".to_owned());
        } else {
            let id_clause = format!("({})", id_searches.join(" OR "));
            filter_components.push(id_clause);
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
                    str_vals.push(Box::new(v.clone()));
                }
            }
            // do not mix value and value_hex; this is a temporary special case.
            if str_vals.is_empty() {
                // create clauses with "?" params for each tag value being searched
                let blob_clause = format!("value_hex IN ({})", repeat_vars(blob_vals.len()));
                // find evidence of the target tag name/value existing for this event.
                let tag_clause = format!(
                    "e.id IN (SELECT t.event_id FROM tag t WHERE (name=? AND {blob_clause}))",
                );
                // add the tag name as the first parameter
                params.push(Box::new(key.to_string()));
                // add all tag values that are blobs as params
                params.append(&mut blob_vals);
                filter_components.push(tag_clause);
            } else if blob_vals.is_empty() {
                // create clauses with "?" params for each tag value being searched
                let str_clause = format!("value IN ({})", repeat_vars(str_vals.len()));
                // find evidence of the target tag name/value existing for this event.
                let tag_clause = format!(
		            "e.id IN (SELECT t.event_id FROM tag t WHERE (name=? AND {str_clause}))",
                );
                // add the tag name as the first parameter
                params.push(Box::new(key.to_string()));
                // add all tag values that are blobs as params
                params.append(&mut str_vals);
                filter_components.push(tag_clause);
            } else {
                debug!("mixed string/blob query");
                // create clauses with "?" params for each tag value being searched
                let str_clause = format!("value IN ({})", repeat_vars(str_vals.len()));
                let blob_clause = format!("value_hex IN ({})", repeat_vars(blob_vals.len()));
                // find evidence of the target tag name/value existing for this event.
                let tag_clause = format!(
		            "e.id IN (SELECT t.event_id FROM tag t WHERE (name=? AND ({str_clause} OR {blob_clause})))",
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
        let _ = write!(query, " ORDER BY e.created_at DESC LIMIT {lim}");
    } else {
        query.push_str(" ORDER BY e.created_at ASC");
    }
    (query, params, idx_name)
}

/// Create a dynamic SQL query string and params from a subscription.
fn _query_from_sub(sub: &Subscription) -> (String, Vec<Box<dyn ToSql>>, Vec<String>) {
    // build a dynamic SQL query for an entire subscription, based on
    // SQL subqueries for filters.
    let mut subqueries: Vec<String> = Vec::new();
    let mut indexes = vec![];
    // subquery params
    let mut params: Vec<Box<dyn ToSql>> = vec![];
    // for every filter in the subscription, generate a subquery
    for f in &sub.filters {
        let (f_subquery, mut f_params, index) = query_from_filter(f);
        if let Some(i) = index {
            indexes.push(i);
        }
        subqueries.push(f_subquery);
        params.append(&mut f_params);
    }
    // encapsulate subqueries into select statements
    let subqueries_selects: Vec<String> = subqueries
        .iter()
        .map(|s| format!("SELECT distinct content, created_at FROM ({s})"))
        .collect();
    let query: String = subqueries_selects.join(" UNION ");
    (query, params,indexes)
}

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

/// Perform database WAL checkpoint on a regular basis
pub async fn db_checkpoint_task(pool: SqlitePool, frequency: Duration, checkpoint_in_progress: Arc<Mutex<u64>>) -> Result<()> {
    // TODO; use acquire_many on the reader semaphore to stop them from interrupting this.
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
                _ = tokio::time::sleep(frequency) => {
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
                            _guard = Some(checkpoint_in_progress.lock().await);
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

/// Display database pool stats every 1 minute
pub async fn monitor_pool(name: &str, pool: SqlitePool) {
    let sleep_dur = Duration::from_secs(60);
    loop {
        log_pool_stats(name, &pool);
        tokio::time::sleep(sleep_dur).await;
    }
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


/// Check if the pool is fully utilized
fn _pool_at_capacity(pool: &SqlitePool) -> bool {
    let state: r2d2::State = pool.state();
    state.idle_connections == 0
}
