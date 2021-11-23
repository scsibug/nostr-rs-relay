use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};
//use serde_json::json;
//use serde_json::Result;


// Container for a request filter
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ReqCmd {

    #[serde(deserialize_with = "u32_from_string")]
    id: u32,
    #[serde(deserialize_with = "u32_from_string")]
    pubkey: u32,
    created_at: u64,
    kind: u8,
    #[serde(deserialize_with = "tag_from_string")]
    tags: Vec<Vec<String>>,
    content: String,
    #[serde(deserialize_with = "u64_from_string")]
    sig: u64,
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

pub struct Request {}
