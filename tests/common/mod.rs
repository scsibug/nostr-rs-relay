use anyhow::{anyhow, Result};
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
//use http::{Request, Response};
use hyper::{Client, StatusCode, Uri};
use std::net::TcpListener;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;
use tracing::{debug, info};

pub struct Relay {
    pub port: u16,
    pub handle: JoinHandle<()>,
    pub shutdown_tx: MpscSender<()>,
}

pub fn start_relay() -> Result<Relay> {
    // setup tracing
    let _trace_sub = tracing_subscriber::fmt::try_init();
    info!("Starting a new relay");
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
    let handle = thread::spawn(move || {
        // server will block the thread it is run on.
        let _ = start_server(&settings, shutdown_rx);
    });
    // how do we know the relay has finished starting up?
    Ok(Relay {
        port,
        handle,
        shutdown_tx,
    })
}

// check if the server is healthy via HTTP request
async fn server_ready(relay: &Relay) -> Result<bool> {
    let uri: String = format!("http://127.0.0.1:{}/", relay.port);
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
        let server_check = server_ready(relay).await;
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
                debug!("Relay not ready, will try again...");
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
// This needed some modification; if multiple tasks all ask for open ports, they will tend to get the same one.
// instead we should try to try these incrementally/globally.

static PORT_COUNTER: AtomicU16 = AtomicU16::new(4030);

fn get_available_port() -> Option<u16> {
    let startsearch = PORT_COUNTER.fetch_add(10, Ordering::SeqCst);
    if startsearch >= 20000 {
        // wrap around
        PORT_COUNTER.store(4030, Ordering::Relaxed);
    }
    (startsearch..20000).find(|port| port_is_available(*port))
}
pub fn port_is_available(port: u16) -> bool {
    info!("checking on port {}", port);
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}
