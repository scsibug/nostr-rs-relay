use std::{fs::File, io};

use tokio::sync::mpsc::Receiver;
use serde::Serialize;

use crate::event::Event;

#[derive(Serialize)]
pub struct KindStatistics {
    pub kind: u64,
    pub count: usize,
    pub last_created_at: u64,
}

#[derive(Serialize)]
pub struct AccountStatistics {
    pub kinds: Vec<KindStatistics>,
}

pub async fn write_user_events(
    pubkey: &str,
    mut events_rx: Receiver<Vec<Event>>,
    mut cancel_rx: tokio::sync::broadcast::Receiver<()>
) -> Result<File, io::Error> {
    let file = File::create(format!("{}.csv", &pubkey))?;
    while let Some(events) = events_rx.recv().await {
        if cancel_rx.try_recv().is_ok() {
            break;
        }
    }
    Ok(file)
}
