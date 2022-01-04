use crate::config;
/// Relay Info
use serde::{Deserialize, Serialize};

const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");

#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct RelayInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_nips: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Default for RelayInfo {
    fn default() -> Self {
        RelayInfo {
            id: None,
            name: None,
            description: None,
            pubkey: None,
            email: None,
            supported_nips: Some(vec![1]),
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
    r.id = info.relay_url.clone();
    r.name = info.name.clone();
    r.description = info.description.clone();
    r.pubkey = info.pubkey.clone();
    r.email = info.email.clone();
    r.to_json()
}

impl RelayInfo {
    pub fn to_json(self) -> String {
        serde_json::to_string_pretty(&self).unwrap()
    }
}
