use futures::SinkExt;
use futures::StreamExt;
use log::*;
use nostr_rs_relay::close::Close;
use nostr_rs_relay::conn;
use nostr_rs_relay::db;
use nostr_rs_relay::error::{Error, Result};
use nostr_rs_relay::event::Event;
use nostr_rs_relay::protostream;
use nostr_rs_relay::protostream::NostrMessage::*;
use nostr_rs_relay::protostream::NostrResponse::*;
use std::collections::HashMap;
use std::env;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio::sync::broadcast;
use tokio::sync::broadcast::{Receiver, Sender};
use tokio::sync::mpsc;
use tokio::sync::oneshot;

/// Start running a Nostr relay server.
fn main() -> Result<(), Error> {
    // setup logger
    let _ = env_logger::try_init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8080".to_string());
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

        // this needs to be large enough to accomodate any slow
        // readers - otherwise messages will be dropped before they
        // can be processed.  Since this is global to all connections,
        // we can tolerate this being rather large (for 4096, the
        // buffer itself is about 1MB).
        let (bcast_tx, _) = broadcast::channel::<Event>(4096);
        // Establish database writer channel. This needs to be
        // accessible from sync code, which is why the broadcast
        // cannot be reused.
        let (event_tx, event_rx) = mpsc::channel::<Event>(16);
        // start the database writer.
        db::db_writer(event_rx).await;
        // setup a broadcast channel for invoking a process shutdown
        let (invoke_shutdown, _) = broadcast::channel::<()>(1);
        let shutdown_handler = invoke_shutdown.clone();
        // listen for ctrl-c interruupts
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.unwrap();
            // Your handler here
            info!("got ctrl-c");
            shutdown_handler.send(()).ok();
        });
        let mut stop_listening = invoke_shutdown.subscribe();
        // shutdown on Ctrl-C, or accept a new connection
        loop {
            tokio::select! {
                _ = stop_listening.recv() => {
                    break;
                }
                Ok((stream, _)) = listener.accept() => {
                    tokio::spawn(nostr_server(
                        stream,
                        bcast_tx.clone(),
                        event_tx.clone(),
                        invoke_shutdown.subscribe(),
                    ));
                }
            }
        }
    });
    Ok(())
}

async fn nostr_server(
    stream: TcpStream,
    broadcast: Sender<Event>,
    event_tx: tokio::sync::mpsc::Sender<Event>,
    mut shutdown: Receiver<()>,
) {
    // get a broadcast channel for clients to communicate on
    // wrap the TCP stream in a websocket.
    let mut bcast_rx = broadcast.subscribe();
    // upgrade the TCP connection to WebSocket
    let conn = tokio_tungstenite::accept_async(stream).await;
    let ws_stream = conn.expect("websocket handshake error");
    // wrap websocket into a stream & sink of Nostr protocol messages
    let mut nostr_stream = protostream::wrap_ws_in_nostr(ws_stream);
    // Track internal client state
    let mut conn = conn::ClientConn::new();
    let cid = conn.get_client_prefix();
    // Create a channel for receiving query results from the database.
    // we will send out the tx handle to any query we generate.
    let (query_tx, mut query_rx) = mpsc::channel::<db::QueryResult>(256);
    // maintain a hashmap of a oneshot channel for active subscriptions.
    // when these subscriptions are cancelled, make a message
    // available to the executing query so it knows to stop.
    //let (abandon_query_tx, _) = oneshot::channel::<()>();
    let mut running_queries: HashMap<String, oneshot::Sender<()>> = HashMap::new();

    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                // server shutting down, exit loop
                break;
            },
            Some(query_result) = query_rx.recv() => {
                info!("Got query result");
                let res = EventRes(query_result.sub_id,query_result.event);
                nostr_stream.send(res).await.ok();
            },
            Ok(global_event) = bcast_rx.recv() => {
                // ignoring closed broadcast errors, there will always be one sender available.
                // Is there a subscription for this event?
                let sub_name_opt = conn.get_matching_subscription(&global_event);
                if sub_name_opt.is_some() {
                    let sub_name = sub_name_opt.unwrap();
                    let event_str = serde_json::to_string(&global_event);
                    if event_str.is_ok() {
                        info!("sub match: client: {}, sub: {}, event: {}",
                              cid, sub_name,
                              global_event.get_event_id_prefix());
                        // create an event response and send it
                        let res = EventRes(sub_name.to_owned(),event_str.unwrap());
                        nostr_stream.send(res).await.ok();
                    } else {
                        warn!("could not convert event to string");
                    }
                }
            },
            // check if this client has a subscription
            proto_next = nostr_stream.next() => {
                match proto_next {
                    Some(Ok(EventMsg(ec))) => {
                        // An EventCmd needs to be validated to be converted into an Event
                        // handle each type of message
                        let parsed : Result<Event> = Result::<Event>::from(ec);
                        match parsed {
                            Ok(e) => {
                                let id_prefix:String = e.id.chars().take(8).collect();
                                info!("Successfully parsed/validated event: {} from client: {}", id_prefix, cid);
                                // Write this to the database
                                event_tx.send(e.clone()).await.ok();
                                // send this event to everyone listening.
                                let bcast_res = broadcast.send(e);
                                if bcast_res.is_err() {
                                    warn!("Could not send broadcast message: {:?}", bcast_res);
                                }
                            },
                            Err(_) => {info!("Client {} sent an invalid event", cid)}
                        }
                    },
                    Some(Ok(SubMsg(s))) => {
                        info!("Client {} requesting a subscription", cid);

                        // subscription handling consists of:
                        // * registering the subscription so future events can be matched
                        // * making a channel to cancel to request later
                        // * sending a request for a SQL query
                        let (abandon_query_tx, abandon_query_rx) = oneshot::channel::<()>();
                        running_queries.insert(s.id.to_owned(), abandon_query_tx);
                        // register this connection
                        conn.subscribe(s.clone()).ok();
                        // start a database query
                        db::db_query(s, query_tx.clone(), abandon_query_rx).await;
                    },
                    Some(Ok(CloseMsg(cc))) => {
                        // closing a request simply removes the subscription.
                        let parsed : Result<Close> = Result::<Close>::from(cc);
                        match parsed {
                            Ok(c) => {
                                let stop_tx = running_queries.remove(&c.id);
                                match stop_tx {
                                    Some(tx) => {
                                        info!("Removing query, telling DB to abandon query");
                                        tx.send(()).ok();
                                    },
                                    None => {}
                                }
                                conn.unsubscribe(c);
                            },
                            Err(_) => {info!("Invalid command ignored");}
                        }
                    },
                    None => {
                        info!("normal websocket close from client: {}",cid);
                        break;
                    },
                    Some(Err(Error::ConnError)) => {
                        info!("got connection close/error, disconnecting client: {}",cid);
                        break;
                    }
                    Some(Err(e)) => {
                        info!("got non-fatal error from client: {}, error: {:?}", cid, e);
                    },
                }
            },
        }
    }
    // connection cleanup - ensure any still running queries are terminated.
    for (_, stop_tx) in running_queries.into_iter() {
        stop_tx.send(()).ok();
    }
    info!("stopping client connection: {}", cid);
}
