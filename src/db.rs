use crate::error::Result;
use crate::event::Event;
use crate::subscription::Subscription;
use hex;
use log::*;
use rusqlite::params;
use rusqlite::Connection;
use rusqlite::OpenFlags;
use std::path::Path;
use tokio::task;

const DB_FILE: &str = "nostr.db";

// schema
const INIT_SQL: &str = r##"
PRAGMA encoding = "UTF-8";
PRAGMA journal_mode=WAL;
PRAGMA main.synchronous=NORMAL;
PRAGMA foreign_keys = ON;
PRAGMA application_id = 1654008667;
PRAGMA user_version = 1;
pragma mmap_size = 536870912; -- 512MB of mmap
CREATE TABLE IF NOT EXISTS event (
id INTEGER PRIMARY KEY,
event_hash BLOB NOT NULL, -- 4-byte hash
first_seen INTEGER NOT NULL, -- when the event was first seen (not authored!) (seconds since 1970)
created_at INTEGER NOT NULL, -- when the event was authored
author BLOB NOT NULL, -- author pubkey
kind INTEGER NOT NULL, -- event kind
content TEXT NOT NULL -- serialized json of event object
);
CREATE UNIQUE INDEX IF NOT EXISTS event_hash_index ON event(event_hash);
CREATE INDEX IF NOT EXISTS created_at_index ON event(created_at);
CREATE INDEX IF NOT EXISTS author_index ON event(author);
CREATE INDEX IF NOT EXISTS kind_index ON event(kind);
CREATE TABLE IF NOT EXISTS event_ref (
id INTEGER PRIMARY KEY,
event_id INTEGER NOT NULL, -- an event ID that contains an #e tag.
referenced_event BLOB NOT NULL, -- the event that is referenced.
FOREIGN KEY(event_id) REFERENCES event(id) ON UPDATE CASCADE ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS event_ref_index ON event_ref(referenced_event);
CREATE TABLE IF NOT EXISTS pubkey_ref (
id INTEGER PRIMARY KEY,
event_id INTEGER NOT NULL, -- an event ID that contains an #p tag.
referenced_pubkey BLOB NOT NULL, -- the pubkey that is referenced.
FOREIGN KEY(event_id) REFERENCES event(id) ON UPDATE RESTRICT ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS pubkey_ref_index ON pubkey_ref(referenced_pubkey);
"##;

/// Spawn a database writer that persists events to the SQLite store.
pub async fn db_writer(
    mut event_rx: tokio::sync::mpsc::Receiver<Event>,
) -> tokio::task::JoinHandle<Result<()>> {
    task::spawn_blocking(move || {
        let mut conn = Connection::open_with_flags(
            Path::new(DB_FILE),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;
        info!("Opened database for writing");
        // TODO: determine if we need to execute the init script.
        // TODO: check database app id / version before proceeding.
        match conn.execute_batch(INIT_SQL) {
            Ok(()) => info!("init completed"),
            Err(err) => info!("update failed: {}", err),
        }
        loop {
            // call blocking read on channel
            let next_event = event_rx.blocking_recv();
            // if the channel has closed, we will never get work
            if next_event.is_none() {
                info!("No more event senders for DB, shutting down.");
                break;
            }
            let event = next_event.unwrap();
            info!("Got event to write: {}", event.get_event_id_prefix());
            match write_event(&mut conn, &event) {
                Ok(updated) => {
                    if updated == 0 {
                        info!("nothing inserted (dupe?)");
                    } else {
                        info!("persisted new event");
                    }
                }
                Err(err) => {
                    info!("event insert failed: {}", err);
                }
            }
        }
        conn.close().ok();
        info!("database connection closed");
        Ok(())
    })
}

pub fn write_event(conn: &mut Connection, e: &Event) -> Result<usize> {
    // start transaction
    let tx = conn.transaction()?;
    // get relevant fields from event and convert to blobs.
    let id_blob = hex::decode(&e.id).ok();
    let pubkey_blob = hex::decode(&e.pubkey).ok();
    let event_str = serde_json::to_string(&e).ok();
    // ignore if the event hash is a duplicate.x
    let ins_count = tx.execute(
        "INSERT OR IGNORE INTO event (event_hash, created_at, kind, author, content, first_seen) VALUES (?1, ?2, ?3, ?4, ?5, strftime('%s','now'));",
        params![id_blob, e.created_at, e.kind, pubkey_blob, event_str]
    )?;
    let ev_id = tx.last_insert_rowid();
    let etags = e.get_event_tags();
    if etags.len() > 0 {
        // this will need to
        for etag in etags.iter() {
            tx.execute(
                "INSERT OR IGNORE INTO event_ref (event_id, referenced_event) VALUES (?1, ?2)",
                params![ev_id, hex::decode(&etag).ok()],
            )?;
        }
    }
    let ptags = e.get_pubkey_tags();
    if ptags.len() > 0 {
        for ptag in ptags.iter() {
            tx.execute(
                "INSERT OR IGNORE INTO event_ref (event_id, referenced_pubkey) VALUES (?1, ?2)",
                params![ev_id, hex::decode(&ptag).ok()],
            )?;
        }
    }
    tx.commit()?;
    Ok(ins_count)
}

// Queries return a subscription identifier and the serialized event.
#[derive(PartialEq, Debug, Clone)]
pub struct QueryResult {
    pub sub_id: String,
    pub event: String,
}

// TODO: make this hex
fn is_alphanum(s: &str) -> bool {
    s.chars().all(|x| char::is_ascii_hexdigit(&x))
}

fn query_from_sub(sub: &Subscription) -> String {
    // build a dynamic SQL query.  all user-input is either an integer
    // (sqli-safe), or a string that is filtered to only contain
    // hexadecimal characters.
    let mut query =
        "SELECT DISTINCT(e.content) FROM event e LEFT JOIN event_ref er ON e.id=er.event_id LEFT JOIN pubkey_ref pr ON e.id=pr.event_id "
            .to_owned();
    // for every filter in the subscription, generate a where clause
    // all individual filter clause strings for this subscription
    let mut filter_clauses: Vec<String> = Vec::new();
    for f in sub.filters.iter() {
        // individual filter components
        let mut filter_components: Vec<String> = Vec::new();
        // Query for "author"
        // https://github.com/fiatjaf/nostr/issues/34
        // I believe the author & authors fields are redundant.
        if f.author.is_some() {
            let author_str = f.author.as_ref().unwrap();
            if is_alphanum(author_str) {
                let author_clause = format!("author = x'{}'", author_str);
                filter_components.push(author_clause);
            }
        }
        // Query for "authors"
        if f.authors.is_some() {
            let authors_escaped: Vec<String> = f
                .authors
                .as_ref()
                .unwrap()
                .iter()
                .filter(|&x| is_alphanum(x))
                .map(|x| format!("x'{}'", x))
                .collect();
            let authors_clause = format!("author IN ({})", authors_escaped.join(", "));
            filter_components.push(authors_clause);
        }
        // Query for Kind
        if f.kind.is_some() {
            // kind is number, no escaping needed
            let kind_clause = format!("kind = {}", f.kind.unwrap());
            filter_components.push(kind_clause);
        }
        // Query for event
        if f.id.is_some() {
            // whitelist characters
            let id_str = f.id.as_ref().unwrap();
            if is_alphanum(id_str) {
                let id_clause = format!("event_hash = x'{}'", id_str);
                filter_components.push(id_clause);
            }
        }
        // Query for referenced event
        if f.event.is_some() {
            // whitelist characters
            let ev_str = f.event.as_ref().unwrap();
            if is_alphanum(ev_str) {
                let ev_clause = format!("referenced_event = x'{}'", ev_str);
                filter_components.push(ev_clause);
            }
        }
        // Query for referenced pet name pubkey
        if f.pubkey.is_some() {
            // whitelist characters
            let pet_str = f.pubkey.as_ref().unwrap();
            if is_alphanum(pet_str) {
                let pet_clause = format!("referenced_pubkey = x'{}'", pet_str);
                filter_components.push(pet_clause);
            }
        }
        // Query for timestamp
        if f.since.is_some() {
            // timestamp is number, no escaping needed
            let created_clause = format!("created_at > {}", f.since.unwrap());
            filter_components.push(created_clause);
        }
        // combine all clauses, and add to filter_clauses
        if filter_components.len() > 0 {
            let mut fc = "( ".to_owned();
            fc.push_str(&filter_components.join(" AND "));
            fc.push_str(" )");
            filter_clauses.push(fc);
        }
    }

    // combine all filters with OR clauses, if any exist
    if filter_clauses.len() > 0 {
        query.push_str(" WHERE ");
        query.push_str(&filter_clauses.join(" OR "));
    }
    info!("Query: {}", query);
    return query;
}

pub async fn db_query(
    sub: Subscription,
    query_tx: tokio::sync::mpsc::Sender<QueryResult>,
    mut abandon_query_rx: tokio::sync::oneshot::Receiver<()>,
) {
    task::spawn_blocking(move || {
        let conn =
            Connection::open_with_flags(Path::new(DB_FILE), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        info!("Opened database for reading");
        info!("Going to query for: {:?}", sub);
        // generate query
        let q = query_from_sub(&sub);

        let mut stmt = conn.prepare(&q).unwrap();
        let mut event_rows = stmt.query([]).unwrap();
        let mut i: usize = 0;
        while let Some(row) = event_rows.next().unwrap() {
            // check if this is still active (we could do this every N rows)
            if abandon_query_rx.try_recv().is_ok() {
                info!("Abandoning query...");
                // we have received a request to abandon the query
                return;
            }
            let event_json = row.get(0).unwrap();
            i += 1;
            info!("Sending event #{}", i);
            query_tx
                .blocking_send(QueryResult {
                    sub_id: sub.get_id(),
                    event: event_json,
                })
                .ok();
        }
        info!("Finished reading");
    });
}
