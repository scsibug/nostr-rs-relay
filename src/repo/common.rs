use async_std::stream::StreamExt;
use sqlx::{Arguments, Database, Execute, IntoArguments, Pool, QueryBuilder, Row};
use std::time::{Duration, Instant};
use sqlx::database::HasArguments;
use sqlx::query::Query;
use crate::db::QueryResult;
use crate::error::Result;
use crate::subscription::{ReqFilter, Subscription};
use tracing::log::trace;
use tracing::{debug, info};

pub(crate) async fn query_sub<'q, FQ, DB>(
    sub: Subscription,
    client_id: String,
    query_tx: tokio::sync::mpsc::Sender<QueryResult>,
    mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
    query_from_filter: FQ,
    pool: &Pool<DB>,
) -> Result<()>
where
    FQ: Fn(&ReqFilter) -> Option<QueryBuilder<'_, DB>>,
    DB: Database
{
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
        let mut results = q_build.fetch(pool);

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
