use nostr_rs_relay::config;
use nostr_rs_relay::event::{single_char_tagname, Event};
use nostr_rs_relay::utils::is_lower_hex;
use sqlx::postgres::PgConnectOptions;
use sqlx::{ConnectOptions, Row};
use sqlx::pool::PoolOptions;
use chrono::{TimeZone, Utc};
use async_std::stream::StreamExt;
use tracing::info;

struct TagInsert {
    name: String,
    value: Option<Vec<u8>>,
    value_hex: Option<Vec<u8>>,
}

fn build_tag_inserts(event: &Event) -> Vec<TagInsert> {
    let mut inserts = Vec::new();
    for tag in event.tags.iter().filter(|x| x.len() > 1) {
        let tag_name = &tag[0];
        if single_char_tagname(tag_name).is_none() {
            continue;
        }
        let tag_val = &tag[1];
        if is_lower_hex(tag_val) && (tag_val.len() % 2 == 0) {
            inserts.push(TagInsert {
                name: tag_name.clone(),
                value: None,
                value_hex: hex::decode(tag_val).ok(),
            });
        } else {
            inserts.push(TagInsert {
                name: tag_name.clone(),
                value: Some(tag_val.as_bytes().to_vec()),
                value_hex: None,
            });
        }
    }
    inserts
}

pub fn main() -> Result<(), Box<dyn std::error::Error>> {
    async_std::task::block_on(async_main())
}

async fn async_main() -> Result<(), Box<dyn std::error::Error>> {
    let _trace_sub = tracing_subscriber::fmt::try_init();
    let settings = config::Settings::new(&None)?;
    let mut options: PgConnectOptions = settings.database.connection.as_str().parse()?;
    options.log_statements(log::LevelFilter::Debug);
    options.log_slow_statements(log::LevelFilter::Warn, std::time::Duration::from_secs(60));

    let pool = PoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await?;

    info!("clearing tag table");
    sqlx::query("DELETE FROM tag;").execute(&pool).await?;

    let mut event_rows = sqlx::query("SELECT id, content FROM event ORDER BY id;").fetch(&pool);
    let mut processed = 0u64;
    while let Some(row_result) = event_rows.next().await {
        let row: sqlx::postgres::PgRow = row_result?;
        let event_id: Vec<u8> = row.get(0);
        let event_bytes: Vec<u8> = row.get(1);
        let event: Event = serde_json::from_slice(&event_bytes)?;
        let tag_inserts = build_tag_inserts(&event);
        for tag in tag_inserts {
            sqlx::query(
                "INSERT INTO tag (event_id, \"name\", value, value_hex, kind, created_at) \
                 VALUES ($1, $2, $3, $4, $5, $6) \
                 ON CONFLICT (event_id, \"name\", value, value_hex) DO NOTHING",
            )
            .bind(&event_id)
            .bind(tag.name)
            .bind(tag.value)
            .bind(tag.value_hex)
            .bind(event.kind as i64)
            .bind(Utc.timestamp_opt(event.created_at as i64, 0).unwrap())
            .execute(&pool)
            .await?;
        }
        processed += 1;
        if processed % 10_000 == 0 {
            info!("processed {} events", processed);
        }
    }
    info!("rebuilt tags for {} events", processed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tag_inserts_hex_and_text() {
        let event = Event {
            id: "abc".to_owned(),
            pubkey: "def".to_owned(),
            delegated_by: None,
            created_at: 0,
            kind: 1,
            tags: vec![
                vec!["e".to_owned(), "abcd".to_owned()],
                vec!["d".to_owned(), "test".to_owned()],
                vec!["alt".to_owned(), "skip".to_owned()],
            ],
            content: "".to_owned(),
            sig: "".to_owned(),
            tagidx: None,
        };
        let inserts = build_tag_inserts(&event);
        assert_eq!(inserts.len(), 2);
        assert_eq!(inserts[0].name, "e");
        assert!(inserts[0].value.is_none());
        assert_eq!(inserts[0].value_hex.as_ref().unwrap(), &vec![0xab, 0xcd]);
        assert_eq!(inserts[1].name, "d");
        assert_eq!(inserts[1].value.as_ref().unwrap(), b"test");
        assert!(inserts[1].value_hex.is_none());
    }
}
