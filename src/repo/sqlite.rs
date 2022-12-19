use crate::error::{Result};
use crate::event::{single_char_tagname, Event};
use crate::utils::{is_hex, is_lower_hex};
use hex;
use r2d2;
use rusqlite::params;
use rusqlite::types::ToSql;
use tracing::{info};

use crate::repo::{Repo};

type PooledConnection = r2d2::PooledConnection<r2d2_sqlite::SqliteConnectionManager>;

pub struct SqliteRepo<'conn> {
    conn: &'conn mut PooledConnection
}

impl SqliteRepo<'_> {
    pub fn new(c: &mut PooledConnection) -> SqliteRepo {
        SqliteRepo {
            conn: c
        }
    }
}

impl Repo for SqliteRepo<'_> {
    fn write_event(self: &mut Self, e: &Event) -> Result<usize> {
        // start transaction
        let tx = self.conn.transaction()?;
        // get relevant fields from event and convert to blobs.
        let id_blob = hex::decode(&e.id).ok();
        let pubkey_blob: Option<Vec<u8>> = hex::decode(&e.pubkey).ok();
        let delegator_blob: Option<Vec<u8>> = e.delegated_by.as_ref().and_then(|d| hex::decode(d).ok());
        let event_str = serde_json::to_string(&e).ok();
        // ignore if the event hash is a duplicate.
        let mut ins_count = tx.execute(
            "INSERT OR IGNORE INTO event (event_hash, created_at, kind, author, delegated_by, content, first_seen, hidden) VALUES (?1, ?2, ?3, ?4, ?5, ?6, strftime('%s','now'), FALSE);",
            params![id_blob, e.created_at, e.kind, pubkey_blob, delegator_blob, event_str]
        )?;
        if ins_count == 0 {
            // if the event was a duplicate, no need to insert event or
            // pubkey references.  This will abort the txn.
            return Ok(ins_count);
        }
        // remember primary key of the event most recently inserted.
        let ev_id = tx.last_insert_rowid();
        // add all tags to the tag table
        for tag in e.tags.iter() {
            // ensure we have 2 values.
            if tag.len() >= 2 {
                let tagname = &tag[0];
                let tagval = &tag[1];
                // only single-char tags are searchable
                let tagchar_opt = single_char_tagname(tagname);
                match &tagchar_opt {
                    Some(_) => {
                        // if tagvalue is lowercase hex;
                        if is_lower_hex(tagval) && (tagval.len() % 2 == 0) {
                            tx.execute(
                                "INSERT OR IGNORE INTO tag (event_id, name, value_hex) VALUES (?1, ?2, ?3)",
                                params![ev_id, &tagname, hex::decode(tagval).ok()],
                            )?;
                        } else {
                            tx.execute(
                                "INSERT OR IGNORE INTO tag (event_id, name, value) VALUES (?1, ?2, ?3)",
                                params![ev_id, &tagname, &tagval],
                            )?;
                        }
                    }
                    None => {}
                }
            }
        }
        // if this event is replaceable update, hide every other replaceable
        // event with the same kind from the same author that was issued
        // earlier than this.
        if e.kind == 0 || e.kind == 3 || (e.kind >= 10000 && e.kind < 20000) {
            let update_count = tx.execute(
                "UPDATE event SET hidden=TRUE WHERE id!=? AND kind=? AND author=? AND created_at <= ? and hidden!=TRUE",
                params![ev_id, e.kind, hex::decode(&e.pubkey).ok(), e.created_at],
            )?;
            if update_count > 0 {
                info!(
                "hid {} older replaceable kind {} events for author: {:?}",
                update_count,
                e.kind,
                e.get_author_prefix()
            );
            }
        }
        // if this event is a deletion, hide the referenced events from the same author.
        if e.kind == 5 {
            let event_candidates = e.tag_values_by_name("e");
            // first parameter will be author
            let mut params: Vec<Box<dyn ToSql>> = vec![Box::new(hex::decode(&e.pubkey)?)];
            event_candidates
                .iter()
                .filter(|x| is_hex(x) && x.len() == 64)
                .filter_map(|x| hex::decode(x).ok())
                .for_each(|x| params.push(Box::new(x)));
            let query = format!(
                "UPDATE event SET hidden=TRUE WHERE kind!=5 AND author=? AND event_hash IN ({})",
                repeat_vars(params.len() - 1)
            );
            let mut stmt = tx.prepare(&query)?;
            let update_count = stmt.execute(rusqlite::params_from_iter(params))?;
            info!(
            "hid {} deleted events for author {:?}",
            update_count,
            e.get_author_prefix()
        );
        } else {
            // check if a deletion has already been recorded for this event.
            // Only relevant for non-deletion events
            let del_count = tx.query_row(
                "SELECT e.id FROM event e LEFT JOIN tag t ON e.id=t.event_id WHERE e.author=? AND t.name='e' AND e.kind=5 AND t.value_hex=? LIMIT 1;",
                params![pubkey_blob, id_blob], |row| row.get::<usize, usize>(0));
            // check if a the query returned a result, meaning we should
            // hid the current event
            if del_count.ok().is_some() {
                // a deletion already existed, mark original event as hidden.
                info!(
                "hid event: {:?} due to existing deletion by author: {:?}",
                e.get_event_id_prefix(),
                e.get_author_prefix()
            );
                let _update_count =
                    tx.execute("UPDATE event SET hidden=TRUE WHERE id=?", params![ev_id])?;
                // event was deleted, so let caller know nothing new
                // arrived, preventing this from being sent to active
                // subscriptions
                ins_count = 0;
            }
        }
        tx.commit()?;
        Ok(ins_count)
    }
}

/// Produce a arbitrary list of '?' parameters.
fn repeat_vars(count: usize) -> String {
    if count == 0 {
        return "".to_owned();
    }
    let mut s = "?,".repeat(count);
    // Remove trailing comma
    s.pop();
    s
}
