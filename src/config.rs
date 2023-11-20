//! Configuration file and settings management
use crate::payment::Processor;
use config::{Config, ConfigError, File};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize, Clone)]
#[allow(unused)]
pub struct Info {
    pub relay_url: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub pubkey: Option<String>,
    pub contact: Option<String>,
    pub favicon: Option<String>,
    pub relay_icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Database {
    pub data_directory: String,
    pub engine: String,
    pub in_memory: bool,
    pub min_conn: u32,
    pub max_conn: u32,
    pub connection: String,
    pub connection_write: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Grpc {
    pub event_admission_server: Option<String>,
    pub restricts_write: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Network {
    pub port: u16,
    pub address: String,
    pub remote_ip_header: Option<String>, // retrieve client IP from this HTTP header if present
    pub ping_interval_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Options {
    pub reject_future_seconds: Option<usize>, // if defined, reject any events with a timestamp more than X seconds in the future
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Retention {
    // TODO: implement
    pub max_events: Option<usize>,                // max events
    pub max_bytes: Option<usize>,                 // max size
    pub persist_days: Option<usize>,              // oldest message
    pub whitelist_addresses: Option<Vec<String>>, // whitelisted addresses (never delete)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Limits {
    pub messages_per_sec: Option<u32>, // Artificially slow down event writing to limit disk consumption (averaged over 1 minute)
    pub subscriptions_per_min: Option<u32>, // Artificially slow down request (db query) creation to prevent abuse (averaged over 1 minute)
    pub db_conns_per_client: Option<u32>, // How many concurrent database queries (not subscriptions) may a client have?
    pub max_blocking_threads: usize,
    pub max_event_bytes: Option<usize>, // Maximum size of an EVENT message
    pub max_ws_message_bytes: Option<usize>,
    pub max_ws_frame_bytes: Option<usize>,
    pub broadcast_buffer: usize, // events to buffer for subscribers (prevents slow readers from consuming memory)
    pub event_persist_buffer: usize, // events to buffer for database commits (block senders if database writes are too slow)
    pub event_kind_blacklist: Option<Vec<u64>>,
    pub event_kind_allowlist: Option<Vec<u64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Authorization {
    pub pubkey_whitelist: Option<Vec<String>>, // If present, only allow these pubkeys to publish events
    pub nip42_auth: bool,                      // if true enables NIP-42 authentication
    pub nip42_dms: bool, // if true send DMs only to their authenticated recipients
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct PayToRelay {
    pub enabled: bool,
    pub admission_cost: u64, // Cost to have pubkey whitelisted
    pub cost_per_event: u64, // Cost author to pay per event
    pub node_url: String,
    pub api_secret: String,
    pub terms_message: String,
    pub sign_ups: bool,       // allow new users to sign up to relay
    pub direct_message: bool, // Send direct message to user with invoice and terms
    pub secret_key: Option<String>,
    pub processor: Processor,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Diagnostics {
    pub tracing: bool, // enables tokio console-subscriber
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum VerifiedUsersMode {
    Enabled,
    Passive,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct VerifiedUsers {
    pub mode: VerifiedUsersMode, // Mode of operation: "enabled" (enforce) or "passive" (check only). If none, this is simply disabled.
    pub domain_whitelist: Option<Vec<String>>, // If present, only allow verified users from these domains can publish events
    pub domain_blacklist: Option<Vec<String>>, // If present, allow all verified users from any domain except these
    pub verify_expiration: Option<String>, // how long a verification is cached for before no longer being used
    pub verify_update_frequency: Option<String>, // how often to attempt to update verification
    pub verify_expiration_duration: Option<Duration>, // internal result of parsing verify_expiration
    pub verify_update_frequency_duration: Option<Duration>, // internal result of parsing verify_update_frequency
    pub max_consecutive_failures: usize, // maximum number of verification failures in a row, before ceasing future checks
}

impl VerifiedUsers {
    pub fn init(&mut self) {
        self.verify_expiration_duration = self.verify_expiration_duration();
        self.verify_update_frequency_duration = self.verify_update_duration();
    }

    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.mode == VerifiedUsersMode::Enabled
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.mode == VerifiedUsersMode::Enabled || self.mode == VerifiedUsersMode::Passive
    }

    #[must_use]
    pub fn is_passive(&self) -> bool {
        self.mode == VerifiedUsersMode::Passive
    }

    #[must_use]
    pub fn verify_expiration_duration(&self) -> Option<Duration> {
        self.verify_expiration
            .as_ref()
            .and_then(|x| parse_duration::parse(x).ok())
    }

    #[must_use]
    pub fn verify_update_duration(&self) -> Option<Duration> {
        self.verify_update_frequency
            .as_ref()
            .and_then(|x| parse_duration::parse(x).ok())
    }

    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.verify_expiration_duration().is_some() && self.verify_update_duration().is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Logging {
    pub folder_path: Option<String>,
    pub file_prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(unused)]
pub struct Settings {
    pub info: Info,
    pub diagnostics: Diagnostics,
    pub database: Database,
    pub grpc: Grpc,
    pub network: Network,
    pub limits: Limits,
    pub authorization: Authorization,
    pub pay_to_relay: PayToRelay,
    pub verified_users: VerifiedUsers,
    pub retention: Retention,
    pub options: Options,
    pub logging: Logging,
}

impl Settings {
    pub fn new(config_file_name: &Option<String>) -> Result<Self, ConfigError> {
        let default_settings = Self::default();
        // attempt to construct settings with file
        let from_file = Self::new_from_default(&default_settings, config_file_name);
        match from_file {
            Err(e) => {
                // pass up the parse error if the config file was specified,
                // otherwise use the default config (with a warning).
                if config_file_name.is_some() {
                    Err(e)
                } else {
                    eprintln!("Error reading config file ({:?})", e);
                    eprintln!("WARNING: Default configuration settings will be used");
                    Ok(default_settings)
                }
            }
            ok => ok,
        }
    }

    fn new_from_default(
        default: &Settings,
        config_file_name: &Option<String>,
    ) -> Result<Self, ConfigError> {
        let default_config_file_name = "config.toml".to_string();
        let config: &String = match config_file_name {
            Some(value) => value,
            None => &default_config_file_name,
        };
        let builder = Config::builder();
        let config: Config = builder
            // use defaults
            .add_source(Config::try_from(default)?)
            // override with file contents
            .add_source(File::with_name(config))
            .build()?;
        let mut settings: Settings = config.try_deserialize()?;
        // ensure connection pool size is logical
        assert!(
            settings.database.min_conn <= settings.database.max_conn,
            "Database min_conn setting ({}) cannot exceed max_conn ({})",
            settings.database.min_conn,
            settings.database.max_conn
        );
        // ensure durations parse
        assert!(
            settings.verified_users.is_valid(),
            "VerifiedUsers time settings could not be parsed"
        );
        // initialize durations for verified users
        settings.verified_users.init();

        // Validate pay to relay settings
        if settings.pay_to_relay.enabled {
            assert_ne!(settings.pay_to_relay.api_secret, "");
            // Should check that url is valid
            assert_ne!(settings.pay_to_relay.node_url, "");
            assert_ne!(settings.pay_to_relay.terms_message, "");

            if settings.pay_to_relay.direct_message {
                assert_ne!(
                    settings.pay_to_relay.secret_key,
                    Some("<nostr nsec>".to_string())
                );
                assert!(settings.pay_to_relay.secret_key.is_some());
            }
        }

        Ok(settings)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            info: Info {
                relay_url: None,
                name: Some("Unnamed nostr-rs-relay".to_owned()),
                description: None,
                pubkey: None,
                contact: None,
                favicon: None,
                relay_icon: None,
            },
            diagnostics: Diagnostics { tracing: false },
            database: Database {
                data_directory: ".".to_owned(),
                engine: "sqlite".to_owned(),
                in_memory: false,
                min_conn: 4,
                max_conn: 8,
                connection: "".to_owned(),
                connection_write: None,
            },
            grpc: Grpc {
                event_admission_server: None,
                restricts_write: false,
            },
            network: Network {
                port: 8080,
                ping_interval_seconds: 300,
                address: "0.0.0.0".to_owned(),
                remote_ip_header: None,
            },
            limits: Limits {
                messages_per_sec: None,
                subscriptions_per_min: None,
                db_conns_per_client: None,
                max_blocking_threads: 16,
                max_event_bytes: Some(2 << 17),      // 128K
                max_ws_message_bytes: Some(2 << 17), // 128K
                max_ws_frame_bytes: Some(2 << 17),   // 128K
                broadcast_buffer: 16384,
                event_persist_buffer: 4096,
                event_kind_blacklist: None,
                event_kind_allowlist: None,
            },
            authorization: Authorization {
                pubkey_whitelist: None, // Allow any address to publish
                nip42_auth: false,      // Disable NIP-42 authentication
                nip42_dms: false,       // Send DMs to everybody
            },
            pay_to_relay: PayToRelay {
                enabled: false,
                admission_cost: 4200,
                cost_per_event: 0,
                terms_message: "".to_string(),
                node_url: "".to_string(),
                api_secret: "".to_string(),
                sign_ups: false,
                direct_message: false,
                secret_key: None,
                processor: Processor::LNBits,
            },
            verified_users: VerifiedUsers {
                mode: VerifiedUsersMode::Disabled,
                domain_whitelist: None,
                domain_blacklist: None,
                verify_expiration: Some("1 week".to_owned()),
                verify_update_frequency: Some("1 day".to_owned()),
                verify_expiration_duration: None,
                verify_update_frequency_duration: None,
                max_consecutive_failures: 20,
            },
            retention: Retention {
                max_events: None,          // max events
                max_bytes: None,           // max size
                persist_days: None,        // oldest message
                whitelist_addresses: None, // whitelisted addresses (never delete)
            },
            options: Options {
                reject_future_seconds: None, // Reject events in the future if defined
            },
            logging: Logging {
                folder_path: None,
                file_prefix: None,
            },
        }
    }
}
