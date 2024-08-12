//! Server process
use clap::Parser;
use console_subscriber::ConsoleLayer;
use nostr_rs_relay::cli::CLIArgs;
use nostr_rs_relay::config;
use nostr_rs_relay::server::start_server;
use std::fs;
use std::path::Path;
use std::process;
use std::sync::mpsc as syncmpsc;
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};
use std::thread;
#[cfg(all(not(target_env = "msvc"), not(target_os = "openbsd")))]
use tikv_jemallocator::Jemalloc;
use tracing::info;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;

#[cfg(all(not(target_env = "msvc"), not(target_os = "openbsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

/// Start running a Nostr relay server.
fn main() {
    let args = CLIArgs::parse();

    // get config file name from args
    let config_file_arg = args.config;

    // Ensure the config file is readable if it was explicitly set
    if let Some(config_path) = config_file_arg.as_ref() {
        let path = Path::new(&config_path);
        if !path.exists() {
            eprintln!("Config file not found: {}", &config_path);
            process::exit(1);
        }
        if !path.is_file() {
            eprintln!("Invalid config file path: {}", &config_path);
            process::exit(1);
        }
        if let Err(err) = fs::metadata(path) {
            eprintln!("Error while accessing file metadata: {}", err);
            process::exit(1);
        }
        if let Err(err) = fs::File::open(path) {
            eprintln!("Config file is not readable: {}", err);
            process::exit(1);
        }
    }

    let mut _log_guard: Option<WorkerGuard> = None;

    // configure settings from the config file (defaults to config.toml)
    // replace default settings with those read from the config file
    let mut settings = config::Settings::new(&config_file_arg).unwrap_or_else(|e| {
        eprintln!("Error reading config file ({:?})", e);
        process::exit(1);
    });

    // setup tracing
    if settings.diagnostics.tracing {
        // enable tracing with tokio-console
        ConsoleLayer::builder().with_default_env().init();
    } else {
        // standard logging
        if let Some(path) = &settings.logging.folder_path {
            // write logs to a folder
            let prefix = match &settings.logging.file_prefix {
                Some(p) => p.as_str(),
                None => "relay",
            };
            let file_appender = tracing_appender::rolling::daily(path, prefix);
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            let filter = EnvFilter::from_default_env();
            // assign to a variable that is not dropped till the program ends
            _log_guard = Some(guard);

            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_writer(non_blocking)
                .try_init()
                .unwrap();
        } else {
            // write to stdout
            tracing_subscriber::fmt::try_init().unwrap();
        }
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
