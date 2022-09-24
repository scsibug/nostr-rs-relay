//! Subscription close request parsing
//!
//! Representation and parsing of `CLOSE` messages sent from clients.
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Close command in network format
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct CloseCmd {
    /// Protocol command, expected to always be "CLOSE".
    cmd: String,
    /// The subscription identifier being closed.
    id: String,
}

/// Identifier of the subscription to be closed.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct Close {
    /// The subscription identifier being closed.
    pub id: String,
}

impl From<CloseCmd> for Result<Close> {
    fn from(cc: CloseCmd) -> Result<Close> {
        // ensure command is correct
        if cc.cmd == "CLOSE" {
            Ok(Close { id: cc.id })
        } else {
            Err(Error::CommandUnknownError)
        }
    }
}
