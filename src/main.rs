use std::{env, io::Error};

//use futures::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use log::{debug, info, warn};
use nostr_rs_relay::proto::Proto;
//use tokio::io::{ReadHalf, WriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio_tungstenite::WebSocketStream;
use tungstenite::error::Error::*;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::frame::CloseFrame;
use tungstenite::protocol::{Message, WebSocketConfig};

fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "0.0.0.0:8888".to_string());
    // configure tokio runtime
    let rt = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_io()
        .thread_name("tokio-ws")
        .on_thread_stop(|| {
            info!("thread stopping");
        })
        .on_thread_start(|| {
            info!("thread starting");
        })
        .build()
        .unwrap();
    // start tokio
    rt.block_on(async {
        // Create the event loop and TCP listener we'll accept connections on.
        let listener = TcpListener::bind(&addr).await.expect("Failed to bind");
        info!("Listening on: {}", addr);
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(nostr_server(stream));
        }
    });
    Ok(())
}

// Todo: Implement sending messages to all other clients; example:
// https://github.com/snapview/tokio-tungstenite/blob/master/examples/server.rs

// Handles new TCP connections
async fn nostr_server(stream: TcpStream) {
    let addr = stream
        .peer_addr()
        .expect("connected streams should have a peer address");
    info!("Peer address: {}", addr);
    let config = WebSocketConfig {
        max_send_queue: None,
        max_message_size: Some(2 << 16), // 64K
        max_frame_size: Some(2 << 16),   // 64k
        accept_unmasked_frames: false,   // follow the spec
    };
    let conn = tokio_tungstenite::accept_async_with_config(stream, Some(config)).await;
    match conn {
        Ok(ws_stream) => {
            info!("New WebSocket connection: {}", addr);
            process_client(ws_stream).await;
        }
        Err(_) => {
            println!("Error");
            info!("Error during websocket handshake");
        }
    };
}

// Handles valid clients who have upgraded to WebSockets
async fn process_client(mut stream: WebSocketStream<TcpStream>) {
    // get a protocol helper;
    let mut proto = Proto::new();
    // TODO: select on a timeout to kill non-responsive clients

    while let Some(mes_res) = stream.next().await {
        // as long as connection is not closed...
        match mes_res {
            Ok(Message::Text(cmd)) => {
                info!("Message received");
                let length = cmd.len();
                debug!("Message: {}", cmd);
                stream
                    .send(Message::Text(format!(
                        "got your message of length {}",
                        length
                    )))
                    .await
                    .expect("send failed");
                // Handle this request. Everything else below is basically websocket error handling.
                let proto_error = proto.process_message(cmd);
                match proto_error {
                    Err(_) => {
                        stream
                            .send(Message::Text(
                                "[\"NOTICE\", \"Failed to process message.\"]".to_owned(),
                            ))
                            .await
                            .expect("send failed");
                    }
                    Ok(_) => {
                        info!("Message processed successfully");
                    }
                }
            }
            Ok(Message::Binary(_)) => {
                info!("Ignoring Binary message");
                stream
                    .send(Message::Text(
                        "[\"NOTICE\", \"BINARY_INVALID: Binary messages are not supported.\"]"
                            .to_owned(),
                    ))
                    .await
                    .expect("send failed");
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => debug!("Ping/Pong"),
            Ok(Message::Close(_)) => {
                info!("Got request to close connection");
                return;
            }
            Err(tungstenite::error::Error::Capacity(
                tungstenite::error::CapacityError::MessageTooLong { size, max_size },
            )) => {
                info!(
                    "Message size too large, disconnecting this client. ({} > {})",
                    size, max_size
                );
                stream.send(Message::Text("[\"NOTICE\", \"MAX_EVENT_SIZE_EXCEEDED: Exceeded maximum event size for this relay.  Closing Connection.\"]".to_owned())).await.expect("send notice failed");
                stream
                    .close(Some(CloseFrame {
                        code: CloseCode::Size,
                        reason: "Exceeded max message size".into(),
                    }))
                    .await
                    .expect("failed to send close frame");
                return;
            }
            Err(AlreadyClosed) => {
                warn!("this connection was already closed, and we tried to operate on it");
                return;
            }
            Err(ConnectionClosed) | Err(Io(_)) => {
                debug!("Closing this connection normally");
                return;
            }
            Err(Tls(_)) | Err(Protocol(_)) | Err(Utf8) | Err(Url(_)) | Err(HttpFormat(_))
            | Err(Http(_)) => {
                info!("websocket/tls/enc protocol error, dropping connection");
                return;
            }
            Err(e) => {
                warn!("Some new kind of error, bailing: {:?}", e);
                return;
            }
        }
    }
    println!("Client is exiting, no longer reading websocket messages");
}
