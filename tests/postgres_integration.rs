use anyhow::Result;
use nostr_rs_relay::event::Event;
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

fn hex_64(byte: u8) -> String {
    format!("{:02x}", byte).repeat(32)
}

fn build_event(
    id: String,
    pubkey: String,
    kind: u64,
    created_at: u64,
    tags: Vec<Vec<String>>,
) -> Event {
    Event {
        id,
        pubkey,
        delegated_by: None,
        created_at,
        kind,
        tags,
        content: String::new(),
        sig: "00".repeat(64),
        tagidx: None,
    }
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

#[tokio::test]
async fn postgres_replaceable_prunes_older() -> Result<()> {
    let Some(url) = postgres_url() else {
        return Ok(());
    };
    let (pool, options, schema) = setup_pool(&url).await?;
    let metrics = build_metrics();
    let repo = PostgresRepo::new(pool.clone(), pool.clone(), metrics);
    repo.migrate_up().await?;

    let pubkey = hex_64(0x11);
    let event_older = build_event(hex_64(0x01), pubkey.clone(), 0, 100, vec![]);
    let event_newer = build_event(hex_64(0x02), pubkey.clone(), 0, 200, vec![]);
    let event_same_time = build_event(hex_64(0x03), pubkey.clone(), 0, 200, vec![]);

    assert_eq!(repo.write_event(&event_older).await?, 1);
    assert_eq!(repo.write_event(&event_newer).await?, 1);
    assert_eq!(repo.write_event(&event_same_time).await?, 0);

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM \"event\" WHERE kind=$1 AND pub_key=$2",
    )
    .bind(event_newer.kind as i64)
    .bind(hex::decode(&pubkey).ok())
    .fetch_one(&pool)
    .await?;
    assert_eq!(count, 1);

    let stored_id: Vec<u8> = sqlx::query_scalar(
        "SELECT id FROM \"event\" WHERE kind=$1 AND pub_key=$2",
    )
    .bind(event_newer.kind as i64)
    .bind(hex::decode(&pubkey).ok())
    .fetch_one(&pool)
    .await?;
    assert_eq!(hex::encode(stored_id), event_newer.id);

    drop(pool);
    drop_schema(options, &schema).await?;
    Ok(())
}

#[tokio::test]
async fn postgres_param_replaceable_prunes_older() -> Result<()> {
    let Some(url) = postgres_url() else {
        return Ok(());
    };
    let (pool, options, schema) = setup_pool(&url).await?;
    let metrics = build_metrics();
    let repo = PostgresRepo::new(pool.clone(), pool.clone(), metrics);
    repo.migrate_up().await?;

    let pubkey = hex_64(0x21);
    let tag = vec![vec!["d".to_string(), "alpha".to_string()]];
    let kind = 30000;
    let event_older = build_event(hex_64(0x10), pubkey.clone(), kind, 100, tag.clone());
    let event_newer = build_event(hex_64(0x11), pubkey.clone(), kind, 101, tag.clone());
    let event_same_time = build_event(hex_64(0x12), pubkey.clone(), kind, 101, tag.clone());

    assert_eq!(repo.write_event(&event_older).await?, 1);
    assert_eq!(repo.write_event(&event_newer).await?, 1);
    assert_eq!(repo.write_event(&event_same_time).await?, 0);

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM \"event\" e \
        JOIN tag t ON e.id = t.event_id \
        WHERE e.kind=$1 AND e.pub_key=$2 AND t.name='d' AND t.value=$3",
    )
    .bind(kind as i64)
    .bind(hex::decode(&pubkey).ok())
    .bind("alpha".as_bytes())
    .fetch_one(&pool)
    .await?;
    assert_eq!(count, 1);

    let stored_id: Vec<u8> = sqlx::query_scalar(
        "SELECT e.id FROM \"event\" e \
        JOIN tag t ON e.id = t.event_id \
        WHERE e.kind=$1 AND e.pub_key=$2 AND t.name='d' AND t.value=$3",
    )
    .bind(kind as i64)
    .bind(hex::decode(&pubkey).ok())
    .bind("alpha".as_bytes())
    .fetch_one(&pool)
    .await?;
    assert_eq!(hex::encode(stored_id), event_newer.id);

    drop(pool);
    drop_schema(options, &schema).await?;
    Ok(())
}
