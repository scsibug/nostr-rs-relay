use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};
//use serde_json::json;
//use serde_json::Result;


// Container for a request filter
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ReqCmd {
    cmds: Vec<String>
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Subscription {
    id: String,
    Vec<ReqFilter>
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ReqFilter {
    id: Option<String>,
    author: Option<String>,
    kind: Option<u8>,
    #[serde(rename = "e#")]
    event: Option<String>,
    #[serde(rename = "p#")]
    pubkey: Option<String>,
    since: Option<u64>,
    authors: Option<Vec<String>>,
}
