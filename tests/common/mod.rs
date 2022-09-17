use anyhow::{anyhow, Result};
use log::*;
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
//use http::{Request, Response};
use hyper::{Client, StatusCode, Uri};
use std::net::TcpListener;
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

pub struct Relay {
    pub port: u16,
    pub handle: JoinHandle<()>,
    pub shutdown_tx: MpscSender<()>,
}

pub fn start_relay() -> Result<Relay> {
    let _ = env_logger::try_init();
    info!("Starting up from main");
    // replace default settings
    let mut settings = config::Settings::default();
    // identify open port
    info!("Checking for address...");
    let port = get_available_port().unwrap();
    info!("Found open port: {}", port);
    // bind to local interface only
    settings.network.address = "127.0.0.1".to_owned();
    settings.network.port = port;
    // create an in-memory DB with multiple readers
    settings.database.in_memory = true;
    settings.database.min_conn = 4;
    settings.database.max_conn = 8;
    let (shutdown_tx, shutdown_rx): (MpscSender<()>, MpscReceiver<()>) = syncmpsc::channel();
    let handle = thread::spawn(|| {
        // server will block the thread it is run on.
        let _ = start_server(settings, shutdown_rx);
    });
    // how do we know the relay has finished starting up?
    return Ok(Relay {
        port,
        handle,
        shutdown_tx,
    });
}

// check if the server is healthy via HTTP request
async fn server_ready(relay: &Relay) -> Result<bool> {
    let uri: String = format!("http://127.0.0.1:{}/", relay.port.to_string());
    let client = Client::new();
    let uri: Uri = uri.parse().unwrap();
    let res = client.get(uri).await?;
    Ok(res.status() == StatusCode::OK)
}

pub async fn wait_for_healthy_relay(relay: &Relay) -> Result<()> {
    // TODO: maximum time to wait for server to become healthy.
    // give it a little time to start up before we start polling
    tokio::time::sleep(Duration::from_millis(10)).await;
    loop {
        let server_check = server_ready(&relay).await;
        match server_check {
            Ok(true) => {
                // server responded with 200-OK.
                break;
            }
            Ok(false) => {
                // server responded with an error, we're done.
                return Err(anyhow!("Got non-200-OK from relay"));
            }
            Err(_) => {
                // server is not yet ready, probably connection refused...
                debug!("Got ERR from Relay!");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
    }
    info!("relay is ready");
    Ok(())
    // simple message sent to web browsers
    //let mut request = Request::builder()
    //    .uri("https://www.rust-lang.org/")
    //    .header("User-Agent", "my-awesome-agent/1.0");
}

// from https://elliotekj.com/posts/2017/07/25/find-available-tcp-port-rust/
fn get_available_port() -> Option<u16> {
    (4030..20000).find(|port| port_is_available(*port))
}
pub fn port_is_available(port: u16) -> bool {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => true,
        Err(_) => false,
    }
}
