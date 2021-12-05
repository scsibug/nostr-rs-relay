use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Close {
    cmd: String,
    id: String,
}

impl Close {
    pub fn parse(json: &str) -> Result<Close> {
        let c: Close = serde_json::from_str(json)?; //.map_err(|e| Error::JsonParseFailed(e));
        if c.cmd != "CLOSE" {
            return Err(Error::CloseParseFailed);
        }
        return Ok(c);
    }

    pub fn get_id(&self) -> String {
        self.id.clone()
    }
}
