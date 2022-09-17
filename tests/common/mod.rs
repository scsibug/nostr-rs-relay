use log::*;
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
use std::thread::JoinHandle;

pub struct Relay {
    pub handle: JoinHandle<()>,
    pub shutdown_tx: MpscSender<()>,
}

pub fn start_relay(port: u16) -> Relay {
    let _ = env_logger::try_init();
    info!("Starting up from main");
    // replace default settings
    let mut settings = config::Settings::default();
    settings.database.in_memory = true;
    settings.network.port = port;
    let (shutdown_tx, shutdown_rx): (MpscSender<()>, MpscReceiver<()>) = syncmpsc::channel();
    let handle = thread::spawn(|| {
        let _ = start_server(settings, shutdown_rx);
    });
    return Relay {
        handle,
        shutdown_tx,
    };
}
