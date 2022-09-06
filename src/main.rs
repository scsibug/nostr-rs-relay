//! Server process
use log::*;
use nostr_rs_relay::config;
use nostr_rs_relay::error::{Error, Result};
use nostr_rs_relay::server::start_server;
use std::env;
use std::thread;

/// Return a requested DB name from command line arguments.
fn db_from_args(args: Vec<String>) -> Option<String> {
    if args.len() == 3 && args.get(1) == Some(&"--db".to_owned()) {
        return args.get(2).map(|x| x.to_owned());
    }
    None
}

/// Start running a Nostr relay server.
fn main() -> Result<(), Error> {
    // setup logger
    let _ = env_logger::try_init();
    info!("Starting up from main");
    // get database directory from args
    let args: Vec<String> = env::args().collect();
    let db_dir: Option<String> = db_from_args(args);
    {
        let mut settings = config::SETTINGS.write().unwrap();
        // replace default settings with those read from config.toml
        let mut c = config::Settings::new();
        // update with database location
        if let Some(db) = db_dir {
            c.database.data_directory = db;
        }
        *settings = c;
    }
    // run this in a new thread
    let handle = thread::spawn(|| {
        let _ = start_server();
    });
    // block on nostr thread to finish.
    handle.join().unwrap();
    Ok(())
}
