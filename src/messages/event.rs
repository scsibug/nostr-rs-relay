//! Event parsing and validation
use crate::config;
use crate::error::Error;
use crate::error::Result;
use bitcoin_hashes::{hex::ToHex, sha256, Hash};
use lazy_static::lazy_static;
use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::value::Value;
use std::collections::HashSet;
use std::fmt::Display;
use std::time::SystemTime;

use super::tags::{Tag, TagType};
use serde::de::Unexpected;

lazy_static! {
    pub static ref SECP: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
}

/// A sha256 has denoting unique id for an Event
pub type EventId = sha256::Hash;

/// An event kind as per NIP01 https://github.com/fiatjaf/nostr/blob/master/nips/01.md#basic-event-kinds
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum EventKind {
    SetMetadata,
    TextNote,
    RecommendedServer,
}

impl Serialize for EventKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::SetMetadata => serializer.serialize_u64(0),
            Self::TextNote => serializer.serialize_u64(1),
            Self::RecommendedServer => serializer.serialize_u64(2),
        }
    }
}

impl Display for EventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", serde_json::to_string(&self).unwrap())
    }
}

impl<'de> Deserialize<'de> for EventKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let received: Value = Deserialize::deserialize(deserializer)?;

        let value = received.as_u64().ok_or(serde::de::Error::invalid_type(
            Unexpected::Other("invalid json value"),
            &"json number",
        ))?;
        match value {
            0 => Ok(Self::SetMetadata),
            1 => Ok(Self::TextNote),
            2 => Ok(Self::RecommendedServer),
            _ => Err(serde::de::Error::invalid_value(
                Unexpected::Unsigned(value),
                &"0, 1 or 2",
            )),
        }
    }
}

/// Seconds since 1970
pub(crate) fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|x| x.as_secs())
        .unwrap_or(0)
}

/// Event parsed
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Event {
    pub(crate) id: EventId,
    pub(crate) pubkey: XOnlyPublicKey,
    pub(crate) created_at: u64,
    pub(crate) kind: EventKind,
    pub(crate) tags: Vec<Tag>,
    pub(crate) content: String,
    pub(crate) sig: schnorr::Signature,
}

// Match Events only by id, ignore other stuffs
impl PartialEq for Event {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl Eq for Event {}

/// Custom [`FromStr`] impl that uses serde deserialization with validation
/// This should be used for all event deserialization instead of [`serde_json::from_str()`]
/// This ensures we always deal with valid events
impl std::str::FromStr for Event {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let event: Self = serde_json::from_str(s)?;
        let check_validation = event.is_valid();
        if check_validation.is_ok() {
            Ok(event)
        } else {
            Err(check_validation.err().expect("error expected"))
        }
    }
}

impl Event {
    /// Create a short event identifier, suitable for logging.
    pub fn get_short_event_id(&self) -> String {
        self.id.to_string()[..8].to_string()
    }

    /// Perform Validation of the event data.
    pub fn is_valid(&self) -> Result<(), Error> {
        // Do not attempt validation for events of distant future
        let config = config::SETTINGS
            .read()
            .map_err(|e| Error::GenericError(e.to_string()))?;
        let max_future_sec = config.options.reject_future_seconds;
        if let Some(allowable_future) = max_future_sec {
            let curr_time = unix_time();
            // calculate difference, plus how far future we allow
            if curr_time + (allowable_future as u64) < self.created_at {
                let delta = self.created_at - curr_time;
                return Err(Error::EventInvalid(format!(
                    "Event is too far in future : {}",
                    delta
                )));
            }
        }

        // Check if correct EventId is provided
        if self.id != self.compute_event_id()? {
            return Err(Error::EventInvalid("Event has wrong event id".to_string()));
        }

        // We have already verified that EventId is correct
        // use that as `Message` and perform signature verification
        let msg = secp256k1::Message::from_slice(self.id.as_inner())
            .map_err(|e| Error::GenericError(e.to_string()))?;
        Ok(SECP
            .verify_schnorr(&self.sig, &msg, &self.pubkey)
            .map_err(|e| Error::EventInvalid(e.to_string()))?)
    }

    /// Compute the `EventId` for the given event
    /// EventId is sha256::hash(`Event Message`)
    ///
    /// The `EventMessage` is described in NIP01 as a json array
    /// [
    ///    0,
    ///    <pubkey, as a (lowercase) hex string>,
    ///    <created_at, as a number>,
    ///    <kind, as a number>,
    ///    <tags, as an array of arrays>,
    ///    <content>, as a string
    ///]
    pub fn compute_event_id(&self) -> Result<EventId, Error> {
        // create a JsonValue for each event element
        let mut c: Vec<Value> = vec![];
        // id must be set to 0
        c.push(serde_json::to_value(0)?);
        // public key
        c.push(serde_json::to_value(&self.pubkey.to_hex())?);
        // creation time
        c.push(serde_json::to_value(&self.created_at)?);
        // kind
        c.push(serde_json::to_value(&self.kind)?);
        // tags
        let tags = self
            .tags
            .iter()
            .map(|tag| serde_json::to_value(tag))
            .collect::<Result<serde_json::Value, _>>()?;
        c.push(tags);
        // content
        c.push(serde_json::to_value(&self.content)?);

        let canonical_event_string = serde_json::to_string(&Value::Array(c))?;

        Ok(EventId::hash(canonical_event_string.as_bytes()))
    }

    /// Fetch tags of specific type
    /// None if type not found in tag list
    fn get_tags_of_type(&self, tagtype: TagType) -> Option<Vec<Tag>> {
        let tags: Vec<&Tag> = self
            .tags
            .iter()
            .filter(|tag| tag.get_type() == tagtype)
            .collect();
        if !tags.is_empty() {
            Some(tags.iter().cloned().cloned().collect())
        } else {
            None
        }
    }

    /// Get a list of event tags.
    /// None if there's no event tag
    pub fn get_event_tags(&self) -> Option<Vec<Tag>> {
        if let Some(tags) = self.get_tags_of_type(TagType::Event) {
            Some(tags)
        } else {
            None
        }
    }

    /// Get a list of pubkey tags.
    /// None if there's no pubkey tag
    pub fn get_pubkey_tags(&self) -> Option<Vec<Tag>> {
        if let Some(tags) = self.get_tags_of_type(TagType::Pubkey) {
            Some(tags)
        } else {
            None
        }
    }

    /// Check if a given [`EventId`] is referenced in event tags.
    pub fn refs_event(&self, event_id: &EventId) -> bool {
        if let Some(tags) = self.get_event_tags() {
            tags.iter()
                .any(|tag| tag.match_event_id(event_id).unwrap_or(false))
        } else {
            false
        }
    }

    /// Check if a given pubkey is referenced in an pubkey tags.
    pub fn refs_pubkey(&self, pubkey: &XOnlyPublicKey) -> bool {
        if let Some(tags) = self.get_pubkey_tags() {
            tags.iter()
                .any(|tag| tag.match_pubkey(pubkey).unwrap_or(false))
        } else {
            false
        }
    }

    /// Given a HashSet<Tag>, find the intersect between
    /// the given set and tags of this event
    /// Returns empty set if no intersection found
    pub fn tag_intersect(&self, tagset: &HashSet<Tag>) -> HashSet<Tag> {
        let own_set: HashSet<Tag> = self.tags.iter().cloned().collect();
        own_set.intersection(tagset).cloned().collect()
    }

    /// Given a TagType and HashSet<Tag>, find the intersect between
    /// the given set and tags of this event for this specific type
    /// Returns empty set if no intersection found
    pub fn tag_type_intersect(&self, tagtype: TagType, tagset: &HashSet<Tag>) -> HashSet<Tag> {
        let tag_intersect = self.tag_intersect(tagset);
        tag_intersect
            .iter()
            .filter(|tag| tag.get_type() == tagtype)
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::super::testvec::event::*;
    use super::*;
    use std::str::FromStr;

    #[test]
    fn serde_roundtrip() {
        let event_1: Event = Event::from_str(VALID_EVENT).unwrap();
        let str_event_1 = serde_json::to_string(&event_1).unwrap();
        let event_2: Event = Event::from_str(&str_event_1).unwrap();
        let str_event_2 = serde_json::to_string(&event_2).unwrap();
        assert_eq!(event_2, event_1);
        assert_eq!(str_event_1, str_event_2);
    }

    #[test]
    fn invalid_id() {
        let res: Result<Event, _> = Event::from_str(ID_INVALID);
        assert!(res
            .err()
            .expect("expect error")
            .to_string()
            .contains("bad hex string length 60 (expected 64)"));
    }

    #[test]
    fn pubkey_malformed() {
        let res: Result<Event, _> = Event::from_str(PUBKEY_MALFORMED);
        assert!(res
            .err()
            .expect("expect error")
            .to_string()
            .contains("secp: malformed public key"));
    }

    #[test]
    fn sig_malformed() {
        let res: Result<Event, _> = Event::from_str(SIG_MALFORMED);
        assert!(res
            .err()
            .expect("expect error")
            .to_string()
            .contains("secp: malformed signature"));
    }

    #[test]
    fn missing_field() {
        let res: Result<Event, _> = Event::from_str(MISSING_FIELD);
        assert!(res
            .err()
            .expect("expect error")
            .to_string()
            .contains("missing field `created_at`"));
    }

    #[test]
    fn check_prefix() {
        let event = Event::from_str(VALID_EVENT).unwrap();
        assert_eq!("5436cab3", &event.get_short_event_id());
    }

    #[test]
    fn get_tags() {
        let event = Event::from_str(VALID_EVENT).unwrap();
        let event_tags = event.get_event_tags().unwrap();
        let pubkey_tags = event.get_pubkey_tags().unwrap();
        let tags: Vec<Tag> = event_tags
            .iter()
            .chain(pubkey_tags.iter())
            .cloned()
            .collect();
        let expected_tags = r#"
        [
            [
                "e",
                "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
            ],
            [
                "e",
                "bdefa1da005259928d0ad0baed9c460945b4b82618c4551de6e95ee09ece25d2"
            ],
            [
                "e",
                "4d7ec2e01be6ba5111a0a0ce821639885ea4cb8f4a652accd911dfb7d5151e17"
            ],
            [
                "p",
                "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
            ],
            [
                "p",
                "859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d"
            ],
            [
                "p",
                "cfc5794db955d560b8aec1bd0f27d41d52e1d9d0b157057f7501ea3d30dca034"
            ]
        ]
        "#;
        let expected_tags: Vec<Tag> = serde_json::from_str(expected_tags).unwrap();
        assert_eq!(expected_tags, tags);
    }

    #[test]
    fn tag_match() {
        let event = Event::from_str(VALID_EVENT).unwrap();
        let refd_event_id =
            EventId::from_str("5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa")
                .unwrap();
        let non_refd_event_id =
            EventId::from_str("859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d")
                .unwrap();
        let refd_pubkey = XOnlyPublicKey::from_str(
            "859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d",
        )
        .unwrap();
        let non_refd_pubkey = XOnlyPublicKey::from_str(
            "18845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166",
        )
        .unwrap();

        assert_eq!(event.refs_event(&refd_event_id), true);
        assert_eq!(event.refs_event(&non_refd_event_id), false);
        assert_eq!(event.refs_pubkey(&refd_pubkey), true);
        assert_eq!(event.refs_pubkey(&non_refd_pubkey), false);
    }

    #[test]
    fn test_intersection() {
        let event = Event::from_str(VALID_EVENT).unwrap();
        let mut external_tags = Vec::new();
        external_tags.push(Tag::from_event_id(
            EventId::from_str("bdefa1da005259928d0ad0baed9c460945b4b82618c4551de6e95ee09ece25d2")
                .unwrap(),
            None,
        ));
        external_tags.push(Tag::from_pubkey(
            XOnlyPublicKey::from_str(
                "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9",
            )
            .unwrap(),
            None,
        ));
        external_tags.push(Tag::from_pubkey(
            XOnlyPublicKey::from_str(
                "18845781f631c48f1c9709e23092067d06837f30aa0cd0544ac887fe91ddd166",
            )
            .unwrap(),
            None,
        ));

        let pubkey_intersect =
            event.tag_type_intersect(TagType::Pubkey, &external_tags.iter().cloned().collect());
        let event_intersect =
            event.tag_type_intersect(TagType::Event, &external_tags.iter().cloned().collect());

        assert_eq!(pubkey_intersect.len(), 1);
        assert_eq!(event_intersect.len(), 1);

        assert_eq!(
            serde_json::to_string(&pubkey_intersect).unwrap(),
            r#"[["p","96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"]]"#
        );
        assert_eq!(
            serde_json::to_string(&event_intersect).unwrap(),
            r#"[["e","bdefa1da005259928d0ad0baed9c460945b4b82618c4551de6e95ee09ece25d2"]]"#
        );
    }

    #[test]
    fn event_kind() {
        let kinds = vec![
            EventKind::SetMetadata,
            EventKind::TextNote,
            EventKind::RecommendedServer,
        ];
        assert_eq!(serde_json::to_string(&kinds).unwrap(), "[0,1,2]");
    }
}
