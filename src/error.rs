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
    #[error("Event validation failed")]
    EventInvalid,
    #[error("Subscription identifier max length exceeded")]
    SubIdMaxLengthError,
    #[error("Maximum concurrent subscription count reached")]
    SubMaxExceededError,
    // this should be used if the JSON is invalid
    #[error("JSON parsing failed")]
    JsonParseFailed(serde_json::Error),
    #[error("WebSocket proto error")]
    WebsocketError(WsError),
    #[error("Command unknown")]
    CommandUnknownError,
    #[error("SQL error")]
    SqlError(rusqlite::Error),
    #[error("Config error")]
    ConfigError(config::ConfigError),
    #[error("Data directory does not exist")]
    DatabaseDirError,
}

impl From<rusqlite::Error> for Error {
    /// Wrap SQL error
    fn from(r: rusqlite::Error) -> Self {
        Error::SqlError(r)
    }
}

impl From<serde_json::Error> for Error {
    /// Wrap JSON error
    fn from(r: serde_json::Error) -> Self {
        Error::JsonParseFailed(r)
    }
}

impl From<WsError> for Error {
    /// Wrap Websocket error
    fn from(r: WsError) -> Self {
        Error::WebsocketError(r)
    }
}

impl From<config::ConfigError> for Error {
    /// Wrap Config error
    fn from(r: config::ConfigError) -> Self {
        Error::ConfigError(r)
    }
}
