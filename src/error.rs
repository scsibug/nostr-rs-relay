//! Error handling
use std::result;
use thiserror::Error;
use tungstenite::error::Error as WsError;

/// Simple `Result` type for errors in this module
pub type Result<T, E = Error> = result::Result<T, E>;

/// Custom error type for Nostr
#[derive(Error, Debug)]
pub enum Error {
    #[error("Protocol parse error")]
    ProtoParseError,
    #[error("Connection error")]
    ConnError,
    #[error("Client write error")]
    ConnWriteError,
    #[error("EVENT parse failed")]
    EventParseFailed,
    #[error("ClOSE message parse failed")]
    CloseParseFailed,
    #[error("Event validation failed, Reason : {0}")]
    EventInvalid(String),
    #[error("Event too large, Size : {0}")]
    EventMaxLengthError(usize),
    #[error("Subscription identifier max length exceeded")]
    SubIdMaxLengthError,
    #[error("Maximum concurrent subscription count reached")]
    SubMaxExceededError,
    #[error("JSON parsing failed, Reason : {0}")]
    JsonParseFailed(#[from] serde_json::Error),
    #[error("WebSocket error : Reason : {0}")]
    WebsocketError(#[from] WsError),
    #[error("Command unknown")]
    CommandUnknownError,
    #[error("SQL error, Reason : {0}")]
    SqlError(#[from] rusqlite::Error),
    #[error("Config error, Reason : {0}")]
    ConfigError(#[from] config::ConfigError),
    #[error("Data directory does not exist")]
    DatabaseDirError,
    #[error("Generic Error, Reason: {0}")]
    GenericError(String),
}
