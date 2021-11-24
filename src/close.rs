use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};

// Container for a request to close a subscription
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(transparent)]
pub struct CloseCmd {
    cmds: Vec<String>,
}

#[derive(PartialEq, Debug, Clone)]
pub struct Close {
    id: String,
}

impl<'de> Deserialize<'de> for Close {
    fn deserialize<D>(deserializer: D) -> Result<Close, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut v: serde_json::Value = Deserialize::deserialize(deserializer)?;
        // this shoud be an exactly 2-element array
        // verify the first element is a String, CLOSE
        // get the subscription from the second element.

        // check for array
        let va = v
            .as_array_mut()
            .ok_or(serde::de::Error::custom("not array"))?;

        // check length
        if va.len() != 2 {
            return Err(serde::de::Error::custom("not exactly 2 fields"));
        }
        let mut i = va.into_iter();
        // get command ("REQ") and ensure it is a string
        let req_cmd_str: serde_json::Value = i.next().unwrap().take();
        let req = req_cmd_str.as_str().ok_or(serde::de::Error::custom(
            "first element of request was not a string",
        ))?;
        if req != "CLOSE" {
            return Err(serde::de::Error::custom("missing CLOSE command"));
        }

        // ensure sub id is a string
        let sub_id_str: serde_json::Value = i.next().unwrap().take();
        let sub_id = sub_id_str
            .as_str()
            .ok_or(serde::de::Error::custom("missing subscription id"))?;

        Ok(Close {
            id: sub_id.to_owned(),
        })
    }
}

impl Close {
    pub fn parse(json: &str) -> Result<Close> {
        serde_json::from_str(json).map_err(|e| Error::JsonParseFailed(e))
    }
    pub fn get_id(&self) -> String {
        self.id.clone()
    }
}
