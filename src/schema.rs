//! Database schema and migrations
use crate::db::PooledConnection;
use crate::error::Result;
use crate::event::{single_char_tagname, Event};
use crate::utils::is_lower_hex;
use log::*;
use rusqlite::limits::Limit;
use rusqlite::params;
use rusqlite::Connection;
use std::time::Instant;

/// Startup DB Pragmas
pub const STARTUP_SQL: &str = r##"
PRAGMA main.synchronous=NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA journal_size_limit=32768;
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
PRAGMA user_version = 6;

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
value_hex BLOB, -- the tag value, if it can be interpreted as a lowercase hex string.
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
                info!("database pragma/schema initialized to v6, and ready");
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
                if is_lower_hex(&tag_value) {
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
        info!("database schema needs update from 5->6");
        // We need to rebuild the tags table.  iterate through the
        // event table.  build event from json, insert tags into a
        // fresh tag table.  This was needed due to a logic error in
        // how hex-like tags got indexed.
        let start = Instant::now();
        let tx = conn.transaction()?;
        {
            // Clear out table
            tx.execute("DELETE FROM tag;", [])?;
            let mut stmt = tx.prepare("select id, content from event order by id;")?;
            let mut tag_rows = stmt.query([])?;
            while let Some(row) = tag_rows.next()? {
                // we want to capture the event_id that had the tag, the tag name, and the tag hex value.
                let event_id: u64 = row.get(0)?;
                let event_json: String = row.get(1)?;
                let event: Event = serde_json::from_str(&event_json)?;
                // look at each event, and each tag, creating new tag entries if appropriate.
                for t in event.tags.iter().filter(|x| x.len() > 1) {
                    let tagname = t.get(0).unwrap();
                    let tagnamechar_opt = single_char_tagname(tagname);
                    if tagnamechar_opt.is_none() {
                        continue;
                    }
                    // safe because len was > 1
                    let tagval = t.get(1).unwrap();
                    // insert as BLOB if we can restore it losslessly.
                    // this means it needs to be even length and lowercase.
                    if (tagval.len() % 2 == 0) && is_lower_hex(tagval) {
                        tx.execute(
                            "INSERT INTO tag (event_id, name, value_hex) VALUES (?1, ?2, ?3);",
                            params![event_id, tagname, hex::decode(&tagval).ok()],
                        )?;
                    } else {
                        // otherwise, insert as text
                        tx.execute(
                            "INSERT INTO tag (event_id, name, value) VALUES (?1, ?2, ?3);",
                            params![event_id, tagname, &tagval],
                        )?;
                    }
                }
            }
            tx.execute("PRAGMA user_version = 6;", [])?;
        }
        tx.commit()?;
        info!("database schema upgraded v5 -> v6 in {:?}", start.elapsed());
        // vacuum after large table modification
        let start = Instant::now();
        conn.execute("VACUUM;", [])?;
        info!("vacuumed DB after tags rebuild in {:?}", start.elapsed());
    } else if curr_version == 6 {
        debug!("Database version was already current (v6)");
    } else if curr_version > 7 {
        panic!(
            "Database version is newer than supported by this executable (v{})",
            curr_version
        );
    }

    // Setup PRAGMA
    conn.execute_batch(STARTUP_SQL)?;
    debug!("SQLite PRAGMA startup completed");
    Ok(())
}
