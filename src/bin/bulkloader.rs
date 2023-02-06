use std::io;
use std::path::Path;
use nostr_rs_relay::utils::is_lower_hex;
use tracing::info;
use nostr_rs_relay::config;
use nostr_rs_relay::event::{Event,single_char_tagname};
use nostr_rs_relay::error::{Error, Result};
use nostr_rs_relay::repo::sqlite::{PooledConnection, build_pool};
use nostr_rs_relay::repo::sqlite_migration::{curr_db_version, DB_VERSION};
use rusqlite::{OpenFlags, Transaction};
use std::sync::mpsc;
use std::thread;
use rusqlite::params;

/// Bulk load JSONL data from STDIN to the database specified in config.toml (or ./nostr.db as a default).
/// The database must already exist, this will not create a new one.
/// Tested against schema v13.

pub fn main() -> Result<()> {
    let _trace_sub = tracing_subscriber::fmt::try_init();
    println!("Nostr-rs-relay Bulk Loader");
    // check for a database file, or create one.
    let settings = config::Settings::new();
    if !Path::new(&settings.database.data_directory).is_dir() {
        info!("Database directory does not exist");
        return Err(Error::DatabaseDirError);
    }
    // Get a database pool
    let pool = build_pool("bulk-loader", &settings, OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE, 1,4,false);
    {
	// check for database schema version
	let mut conn: PooledConnection = pool.get()?;
	let version = curr_db_version(&mut conn)?;
	info!("current version is: {:?}", version);
	// ensure the schema version is current.
	if version != DB_VERSION {
	    info!("version is not current, exiting");
	    panic!("cannot write to schema other than v{DB_VERSION}");
	}
    }
    // this channel will contain parsed events ready to be inserted
    let (event_tx, event_rx) = mpsc::sync_channel(100_000);
    // Thread for reading events
    let _stdin_reader_handler = thread::spawn(move || {
	let stdin = io::stdin();
	for readline in stdin.lines() {
	    if let Ok(line) = readline {
		// try to parse a nostr event
		let eres: Result<Event, serde_json::Error> = serde_json::from_str(&line);
		if let Ok(mut e) = eres {
		    if let Ok(()) = e.validate() {
			e.build_index();
			//debug!("Event: {:?}", e);
			event_tx.send(Some(e)).ok();
		    } else {
			info!("could not validate event");
		    }
		} else {
		    info!("error reading event: {:?}", eres);
		}
	    } else {
		// error reading
		info!("error reading: {:?}", readline);
	    }
	}
	info!("finished parsing events");
	event_tx.send(None).ok();
	let ok: Result<()> = Ok(());
        ok
    });
    let mut conn: PooledConnection = pool.get()?;
    let mut events_read = 0;
    let event_batch_size =50_000;
    let mut new_events = 0;
    let mut has_more_events = true;
    while has_more_events {
	// begin a transaction
	let tx = conn.transaction()?;
	// read in batch_size events and commit
	for _ in 0..event_batch_size {
	    match event_rx.recv() {
		Ok(Some(e)) => {
		    events_read += 1;
		    // ignore ephemeral events
		    if !(e.kind >= 20000 && e.kind < 30000) {
			match write_event(&tx, e) {
			    Ok(c) => {
				new_events += c;
			    },
			    Err(e) => {
				info!("error inserting event: {:?}", e);
			    }
			}
		    }
		},
		Ok(None) => {
		    // signal that the sender will never produce more
		    // events
		    has_more_events=false;
		    break;
		},
		Err(_) => {
		    info!("sender is closed");
		    // sender is done
		}
	    }
	}
	info!("committed {} events...", new_events);
	tx.commit()?;
	conn.execute_batch("pragma wal_checkpoint(truncate)")?;

    }
    info!("processed {} events", events_read);
    info!("stored {} new events", new_events);
    // get a connection for writing events
    // read standard in.
    info!("finished reading input");
    Ok(())
}

/// Write an event and update the tag table.
/// Assumes the event has its index built.
fn write_event(tx: &Transaction, e: Event) -> Result<usize> {
    let id_blob = hex::decode(&e.id).ok();
    let pubkey_blob: Option<Vec<u8>> = hex::decode(&e.pubkey).ok();
    let delegator_blob: Option<Vec<u8>> = e.delegated_by.as_ref().and_then(|d| hex::decode(d).ok());
    let event_str = serde_json::to_string(&e).ok();
    // ignore if the event hash is a duplicate.
    let ins_count = tx.execute(
	"INSERT OR IGNORE INTO event (event_hash, created_at, kind, author, delegated_by, content, first_seen, hidden) VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'), FALSE);",
	params![id_blob, e.created_at, e.kind, pubkey_blob, delegator_blob, event_str]
    )?;
    if ins_count == 0 {
	return Ok(0);
    }
    // we want to capture the event_id that had the tag, the tag name, and the tag hex value.
    let event_id = tx.last_insert_rowid();
    // look at each event, and each tag, creating new tag entries if appropriate.
    for t in e.tags.iter().filter(|x| x.len() > 1) {
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
                params![event_id, tagname, hex::decode(tagval).ok()],
	    )?;
        } else {
	    // otherwise, insert as text
	    tx.execute(
                "INSERT INTO tag (event_id, name, value) VALUES (?1, ?2, ?3);",
                params![event_id, tagname, &tagval],
            )?;
        }
    }
    if e.is_replaceable() {
	//let query = "SELECT id FROM event WHERE kind=? AND author=? ORDER BY created_at DESC LIMIT 1;";
	//let count: usize = tx.query_row(query, params![e.kind, pubkey_blob], |row| row.get(0))?;
	//info!("found {} rows that /would/ be preserved", count);
	match tx.execute(
	    "DELETE FROM event WHERE kind=? and author=? and id NOT IN (SELECT id FROM event WHERE kind=? AND author=? ORDER BY created_at DESC LIMIT 1);",
	    params![e.kind, pubkey_blob, e.kind, pubkey_blob],
	) {
	    Ok(_) => {},
	    Err(x) => {info!("error deleting replaceable event: {:?}",x);}
	}
    }
    Ok(ins_count)
}
