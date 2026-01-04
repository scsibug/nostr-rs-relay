use anyhow::Result;
use nostr_rs_relay::repo::postgres::{PostgresPool, PostgresRepo};
use nostr_rs_relay::repo::NostrRepo;
use nostr_rs_relay::server::NostrMetrics;
use prometheus::{Histogram, HistogramOpts, IntCounter, IntCounterVec, IntGauge, Opts};
use sqlx::pool::PoolOptions;
use sqlx::postgres::{PgConnectOptions, PgConnection};
use sqlx::Connection;
use uuid::Uuid;

fn postgres_url() -> Option<String> {
    std::env::var("POSTGRES_URL").ok()
}

fn build_metrics() -> NostrMetrics {
    let query_sub = Histogram::with_opts(HistogramOpts::new(
        "nostr_query_seconds",
        "Subscription response times",
    ))
    .unwrap();
    let query_db = Histogram::with_opts(HistogramOpts::new(
        "nostr_filter_seconds",
        "Filter SQL query times",
    ))
    .unwrap();
    let write_events = Histogram::with_opts(HistogramOpts::new(
        "nostr_events_write_seconds",
        "Event writing response times",
    ))
    .unwrap();
    let sent_events = IntCounterVec::new(
        Opts::new("nostr_events_sent_total", "Events sent to clients"),
        vec!["source"].as_slice(),
    )
    .unwrap();
    let connections =
        IntCounter::with_opts(Opts::new("nostr_connections_total", "New connections")).unwrap();
    let db_connections = IntGauge::with_opts(Opts::new(
        "nostr_db_connections",
        "Active database connections",
    ))
    .unwrap();
    let query_aborts = IntCounterVec::new(
        Opts::new("nostr_query_abort_total", "Aborted queries"),
        vec!["reason"].as_slice(),
    )
    .unwrap();
    let cmd_req = IntCounter::with_opts(Opts::new("nostr_cmd_req_total", "REQ commands")).unwrap();
    let cmd_event =
        IntCounter::with_opts(Opts::new("nostr_cmd_event_total", "EVENT commands")).unwrap();
    let cmd_close =
        IntCounter::with_opts(Opts::new("nostr_cmd_close_total", "CLOSE commands")).unwrap();
    let cmd_auth =
        IntCounter::with_opts(Opts::new("nostr_cmd_auth_total", "AUTH commands")).unwrap();
    let disconnects = IntCounterVec::new(
        Opts::new("nostr_disconnects_total", "Client disconnects"),
        vec!["reason"].as_slice(),
    )
    .unwrap();
    NostrMetrics {
        query_sub,
        query_db,
        write_events,
        sent_events,
        connections,
        db_connections,
        disconnects,
        query_aborts,
        cmd_req,
        cmd_event,
        cmd_close,
        cmd_auth,
    }
}

async fn setup_pool(url: &str) -> Result<(PostgresPool, PgConnectOptions, String)> {
    let schema = format!("test_{}", Uuid::new_v4().simple());
    let options: PgConnectOptions = url.parse()?;
    let mut conn = PgConnection::connect_with(&options).await?;
    let create_schema = format!("CREATE SCHEMA \"{schema}\"");
    sqlx::query(&create_schema).execute(&mut conn).await?;
    drop(conn);

    let schema_name = schema.clone();
    let pool = PoolOptions::new()
        .max_connections(2)
        .after_connect(move |conn, _meta| {
            let schema = schema_name.clone();
            Box::pin(async move {
                let set_search_path = format!("SET search_path TO \"{schema}\"");
                sqlx::query(&set_search_path).execute(conn).await?;
                Ok(())
            })
        })
        .connect_with(options.clone())
        .await?;
    Ok((pool, options, schema))
}

async fn drop_schema(options: PgConnectOptions, schema: &str) -> Result<()> {
    let mut conn = PgConnection::connect_with(&options).await?;
    let drop_schema = format!("DROP SCHEMA \"{schema}\" CASCADE");
    sqlx::query(&drop_schema).execute(&mut conn).await?;
    Ok(())
}

#[tokio::test]
async fn postgres_migrations_run() -> Result<()> {
    let Some(url) = postgres_url() else {
        return Ok(());
    };
    let (pool, options, schema) = setup_pool(&url).await?;
    let metrics = build_metrics();
    let repo = PostgresRepo::new(pool.clone(), pool.clone(), metrics);
    let version = repo.migrate_up().await?;
    assert!(version >= 7);
    drop(pool);
    drop_schema(options, &schema).await?;
    Ok(())
}
