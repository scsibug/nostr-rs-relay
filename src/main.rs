use std::{env, io::Error};

use futures_util::{SinkExt, StreamExt};
use log::{debug, info};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio_tungstenite::WebSocketStream;
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::frame::CloseFrame;
use tungstenite::protocol::{Message, WebSocketConfig};

fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());
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
async fn process_client(stream: WebSocketStream<TcpStream>) {
    let (mut write, mut read) = stream.split();
    // TODO: error on binary messages
    // TODO: error on text messages > MAX_EVENT_SIZE
    // TODO: select on a timeout to kill non-responsive clients

    while let Some(mes_res) = read.next().await {
        // as long as connection is not closed...
        match mes_res {
            Ok(Message::Text(cmd)) => {
                let length = cmd.len();
                write
                    .send(Message::Text(format!(
                        "got your message of length {}",
                        length
                    )))
                    .await
                    .expect("send failed");
            }
            Ok(Message::Binary(_)) => {
                info!("Ignoring Binary message");
                write
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
            Err(e) => {
                // TODO: check for specific error: (Capacity(MessageTooLong { size: xxxxx, max_size: 131072 }))
                info!(
                    "Message size too large, disconnecting this client: ({:?})",
                    e
                );
                write.send(Message::Text("[\"NOTICE\", \"MAX_EVENT_SIZE_EXCEEDED: Exceeded maximum event size for this relay.  Closing Connection.\"]".to_owned())).await.expect("send failed");
                write
                    .reunite(read)
                    .expect("reunite failed")
                    .close(Some(CloseFrame {
                        code: CloseCode::Size,
                        reason: "Exceeded max message size".into(),
                    }))
                    .await
                    .expect("failed to send close frame");
                return;
            }
        }
    }
    println!("Client is exiting, no longer reading websocket messages");
}
