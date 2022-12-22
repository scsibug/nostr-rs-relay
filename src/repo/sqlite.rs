use std::borrow::BorrowMut;
use crate::db::QueryResult;
use crate::error::Result;
use crate::event::{single_char_tagname, Event};
use crate::hexrange::{hex_range, HexSearch};
use crate::nip05::{Nip05Name, VerificationRecord};
use crate::subscription::{ReqFilter, Subscription};
use crate::utils::{is_hex, is_lower_hex, unix_time};
use async_trait::async_trait;
use futures_util::StreamExt;
use hex;
use rand::Rng;
use sqlx::sqlite::{SqliteRow};
use sqlx::{Error, Execute, FromRow, QueryBuilder, Row, Sqlite, SqlitePool};
use std::time::{Duration, Instant};
use tracing::{debug, info, trace};

use crate::repo::{Nip05Repo, NostrRepo, RepoMigrate};
use crate::repo::sqlite_migration::upgrade_db;

#[derive(Clone)]
pub struct SqliteRepo {
    conn: SqlitePool
}

impl SqliteRepo {
    pub fn new(c: SqlitePool) -> SqliteRepo {
        SqliteRepo { conn: c }
    }
}

#[async_trait]
impl RepoMigrate for SqliteRepo {
    async fn migrate_up(&mut self) -> Result<usize>{
        upgrade_db(&self.conn).await
    }
}

#[async_trait]
impl NostrRepo for SqliteRepo {
    async fn write_event(&mut self, e: &Event) -> Result<u64> {
        // start transaction
        let mut tx = self.conn.begin().await?;

        // get relevant fields from event and convert to blobs.
        let id_blob = hex::decode(&e.id).ok();
        let pubkey_blob: Option<Vec<u8>> = hex::decode(&e.pubkey).ok();
        let delegator_blob: Option<Vec<u8>> =
            e.delegated_by.as_ref().and_then(|d| hex::decode(d).ok());
        let event_str = serde_json::to_string(&e).ok();

        // ignore if the event hash is a duplicate.
        let mut ins_count = sqlx::query(
            "INSERT OR IGNORE INTO event (event_hash, created_at, kind, author, delegated_by, content, first_seen, hidden) VALUES (?, ?, ?, ?, ?, ?, strftime('%s','now'), FALSE);",
        )
            .bind(&id_blob)
            .bind(e.created_at as i64)
            .bind(e.kind as i64)
            .bind(&pubkey_blob)
            .bind(delegator_blob)
            .bind(event_str)
            .execute(&mut tx)
            .await?
            .rows_affected();

        if ins_count == 0 {
            // if the event was a duplicate, no need to insert event or
            // pubkey references.  This will abort the txn.
            return Ok(0);
        }

        // remember primary key of the event most recently inserted.
        let ev_id = sqlx::query_scalar::<_, i64>("SELECT last_insert_rowid()")
            .fetch_one(&mut tx)
            .await?;

        // add all tags to the tag table
        for tag in e.tags.iter() {
            // ensure we have 2 values.
            if tag.len() >= 2 {
                let tag_name = &tag[0];
                let tag_val = &tag[1];
                // only single-char tags are searchable
                let tag_char_opt = single_char_tagname(tag_name);
                match &tag_char_opt {
                    Some(_) => {
                        // if tag value is lowercase hex;
                        if is_lower_hex(tag_val) && (tag_val.len() % 2 == 0) {
                            sqlx::query("INSERT OR IGNORE INTO tag (event_id, name, value_hex) VALUES (?1, ?2, ?3)")
                                .bind(ev_id)
                                .bind(&tag_name)
                                .bind(hex::decode(tag_val).ok())
                                .execute(&mut tx)
                                .await?;
                        } else {
                            sqlx::query("INSERT OR IGNORE INTO tag (event_id, name, value) VALUES (?1, ?2, ?3)")
                                .bind(ev_id)
                                .bind(&tag_name)
                                .bind(&tag_val)
                                .execute(&mut tx)
                                .await?;
                        }
                    }
                    None => {}
                }
            }
        }

        // if this event is replaceable update, hide every other replaceable
        // event with the same kind from the same author that was issued
        // earlier than this.
        if e.kind == 0 || e.kind == 3 || (e.kind >= 10000 && e.kind < 20000) {
            let update_count = sqlx::query("UPDATE event SET hidden = TRUE WHERE id != ?1 AND kind = ?2 AND author = ?3 AND created_at <= ?4 and hidden != TRUE")
                .bind(ev_id)
                .bind(e.kind as i64)
                .bind(hex::decode(&e.pubkey).ok())
                .bind(e.created_at as i64)
                .execute(&mut tx)
                .await?
                .rows_affected();
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
            let pubKeys: Vec<Vec<u8>> = event_candidates
                .iter()
                .filter(|x| is_hex(x) && x.len() == 64)
                .filter_map(|x| hex::decode(x).ok())
                .collect();

            let mut builder =
                QueryBuilder::new("UPDATE event SET hidden = TRUE WHERE kind != 5 AND author = ");
            builder.push_bind(hex::decode(&e.pubkey).ok());
            builder.push(" AND event_hash IN (");

            let mut sep = builder.separated(", ");
            for pk in pubKeys {
                sep.push_bind(pk);
            }
            sep.push_unseparated(")");

            let update_count = builder.build().execute(&mut tx).await?.rows_affected();
            info!(
                "hid {} deleted events for author {:?}",
                update_count,
                e.get_author_prefix()
            );
        } else {
            // check if a deletion has already been recorded for this event.
            // Only relevant for non-deletion events
            let del_count = sqlx::query("SELECT e.id FROM event e LEFT JOIN tag t ON e.id = t.event_id WHERE e.author = ? AND t.name = 'e' AND e.kind = 5 AND t.value_hex = ? LIMIT 1")
                .bind(&pubkey_blob)
                .bind(&id_blob)
                .fetch_optional(&mut tx)
                .await?;

            // check if a the query returned a result, meaning we should
            // hid the current event
            if del_count.is_some() {
                // a deletion already existed, mark original event as hidden.
                info!(
                    "hid event: {:?} due to existing deletion by author: {:?}",
                    e.get_event_id_prefix(),
                    e.get_author_prefix()
                );
                sqlx::query("UPDATE event SET hidden = TRUE WHERE id = ?")
                    .bind(ev_id)
                    .execute(&mut tx)
                    .await?;
                // event was deleted, so let caller know nothing new
                // arrived, preventing this from being sent to active
                // subscriptions
                ins_count = 0;
            }
        }
        tx.commit().await?;
        Ok(ins_count)
    }

    async fn query_subscription(
        &mut self,
        sub: Subscription,
        client_id: String,
        query_tx: tokio::sync::mpsc::Sender<QueryResult>,
        mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<()> {
        let start = Instant::now();
        let mut row_count: usize = 0;

        for filter in sub.filters.iter() {
            let start = Instant::now();
            // generate SQL query
            let q_filter = query_from_filter(filter);
            if q_filter.is_none() {
                debug!("Failed to generate query!");
                continue;
            }

            debug!("SQL generated in {:?}", start.elapsed());

            // cutoff for displaying slow queries
            let slow_cutoff = Duration::from_millis(2000);

            // any client that doesn't cause us to generate new rows in 5
            // seconds gets dropped.
            let abort_cutoff = Duration::from_secs(5);

            let start = Instant::now();
            let mut slow_first_event;
            let mut last_successful_send = Instant::now();

            // execute the query. Don't cache, since queries vary so much.
            let mut q_filter = q_filter.unwrap();
            let q_build = q_filter.build();
            let sql = q_build.sql();
            let mut results = q_build.fetch(&self.conn);

            let mut first_result = true;
            while let Some(row) = results.next().await {
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
                if slow_first_event && client_id.starts_with("00") {
                    debug!(
                        "query req (slow): {:?} (cid: {}, sub: {:?})",
                        &sub, client_id, sub.id
                    );
                    debug!(
                        "query string (slow): {} (cid: {}, sub: {:?})",
                        sql, client_id, sub.id
                    );
                } else {
                    trace!(
                        "query req: {:?} (cid: {}, sub: {:?})",
                        &sub,
                        client_id,
                        sub.id
                    );
                    trace!(
                        "query string: {} (cid: {}, sub: {:?})",
                        sql,
                        client_id,
                        sub.id
                    );
                }

                // check if this is still active; every 100 rows
                if row_count % 100 == 0 && abandon_query_rx.try_recv().is_ok() {
                    debug!("query aborted (cid: {}, sub: {:?})", client_id, sub.id);
                    return Ok(());
                }

                row_count += 1;
                let event_json: String = row.unwrap().get(0);
                loop {
                    if query_tx.capacity() != 0 {
                        // we have capacity to add another item
                        break;
                    } else {
                        // the queue is full
                        trace!("db reader thread is stalled");
                        if last_successful_send + abort_cutoff < Instant::now() {
                            // the queue has been full for too long, abort
                            info!("aborting database query due to slow client");
                            return Ok(());
                        }
                        // give the queue a chance to clear before trying again
                        async_std::task::sleep(Duration::from_millis(100)).await;
                    }
                }

                // TODO: we could use try_send, but we'd have to juggle
                // getting the query result back as part of the error
                // result.
                query_tx
                    .send(QueryResult {
                        sub_id: sub.get_id(),
                        event: event_json,
                    })
                    .await.ok();
                last_successful_send = Instant::now();
            }
        }
        query_tx
            .send(QueryResult {
                sub_id: sub.get_id(),
                event: "EOSE".to_string(),
            })
            .await.ok();
        debug!(
            "query completed in {:?} (cid: {}, sub: {:?}, db_time: {:?}, rows: {})",
            start.elapsed(),
            client_id,
            sub.id,
            start.elapsed(),
            row_count
        );
        Ok(())
    }

    async fn optimize_db(&mut self) -> Result<()> {
        sqlx::query("PRAGMA optimize").execute(&self.conn).await?;
        Ok(())
    }
}

#[async_trait]
impl Nip05Repo for SqliteRepo {
    async fn create_verification_record(&mut self, event_id: &str, name: &str) -> Result<()> {
        let mut tx = self.conn.begin().await?;

        // if we create a /new/ one, we should get rid of any old ones.  or group the new ones by name and only consider the latest.
        sqlx::query(r#"
        DELETE FROM user_verification WHERE name = ?2;
        INSERT INTO user_verification (metadata_event, name, verified_at) VALUES ((SELECT id from event WHERE event_hash = ?1, ?2, strftime('%s','now'))"#)
            .bind(hex::decode(event_id).ok())
            .bind(name)
            .execute(&mut tx)
            .await?;

        tx.commit().await?;
        info!("saved new verification record for ({:?})", name);
        Ok(())
    }

    async fn update_verification_timestamp(&mut self, id: u64) -> Result<()> {
        // add some jitter to the verification to prevent everything from stacking up together.
        let verify_time = now_jitter(600);
        let mut tx = self.conn.begin().await?;

        // update verification time and reset any failure count
        sqlx::query("UPDATE user_verification SET verified_at = ?, failure_count = 0 WHERE id = ?")
            .bind(verify_time as i64)
            .bind(id as i64)
            .execute(&mut tx)
            .await?;

        tx.commit().await?;
        info!("verification updated for {}", id);
        Ok(())
    }

    async fn fail_verification(&mut self, id: u64) -> Result<()> {
        sqlx::query("UPDATE user_verification SET failed_at = ?, failure_count = failure_count + 1 WHERE id = ?")
            .bind(unix_time() as i64)
            .bind(id as i64)
            .execute(&self.conn)
            .await?;
        Ok(())
    }

    async fn delete_verification(&mut self, id: u64) -> Result<()> {
        sqlx::query("DELETE FROM user_verification WHERE id = ?")
            .bind(id as i64)
            .execute(&self.conn)
            .await?;
        Ok(())
    }

    async fn get_latest_user_verification(&mut self, pub_key: &str) -> Result<VerificationRecord> {
        let query = r#"SELECT
            v.id,
            v.name,
            e.event_hash,
            e.author,
            e.created_at,
            v.verified_at,
            v.failed_at,
            v.failure_count
            FROM user_verification v
            LEFT JOIN event e ON e.id = v.metadata_event
            WHERE e.author = ?
            ORDER BY e.created_at DESC, v.verified_at DESC, v.failed_at DESC
            LIMIT 1"#;
        let res = sqlx::query_as::<_, VerificationRecord>(query)
            .bind(hex::decode(pub_key).ok())
            .fetch_one(&self.conn)
            .await?;
        Ok(res)
    }

    async fn get_oldest_user_verification(&mut self, before: u64) -> Result<VerificationRecord> {
        let query = r#"SELECT
            v.id,
            v.name,
            e.event_hash,
            e.author,
            e.created_at,
            v.verified_at,
            v.failed_at,
            v.failure_count
            FROM user_verification v
            LEFT JOIN event e ON e.id = v.metadata_event
                WHERE (v.verified_at < ?1 OR v.verified_at IS NULL)
                AND (v.failed_at < ?1 OR v.failed_at IS NULL)
            ORDER BY v.verified_at ASC, v.failed_at ASC
            LIMIT 1"#;
        let res = sqlx::query_as::<_, VerificationRecord>(query)
            .bind(before as i64)
            .fetch_one(&self.conn)
            .await?;
        Ok(res)
    }
}

// Current time, with a slight foward jitter in seconds
fn now_jitter(sec: u64) -> u64 {
    // random time between now, and 10min in future.
    let mut rng = rand::thread_rng();
    let jitter_amount = rng.gen_range(0..sec);
    let now = unix_time();
    now.saturating_add(jitter_amount)
}

/// Create a dynamic SQL query and params from a subscription filter.
fn query_from_filter(f: &ReqFilter) -> Option<QueryBuilder<Sqlite>> {
    // if the filter is malformed, don't return anything.
    if f.force_no_match {
        return None;
    }

    let mut query = QueryBuilder::new("SELECT e.content, e.created_at FROM event e WHERE ");

    let mut push_and = false;
    // Query for "authors", allowing prefix matches
    if let Some(auth_vec) = &f.authors {
        let mut range_authors = query.separated(" OR ");
        for auth in auth_vec {
            match hex_range(auth) {
                Some(HexSearch::Exact(ex)) => {
                    range_authors
                        .push("(e.author = ")
                        .push_bind_unseparated(ex.clone())
                        .push_unseparated(" OR e.delegated_by = ")
                        .push_bind_unseparated(ex)
                        .push_unseparated(")");
                }
                Some(HexSearch::Range(lower, upper)) => {
                    range_authors
                        .push("(e.author > ")
                        .push_bind_unseparated(lower.clone())
                        .push_unseparated(" AND e.author < ")
                        .push_bind_unseparated(upper.clone())
                        .push_unseparated(" OR (e.delegated_by > ")
                        .push_bind_unseparated(lower)
                        .push_unseparated(" AND e.delegated_by < ")
                        .push_bind_unseparated(upper)
                        .push_unseparated(")");
                }
                Some(HexSearch::LowerOnly(lower)) => {
                    range_authors
                        .push("(e.author > ")
                        .push_bind_unseparated(lower.clone())
                        .push_unseparated(" OR e.delegated_by > ")
                        .push_bind_unseparated(lower)
                        .push_unseparated(")");
                }
                None => {
                    info!("Could not parse hex range from author {:?}", auth);
                }
            }
            push_and = true;
        }
    }

    // Query for Kind
    if let Some(ks) = &f.kinds {
        if !ks.is_empty() {
            if push_and {
                query.push(" AND ");
            }
            push_and = true;
            // kind is number, no escaping needed
            let str_kinds: Vec<String> = ks.iter().map(|x| x.to_string()).collect();

            query.push("e.kind in (");
            let mut list_query = query.separated(", ");
            for k in str_kinds {
                list_query.push_bind(k);
            }
            query.push(")");
        }
    }

    // Query for event, allowing prefix matches
    if let Some(id_vec) = &f.ids {
        if !id_vec.is_empty() {
            if push_and {
                query.push(" AND ");
            }
            push_and = true;

            // take each author and convert to a hex search
            let mut id_query = query.separated(" OR ");
            for id in id_vec {
                match hex_range(id) {
                    Some(HexSearch::Exact(ex)) => {
                        id_query
                            .push("(event_hash = ")
                            .push_bind_unseparated(ex)
                            .push_unseparated(")");
                    }
                    Some(HexSearch::Range(lower, upper)) => {
                        id_query
                            .push("(event_hash > ")
                            .push_bind_unseparated(lower)
                            .push_unseparated(" AND event_hash < ")
                            .push_bind_unseparated(upper)
                            .push_unseparated(")");
                    }
                    Some(HexSearch::LowerOnly(lower)) => {
                        id_query
                            .push("(event_hash > ")
                            .push_bind_unseparated(lower)
                            .push_unseparated(")");
                    }
                    None => {
                        info!("Could not parse hex range from id {:?}", id);
                    }
                }
            }
        }
    }

    // Query for tags
    if let Some(map) = &f.tags {
        if !map.is_empty() {
            if push_and {
                query.push(" AND ");
            }
            push_and = true;

            for (key, val) in map.iter() {
                query.push("e.id IN (SELECT ee.id FROM event ee LEFT JOIN tag t on ee.id = t.event_id WHERE ee.hidden != TRUE and (t.name = ")
                    .push_bind(key.to_string())
                    .push(" AND (value in (");

                // plain value match first
                let mut tag_query = query.separated(", ");
                for v in val
                    .iter()
                    .filter(|a| (a.len() % 2 != 0) && !is_lower_hex(a))
                {
                    tag_query.push_bind(v);
                }
                query.push(") OR value_hex in (");

                // hex value match
                let mut tag_query = query.separated(", ");
                for v in val.iter().filter(|a| (a.len() % 2 == 0) && is_lower_hex(a)) {
                    tag_query.push_bind(v);
                }
                query.push("))))");
            }
        }
    }

    // Query for timestamp
    if f.since.is_some() {
        if push_and {
            query.push(" AND ");
        }
        push_and = true;
        query
            .push("e.created_at > ")
            .push_bind(f.since.unwrap() as i64);
    }

    // Query for timestamp
    if f.until.is_some() {
        if push_and {
            query.push(" AND ");
        }
        push_and = true;
        query
            .push("e.created_at < ")
            .push_bind(f.until.unwrap() as i64);
    }

    // never display hidden events
    if push_and {
        query.push(" AND hidden != TRUE ");
    } else {
        query.push("hidden != TRUE");
    }

    // Apply per-filter limit to this query.
    // The use of a LIMIT implies a DESC order, to capture only the most recent events.
    if let Some(lim) = f.limit {
        query.push(" ORDER BY e.created_at DESC LIMIT ");
        query.push(lim);
    } else {
        query.push(" ORDER BY e.created_at ASC");
    }
    Some(query)
}

fn log_pool_stats(pool: &SqlitePool) {
    todo!()
}

impl FromRow<'_, SqliteRow> for VerificationRecord {
    fn from_row(row: &'_ SqliteRow) -> std::result::Result<Self, Error> {
        let name = Nip05Name::try_from(row.get::<'_, &str, &str>("name"))
            .or(Err(Error::RowNotFound))?;
        Ok(VerificationRecord {
            rowid: row.get::<'_, i64, &str>("id") as u64,
            name,
            address: row.get("author"),
            event: row.get("event_hash"),
            event_created: row.get::<'_, i64, &str>("created_at") as u64,
            last_success: None,
            last_failure: match row.try_get::<'_, i64, &str>("failed_at") {
                Ok(x) => Some(x as u64),
                _ => None
            },
            failure_count: row.get::<'_, i64, &str>("failure_count") as u64
        })
    }
}
