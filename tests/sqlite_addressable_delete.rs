#![forbid(unsafe_code)]

use anyhow::Result;
use nostr_rs_relay::config;
use nostr_rs_relay::event::Event;
use nostr_rs_relay::repo::sqlite::{build_pool, SqliteRepo};
use nostr_rs_relay::repo::sqlite_migration::upgrade_db;
use rusqlite::params;
use rusqlite::OpenFlags;

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

#[test]
fn sqlite_addressable_delete_hides_listing() -> Result<()> {
    let mut settings = config::Settings::default();
    settings.database.in_memory = true;
    settings.database.min_conn = 1;
    settings.database.max_conn = 1;

    let pool = build_pool(
        "sqlite-addressable-delete",
        &settings,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        1,
        1,
        false,
    );
    let mut conn = pool.get()?;
    upgrade_db(&mut conn)?;

    let pubkey = hex_64(0x11);
    let d_tag = "listing-1".to_string();
    let listing = build_event(
        hex_64(0x01),
        pubkey.clone(),
        30402,
        100,
        vec![vec!["d".to_string(), d_tag.clone()]],
    );
    assert_eq!(SqliteRepo::persist_event(&mut conn, &listing)?, 1);

    let address = format!("30402:{}:{}", pubkey, d_tag);
    let delete_event = build_event(
        hex_64(0x02),
        pubkey.clone(),
        5,
        200,
        vec![vec!["a".to_string(), address]],
    );
    assert_eq!(SqliteRepo::persist_event(&mut conn, &delete_event)?, 1);

    let pubkey_blob = hex::decode(&pubkey)?;
    let hidden: i64 = conn.query_row(
        "SELECT hidden FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.kind=? AND e.author=? AND t.name='d' AND t.value=? LIMIT 1;",
        params![listing.kind, pubkey_blob, d_tag],
        |row| row.get(0),
    )?;
    assert!(hidden != 0);

    Ok(())
}
