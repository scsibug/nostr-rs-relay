use std::io;
use std::time;

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
) -> Result<(String, Vec<u8>), io::Error> {
    let file_name = format!("{}_{}.csv",
        &pubkey,
        time::SystemTime::now().duration_since(time::UNIX_EPOCH).unwrap_or_default().as_secs()
    );
    let mut buff: Vec<u8> = Vec::new();
    buff.extend_from_slice(b"id,pubkey,delegated_by,created_at,kind,content\n");
    while let Some(events) = events_rx.recv().await {
        if cancel_rx.try_recv().is_ok() {
            break;
        }
        for event in events {
            let mut line = format!(
                r#"{},{},{},{},{},"{}""#,
                &event.id,
                &event.pubkey,
                &event.delegated_by.unwrap_or_default(),
                &event.created_at,
                &event.kind,
                &event.content,
            );
            line.push('\n');
            buff.extend_from_slice(line.as_bytes());
        }
    }
    Ok((file_name, buff))
}
