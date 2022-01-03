use crate::config;
/// Relay Info
use serde::{Deserialize, Serialize};
use serde_json::value::Value;

const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct RelayInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_nips: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Default for RelayInfo {
    fn default() -> Self {
        RelayInfo {
            name: None,
            descr: None,
            pubkey: None,
            email: None,
            supported_nips: Some(vec!["NIP-01".to_owned()]),
            software: Some("https://git.sr.ht/~gheartsfield/nostr-rs-relay".to_owned()),
            version: CARGO_PKG_VERSION.map(|x| x.to_owned()),
        }
    }
}

/// Convert an Info struct into Relay Info json string
pub fn relay_info_json(info: &config::Info) -> String {
    // get a default RelayInfo
    let mut r = RelayInfo::default();
    // update fields from Info, if present
    r.name = info.name.clone();
    r.descr = info.descr.clone();
    r.pubkey = info.pubkey.clone();
    r.email = info.email.clone();
    r.to_json()
}

impl RelayInfo {
    pub fn to_json(self) -> String {
        // create the info ARRAY
        let mut info_arr: Vec<Value> = vec![];
        info_arr.push(Value::String("NOSTR_SERVER_INFO".to_owned()));
        info_arr.push(serde_json::to_value(&self).unwrap());
        serde_json::to_string_pretty(&info_arr).unwrap()
    }
}
