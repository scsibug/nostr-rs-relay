use lazy_static::lazy_static;
use log::*;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;

// initialize a singleton default configuration
lazy_static! {
    pub static ref SETTINGS: RwLock<Settings> = RwLock::new(Settings::default());
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Database {
    pub data_directory: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Network {
    pub port: u16,
    pub address: String,
}

//
#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Options {
    pub reject_future_seconds: Option<usize>, // if defined, reject any events with a timestamp more than X seconds in the future
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Retention {
    // TODO: implement
    pub max_events: Option<usize>,                // max events
    pub max_bytes: Option<usize>,                 // max size
    pub persist_days: Option<usize>,              // oldest message
    pub whitelist_addresses: Option<Vec<String>>, // whitelisted addresses (never delete)
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Limits {
    pub messages_per_sec: Option<u32>, // Artificially slow down event writing to limit disk consumption (averaged over 1 minute)
    pub max_event_bytes: Option<usize>, // Maximum size of an EVENT message
    pub max_ws_message_bytes: Option<usize>,
    pub max_ws_frame_bytes: Option<usize>,
    pub broadcast_buffer: usize, // events to buffer for subscribers (prevents slow readers from consuming memory)
    pub event_persist_buffer: usize, // events to buffer for database commits (block senders if database writes are too slow)
}

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Settings {
    pub database: Database,
    pub network: Network,
    pub limits: Limits,
    pub retention: Retention,
    pub options: Options,
}

impl Settings {
    pub fn new() -> Self {
        let d = Self::default();
        // attempt to construct settings with file
        //        Self::new_from_default(&d).unwrap_or(d)
        let from_file = Self::new_from_default(&d);
        match from_file {
            Ok(f) => f,
            Err(e) => {
                warn!("Error reading config file ({:?})", e);
                d
            }
        }
    }

    fn new_from_default(default: &Settings) -> Result<Self, config::ConfigError> {
        let config: config::Config = config::Config::new();
        let settings: Settings = config
            // use defaults
            .with_merged(config::Config::try_from(default).unwrap())?
            // override with file contents
            .with_merged(config::File::with_name("config"))?
            .try_into()?;
        Ok(settings)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            database: Database {
                data_directory: ".".to_owned(),
            },
            network: Network {
                port: 8080,
                address: "0.0.0.0".to_owned(),
            },
            limits: Limits {
                messages_per_sec: None,
                max_event_bytes: Some(2 << 17),      // 128K
                max_ws_message_bytes: Some(2 << 17), // 128K
                max_ws_frame_bytes: Some(2 << 17),   // 128K
                broadcast_buffer: 4096,
                event_persist_buffer: 16,
            },
            retention: Retention {
                max_events: None,          // max events
                max_bytes: None,           // max size
                persist_days: None,        // oldest message
                whitelist_addresses: None, // whitelisted addresses (never delete)
            },
            options: Options {
                reject_future_seconds: Some(30 * 60), // Reject events 30min in the future or greater
            },
        }
    }
}
