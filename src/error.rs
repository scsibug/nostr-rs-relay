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
    #[error("CLOSE message parse failed")]
    CloseParseFailed,
    #[error("Event invalid signature")]
    EventInvalidSignature,
    #[error("Event invalid id")]
    EventInvalidId,
    #[error("Event malformed pubkey")]
    EventMalformedPubkey,
    #[error("Event could not canonicalize")]
    EventCouldNotCanonicalize,
    #[error("Event too large")]
    EventMaxLengthError(usize),
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
    #[error("Config error : {0}")]
    ConfigError(config::ConfigError),
    #[error("Data directory does not exist")]
    DatabaseDirError,
    #[error("Database Connection Pool Error")]
    DatabasePoolError(r2d2::Error),
    #[error("SQL error")]
    SqlxError(sqlx::Error),
    #[error("Database Connection Pool Error")]
    SqlxDatabasePoolError(sqlx::Error),
    #[error("Custom Error : {0}")]
    CustomError(String),
    #[error("Task join error")]
    JoinError,
    #[error("Hyper Client error")]
    HyperError(hyper::Error),
    #[error("Hex encoding error")]
    HexError(hex::FromHexError),
    #[error("Delegation parse error")]
    DelegationParseError,
    #[error("Channel closed error")]
    ChannelClosed,
    #[error("Authz error")]
    AuthzError,
    #[cfg(feature = "grpc")]
    #[error("Tonic GRPC error")]
    TonicError(tonic::Status),
    #[error("Invalid AUTH message")]
    AuthFailure,
    #[error("I/O Error")]
    IoError(std::io::Error),
    #[error("Event builder error")]
    EventError(nostr::event::builder::Error),
    #[error("Nostr key error")]
    NostrKeyError(nostr::key::Error),
    #[error("Payment hash mismatch")]
    PaymentHash,
    #[error("Error parsing url")]
    URLParseError(url::ParseError),
    #[error("HTTP error")]
    HTTPError(http::Error),
    #[error("Unknown/Undocumented")]
    UnknownError,
}

//impl From<Box<dyn std::error::Error>> for Error {
//    fn from(e: Box<dyn std::error::Error>) -> Self {
//        Error::CustomError("error".to_owned())
//    }
//}

impl From<hex::FromHexError> for Error {
    fn from(h: hex::FromHexError) -> Self {
        Error::HexError(h)
    }
}

impl From<hyper::Error> for Error {
    fn from(h: hyper::Error) -> Self {
        Error::HyperError(h)
    }
}

impl From<r2d2::Error> for Error {
    fn from(d: r2d2::Error) -> Self {
        Error::DatabasePoolError(d)
    }
}

impl From<tokio::task::JoinError> for Error {
    /// Wrap SQL error
    fn from(_j: tokio::task::JoinError) -> Self {
        Error::JoinError
    }
}

impl From<rusqlite::Error> for Error {
    /// Wrap SQL error
    fn from(r: rusqlite::Error) -> Self {
        Error::SqlError(r)
    }
}

impl From<sqlx::Error> for Error {
    fn from(d: sqlx::Error) -> Self {
        Error::SqlxDatabasePoolError(d)
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

#[cfg(feature = "grpc")]
impl From<tonic::Status> for Error {
    /// Wrap Config error
    fn from(r: tonic::Status) -> Self {
        Error::TonicError(r)
    }
}

impl From<std::io::Error> for Error {
    fn from(r: std::io::Error) -> Self {
        Error::IoError(r)
    }
}
impl From<nostr::event::builder::Error> for Error {
    /// Wrap event builder error
    fn from(r: nostr::event::builder::Error) -> Self {
        Error::EventError(r)
    }
}

impl From<nostr::key::Error> for Error {
    /// Wrap nostr key error
    fn from(r: nostr::key::Error) -> Self {
        Error::NostrKeyError(r)
    }
}

impl From<url::ParseError> for Error {
    /// Wrap nostr key error
    fn from(r: url::ParseError) -> Self {
        Error::URLParseError(r)
    }
}

impl From<http::Error> for Error {
    /// Wrap nostr key error
    fn from(r: http::Error) -> Self {
        Error::HTTPError(r)
    }
}
