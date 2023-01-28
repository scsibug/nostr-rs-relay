use crate::db::QueryResult;
use crate::error::Result;
use crate::event::{single_char_tagname, Event};
use crate::nip05::{Nip05Name, VerificationRecord};
use crate::repo::{now_jitter, NostrRepo};
use crate::subscription::{ReqFilter, Subscription};
use async_std::stream::StreamExt;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use sqlx::postgres::PgRow;
use sqlx::{Error, Execute, FromRow, Postgres, QueryBuilder, Row};
use std::time::{Duration, Instant};
use sqlx::Error::RowNotFound;

use crate::hexrange::{hex_range, HexSearch};
use crate::repo::postgres_migration::run_migrations;
use crate::server::NostrMetrics;
use crate::utils::{is_hex, is_lower_hex};
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot::Receiver;
use tracing::log::trace;
use tracing::{debug, error, info};
use crate::error;

pub type PostgresPool = sqlx::pool::Pool<Postgres>;

pub struct PostgresRepo {
    conn: PostgresPool,
    metrics: NostrMetrics,
}

impl PostgresRepo {
    pub fn new(c: PostgresPool, m: NostrMetrics) -> PostgresRepo {
        PostgresRepo {
            conn: c,
            metrics: m,
        }
    }
}

#[async_trait]
impl NostrRepo for PostgresRepo {

    async fn start(&self) -> Result<()> {
	info!("not implemented");
	Ok(())
    }

    async fn migrate_up(&self) -> Result<usize> {
        Ok(run_migrations(&self.conn).await?)
    }

    async fn write_event(&self, e: &Event) -> Result<u64> {
        // start transaction
        let mut tx = self.conn.begin().await?;
        let start = Instant::now();

        // get relevant fields from event and convert to blobs.
        let id_blob = hex::decode(&e.id).ok();
        let pubkey_blob: Option<Vec<u8>> = hex::decode(&e.pubkey).ok();
        let delegator_blob: Option<Vec<u8>> =
            e.delegated_by.as_ref().and_then(|d| hex::decode(d).ok());
        let event_str = serde_json::to_string(&e).unwrap();

        // ignore if the event hash is a duplicate.
        let mut ins_count = sqlx::query(
            r#"INSERT INTO "event"
(id, pub_key, created_at, kind, "content", delegated_by)
VALUES($1, $2, $3, $4, $5, $6)
ON CONFLICT (id) DO NOTHING"#,
        )
            .bind(&id_blob)
            .bind(&pubkey_blob)
            .bind(Utc.timestamp_opt(e.created_at as i64, 0).unwrap())
            .bind(e.kind as i64)
            .bind(event_str.into_bytes())
            .bind(delegator_blob)
            .execute(&mut tx)
            .await?
            .rows_affected();

        if ins_count == 0 {
            // if the event was a duplicate, no need to insert event or
            // pubkey references.  This will abort the txn.
            return Ok(0);
        }

        // add all tags to the tag table
        for tag in e.tags.iter() {
            // ensure we have 2 values.
            if tag.len() >= 2 {
                let tag_name = &tag[0];
                let tag_val = &tag[1];
                // only single-char tags are searchable
                let tag_char_opt = single_char_tagname(tag_name);
                let query = "INSERT INTO tag (event_id, \"name\", value) VALUES($1, $2, $3) \
                    ON CONFLICT (event_id, \"name\", value) DO NOTHING";
                match &tag_char_opt {
                    Some(_) => {
                        // if tag value is lowercase hex;
                        if is_lower_hex(tag_val) && (tag_val.len() % 2 == 0) {
                            sqlx::query(query)
                                .bind(&id_blob)
                                .bind(tag_name)
                                .bind(hex::decode(tag_val).ok())
                                .execute(&mut tx)
                                .await?;
                        } else {
                            sqlx::query(query)
                                .bind(&id_blob)
                                .bind(tag_name)
                                .bind(tag_val.as_bytes())
                                .execute(&mut tx)
                                .await?;
                        }
                    }
                    None => {}
                }
            }
        }
        if e.is_replaceable() {
            let update_count = sqlx::query("DELETE FROM \"event\" WHERE kind=$1 and pub_key = $2 and id not in (select id from \"event\" where kind=$1 and pub_key=$2 order by created_at desc limit 1);")
                .bind(e.kind as i64)
                .bind(hex::decode(&e.pubkey).ok())
                .execute(&mut tx)
                .await?.rows_affected();
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
            let pub_keys: Vec<Vec<u8>> = event_candidates
                .iter()
                .filter(|x| is_hex(x) && x.len() == 64)
                .filter_map(|x| hex::decode(x).ok())
                .collect();

            let mut builder = QueryBuilder::new(
                "UPDATE \"event\" SET hidden = 1::bit(1) WHERE kind != 5 AND pub_key = ",
            );
            builder.push_bind(hex::decode(&e.pubkey).ok());
            builder.push(" AND id IN (");

            let mut sep = builder.separated(", ");
            for pk in pub_keys {
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
            let del_count = sqlx::query(
                "SELECT e.id FROM \"event\" e \
            LEFT JOIN tag t ON e.id = t.event_id \
            WHERE e.pub_key = $1 AND t.\"name\" = 'e' AND e.kind = 5 AND t.value = $2 LIMIT 1",
            )
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
                sqlx::query("UPDATE \"event\" SET hidden = 1::bit(1) WHERE id = $1")
                    .bind(&id_blob)
                    .execute(&mut tx)
                    .await?;
                // event was deleted, so let caller know nothing new
                // arrived, preventing this from being sent to active
                // subscriptions
                ins_count = 0;
            }
        }
        tx.commit().await?;
        self.metrics
            .write_events
            .observe(start.elapsed().as_secs_f64());
        Ok(ins_count)
    }

    async fn query_subscription(
        &self,
        sub: Subscription,
        client_id: String,
        query_tx: Sender<QueryResult>,
        mut abandon_query_rx: Receiver<()>,
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
                if let Err(e) = row {
                    error!("Query failed: {} {} {:?}", e, sql, filter);
                    break;
                }
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
                } else {
                    trace!(
                        "query req: {:?} (cid: {}, sub: {:?})",
                        &sub,
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
                let event_json: Vec<u8> = row.unwrap().get(0);
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
                        event: String::from_utf8(event_json).unwrap(),
                    })
                    .await
                    .ok();
                last_successful_send = Instant::now();
            }
        }
        query_tx
            .send(QueryResult {
                sub_id: sub.get_id(),
                event: "EOSE".to_string(),
            })
            .await
            .ok();
        self.metrics
            .query_sub
            .observe(start.elapsed().as_secs_f64());
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

    async fn optimize_db(&self) -> Result<()> {
        // Not implemented
        Ok(())
    }

    async fn create_verification_record(&self, event_id: &str, name: &str) -> Result<()> {
        let mut tx = self.conn.begin().await?;

        sqlx::query("DELETE FROM user_verification WHERE \"name\" = $1")
            .bind(name)
            .execute(&mut tx)
            .await?;

        sqlx::query("INSERT INTO user_verification (event_id, \"name\", verified_at) VALUES ($1, $2, now())")
            .bind(hex::decode(event_id).ok())
            .bind(name)
            .execute(&mut tx)
            .await?;

        tx.commit().await?;
        info!("saved new verification record for ({:?})", name);
        Ok(())
    }

    async fn update_verification_timestamp(&self, id: u64) -> Result<()> {
        // add some jitter to the verification to prevent everything from stacking up together.
        let verify_time = now_jitter(600);

        // update verification time and reset any failure count
        sqlx::query(
            "UPDATE user_verification SET verified_at = $1, fail_count = 0 WHERE id = $2",
        )
            .bind(Utc.timestamp_opt(verify_time as i64, 0).unwrap())
            .bind(id as i64)
            .execute(&self.conn)
            .await?;

        info!("verification updated for {}", id);
        Ok(())
    }

    async fn fail_verification(&self, id: u64) -> Result<()> {
        sqlx::query("UPDATE user_verification SET failed_at = now(), fail_count = fail_count + 1 WHERE id = $1")
            .bind(id as i64)
            .execute(&self.conn)
            .await?;
        Ok(())
    }

    async fn delete_verification(&self, id: u64) -> Result<()> {
        sqlx::query("DELETE FROM user_verification WHERE id = $1")
            .bind(id as i64)
            .execute(&self.conn)
            .await?;
        Ok(())
    }

    async fn get_latest_user_verification(&self, pub_key: &str) -> Result<VerificationRecord> {
        let query = r#"SELECT
            v.id,
            v."name",
            e.id as event_id,
            e.pub_key,
            e.created_at,
            v.verified_at,
            v.failed_at,
            v.fail_count
            FROM user_verification v
            LEFT JOIN "event" e ON e.id = v.event_id
            WHERE e.pub_key = $1
            ORDER BY e.created_at DESC, v.verified_at DESC, v.failed_at DESC
            LIMIT 1"#;
        sqlx::query_as::<_, VerificationRecord>(query)
            .bind(hex::decode(pub_key).ok())
            .fetch_optional(&self.conn)
            .await?
            .ok_or(error::Error::SqlxError(RowNotFound))
    }

    async fn get_oldest_user_verification(&self, before: u64) -> Result<VerificationRecord> {
        let query = r#"SELECT
            v.id,
            v."name",
            e.id as event_id,
            e.pub_key,
            e.created_at,
            v.verified_at,
            v.failed_at,
            v.fail_count
            FROM user_verification v
            LEFT JOIN "event" e ON e.id = v.event_id
                WHERE (v.verified_at < $1 OR v.verified_at IS NULL)
                AND (v.failed_at < $1 OR v.failed_at IS NULL)
            ORDER BY v.verified_at ASC, v.failed_at ASC
            LIMIT 1"#;
        sqlx::query_as::<_, VerificationRecord>(query)
            .bind(Utc.timestamp_opt(before as i64, 0).unwrap())
            .fetch_optional(&self.conn)
            .await?
            .ok_or(error::Error::SqlxError(RowNotFound))
    }
}

/// Create a dynamic SQL query and params from a subscription filter.
fn query_from_filter(f: &ReqFilter) -> Option<QueryBuilder<Postgres>> {
    // if the filter is malformed, don't return anything.
    if f.force_no_match {
        return None;
    }

    let mut query = QueryBuilder::new("SELECT e.\"content\", e.created_at FROM \"event\" e WHERE ");

    let mut push_and = false;
    // Query for "authors", allowing prefix matches
    if let Some(auth_vec) = &f.authors {
        // filter out non-hex values
        let auth_vec: Vec<&String> = auth_vec.iter().filter(|a| is_hex(a)).collect();

        if !auth_vec.is_empty() {
            query.push("(");

            // shortcut authors into "IN" query
            let any_is_range = auth_vec.iter().any(|pk| pk.len() != 64);
            if !any_is_range {
                query.push("e.pub_key in (");
                let mut pk_sep = query.separated(", ");
                for pk in auth_vec.iter() {
                    pk_sep.push_bind(hex::decode(pk).ok());
                }
                query.push(") OR e.delegated_by in (");
                let mut pk_delegated_sep = query.separated(", ");
                for pk in auth_vec.iter() {
                    pk_delegated_sep.push_bind(hex::decode(pk).ok());
                }
                query.push(")");
                push_and = true;
            } else {
                let mut range_authors = query.separated(" OR ");
                for auth in auth_vec {
                    match hex_range(auth) {
                        Some(HexSearch::Exact(ex)) => {
                            range_authors
                                .push("(e.pub_key = ")
                                .push_bind_unseparated(ex.clone())
                                .push_unseparated(" OR e.delegated_by = ")
                                .push_bind_unseparated(ex)
                                .push_unseparated(")");
                        }
                        Some(HexSearch::Range(lower, upper)) => {
                            range_authors
                                .push("((e.pub_key > ")
                                .push_bind_unseparated(lower.clone())
                                .push_unseparated(" AND e.pub_key < ")
                                .push_bind_unseparated(upper.clone())
                                .push_unseparated(") OR (e.delegated_by > ")
                                .push_bind_unseparated(lower)
                                .push_unseparated(" AND e.delegated_by < ")
                                .push_bind_unseparated(upper)
                                .push_unseparated("))");
                        }
                        Some(HexSearch::LowerOnly(lower)) => {
                            range_authors
                                .push("(e.pub_key > ")
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
            query.push(")");
        }
    }

    // Query for Kind
    if let Some(ks) = &f.kinds {
        if !ks.is_empty() {
            if push_and {
                query.push(" AND ");
            }
            push_and = true;

            query.push("e.kind in (");
            let mut list_query = query.separated(", ");
            for k in ks.iter() {
                list_query.push_bind(*k as i64);
            }
            query.push(")");
        }
    }

    // Query for event, allowing prefix matches
    if let Some(id_vec) = &f.ids {
        // filter out non-hex values
        let id_vec: Vec<&String> = id_vec.iter().filter(|a| is_hex(a)).collect();

        if !id_vec.is_empty() {
            if push_and {
                query.push(" AND (");
            } else {
                query.push("(");
            }
            push_and = true;

            // shortcut ids into "IN" query
            let any_is_range = id_vec.iter().any(|pk| pk.len() != 64);
            if !any_is_range {
                query.push("id in (");
                let mut sep = query.separated(", ");
                for id in id_vec.iter() {
                    sep.push_bind(hex::decode(id).ok());
                }
                query.push(")");
            } else {
                // take each author and convert to a hex search
                let mut id_query = query.separated(" OR ");
                for id in id_vec {
                    match hex_range(id) {
                        Some(HexSearch::Exact(ex)) => {
                            id_query
                                .push("(id = ")
                                .push_bind_unseparated(ex)
                                .push_unseparated(")");
                        }
                        Some(HexSearch::Range(lower, upper)) => {
                            id_query
                                .push("(id > ")
                                .push_bind_unseparated(lower)
                                .push_unseparated(" AND id < ")
                                .push_bind_unseparated(upper)
                                .push_unseparated(")");
                        }
                        Some(HexSearch::LowerOnly(lower)) => {
                            id_query
                                .push("(id > ")
                                .push_bind_unseparated(lower)
                                .push_unseparated(")");
                        }
                        None => {
                            info!("Could not parse hex range from id {:?}", id);
                        }
                    }
                }
            }

            query.push(")");
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
                query.push("e.id IN (SELECT ee.id FROM \"event\" ee LEFT JOIN tag t on ee.id = t.event_id WHERE ee.hidden != 1::bit(1) and (t.\"name\" = ")
                    .push_bind(key.to_string())
                    .push(" AND (value in (");

                // plain value match first
                let mut tag_query = query.separated(", ");
                for v in val.iter() {
                    if (v.len() % 2 != 0) && !is_lower_hex(v) {
                        tag_query.push_bind(v.as_bytes());
                    } else {
                        tag_query.push_bind(hex::decode(v).ok());
                    }
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
            .push_bind(Utc.timestamp_opt(f.since.unwrap() as i64, 0).unwrap());
    }

    // Query for timestamp
    if f.until.is_some() {
        if push_and {
            query.push(" AND ");
        }
        push_and = true;
        query
            .push("e.created_at < ")
            .push_bind(Utc.timestamp_opt(f.until.unwrap() as i64, 0).unwrap());
    }

    // never display hidden events
    if push_and {
        query.push(" AND e.hidden != 1::bit(1)");
    } else {
        query.push("e.hidden != 1::bit(1)");
    }

    // Apply per-filter limit to this query.
    // The use of a LIMIT implies a DESC order, to capture only the most recent events.
    if let Some(lim) = f.limit {
        query.push(" ORDER BY e.created_at DESC LIMIT ");
        query.push(lim.min(1000));
    } else {
        query.push(" ORDER BY e.created_at ASC LIMIT ");
        query.push(1000);
    }
    Some(query)
}

impl FromRow<'_, PgRow> for VerificationRecord {
    fn from_row(row: &'_ PgRow) -> std::result::Result<Self, Error> {
        let name =
            Nip05Name::try_from(row.get::<'_, &str, &str>("name")).or(Err(RowNotFound))?;
        Ok(VerificationRecord {
            rowid: row.get::<'_, i64, &str>("id") as u64,
            name,
            address: hex::encode(row.get::<'_, Vec<u8>, &str>("pub_key")),
            event: hex::encode(row.get::<'_, Vec<u8>, &str>("event_id")),
            event_created: row.get::<'_, DateTime<Utc>, &str>("created_at").timestamp() as u64,
            last_success: None,
            last_failure: match row.try_get::<'_, DateTime<Utc>, &str>("failed_at") {
                Ok(x) => Some(x.timestamp() as u64),
                _ => None,
            },
            failure_count: row.get::<'_, i32, &str>("fail_count") as u64,
        })
    }
}
