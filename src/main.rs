//! Server process

use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
use std::env;
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
use tracing::info;

use console_subscriber::ConsoleLayer;

/// Return a requested DB name from command line arguments.
fn db_from_args(args: &[String]) -> Option<String> {
    if args.len() == 3 && args.get(1) == Some(&"--db".to_owned()) {
        return args.get(2).map(std::clone::Clone::clone);
    }
    None
}

/// Start running a Nostr relay server.
fn main() {
    // setup tracing
    let _trace_sub = tracing_subscriber::fmt::try_init();
    info!("Starting up from main");
    // get database directory from args
    let args: Vec<String> = env::args().collect();
    let db_dir: Option<String> = db_from_args(&args);
    // configure settings from config.toml
    // replace default settings with those read from config.toml
    let mut settings = config::Settings::new();

    if settings.diagnostics.tracing {
        // enable tracing with tokio-console
        ConsoleLayer::builder().with_default_env().init();
    }
    // update with database location
    if let Some(db) = db_dir {
        settings.database.data_directory = db;
    }

    let (_, ctrl_rx): (MpscSender<()>, MpscReceiver<()>) = syncmpsc::channel();
    // run this in a new thread
    let handle = thread::spawn(|| {
        // we should have a 'control plane' channel to monitor and bump the server.
        // this will let us do stuff like clear the database, shutdown, etc.
        let _svr = start_server(settings, ctrl_rx);
    });
    // block on nostr thread to finish.
    handle.join().unwrap();
}
