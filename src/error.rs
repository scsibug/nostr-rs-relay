//! Error handling.

use std::result;
use thiserror::Error;
use tungstenite::error::Error as WsError;

pub type Result<T, E = Error> = result::Result<T, E>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("Protocol parse error")]
    ProtoParseError,
    #[error("Connection error")]
    ConnError,
    #[error("Client write error")]
    ConnWriteError,
    #[error("Event parse failed")]
    EventParseFailed,
    #[error("Event validation failed")]
    EventInvalid,
    #[error("JSON parsing failed")]
    JsonParseFailed(serde_json::Error),
    #[error("WebSocket proto error")]
    WebsocketError(WsError),
    #[error("Command unknown")]
    CommandUnknownError,
}

impl From<serde_json::Error> for Error {
    fn from(r: serde_json::Error) -> Self {
        Error::JsonParseFailed(r)
    }
}

impl From<WsError> for Error {
    fn from(r: WsError) -> Self {
        Error::WebsocketError(r)
    }
}
