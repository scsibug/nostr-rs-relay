use crate::db::QueryResult;
use crate::error::Result;
use crate::event::{single_char_tagname, Event};
use crate::nip05::{Nip05Name, VerificationRecord};
use crate::payment::{InvoiceInfo, InvoiceStatus};
use crate::repo::{now_jitter, NostrRepo};
use crate::subscription::{ReqFilter, Subscription};
use async_std::stream::StreamExt;
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use sqlx::postgres::PgRow;
use sqlx::Error::RowNotFound;
use sqlx::{Error, Execute, FromRow, Postgres, QueryBuilder, Row};
use std::time::{Duration, Instant};

use crate::error;
use crate::repo::postgres_migration::run_migrations;
use crate::server::NostrMetrics;
use crate::utils::{self, is_hex, is_lower_hex};
use nostr::key::Keys;
use tokio::sync::mpsc::Sender;
use tokio::sync::oneshot::Receiver;
use tracing::{debug, error, info, trace, warn};

pub type PostgresPool = sqlx::pool::Pool<Postgres>;

pub struct PostgresRepo {
    conn: PostgresPool,
    conn_write: PostgresPool,
    metrics: NostrMetrics,
}

impl PostgresRepo {
    pub fn new(c: PostgresPool, cw: PostgresPool, m: NostrMetrics) -> PostgresRepo {
        PostgresRepo {
            conn: c,
            conn_write: cw,
            metrics: m,
        }
    }
}

/// Cleanup expired events on a regular basis
async fn cleanup_expired(conn: PostgresPool, frequency: Duration) -> Result<()> {
    tokio::task::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(frequency) => {
                    let start = Instant::now();
                    let exp_res = delete_expired(conn.clone()).await;
                    match exp_res {
                        Ok(exp_count) => {
                            if exp_count > 0 {
                                info!("removed {} expired events in: {:?}", exp_count, start.elapsed());
                            }
                        },
                        Err(e) => {
                            warn!("could not remove expired events due to error: {:?}", e);
                        }
                    }
                }
            };
        }
    });
    Ok(())
}

/// One-time deletion of all expired events
async fn delete_expired(conn: PostgresPool) -> Result<u64> {
    let mut tx = conn.begin().await?;
    let update_count = sqlx::query("DELETE FROM \"event\" WHERE expires_at <= $1;")
        .bind(Utc.timestamp_opt(utils::unix_time() as i64, 0).unwrap())
        .execute(&mut tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(update_count)
}

#[async_trait]
impl NostrRepo for PostgresRepo {
    async fn start(&self) -> Result<()> {
        // begin a cleanup task for expired events.
        cleanup_expired(self.conn_write.clone(), Duration::from_secs(600)).await?;
        Ok(())
    }

    async fn migrate_up(&self) -> Result<usize> {
        Ok(run_migrations(&self.conn_write).await?)
    }

    async fn write_event(&self, e: &Event) -> Result<u64> {
        // start transaction
        let mut tx = self.conn_write.begin().await?;
        let start = Instant::now();

        // get relevant fields from event and convert to blobs.
        let id_blob = hex::decode(&e.id).ok();
        let pubkey_blob: Option<Vec<u8>> = hex::decode(&e.pubkey).ok();
        let delegator_blob: Option<Vec<u8>> =
            e.delegated_by.as_ref().and_then(|d| hex::decode(d).ok());
        let event_str = serde_json::to_string(&e).unwrap();

        // determine if this event would be shadowed by an existing
        // replaceable event or parameterized replaceable event.
        if e.is_replaceable() {
            let repl_count = sqlx::query(
                "SELECT e.id FROM event e WHERE e.pub_key=$1 AND e.kind=$2 AND e.created_at >= $3 LIMIT 1;")
                .bind(&pubkey_blob)
                .bind(e.kind as i64)
                .bind(Utc.timestamp_opt(e.created_at as i64, 0).unwrap())
                .fetch_optional(&mut tx)
                .await?;
            if repl_count.is_some() {
                return Ok(0);
            }
        }
        if let Some(d_tag) = e.distinct_param() {
            let repl_count: i64 = if is_lower_hex(&d_tag) && (d_tag.len() % 2 == 0) {
                sqlx::query_scalar(
                    "SELECT count(*) AS count FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.pub_key=$1 AND e.kind=$2 AND t.name='d' AND t.value_hex=$3 AND e.created_at >= $4 LIMIT 1;")
                    .bind(hex::decode(&e.pubkey).ok())
                    .bind(e.kind as i64)
                    .bind(hex::decode(d_tag).ok())
                    .bind(Utc.timestamp_opt(e.created_at as i64, 0).unwrap())
                    .fetch_one(&mut tx)
                    .await?
            } else {
                sqlx::query_scalar(
                    "SELECT count(*) AS count FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.pub_key=$1 AND e.kind=$2 AND t.name='d' AND t.value=$3 AND e.created_at >= $4 LIMIT 1;")
                    .bind(hex::decode(&e.pubkey).ok())
                    .bind(e.kind as i64)
                    .bind(d_tag.as_bytes())
                    .bind(Utc.timestamp_opt(e.created_at as i64, 0).unwrap())
                    .fetch_one(&mut tx)
                    .await?
            };
            // if any rows were returned, then some newer event with
            // the same author/kind/tag value exist, and we can ignore
            // this event.
            if repl_count > 0 {
                return Ok(0);
            }
        }
        // ignore if the event hash is a duplicate.
        let mut ins_count = sqlx::query(
            r#"INSERT INTO "event"
(id, pub_key, created_at, expires_at, kind, "content", delegated_by)
VALUES($1, $2, $3, $4, $5, $6, $7)
ON CONFLICT (id) DO NOTHING"#,
        )
        .bind(&id_blob)
        .bind(&pubkey_blob)
        .bind(Utc.timestamp_opt(e.created_at as i64, 0).unwrap())
        .bind(
            e.expiration()
                .and_then(|x| Utc.timestamp_opt(x as i64, 0).latest()),
        )
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
                match &tag_char_opt {
                    Some(_) => {
                        // if tag value is lowercase hex;
                        if is_lower_hex(tag_val) && (tag_val.len() % 2 == 0) {
                            sqlx::query("INSERT INTO tag (event_id, \"name\", value, value_hex) VALUES($1, $2, NULL, $3) \
                    ON CONFLICT (event_id, \"name\", value, value_hex) DO NOTHING")
                                .bind(&id_blob)
                                .bind(tag_name)
                                .bind(hex::decode(tag_val).ok())
                                .execute(&mut tx)
                                .await
                                .unwrap();
                        } else {
                            sqlx::query("INSERT INTO tag (event_id, \"name\", value, value_hex) VALUES($1, $2, $3, NULL) \
                    ON CONFLICT (event_id, \"name\", value, value_hex) DO NOTHING")
                                .bind(&id_blob)
                                .bind(tag_name)
                                .bind(tag_val.as_bytes())
                                .execute(&mut tx)
                                .await
                                .unwrap();
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
        // parameterized replaceable events
        // check for parameterized replaceable events that would be hidden; don't insert these either.
        if let Some(d_tag) = e.distinct_param() {
            let update_count = if is_lower_hex(&d_tag) && (d_tag.len() % 2 == 0) {
                sqlx::query("DELETE FROM event WHERE kind=$1 AND pub_key=$2 AND id IN (SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.kind=$1 AND e.pub_key=$2 AND t.name='d' AND t.value_hex=$3 ORDER BY created_at DESC OFFSET 1);")
                    .bind(e.kind as i64)
                    .bind(hex::decode(&e.pubkey).ok())
                    .bind(hex::decode(d_tag).ok())
                    .execute(&mut tx)
                    .await?.rows_affected()
            } else {
                sqlx::query("DELETE FROM event WHERE kind=$1 AND pub_key=$2 AND id IN (SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.kind=$1 AND e.pub_key=$2 AND t.name='d' AND t.value=$3 ORDER BY created_at DESC OFFSET 1);")
                    .bind(e.kind as i64)
                    .bind(hex::decode(&e.pubkey).ok())
                    .bind(d_tag.as_bytes())
                    .execute(&mut tx)
                    .await?.rows_affected()
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
        let metrics = &self.metrics;

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
                    debug!(
                        "query cancelled by client (cid: {}, sub: {:?})",
                        client_id, sub.id
                    );
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
                            metrics
                                .query_aborts
                                .with_label_values(&["slowclient"])
                                .inc();
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
        let mut tx = self.conn_write.begin().await?;

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
        sqlx::query("UPDATE user_verification SET verified_at = $1, fail_count = 0 WHERE id = $2")
            .bind(Utc.timestamp_opt(verify_time as i64, 0).unwrap())
            .bind(id as i64)
            .execute(&self.conn_write)
            .await?;

        info!("verification updated for {}", id);
        Ok(())
    }

    async fn fail_verification(&self, id: u64) -> Result<()> {
        sqlx::query("UPDATE user_verification SET failed_at = now(), fail_count = fail_count + 1 WHERE id = $1")
            .bind(id as i64)
            .execute(&self.conn_write)
            .await?;
        Ok(())
    }

    async fn delete_verification(&self, id: u64) -> Result<()> {
        sqlx::query("DELETE FROM user_verification WHERE id = $1")
            .bind(id as i64)
            .execute(&self.conn_write)
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

    async fn create_account(&self, pub_key: &Keys) -> Result<bool> {
        let pub_key = pub_key.public_key().to_string();
        let mut tx = self.conn_write.begin().await?;

        let result = sqlx::query("INSERT INTO account (pubkey, balance) VALUES ($1, 0);")
            .bind(pub_key)
            .execute(&mut tx)
            .await;

        let success = match result {
            Ok(res) => {
                tx.commit().await?;
                res.rows_affected() == 1
            }
            Err(_err) => false,
        };

        Ok(success)
    }

    /// Admit account
    async fn admit_account(&self, pub_key: &Keys, admission_cost: u64) -> Result<()> {
        let pub_key = pub_key.public_key().to_string();
        sqlx::query(
            "UPDATE account SET is_admitted = TRUE, balance = balance - $1 WHERE pubkey = $2",
        )
        .bind(admission_cost as i64)
        .bind(pub_key)
        .execute(&self.conn_write)
        .await?;
        Ok(())
    }

    /// Gets if the account is admitted and balance
    async fn get_account_balance(&self, pub_key: &Keys) -> Result<(bool, u64)> {
        let pub_key = pub_key.public_key().to_string();
        let query = r#"SELECT
            is_admitted,
            balance
            FROM account
            WHERE pubkey = $1
            LIMIT 1"#;

        let result = sqlx::query_as::<_, (bool, i64)>(query)
            .bind(pub_key)
            .fetch_optional(&self.conn_write)
            .await?
            .ok_or(error::Error::SqlxError(RowNotFound))?;

        Ok((result.0, result.1 as u64))
    }

    /// Update account balance
    async fn update_account_balance(
        &self,
        pub_key: &Keys,
        positive: bool,
        new_balance: u64,
    ) -> Result<()> {
        let pub_key = pub_key.public_key().to_string();
        match positive {
            true => {
                sqlx::query("UPDATE account SET balance = balance + $1 WHERE pubkey = $2")
                    .bind(new_balance as i64)
                    .bind(pub_key)
                    .execute(&self.conn_write)
                    .await?
            }
            false => {
                sqlx::query("UPDATE account SET balance = balance - $1 WHERE pubkey = $2")
                    .bind(new_balance as i64)
                    .bind(pub_key)
                    .execute(&self.conn_write)
                    .await?
            }
        };
        Ok(())
    }

    /// Create invoice record
    async fn create_invoice_record(&self, pub_key: &Keys, invoice_info: InvoiceInfo) -> Result<()> {
        let pub_key = pub_key.public_key().to_string();
        let mut tx = self.conn_write.begin().await?;

        sqlx::query(
            "INSERT INTO invoice (pubkey, payment_hash, amount, status, description, created_at, invoice) VALUES ($1, $2, $3, $4, $5, now(), $6)",
        )
            .bind(pub_key)
            .bind(invoice_info.payment_hash)
            .bind(invoice_info.amount as i64)
            .bind(invoice_info.status)
            .bind(invoice_info.memo)
            .bind(invoice_info.bolt11)
            .execute(&mut tx)
            .await.unwrap();

        debug!("Invoice added");

        tx.commit().await?;
        Ok(())
    }

    /// Update invoice record
    async fn update_invoice(&self, payment_hash: &str, status: InvoiceStatus) -> Result<String> {
        debug!("Payment Hash: {}", payment_hash);
        let query = "SELECT pubkey, status, amount FROM invoice WHERE payment_hash=$1;";
        let (pubkey, prev_invoice_status, amount) =
            sqlx::query_as::<_, (String, InvoiceStatus, i64)>(query)
                .bind(payment_hash)
                .fetch_optional(&self.conn_write)
                .await?
                .ok_or(error::Error::SqlxError(RowNotFound))?;

        // If the invoice is paid update the confirmed at timestamp
        let query = if status.eq(&InvoiceStatus::Paid) {
            "UPDATE invoice SET status=$1, confirmed_at = now() WHERE payment_hash=$2;"
        } else {
            "UPDATE invoice SET status=$1 WHERE payment_hash=$2;"
        };

        sqlx::query(query)
            .bind(&status)
            .bind(payment_hash)
            .execute(&self.conn_write)
            .await?;

        if prev_invoice_status.eq(&InvoiceStatus::Unpaid) && status.eq(&InvoiceStatus::Paid) {
            sqlx::query("UPDATE account SET balance = balance + $1 WHERE pubkey = $2")
                .bind(amount)
                .bind(&pubkey)
                .execute(&self.conn_write)
                .await?;
        }

        Ok(pubkey)
    }

    /// Get the most recent invoice for a given pubkey
    /// invoice must be unpaid and not expired
    async fn get_unpaid_invoice(&self, pubkey: &Keys) -> Result<Option<InvoiceInfo>> {
        let query = r#"
SELECT amount, payment_hash, description, invoice
FROM invoice
WHERE pubkey = $1
ORDER BY created_at DESC
LIMIT 1;
        "#;
        match sqlx::query_as::<_, (i64, String, String, String)>(query)
            .bind(pubkey.public_key().to_string())
            .fetch_optional(&self.conn_write)
            .await
            .unwrap()
        {
            Some((amount, payment_hash, description, invoice)) => Ok(Some(InvoiceInfo {
                pubkey: pubkey.public_key().to_string(),
                payment_hash,
                bolt11: invoice,
                amount: amount as u64,
                status: InvoiceStatus::Unpaid,
                memo: description,
                confirmed_at: None,
            })),
            None => Ok(None),
        }
    }
}

/// Create a dynamic SQL query and params from a subscription filter.
fn query_from_filter(f: &ReqFilter) -> Option<QueryBuilder<Postgres>> {
    // if the filter is malformed, don't return anything.
    if f.force_no_match {
        return None;
    }

    let mut query = QueryBuilder::new("SELECT e.\"content\", e.created_at FROM \"event\" e WHERE ");

    // This tracks whether we need to push a prefix AND before adding another clause
    let mut push_and = false;
    // Query for "authors", allowing prefix matches
    if let Some(auth_vec) = &f.authors {
        // filter out non-hex values
        let auth_vec: Vec<&String> = auth_vec.iter().filter(|a| is_hex(a)).collect();

        if auth_vec.is_empty() {
            return None;
        }
        query.push("(e.pub_key in (");

        let mut pk_sep = query.separated(", ");
        for pk in auth_vec.iter() {
            pk_sep.push_bind(hex::decode(pk).ok());
        }
        query.push(") OR e.delegated_by in (");
        let mut pk_delegated_sep = query.separated(", ");
        for pk in auth_vec.iter() {
            pk_delegated_sep.push_bind(hex::decode(pk).ok());
        }
        push_and = true;
        query.push("))");
    }

    // Query for Kind
    if let Some(ks) = &f.kinds {
        if ks.is_empty() {
            return None;
        }
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

    // Query for event,
    if let Some(id_vec) = &f.ids {
        // filter out non-hex values
        let id_vec: Vec<&String> = id_vec.iter().filter(|a| is_hex(a)).collect();
        if id_vec.is_empty() {
            return None;
        }
        if push_and {
            query.push(" AND (");
        } else {
            query.push("(");
        }
        push_and = true;

        query.push("id in (");
        let mut sep = query.separated(", ");
        for id in id_vec.iter() {
            sep.push_bind(hex::decode(id).ok());
        }
        query.push("))");
    }

    // Query for tags
    if let Some(map) = &f.tags {
        if !map.is_empty() {
            if push_and {
                query.push(" AND ");
            }
            push_and = true;

            let mut push_or = false;
            query.push("e.id IN (SELECT ee.id FROM \"event\" ee LEFT JOIN tag t on ee.id = t.event_id WHERE ee.hidden != 1::bit(1) and ");
            for (key, val) in map.iter() {
                if val.is_empty() {
                    return None;
                }
                if push_or {
                    query.push(" OR ");
                }
                query
                    .push("(t.\"name\" = ")
                    .push_bind(key.to_string())
                    .push(" AND (");

                let has_plain_values = val.iter().any(|v| (v.len() % 2 != 0 || !is_lower_hex(v)));
                let has_hex_values = val.iter().any(|v| v.len() % 2 == 0 && is_lower_hex(v));
                if has_plain_values {
                    query.push("value in (");
                    // plain value match first
                    let mut tag_query = query.separated(", ");
                    for v in val.iter().filter(|v| v.len() % 2 != 0 || !is_lower_hex(v)) {
                        tag_query.push_bind(v.as_bytes());
                    }
                }
                if has_plain_values && has_hex_values {
                    query.push(") OR ");
                }
                if has_hex_values {
                    query.push("value_hex in (");
                    // plain value match first
                    let mut tag_query = query.separated(", ");
                    for v in val.iter().filter(|v| v.len() % 2 == 0 && is_lower_hex(v)) {
                        tag_query.push_bind(hex::decode(v).ok());
                    }
                }

                query.push(")))");
                push_or = true;
            }
            query.push(")");
        }
    }

    // Query for timestamp
    if f.since.is_some() {
        if push_and {
            query.push(" AND ");
        }
        push_and = true;
        query
            .push("e.created_at >= ")
            .push_bind(Utc.timestamp_opt(f.since.unwrap() as i64, 0).unwrap());
    }

    // Query for timestamp
    if f.until.is_some() {
        if push_and {
            query.push(" AND ");
        }
        push_and = true;
        query
            .push("e.created_at <= ")
            .push_bind(Utc.timestamp_opt(f.until.unwrap() as i64, 0).unwrap());
    }

    // never display hidden events
    if push_and {
        query.push(" AND e.hidden != 1::bit(1)");
    } else {
        query.push("e.hidden != 1::bit(1)");
    }
    // never display expired events
    query.push(" AND (e.expires_at IS NULL OR e.expires_at > now())");

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
        let name = Nip05Name::try_from(row.get::<'_, &str, &str>("name")).or(Err(RowNotFound))?;
        Ok(VerificationRecord {
            rowid: row.get::<'_, i64, &str>("id") as u64,
            name,
            address: hex::encode(row.get::<'_, Vec<u8>, &str>("pub_key")),
            event: hex::encode(row.get::<'_, Vec<u8>, &str>("event_id")),
            event_created: row.get::<'_, DateTime<Utc>, &str>("created_at").timestamp() as u64,
            last_success: match row.try_get::<'_, DateTime<Utc>, &str>("verified_at") {
                Ok(x) => Some(x.timestamp() as u64),
                _ => None,
            },
            last_failure: match row.try_get::<'_, DateTime<Utc>, &str>("failed_at") {
                Ok(x) => Some(x.timestamp() as u64),
                _ => None,
            },
            failure_count: row.get::<'_, i32, &str>("fail_count") as u64,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_query_gen_tag_value_hex() {
        let filter = ReqFilter {
            ids: None,
            kinds: Some(vec![1000]),
            since: None,
            until: None,
            authors: Some(vec![
                "84de35e2584d2b144aae823c9ed0b0f3deda09648530b93d1a2a146d1dea9864".to_owned(),
            ]),
            limit: None,
            tags: Some(HashMap::from([(
                'p',
                HashSet::from([
                    "63fe6318dc58583cfe16810f86dd09e18bfd76aabc24a0081ce2856f330504ed".to_owned(),
                ]),
            )])),
            force_no_match: false,
        };

        let q = query_from_filter(&filter).unwrap();
        assert_eq!(q.sql(), "SELECT e.\"content\", e.created_at FROM \"event\" e WHERE (e.pub_key in ($1) OR e.delegated_by in ($2)) AND e.kind in ($3) AND e.id IN (SELECT ee.id FROM \"event\" ee LEFT JOIN tag t on ee.id = t.event_id WHERE ee.hidden != 1::bit(1) and (t.\"name\" = $4 AND (value_hex in ($5)))) AND e.hidden != 1::bit(1) AND (e.expires_at IS NULL OR e.expires_at > now()) ORDER BY e.created_at ASC LIMIT 1000")
    }

    #[test]
    fn test_query_gen_tag_value() {
        let filter = ReqFilter {
            ids: None,
            kinds: Some(vec![1000]),
            since: None,
            until: None,
            authors: Some(vec![
                "84de35e2584d2b144aae823c9ed0b0f3deda09648530b93d1a2a146d1dea9864".to_owned(),
            ]),
            limit: None,
            tags: Some(HashMap::from([('d', HashSet::from(["test".to_owned()]))])),
            force_no_match: false,
        };

        let q = query_from_filter(&filter).unwrap();
        assert_eq!(q.sql(), "SELECT e.\"content\", e.created_at FROM \"event\" e WHERE (e.pub_key in ($1) OR e.delegated_by in ($2)) AND e.kind in ($3) AND e.id IN (SELECT ee.id FROM \"event\" ee LEFT JOIN tag t on ee.id = t.event_id WHERE ee.hidden != 1::bit(1) and (t.\"name\" = $4 AND (value in ($5)))) AND e.hidden != 1::bit(1) AND (e.expires_at IS NULL OR e.expires_at > now()) ORDER BY e.created_at ASC LIMIT 1000")
    }

    #[test]
    fn test_query_gen_tag_value_and_value_hex() {
        let filter = ReqFilter {
            ids: None,
            kinds: Some(vec![1000]),
            since: None,
            until: None,
            authors: Some(vec![
                "84de35e2584d2b144aae823c9ed0b0f3deda09648530b93d1a2a146d1dea9864".to_owned(),
            ]),
            limit: None,
            tags: Some(HashMap::from([(
                'd',
                HashSet::from([
                    "test".to_owned(),
                    "63fe6318dc58583cfe16810f86dd09e18bfd76aabc24a0081ce2856f330504ed".to_owned(),
                ]),
            )])),
            force_no_match: false,
        };

        let q = query_from_filter(&filter).unwrap();
        assert_eq!(q.sql(), "SELECT e.\"content\", e.created_at FROM \"event\" e WHERE (e.pub_key in ($1) OR e.delegated_by in ($2)) AND e.kind in ($3) AND e.id IN (SELECT ee.id FROM \"event\" ee LEFT JOIN tag t on ee.id = t.event_id WHERE ee.hidden != 1::bit(1) and (t.\"name\" = $4 AND (value in ($5) OR value_hex in ($6)))) AND e.hidden != 1::bit(1) AND (e.expires_at IS NULL OR e.expires_at > now()) ORDER BY e.created_at ASC LIMIT 1000")
    }

    #[test]
    fn test_query_multiple_tags() {
        let filter = ReqFilter {
            ids: None,
            kinds: Some(vec![30_001]),
            since: None,
            until: None,
            authors: None,
            limit: None,
            tags: Some(HashMap::from([
                ('d', HashSet::from(["follow".to_owned()])),
                ('t', HashSet::from(["siamstr".to_owned()])),
            ])),
            force_no_match: false,
        };
        let q = query_from_filter(&filter).unwrap();
        assert_eq!(q.sql(), "SELECT e.\"content\", e.created_at FROM \"event\" e WHERE e.kind in ($1) AND e.id IN (SELECT ee.id FROM \"event\" ee LEFT JOIN tag t on ee.id = t.event_id WHERE ee.hidden != 1::bit(1) and (t.\"name\" = $2 AND (value in ($3))) OR (t.\"name\" = $4 AND (value in ($5)))) AND e.hidden != 1::bit(1) AND (e.expires_at IS NULL OR e.expires_at > now()) ORDER BY e.created_at ASC LIMIT 1000")
    }

    #[test]
    fn test_query_empty_tags() {
        let filter = ReqFilter {
            ids: None,
            kinds: Some(vec![1, 6, 16, 30023, 1063, 6969]),
            since: Some(1700697846),
            until: None,
            authors: None,
            limit: None,
            tags: Some(HashMap::from([('a', HashSet::new())])),
            force_no_match: false,
        };
        assert!(query_from_filter(&filter).is_none());
    }
}
