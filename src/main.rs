use std::{env, io::Error};

use futures_util::{future, StreamExt, TryStreamExt};
use log::{info, warn};
use tokio::net::{TcpListener, TcpStream};

#[tokio::main]
async fn main() -> Result<(), Error> {
    let _ = env_logger::try_init();
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());

    // Create the event loop and TCP listener we'll accept connections on.
    let try_socket = TcpListener::bind(&addr).await;
    let listener = try_socket.expect("Failed to bind");
    info!("Listening on: {}", addr);

    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(accept_connection(stream));
    }

    Ok(())
}

async fn accept_connection(stream: TcpStream) {
    let addr = stream
        .peer_addr()
        .expect("connected streams should have a peer address");
    info!("Peer address: {}", addr);

    let ws_stream_res = tokio_tungstenite::accept_async(stream).await;

    match ws_stream_res {
        Ok(ws_stream) => {
            info!("New WebSocket connection: {}", addr);

            let (write, read) = ws_stream.split();
            // We should not forward messages other than text or binary.
            read.try_filter(|msg| future::ready(msg.is_text() || msg.is_binary()))
                .forward(write)
                .await
                .unwrap_or_else(|_| warn!("Failed to forward message"));
        }
        Err(_) => {
            println!("Error");
            info!("Error during websocket handshake");
        }
    };
}
