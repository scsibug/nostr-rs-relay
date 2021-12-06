use futures::StreamExt;
use log::*;
use nostr_rs_relay::close::Close;
use nostr_rs_relay::conn;
use nostr_rs_relay::error::{Error, Result};
use nostr_rs_relay::event::Event;
use nostr_rs_relay::protostream;
use nostr_rs_relay::protostream::NostrMessage::*;
use rusqlite::Result as SQLResult;
use std::env;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio::sync::broadcast;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc;

/// Start running a Nostr relay server.
fn main() -> Result<(), Error> {
    // setup logger
    let _ = env_logger::try_init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8888".to_string());
    // configure tokio runtime
    let rt = Builder::new_multi_thread()
        .enable_all()
        .thread_name("tokio-ws")
        .build()
        .unwrap();
    // start tokio
    rt.block_on(async {
        let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
        info!("Listening on: {}", addr);
        // Establish global broadcast channel.  This is where all
        // accepted events will be distributed for other connected clients.
        let (bcast_tx, _) = broadcast::channel::<Event>(64);
        // Establish database writer channel. This needs to be
        // accessible from sync code, which is why the broadcast
        // cannot be reused.
        let (event_tx, _) = mpsc::channel::<Event>(64);
        // start the database writer.
        // TODO: manage program termination, to close the DB.
        //let _db_handle = db_writer(event_rx).await;
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(nostr_server(stream, bcast_tx.clone(), event_tx.clone()));
        }
    });
    Ok(())
}

async fn _db_writer(_event_rx: tokio::sync::mpsc::Receiver<Event>) -> SQLResult<()> {
    unimplemented!();
}

async fn nostr_server(
    stream: TcpStream,
    broadcast: Sender<Event>,
    _event_tx: tokio::sync::mpsc::Sender<Event>,
) {
    // get a broadcast channel for clients to communicate on
    // wrap the TCP stream in a websocket.
    let mut _bcast_rx = broadcast.subscribe();
    let conn = tokio_tungstenite::accept_async(stream).await;
    let ws_stream = conn.expect("websocket handshake error");
    // a stream & sink of Nostr protocol messages
    let mut nostr_stream = protostream::wrap_ws_in_nostr(ws_stream);
    //let task_queue = mpsc::channel::<NostrMessage>(16);
    // track connection state so we can break when it fails
    // Track internal client state
    let mut conn = conn::ClientConn::new();
    let mut conn_good = true;
    loop {
        tokio::select! {
            proto_next = nostr_stream.next() => {
                match proto_next {
                    Some(Ok(EventMsg(ec))) => {
                        // An EventCmd needs to be validated to be converted into an Event
                        // handle each type of message
                        let parsed : Result<Event> = Result::<Event>::from(ec);
                        match parsed {
                            Ok(e) => {
                                let id_prefix:String = e.id.chars().take(8).collect();
                                info!("Successfully parsed/validated event: {}", id_prefix)},
                            Err(_) => {info!("Invalid event ignored")}
                        }
                    },
                    Some(Ok(SubMsg(s))) => {
                        // subscription handling consists of:
                        // adding new subscriptions to the client conn:
                        conn.subscribe(s).ok();
                        // TODO: sending a request for a SQL query
                    },
                    Some(Ok(CloseMsg(cc))) => {
                        // closing a request simply removes the subscription.
                        let parsed : Result<Close> = Result::<Close>::from(cc);
                        match parsed {
                            Ok(c) => {conn.unsubscribe(c);},
                            Err(_) => {info!("Invalid command ignored");}
                        }
                    },
                    None => {
                        info!("stream ended");
                    },
                    Some(Err(Error::ConnError)) => {
                        debug!("got connection error, disconnecting");
                        conn_good = false;
                       if conn_good {
                           info!("Lint bug?, https://github.com/rust-lang/rust/pull/57302");
                       }
                        return
                    }
                    Some(Err(e)) => {
                        info!("got error, continuing: {:?}", e);
                    },
                }
            }
        }
        if !conn_good {
            break;
        }
    }
}
