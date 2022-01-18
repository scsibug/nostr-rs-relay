//! Tags used in events to link to another event or a pubkey
//!
//! Reference specification NIP01: https://github.com/fiatjaf/nostr/blob/master/nips/01.md#events-and-signatures
//!

use bitcoin_hashes::hex::ToHex;
use bitcoin_hashes::sha256;
use secp256k1::XOnlyPublicKey;
use std::str::FromStr;

use serde::de::Unexpected;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serializer};
use serde_json::Value;

use crate::error::Error;

type EventId = sha256::Hash;

#[derive(Debug, PartialEq)]
struct EventTag {
    event_id: EventId,
    recommended_url: Option<String>,
}

#[derive(Debug, PartialEq)]
struct PubkeyTag {
    pubkey: XOnlyPublicKey,
    recommended_url: Option<String>,
}

// Tag structure representing two possible types of tags
#[derive(Debug, PartialEq)]
enum Tag {
    Event(EventTag),
    Pubkey(PubkeyTag),
}

// Custom json serialization into protocol network format
// Event tag : ["e", "<32 byte event-id>", "optional<url>"]
// Pubkey tag : ["p", "<32 byte Xonly Pubkey>", "optional<url>"]
impl serde::Serialize for Tag {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Event(event_tag) => {
                let mut seq = serializer.serialize_seq(None)?;
                seq.serialize_element("e")?;
                seq.serialize_element(&event_tag.event_id.to_hex())?;
                if let Some(url) = &event_tag.recommended_url {
                    seq.serialize_element(url)?;
                } else {
                }
                seq.end()
            }
            Self::Pubkey(pubkey_tag) => {
                let mut seq = serializer.serialize_seq(None)?;
                seq.serialize_element("p")?;
                seq.serialize_element(&pubkey_tag.pubkey.to_hex())?;
                if let Some(url) = &pubkey_tag.recommended_url {
                    seq.serialize_element(url)?;
                } else {
                }
                seq.end()
            }
        }
    }
}

// Custom json deserialization from protocol network format
impl<'de> serde::Deserialize<'de> for Tag {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Receive incoming data in a json object
        let received: Value = Deserialize::deserialize(deserializer)?;

        // Check received data is a json array
        let values = received.as_array().ok_or_else(|| {
            serde::de::Error::invalid_type(Unexpected::Other("tag json object"), &"json array")
        })?;

        // Check json array contains only string
        let values = values
            .iter()
            .map(|value| {
                value.as_str().ok_or_else(|| {
                    serde::de::Error::invalid_type(
                        Unexpected::Other("tag json data"),
                        &"json string",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Check length is not more tha 3
        if values.len() > 3 {
            Err(serde::de::Error::invalid_length(
                values.len(),
                &"tag length is 2 or 3",
            ))
        } else {
            // Parse the json array into appropriate types
            match values[0] {
                // This denotes an event type tag
                "e" => {
                    let event_id = EventId::from_str(values[1])
                        .map_err(|e| serde::de::Error::custom(e.to_string()))?;
                    let recomended_url = if values.len() == 3 {
                        Some(values[2].into())
                    } else {
                        None
                    };
                    Ok(Tag::Event(EventTag {
                        event_id,
                        recommended_url: recomended_url,
                    }))
                }
                // This denotes a pubkey type tag
                "p" => {
                    let pubkey = XOnlyPublicKey::from_str(values[1])
                        .map_err(|e| serde::de::Error::custom(e.to_string()))?;
                    let recomended_url = if values.len() == 3 {
                        Some(values[2].into())
                    } else {
                        None
                    };

                    Ok(Tag::Pubkey(PubkeyTag {
                        pubkey,
                        recommended_url: recomended_url,
                    }))
                }
                // Any other tag type is currently not supported
                _ => Err(serde::de::Error::invalid_value(
                    Unexpected::Other("tag type flag"),
                    &"'e' or 'p'",
                )),
            }
        }
    }
}

#[allow(dead_code)]
// Some api (not public currently) to use Tags
impl Tag {
    fn new_event_tag(event_id: EventId, recomended_url: Option<String>) -> Self {
        Self::Event(EventTag {
            event_id,
            recommended_url: recomended_url,
        })
    }

    fn new_pubkey_tag(pubkey: XOnlyPublicKey, recomended_url: Option<String>) -> Self {
        Self::Pubkey(PubkeyTag {
            pubkey,
            recommended_url: recomended_url,
        })
    }

    fn get_recomended_url(&self) -> Option<String> {
        match self {
            Self::Event(EventTag {
                event_id: _,
                recommended_url,
            }) => recommended_url.clone(),
            Self::Pubkey(PubkeyTag {
                pubkey: _,
                recommended_url,
            }) => recommended_url.clone(),
        }
    }

    fn get_event_id(&self) -> Result<EventId, Error> {
        match self {
            Self::Event(EventTag {
                event_id,
                recommended_url: _,
            }) => Ok(*event_id),
            _ => Err(Error::CustomError("Expected event tag".to_string())),
        }
    }

    fn get_pubkey(&self) -> Result<XOnlyPublicKey, Error> {
        match self {
            Self::Pubkey(PubkeyTag {
                pubkey,
                recommended_url: _,
            }) => Ok(*pubkey),
            _ => Err(Error::CustomError("Expected pubkey tag".to_string())),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use bitcoin_hashes::Hash;

    #[test]
    fn serde_roundtrip() {
        let pubkey = XOnlyPublicKey::from_str(
            "18845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166",
        )
        .unwrap();
        let url = "wss://rsslay.fiatjaf.com";
        static HASH_BYTES: [u8; 32] = [
            0xef, 0x53, 0x7f, 0x25, 0xc8, 0x95, 0xbf, 0xa7, 0x82, 0x52, 0x65, 0x29, 0xa9, 0xb6,
            0x3d, 0x97, 0xaa, 0x63, 0x15, 0x64, 0xd5, 0xd7, 0x89, 0xc2, 0xb7, 0x65, 0x44, 0x8c,
            0x86, 0x35, 0xfb, 0x6c,
        ];
        let event_id = EventId::from_slice(&HASH_BYTES).expect("right number of bytes");

        let event_tag = Tag::new_event_tag(event_id, Some(url.to_string()));
        let pubkey_tag = Tag::new_pubkey_tag(pubkey, Some(url.to_string()));

        let ser_event_tag = serde_json::to_string(&event_tag).unwrap();
        let ser_pubkey_tag = serde_json::to_string(&pubkey_tag).unwrap();

        let deser_event_tag: Tag = serde_json::from_str(&ser_event_tag).unwrap();
        let deser_pubkey_tag: Tag = serde_json::from_str(&ser_pubkey_tag).unwrap();

        assert_eq!(deser_event_tag, event_tag);
        assert_eq!(deser_pubkey_tag, pubkey_tag);
    }

    #[test]
    fn invalid_pubkey() {
        let test_string = r#"["p","845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166","wss://rsslay.fiatjaf.com"]"#;
        let tag: Result<Tag, _> = serde_json::from_str(test_string);
        assert_eq!(
            tag.err().expect("expect error").to_string(),
            "secp: malformed public key".to_string()
        );
    }

    #[test]
    fn invalid_datatype() {
        let test_string = r#"["p", 188457, "wss://rsslay.fiatjaf.com"]"#;
        let tag: Result<Tag, _> = serde_json::from_str(test_string);
        assert_eq!(
            tag.err().expect("expect error").to_string(),
            "invalid type: tag json data, expected json string".to_string()
        );
    }

    #[test]
    fn invalid_tagbyte() {
        let test_string = r#"["q", "18845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166", "wss://rsslay.fiatjaf.com"]"#;
        let tag: Result<Tag, _> = serde_json::from_str(test_string);
        assert_eq!(
            tag.err().expect("expect error").to_string(),
            "invalid value: tag type flag, expected 'e' or 'p'".to_string()
        );
    }

    #[test]
    fn invalid_event_id() {
        let test_string = r#"["e","ef537f25c895bfa782526529a9b63d97aa631564d5d78c8635fb6c","wss://rsslay.fiatjaf.com"]"#;
        let tag: Result<Tag, _> = serde_json::from_str(test_string);
        assert_eq!(
            tag.err().expect("expect error").to_string(),
            "bad hex string length 54 (expected 64)".to_string()
        );
    }

    #[test]
    fn invalid_url_type() {
        let test_string = r#"["e","ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c", 123456788]"#;
        let tag: Result<Tag, _> = serde_json::from_str(test_string);
        assert_eq!(
            tag.err().expect("expect error").to_string(),
            "invalid type: tag json data, expected json string".to_string()
        );
    }

    #[test]
    fn invalid_length() {
        let test_string = r#"["e","ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c","wss://rsslay.fiatjaf.com", "Random extra data"]"#;
        let tag: Result<Tag, _> = serde_json::from_str(test_string);
        assert_eq!(
            tag.err().expect("expect error").to_string(),
            "invalid length 4, expected tag length is 2 or 3".to_string()
        );
    }

    #[test]
    fn invalid_json_type() {
        let test_string = r#"{"type": "e","event": "ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c"}"#;
        let tag: Result<Tag, _> = serde_json::from_str(test_string);
        assert_eq!(
            tag.err().expect("expect error").to_string(),
            "invalid type: tag json object, expected json array".to_string()
        );
    }

    #[test]
    fn evenrt_tag_missing_url() {
        let test_string =
            r#"["e","ef537f25c895bfa782526529a9b63d97aa631564d5d789c2b765448c8635fb6c"]"#;
        let tag: Tag = serde_json::from_str(test_string).unwrap();
        assert!(tag.get_recomended_url().is_none());
    }

    #[test]
    fn pubkey_tag_missing_url() {
        let test_string =
            r#"["p","18845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166"]"#;
        let tag: Tag = serde_json::from_str(test_string).unwrap();
        assert!(tag.get_recomended_url().is_none());
    }
}
