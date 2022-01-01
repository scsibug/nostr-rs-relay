//! Server process
use futures::SinkExt;
use futures::StreamExt;
use hyper::service::{make_service_fn, service_fn};
use hyper::upgrade::Upgraded;
use hyper::{
    header, server::conn::AddrStream, upgrade, Body, Request, Response, Server, StatusCode,
};
use log::*;
use nostr_rs_relay::close::Close;
use nostr_rs_relay::config;
use nostr_rs_relay::conn;
use nostr_rs_relay::db;
use nostr_rs_relay::error::{Error, Result};
use nostr_rs_relay::event::Event;
use nostr_rs_relay::protostream;
use nostr_rs_relay::protostream::NostrMessage::*;
use nostr_rs_relay::protostream::NostrResponse::*;
use std::collections::HashMap;
use std::convert::Infallible;
use std::env;
use std::net::SocketAddr;
use std::path::Path;
use tokio::runtime::Builder;
use tokio::sync::broadcast;
use tokio::sync::broadcast::{Receiver, Sender};
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_tungstenite::WebSocketStream;
use tungstenite::handshake;
use tungstenite::protocol::WebSocketConfig;

fn db_from_args(args: Vec<String>) -> Option<String> {
    if args.len() == 3 && args.get(1) == Some(&"--db".to_owned()) {
        return args.get(2).map(|x| x.to_owned());
    }
    None
}
async fn handle_web_request(
    mut request: Request<Body>,
    remote_addr: SocketAddr,
    broadcast: Sender<Event>,
    event_tx: tokio::sync::mpsc::Sender<Event>,
    shutdown: Receiver<()>,
) -> Result<Response<Body>, Infallible> {
    match (
        request.uri().path(),
        request.headers().contains_key(header::UPGRADE),
    ) {
        //if the request is ws_echo and the request headers contains an Upgrade key
        ("/", true) => {
            debug!("websocket with upgrade request");
            //assume request is a handshake, so create the handshake response
            let response = match handshake::server::create_response_with_body(&request, || {
                Body::empty()
            }) {
                Ok(response) => {
                    //in case the handshake response creation succeeds,
                    //spawn a task to handle the websocket connection
                    tokio::spawn(async move {
                        //using the hyper feature of upgrading a connection
                        match upgrade::on(&mut request).await {
                            //if successfully upgraded
                            Ok(upgraded) => {
                                //create a websocket stream from the upgraded object
                                let ws_stream = WebSocketStream::from_raw_socket(
                                    //pass the upgraded object
                                    //as the base layer stream of the Websocket
                                    upgraded,
                                    tokio_tungstenite::tungstenite::protocol::Role::Server,
                                    None,
                                )
                                .await;
                                tokio::spawn(nostr_server(
                                    ws_stream, broadcast, event_tx, shutdown,
                                ));
                            }
                            Err(e) => println!(
                                "error when trying to upgrade connection \
                                 from address {} to websocket connection. \
                                 Error is: {}",
                                remote_addr, e
                            ),
                        }
                    });
                    //return the response to the handshake request
                    response
                }
                Err(error) => {
                    warn!("websocket response failed");
                    let mut res =
                        Response::new(Body::from(format!("Failed to create websocket: {}", error)));
                    *res.status_mut() = StatusCode::BAD_REQUEST;
                    return Ok(res);
                }
            };
            Ok::<_, Infallible>(response)
        }
        ("/", false) => {
            // handle request at root with no upgrade header
            Ok(Response::new(Body::from(
                "This is a Nostr relay.\n".to_string(),
            )))
        }
        (_, _) => {
            //handle any other url
            Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Nothing here."))
                .unwrap())
        }
    }
}

async fn shutdown_signal() {
    // Wait for the CTRL+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
}

/// Start running a Nostr relay server.
fn main() -> Result<(), Error> {
    // setup logger
    let _ = env_logger::try_init();
    // get database directory from args
    let args: Vec<String> = env::args().collect();
    let db_dir: Option<String> = db_from_args(args);
    {
        let mut settings = config::SETTINGS.write().unwrap();
        // replace default settings with those read from config.toml
        let mut c = config::Settings::new();
        // update with database location
        if let Some(db) = db_dir {
            c.database.data_directory = db;
        }
        *settings = c;
    }

    let config = config::SETTINGS.read().unwrap();
    // do some config validation.
    if !Path::new(&config.database.data_directory).is_dir() {
        error!("Database directory does not exist");
        return Err(Error::DatabaseDirError);
    }
    debug!("config: {:?}", config);
    let addr = format!("{}:{}", config.network.address.trim(), config.network.port);
    let socket_addr = addr.parse().expect("listening address not valid");
    // configure tokio runtime
    let rt = Builder::new_multi_thread()
        .enable_all()
        .thread_name("tokio-ws")
        .build()
        .unwrap();
    // start tokio
    rt.block_on(async {
        let settings = config::SETTINGS.read().unwrap();
        info!("listening on: {}", socket_addr);
        // all client-submitted valid events are broadcast to every
        // other client on this channel.  This should be large enough
        // to accomodate slower readers (messages are dropped if
        // clients can not keep up).
        let (bcast_tx, _) = broadcast::channel::<Event>(settings.limits.broadcast_buffer);
        // validated events that need to be persisted are sent to the
        // database on via this channel.
        let (event_tx, event_rx) = mpsc::channel::<Event>(settings.limits.event_persist_buffer);
        // establish a channel for letting all threads now about a
        // requested server shutdown.
        let (invoke_shutdown, _) = broadcast::channel::<()>(1);
        let ctrl_c_shutdown = invoke_shutdown.clone();
        // // listen for ctrl-c interruupts
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.unwrap();
            info!("shutting down due to SIGINT");
            ctrl_c_shutdown.send(()).ok();
        });
        // start the database writer thread.  Give it a channel for
        // writing events, and for publishing events that have been
        // written (to all connected clients).
        db::db_writer(event_rx, bcast_tx.clone(), invoke_shutdown.subscribe()).await;
        info!("db writer created");
        // A `Service` is needed for every connection, so this
        // creates one from our `handle_request` function.
        let make_svc = make_service_fn(|conn: &AddrStream| {
            let remote_addr = conn.remote_addr();
            let bcast = bcast_tx.clone();
            let event = event_tx.clone();
            let stop = invoke_shutdown.clone();
            async move {
                // service_fn converts our function into a `Service`
                Ok::<_, Infallible>(service_fn(move |request: Request<Body>| {
                    handle_web_request(
                        request,
                        remote_addr,
                        bcast.clone(),
                        event.clone(),
                        stop.subscribe(),
                    )
                }))
            }
        });
        let server = Server::bind(&socket_addr)
            .serve(make_svc)
            .with_graceful_shutdown(shutdown_signal());
        // run hyper
        if let Err(e) = server.await {
            eprintln!("server error: {}", e);
        }
        // our code
    });
    Ok(())
}

/// Handle new client connections.  This runs through an event loop
/// for all client communication.
async fn nostr_server(
    ws_stream: WebSocketStream<Upgraded>,
    broadcast: Sender<Event>,
    event_tx: tokio::sync::mpsc::Sender<Event>,
    mut shutdown: Receiver<()>,
) {
    // get a broadcast channel for clients to communicate on
    let mut bcast_rx = broadcast.subscribe();
    let mut config = WebSocketConfig::default();
    {
        let settings = config::SETTINGS.read().unwrap();
        config.max_message_size = settings.limits.max_ws_message_bytes;
        config.max_frame_size = settings.limits.max_ws_frame_bytes;
    }
    // upgrade the TCP connection to WebSocket
    //let conn = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await;
    //let ws_stream = conn.expect("websocket handshake error");
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
    // for stats, keep track of how many events the client published,
    // and how many it received from queries.
    let mut client_published_event_count: usize = 0;
    let mut client_received_event_count: usize = 0;
    info!("new connection for client: {}", cid);
    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                // server shutting down, exit loop
                break;
            },
            Some(query_result) = query_rx.recv() => {
                // database informed us of a query result we asked for
                let res = EventRes(query_result.sub_id,query_result.event);
                client_received_event_count += 1;
                nostr_stream.send(res).await.ok();
            },
            Ok(global_event) = bcast_rx.recv() => {
                // an event has been broadcast to all clients
                // first check if there is a subscription for this event.
                let matching_subs = conn.get_matching_subscriptions(&global_event);
                for s in matching_subs {
                    // TODO: serialize at broadcast time, instead of
                    // once for each consumer.
                    if let Ok(event_str) = serde_json::to_string(&global_event) {
                        debug!("sub match: client: {}, sub: {}, event: {}",
                               cid, s,
                               global_event.get_event_id_prefix());
                        // create an event response and send it
                        let res = EventRes(s.to_owned(),event_str);
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
                                debug!("successfully parsed/validated event: {} from client: {}", id_prefix, cid);
                                // Write this to the database
                                event_tx.send(e.clone()).await.ok();
                                client_published_event_count += 1;
                            },
                            Err(_) => {
                                info!("client {} sent an invalid event", cid);
                                nostr_stream.send(NoticeRes("event was invalid".to_owned())).await.ok();
                            }
                        }
                    },
                    Some(Ok(SubMsg(s))) => {
                        debug!("client {} requesting a subscription", cid);
                        // subscription handling consists of:
                        // * registering the subscription so future events can be matched
                        // * making a channel to cancel to request later
                        // * sending a request for a SQL query
                        let (abandon_query_tx, abandon_query_rx) = oneshot::channel::<()>();
                        match conn.subscribe(s.clone()) {
                            Ok(()) => {
                                running_queries.insert(s.id.to_owned(), abandon_query_tx);
                                // start a database query
                                db::db_query(s, query_tx.clone(), abandon_query_rx).await;
                            },
                            Err(e) => {
                                info!("Subscription error: {}", e);
                                nostr_stream.send(NoticeRes(format!("{}",e))).await.ok();

                            }
                        }
                    },
                    Some(Ok(CloseMsg(cc))) => {
                        // closing a request simply removes the subscription.
                        let parsed : Result<Close> = Result::<Close>::from(cc);
                        match parsed {
                            Ok(c) => {
                                // check if a query is currently
                                // running, and remove it if so.
                                let stop_tx = running_queries.remove(&c.id);
                                if let Some(tx) = stop_tx {
                                    tx.send(()).ok();
                                }
                                // stop checking new events against
                                // the subscription
                                conn.unsubscribe(c);
                            },
                            Err(_) => {
                                info!("invalid command ignored");

                            }
                        }
                    },
                    None => {
                        debug!("normal websocket close from client: {}",cid);
                        break;
                    },
                    Some(Err(Error::ConnError)) => {
                        debug!("got connection close/error, disconnecting client: {}",cid);
                        break;
                    }
                    Some(Err(Error::EventMaxLengthError(s))) => {
                        info!("client {} sent event larger ({} bytes) than max size", cid, s);
                        nostr_stream.send(NoticeRes("event exceeded max size".to_owned())).await.ok();
                    },
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
    info!(
        "stopping connection for client: {} (client sent {} event(s), received {})",
        cid, client_published_event_count, client_received_event_count
    );
}
