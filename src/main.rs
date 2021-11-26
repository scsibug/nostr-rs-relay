use log::info;
use nostr_rs_relay::proto::Proto;
use std::{env, io::Error};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tungstenite::protocol::WebSocketConfig;

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

// Wrap/Upgrade TCP connections in WebSocket Streams, and hand off to Nostr protocol handler.
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
            // create a nostr protocol handler, and give it the stream
            let mut proto = Proto::new(ws_stream);
            proto.process_client().await
        }
        Err(_) => {
            println!("Error");
            info!("Error during websocket handshake");
        }
    };
}
