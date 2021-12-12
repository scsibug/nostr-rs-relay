//! Subscription close request parsing
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

/// Close command in network format
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct CloseCmd {
    /// Protocol command, expected to always be "CLOSE".
    cmd: String,
    /// The subscription identifier being closed.
    id: String,
}

/// Close command parsed
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Close {
    /// The subscription identifier being closed.
    pub id: String,
}

impl From<CloseCmd> for Result<Close> {
    fn from(cc: CloseCmd) -> Result<Close> {
        // ensure command is correct
        if cc.cmd != "CLOSE" {
            return Err(Error::CommandUnknownError);
        } else {
            return Ok(Close { id: cc.id });
        }
    }
}
