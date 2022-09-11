use console_subscriber::ConsoleLayer;
use log::*;
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
use std::thread::JoinHandle;
use std::time::Duration;

struct Relay {
    handle: JoinHandle<()>,
    shutdown_tx: MpscSender<()>,
}

fn start_relay(port: u16) -> Relay {
    ConsoleLayer::builder().with_default_env().init();
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

#[test]
fn startup() {
    let relay = start_relay(8080);
    // just make sure we can startup and shut down.
    // if we send a shutdown message before the server is listening,
    // we will get a SendError.  Keep sending until someone is
    // listening.
    loop {
        let shutdown_res = relay.shutdown_tx.send(());
        match shutdown_res {
            Ok(()) => {
                break;
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    // wait for relay to shutdown
    let thread_join = relay.handle.join();
    assert!(thread_join.is_ok());
}
