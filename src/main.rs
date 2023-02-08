//! Server process
use clap::Parser;
use console_subscriber::ConsoleLayer;
use nostr_rs_relay::cli::CLIArgs;
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
use tracing::info;

/// Start running a Nostr relay server.
fn main() {
    let args = CLIArgs::parse();

    // get config file name from args
    let config_file_arg = args.config;

    // configure settings from the config file (defaults to config.toml)
    // replace default settings with those read from the config file
    let mut settings = config::Settings::new(&config_file_arg);

    // setup tracing
    if settings.diagnostics.tracing {
        // enable tracing with tokio-console
        ConsoleLayer::builder().with_default_env().init();
    } else {
        // standard logging
        tracing_subscriber::fmt::try_init().unwrap();
    }
    info!("Starting up from main");

    // get database directory from args
    let db_dir_arg = args.db;

    // update with database location from args, if provided
    if let Some(db_dir) = db_dir_arg {
        settings.database.data_directory = db_dir;
    }
    // we should have a 'control plane' channel to monitor and bump
    // the server.  this will let us do stuff like clear the database,
    // shutdown, etc.; for now all this does is initiate shutdown if
    // `()` is sent.  This will change in the future, this is just a
    // stopgap to shutdown the relay when it is used as a library.
    let (_, ctrl_rx): (MpscSender<()>, MpscReceiver<()>) = syncmpsc::channel();
    // run this in a new thread
    let handle = thread::spawn(move || {
        let _svr = start_server(&settings, ctrl_rx);
    });
    // block on nostr thread to finish.
    handle.join().unwrap();
}
