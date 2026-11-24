//! Relay metadata using NIP-11
/// Relay Info
use crate::config::Settings;
use serde::{Deserialize, Serialize};

pub const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
pub const UNIT: &str = "msats";

/// Limitations of the relay as specified in NIP-11
#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Limitation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_message_length: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_subscriptions: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_limit: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_subid_length: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_event_tags: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_content_length: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_pow_difficulty: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_required: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_required: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub restricted_writes: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at_lower_limit: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at_upper_limit: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_limit: Option<usize>,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(unused)]
pub struct Fees {
    #[serde(skip_serializing_if = "Option::is_none")]
    admission: Option<Vec<Fee>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    publication: Option<Vec<Fee>>,
}

#[derive(Serialize, Deserialize, Debug)]
#[allow(unused)]
pub struct Fee {
    amount: u64,
    unit: String,
}

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
    pub contact: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supported_nips: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limitation: Option<Limitation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payments_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fees: Option<Fees>,
}

/// Convert an Info configuration into public Relay Info
impl From<Settings> for RelayInfo {
    fn from(c: Settings) -> Self {
        let mut supported_nips = vec![1, 2, 9, 11, 12, 15, 16, 20, 22, 33, 40];

        if c.authorization.nip42_auth {
            supported_nips.push(42);
        }
        if c.negentropy.enabled {
            supported_nips.push(77);
        }
        supported_nips.sort();

        let i = c.info;
        let p = c.pay_to_relay;

        let created_at_upper_limit = c
            .limits
            .created_at_upper_limit
            .or(c.options.reject_future_seconds);

        let limitations = Limitation {
            max_message_length: c.limits.max_ws_message_bytes,
            max_subscriptions: c.limits.max_subscriptions,
            max_limit: c.limits.max_limit,
            max_subid_length: c.limits.max_subid_length,
            max_event_tags: c.limits.max_event_tags,
            max_content_length: c.limits.max_content_length,
            min_pow_difficulty: c.limits.min_pow_difficulty,
            auth_required: c.limits.auth_required,
            payment_required: c.limits.payment_required.or(Some(p.enabled)),
            restricted_writes: c.limits.restricted_writes.or(Some(
                p.enabled
                    || c.verified_users.is_enabled()
                    || c.authorization.pubkey_whitelist.is_some()
                    || c.grpc.restricts_write,
            )),
            created_at_lower_limit: c.limits.created_at_lower_limit,
            created_at_upper_limit,
            default_limit: c.limits.default_limit,
        };

        let (payment_url, fees) = if p.enabled {
            let admission_fee = if p.admission_cost > 0 {
                Some(vec![Fee {
                    amount: p.admission_cost * 1000,
                    unit: UNIT.to_string(),
                }])
            } else {
                None
            };

            let post_fee = if p.cost_per_event > 0 {
                Some(vec![Fee {
                    amount: p.cost_per_event * 1000,
                    unit: UNIT.to_string(),
                }])
            } else {
                None
            };

            let fees = Fees {
                admission: admission_fee,
                publication: post_fee,
            };

            let payment_url = if p.enabled && i.relay_url.is_some() {
                Some(format!(
                    "{}join",
                    i.relay_url.clone().unwrap().replace("ws", "http")
                ))
            } else {
                None
            };
            (payment_url, Some(fees))
        } else {
            (None, None)
        };

        RelayInfo {
            id: i.relay_url,
            name: i.name,
            description: i.description,
            pubkey: i.pubkey,
            contact: i.contact,
            supported_nips: Some(supported_nips),
            software: Some("https://git.sr.ht/~gheartsfield/nostr-rs-relay".to_owned()),
            version: CARGO_PKG_VERSION.map(std::borrow::ToOwned::to_owned),
            limitation: Some(limitations),
            // NIP-11 standardized on the plural form.
            payments_url: payment_url.clone(),
            // Kept for backwards compat. Consider removing eventually.
            payment_url,
            fees,
            icon: i.relay_icon,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nip11_limitation_serialization() {
        let lim = Limitation {
            max_message_length: Some(16384),
            max_subscriptions: Some(300),
            max_limit: Some(5000),
            max_subid_length: Some(100),
            max_event_tags: Some(100),
            max_content_length: Some(8196),
            min_pow_difficulty: Some(30),
            auth_required: Some(true),
            payment_required: Some(true),
            restricted_writes: Some(true),
            created_at_lower_limit: Some(31536000),
            created_at_upper_limit: Some(3),
            default_limit: Some(500),
        };

        let json = serde_json::to_string(&lim).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(value["max_message_length"], 16384);
        assert_eq!(value["max_subscriptions"], 300);
        assert_eq!(value["max_limit"], 5000);
        assert_eq!(value["max_subid_length"], 100);
        assert_eq!(value["max_event_tags"], 100);
        assert_eq!(value["max_content_length"], 8196);
        assert_eq!(value["min_pow_difficulty"], 30);
        assert_eq!(value["auth_required"], true);
        assert_eq!(value["payment_required"], true);
        assert_eq!(value["restricted_writes"], true);
        assert_eq!(value["created_at_lower_limit"], 31536000);
        assert_eq!(value["created_at_upper_limit"], 3);
        assert_eq!(value["default_limit"], 500);
    }

    #[test]
    fn nip11_limitation_from_settings() {
        let mut settings = Settings::default();
        settings.limits.max_subscriptions = Some(50);
        settings.limits.max_limit = Some(1000);

        let info = RelayInfo::from(settings);
        let lim = info.limitation.unwrap();

        assert_eq!(lim.max_subscriptions, Some(50));
        assert_eq!(lim.max_limit, Some(1000));
    }
}
