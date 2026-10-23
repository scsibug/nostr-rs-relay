//! Relay metadata using NIP-11
/// Relay Info
use crate::config::Settings;
use crate::conn::{MAX_SUBSCRIPTIONS, MAX_SUBSCRIPTION_ID_LEN};
use nostr::key::{FromSkStr, Keys};
use serde::{Deserialize, Serialize};

pub const CARGO_PKG_VERSION: Option<&'static str> = option_env!("CARGO_PKG_VERSION");
pub const UNIT: &str = "msats";

/// Limitations of the relay, as specified in NIP-11 (`limitation`)
#[derive(Debug, Serialize, Deserialize)]
#[allow(unused)]
pub struct Limitation {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_message_length: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    max_subscriptions: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    max_subid_length: Option<usize>,

    #[serde(skip_serializing_if = "Option::is_none")]
    auth_required: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    payment_required: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    restricted_writes: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    created_at_upper_limit: Option<usize>,
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
    pub banner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pubkey: Option<String>,
    /// The relay's own pubkey (NIP-11 `self`)
    #[serde(rename = "self", skip_serializing_if = "Option::is_none")]
    pub self_pubkey: Option<String>,
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
    pub terms_of_service: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limitation: Option<Limitation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payments_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fees: Option<Fees>,
}

/// Build an HTTP(S) URL for a page served by the relay itself, from the
/// advertised websocket URL. Only the scheme is changed (`ws` to `http`,
/// `wss` to `https`); `path` is resolved relative to the relay URL.
fn relay_http_url(relay_url: &str, path: &str) -> Option<String> {
    let mut url = url::Url::parse(relay_url).ok()?;
    let scheme = match url.scheme() {
        "ws" => "http",
        "wss" => "https",
        "http" | "https" => url.scheme(),
        _ => return None,
    }
    .to_owned();
    url.set_scheme(&scheme).ok()?;
    url.join(path).ok().map(String::from)
}

/// Convert an Info configuration into public Relay Info
impl From<Settings> for RelayInfo {
    fn from(c: Settings) -> Self {
        let mut supported_nips = vec![1, 2, 9, 11, 12, 15, 16, 20, 22, 33, 40];

        if c.authorization.nip42_auth {
            supported_nips.push(42);
            supported_nips.sort();
        }

        let i = c.info;
        let p = c.pay_to_relay;

        let limitations = Limitation {
            max_message_length: c.limits.max_ws_message_bytes,
            max_subscriptions: Some(MAX_SUBSCRIPTIONS),
            max_subid_length: Some(MAX_SUBSCRIPTION_ID_LEN),
            // NIP-42 AUTH is never required before a connection can act;
            // it only gates specific actions (e.g. reading DMs).
            auth_required: Some(false),
            payment_required: Some(p.enabled),
            restricted_writes: Some(
                p.enabled
                    || c.verified_users.is_enabled()
                    || c.authorization.pubkey_whitelist.is_some()
                    || c.limits.event_kind_allowlist.is_some()
                    || c.grpc.restricts_write,
            ),
            created_at_upper_limit: c.options.reject_future_seconds,
        };

        let (payments_url, fees) = if p.enabled {
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

            let payments_url = i
                .relay_url
                .as_deref()
                .and_then(|url| relay_http_url(url, "join"));
            (payments_url, Some(fees))
        } else {
            (None, None)
        };

        // The pay-to-relay key signs the relay's own DMs, so it is the
        // relay's identity.
        let self_pubkey = if p.enabled {
            p.secret_key
                .as_deref()
                .and_then(|sk| Keys::from_sk_str(sk).ok())
                .map(|keys| keys.public_key().to_string())
        } else {
            None
        };

        // Pay-to-relay serves its terms at /terms.
        let terms_of_service = i.terms_of_service.or_else(|| {
            if p.enabled && !p.terms_message.is_empty() {
                i.relay_url
                    .as_deref()
                    .and_then(|url| relay_http_url(url, "terms"))
            } else {
                None
            }
        });

        RelayInfo {
            id: i.relay_url,
            name: i.name,
            description: i.description,
            banner: i.banner,
            pubkey: i.pubkey,
            self_pubkey,
            contact: i.contact,
            supported_nips: Some(supported_nips),
            software: Some("https://git.sr.ht/~gheartsfield/nostr-rs-relay".to_owned()),
            version: CARGO_PKG_VERSION.map(std::borrow::ToOwned::to_owned),
            terms_of_service,
            limitation: Some(limitations),
            payments_url,
            fees,
            icon: i.relay_icon,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paid_settings() -> Settings {
        let mut settings = Settings::default();
        settings.info.relay_url = Some("wss://relay.example.com/".to_owned());
        settings.pay_to_relay.enabled = true;
        settings.pay_to_relay.admission_cost = 4200;
        settings.pay_to_relay.cost_per_event = 1;
        settings
    }

    #[test]
    fn limitation_from_config() {
        let mut settings = Settings::default();
        settings.limits.max_ws_message_bytes = Some(65536);
        settings.options.reject_future_seconds = Some(1800);
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        let l = &json["limitation"];
        assert_eq!(l["max_message_length"], 65536);
        assert_eq!(l["max_subscriptions"], 32);
        assert_eq!(l["max_subid_length"], 256);
        assert_eq!(l["auth_required"], false);
        assert_eq!(l["payment_required"], false);
        assert_eq!(l["restricted_writes"], false);
        assert_eq!(l["created_at_upper_limit"], 1800);
    }

    #[test]
    fn limitation_omits_unset_limits() {
        let mut settings = Settings::default();
        settings.limits.max_ws_message_bytes = None;
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        assert!(json["limitation"].get("max_message_length").is_none());
        assert!(json["limitation"].get("created_at_upper_limit").is_none());
    }

    #[test]
    fn kind_allowlist_restricts_writes() {
        let mut settings = Settings::default();
        settings.limits.event_kind_allowlist = Some(vec![1, 7]);
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        assert_eq!(json["limitation"]["restricted_writes"], true);
    }

    #[test]
    fn banner_and_terms_from_info_config() {
        let mut settings = Settings::default();
        settings.info.banner = Some("https://example.test/banner.png".to_owned());
        settings.info.terms_of_service = Some("https://example.test/tos".to_owned());
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        assert_eq!(json["banner"], "https://example.test/banner.png");
        assert_eq!(json["terms_of_service"], "https://example.test/tos");
    }

    #[test]
    fn terms_of_service_defaults_to_pay_to_relay_terms() {
        let mut settings = paid_settings();
        settings.pay_to_relay.terms_message = "Be nice.".to_owned();
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        assert_eq!(json["terms_of_service"], "https://relay.example.com/terms");
    }

    #[test]
    fn self_pubkey_from_pay_to_relay_secret_key() {
        let mut settings = paid_settings();
        // secret key 1 (the generator point)
        settings.pay_to_relay.secret_key = Some(format!("{:0>64}", "1"));
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        assert_eq!(
            json["self"],
            "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798"
        );

        let mut settings = paid_settings();
        settings.pay_to_relay.secret_key = Some("<nostr nsec>".to_owned());
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        assert!(json.get("self").is_none());
    }

    #[test]
    fn payments_url_uses_nip11_field_name() {
        let info = RelayInfo::from(paid_settings());
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["payments_url"], "https://relay.example.com/join");
        assert!(json.get("payment_url").is_none());
    }

    #[test]
    fn payments_url_only_rewrites_scheme() {
        assert_eq!(
            relay_http_url("wss://ws.example.com/", "join").as_deref(),
            Some("https://ws.example.com/join")
        );
        assert_eq!(
            relay_http_url("ws://news.example.com", "join").as_deref(),
            Some("http://news.example.com/join")
        );
        assert_eq!(
            relay_http_url("wss://example.com/relay/", "join").as_deref(),
            Some("https://example.com/relay/join")
        );
        assert_eq!(relay_http_url("not a url", "join"), None);
    }

    #[test]
    fn fees_from_pay_to_relay_in_msats() {
        let info = RelayInfo::from(paid_settings());
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["fees"]["admission"][0]["amount"], 4_200_000);
        assert_eq!(json["fees"]["admission"][0]["unit"], "msats");
        assert_eq!(json["fees"]["publication"][0]["amount"], 1000);
        assert_eq!(json["limitation"]["payment_required"], true);
    }

    #[test]
    fn no_payment_fields_when_pay_to_relay_disabled() {
        let mut settings = paid_settings();
        settings.pay_to_relay.enabled = false;
        let json = serde_json::to_value(RelayInfo::from(settings)).unwrap();
        assert!(json.get("payments_url").is_none());
        assert!(json.get("fees").is_none());
        assert_eq!(json["limitation"]["payment_required"], false);
    }
}
