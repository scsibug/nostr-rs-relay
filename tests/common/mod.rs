use anyhow::Result;
use log::*;
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
use std::net::TcpListener;
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
use std::thread::JoinHandle;

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
    let port = get_available_port().unwrap();
    info!("Starting relay on port {}", port);
    // bind to local interface only
    settings.network.address = "127.0.0.1".to_owned();
    settings.network.port = port;
    // create an in-memory DB with multiple readers
    settings.database.in_memory = true;
    settings.database.min_conn = 4;
    settings.database.max_conn = 8;
    let (shutdown_tx, shutdown_rx): (MpscSender<()>, MpscReceiver<()>) = syncmpsc::channel();
    let handle = thread::spawn(|| {
        let _ = start_server(settings, shutdown_rx);
    });
    return Ok(Relay {
        port,
        handle,
        shutdown_tx,
    });
}

// from https://elliotekj.com/posts/2017/07/25/find-available-tcp-port-rust/
fn get_available_port() -> Option<u16> {
    (4000..20000).find(|port| port_is_available(*port))
}
fn port_is_available(port: u16) -> bool {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => true,
        Err(_) => false,
    }
}
