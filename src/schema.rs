//! Database schema and migrations
use crate::db::PooledConnection;
use crate::error::Result;
use crate::utils::is_hex;
use log::*;
use rusqlite::limits::Limit;
use rusqlite::params;
use rusqlite::Connection;

// TODO: drop the pubkey_ref and event_ref tables

/// Startup DB Pragmas
pub const STARTUP_SQL: &str = r##"
PRAGMA main.synchronous=NORMAL;
PRAGMA foreign_keys = ON;
pragma mmap_size = 536870912; -- 512MB of mmap
"##;

/// Schema definition
const INIT_SQL: &str = r##"
-- Database settings
PRAGMA encoding = "UTF-8";
PRAGMA journal_mode=WAL;
PRAGMA main.synchronous=NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA application_id = 1654008667;
PRAGMA user_version = 5;

-- Event Table
CREATE TABLE IF NOT EXISTS event (
id INTEGER PRIMARY KEY,
event_hash BLOB NOT NULL, -- 4-byte hash
first_seen INTEGER NOT NULL, -- when the event was first seen (not authored!) (seconds since 1970)
created_at INTEGER NOT NULL, -- when the event was authored
author BLOB NOT NULL, -- author pubkey
kind INTEGER NOT NULL, -- event kind
hidden INTEGER, -- relevant for queries
content TEXT NOT NULL -- serialized json of event object
);

-- Event Indexes
CREATE UNIQUE INDEX IF NOT EXISTS event_hash_index ON event(event_hash);
CREATE INDEX IF NOT EXISTS created_at_index ON event(created_at);
CREATE INDEX IF NOT EXISTS author_index ON event(author);
CREATE INDEX IF NOT EXISTS kind_index ON event(kind);

-- Tag Table
-- Tag values are stored as either a BLOB (if they come in as a
-- hex-string), or TEXT otherwise.
-- This means that searches need to select the appropriate column.
CREATE TABLE IF NOT EXISTS tag (
id INTEGER PRIMARY KEY,
event_id INTEGER NOT NULL, -- an event ID that contains a tag.
name TEXT, -- the tag name ("p", "e", whatever)
value TEXT, -- the tag value, if not hex.
value_hex BLOB, -- the tag value, if it can be interpreted as a hex string.
FOREIGN KEY(event_id) REFERENCES event(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS tag_val_index ON tag(value);
CREATE INDEX IF NOT EXISTS tag_val_hex_index ON tag(value_hex);

-- NIP-05 User Validation
CREATE TABLE IF NOT EXISTS user_verification (
id INTEGER PRIMARY KEY,
metadata_event INTEGER NOT NULL, -- the metadata event used for this validation.
name TEXT NOT NULL, -- the nip05 field value (user@domain).
verified_at INTEGER, -- timestamp this author/nip05 was most recently verified.
failed_at INTEGER, -- timestamp a verification attempt failed (host down).
failure_count INTEGER DEFAULT 0, -- number of consecutive failures.
FOREIGN KEY(metadata_event) REFERENCES event(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS user_verification_name_index ON user_verification(name);
CREATE INDEX IF NOT EXISTS user_verification_event_index ON user_verification(metadata_event);
"##;

/// Determine the current application database schema version.
pub fn db_version(conn: &mut Connection) -> Result<usize> {
    let query = "PRAGMA user_version;";
    let curr_version = conn.query_row(query, [], |row| row.get(0))?;
    Ok(curr_version)
}

/// Upgrade DB to latest version, and execute pragma settings
pub fn upgrade_db(conn: &mut PooledConnection) -> Result<()> {
    // check the version.
    let mut curr_version = db_version(conn)?;
    info!("DB version = {:?}", curr_version);

    debug!(
        "SQLite max query parameters: {}",
        conn.limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER)
    );
    debug!(
        "SQLite max table/blob/text length: {} MB",
        (conn.limit(Limit::SQLITE_LIMIT_LENGTH) as f64 / (1024 * 1024) as f64).floor()
    );
    debug!(
        "SQLite max SQL length: {} MB",
        (conn.limit(Limit::SQLITE_LIMIT_SQL_LENGTH) as f64 / (1024 * 1024) as f64).floor()
    );

    // initialize from scratch
    if curr_version == 0 {
        match conn.execute_batch(INIT_SQL) {
            Ok(()) => {
                info!("database pragma/schema initialized to v4, and ready");
            }
            Err(err) => {
                error!("update failed: {}", err);
                panic!("database could not be initialized");
            }
        }
    }
    if curr_version == 1 {
        // only change is adding a hidden column to events.
        let upgrade_sql = r##"
ALTER TABLE event ADD hidden INTEGER;
UPDATE event SET hidden=FALSE;
PRAGMA user_version = 2;
"##;
        match conn.execute_batch(upgrade_sql) {
            Ok(()) => {
                info!("database schema upgraded v1 -> v2");
                curr_version = 2;
            }
            Err(err) => {
                error!("update failed: {}", err);
                panic!("database could not be upgraded");
            }
        }
    }
    if curr_version == 2 {
        // this version lacks the tag column
        info!("database schema needs update from 2->3");
        let upgrade_sql = r##"
CREATE TABLE IF NOT EXISTS tag (
id INTEGER PRIMARY KEY,
event_id INTEGER NOT NULL, -- an event ID that contains a tag.
name TEXT, -- the tag name ("p", "e", whatever)
value TEXT, -- the tag value, if not hex.
value_hex BLOB, -- the tag value, if it can be interpreted as a hex string.
FOREIGN KEY(event_id) REFERENCES event(id) ON UPDATE CASCADE ON DELETE CASCADE
);
PRAGMA user_version = 3;
"##;
        // TODO: load existing refs into tag table
        match conn.execute_batch(upgrade_sql) {
            Ok(()) => {
                info!("database schema upgraded v2 -> v3");
                curr_version = 3;
            }
            Err(err) => {
                error!("update failed: {}", err);
                panic!("database could not be upgraded");
            }
        }
        info!("Starting transaction");
        // iterate over every event/pubkey tag
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare("select event_id, \"e\", lower(hex(referenced_event)) from event_ref union select event_id, \"p\", lower(hex(referenced_pubkey)) from pubkey_ref;")?;
            let mut tag_rows = stmt.query([])?;
            while let Some(row) = tag_rows.next()? {
                // we want to capture the event_id that had the tag, the tag name, and the tag hex value.
                let event_id: u64 = row.get(0)?;
                let tag_name: String = row.get(1)?;
                let tag_value: String = row.get(2)?;
                // this will leave behind p/e tags that were non-hex, but they are invalid anyways.
                if is_hex(&tag_value) {
                    tx.execute(
                        "INSERT INTO tag (event_id, name, value_hex) VALUES (?1, ?2, ?3);",
                        params![event_id, tag_name, hex::decode(&tag_value).ok()],
                    )?;
                }
            }
        }
        tx.commit()?;
        info!("Upgrade complete");
    }
    if curr_version == 3 {
        info!("database schema needs update from 3->4");
        let upgrade_sql = r##"
-- incoming metadata events with nip05
CREATE TABLE IF NOT EXISTS user_verification (
id INTEGER PRIMARY KEY,
metadata_event INTEGER NOT NULL, -- the metadata event used for this validation.
name TEXT NOT NULL, -- the nip05 field value (user@domain).
verified_at INTEGER, -- timestamp this author/nip05 was most recently verified.
failed_at INTEGER, -- timestamp a verification attempt failed (host down).
failure_count INTEGER DEFAULT 0, -- number of consecutive failures.
FOREIGN KEY(metadata_event) REFERENCES event(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS user_verification_name_index ON user_verification(name);
CREATE INDEX IF NOT EXISTS user_verification_event_index ON user_verification(metadata_event);
PRAGMA user_version = 4;
"##;
        match conn.execute_batch(upgrade_sql) {
            Ok(()) => {
                info!("database schema upgraded v3 -> v4");
                curr_version = 4;
            }
            Err(err) => {
                error!("update failed: {}", err);
                panic!("database could not be upgraded");
            }
        }
    }

    if curr_version == 4 {
        info!("database schema needs update from 4->5");
        let upgrade_sql = r##"
DROP TABLE IF EXISTS event_ref;
DROP TABLE IF EXISTS pubkey_ref;
PRAGMA user_version=5;
"##;
        match conn.execute_batch(upgrade_sql) {
            Ok(()) => {
                info!("database schema upgraded v4 -> v5");
                // uncomment if we have a newer version
                //curr_version = 5;
            }
            Err(err) => {
                error!("update failed: {}", err);
                panic!("database could not be upgraded");
            }
        }
    } else if curr_version == 5 {
        debug!("Database version was already current");
    } else if curr_version > 5 {
        panic!("Database version is newer than supported by this executable");
    }

    // Setup PRAGMA
    conn.execute_batch(STARTUP_SQL)?;
    debug!("SQLite PRAGMA startup completed");
    Ok(())
}
