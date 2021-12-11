use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct CloseCmd {
    cmd: String,
    id: String,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Close {
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

impl Close {
    pub fn get_id(&self) -> String {
        self.id.clone()
    }
}
