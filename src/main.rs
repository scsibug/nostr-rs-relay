use std::{env, io::Error};

use futures_util::StreamExt;
use log::info;
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Builder;
use tokio_tungstenite::WebSocketStream;
//use tungstenite::protocol::Message;

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

async fn nostr_server(stream: TcpStream) {
    let addr = stream
        .peer_addr()
        .expect("connected streams should have a peer address");
    info!("Peer address: {}", addr);
    let conn = tokio_tungstenite::accept_async(stream).await;
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
async fn process_client(stream: WebSocketStream<TcpStream>) {
    let (_write, mut read) = stream.split();
    // TODO: error on binary messages
    // TODO: error on text messages > MAX_EVENT_SIZE
    while let Some(mes_res) = read.next().await {
        println!("got {:?}", mes_res);
    }
}
