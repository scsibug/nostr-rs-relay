//! Subscription and filter parsing
use super::event::{Event, EventId, EventKind};
use crate::error::Result;
use serde::de::Unexpected;
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;

use serde_json::Value;

use secp256k1::XOnlyPublicKey;

use super::tags::{Tag, TagType};

/// A Request Filter as per NIP01 https://github.com/fiatjaf/nostr/blob/master/nips/01.md#communication-between-clients-and-relays
///
/// This represents a subscription request. All the fields are
/// optional. Filtering is done for only included fields.
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ReqFilter {
    /// Event Ids
    pub ids: Option<Vec<EventId>>,
    /// Event kinds
    pub kinds: Option<Vec<EventKind>>,
    /// Referenced event ids
    #[serde(rename = "#e")]
    pub events: Option<Vec<EventId>>,
    /// Referenced public keys
    #[serde(rename = "#p")]
    pub pubkeys: Option<Vec<XOnlyPublicKey>>,
    /// Events published after this time
    pub since: Option<u64>,
    /// Events published before this time
    pub until: Option<u64>,
    /// List of author public keys
    pub authors: Option<Vec<XOnlyPublicKey>>,
}

/// Subscription defining a set of [`ReqFilter`]
#[derive(PartialEq, Debug, Clone)]
pub struct Subscription {
    id: String,
    filters: Vec<ReqFilter>,
}

impl Serialize for Subscription {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element("REQ")?;
        seq.serialize_element(&self.id)?;
        for filter in self.get_filters() {
            seq.serialize_element(filter)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Subscription {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let received: Vec<Value> = Deserialize::deserialize(deserializer)?;
        // We should never get a subscription message of smaller length.
        if received.len() < 3 {
            return Err(serde::de::Error::custom(
                "Not enough data in subscription message",
            ));
        }
        // Check if the message flag is set correctly.
        match &received[0] {
            Value::String(tag) => {
                if tag != "REQ" {
                    return Err(serde::de::Error::custom(
                        "Invalid tag in subscription message",
                    ));
                }
            }
            _ => {
                return Err(serde::de::Error::invalid_type(
                    Unexpected::Other("json type in subscription message flag"),
                    &"json string",
                ))
            }
        }

        // Try parsing the rest of the data, or emit error
        if let Value::String(id) = &received[1] {
            let filters: Vec<ReqFilter> =
                serde_json::from_value(Value::Array(received[2..].to_vec()))
                    .map_err(|e| serde::de::Error::custom(e.to_string()))?;
            Ok(Self {
                id: id.clone(),
                filters,
            })
        } else {
            Err(serde::de::Error::invalid_type(
                Unexpected::Other("json type in subscription id"),
                &"json string",
            ))
        }
    }
}

impl Subscription {
    /// Get the subscription id
    pub fn get_id(&self) -> &str {
        &self.id
    }

    /// Get Subscription filters
    pub fn get_filters(&self) -> Vec<&ReqFilter> {
        self.filters.iter().collect()
    }

    /// Determine if this subscription matches a given [`Event`].  Any
    /// individual filter match is sufficient.
    pub fn interested_in_event(&self, event: &Event) -> bool {
        self.filters
            .iter()
            .any(|filter| filter.interested_in_event(event))
    }
}

impl ReqFilter {
    /// Check for EventId match, skip if None
    fn ids_match(&self, event: &Event) -> bool {
        if let Some(ids) = &self.ids {
            ids.contains(&event.id)
        } else {
            true
        }
    }

    /// Check for author match, skip if None
    fn author_match(&self, event: &Event) -> bool {
        if let Some(authors) = &self.authors {
            authors.contains(&event.pubkey)
        } else {
            true
        }
    }

    /// Find tag intersection between an Event and this filter.
    /// Returns empty set if no tag intersection found.
    pub fn tag_intersect(&self, event: &Event) -> HashSet<Tag> {
        let mut event_tags: HashSet<Tag> = self
            .events
            .as_ref()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|id| Tag::from_event_id(*id, None))
            .collect();
        let pubkey_tags: HashSet<Tag> = self
            .pubkeys
            .as_ref()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|pubkey| Tag::from_pubkey(*pubkey, None))
            .collect();
        event_tags.extend(pubkey_tags);
        event.tag_intersect(&event_tags)
    }

    /// Check for event_tags intersection, skip if None
    fn event_tag_match(&self, event: &Event) -> bool {
        if let Some(_) = &self.events {
            // self.tag_intersect(event)
            //     .iter()
            //     .any(|tag| tag.get_type() == TagType::Event)

            let intersect = self.tag_intersect(event);
            intersect.iter().any(|tag| tag.get_type() == TagType::Event)
        } else {
            true
        }
    }

    /// Check for pubkey_tags intersection, skip if None
    fn pubkey_tag_match(&self, event: &Event) -> bool {
        if let Some(_) = &self.pubkeys {
            self.tag_intersect(event)
                .iter()
                .any(|tag| tag.get_type() == TagType::Pubkey)
        } else {
            true
        }
    }

    /// Check for kind match, skip if None
    fn kind_match(&self, event: &Event) -> bool {
        if let Some(kinds) = &self.kinds {
            kinds.contains(&event.kind)
        } else {
            true
        }
    }

    /// Check for since match, skip if None
    fn since_match(&self, event: &Event) -> bool {
        if let Some(since) = self.since {
            event.created_at > since
        } else {
            true
        }
    }

    // Check for until match, skip if None
    fn until_match(&self, event: &Event) -> bool {
        if let Some(until) = self.until {
            event.created_at < until
        } else {
            true
        }
    }

    /// Check for all matches.
    pub fn interested_in_event(&self, event: &Event) -> bool {
        self.ids_match(event)
            && self.since_match(event)
            && self.until_match(event)
            && self.kind_match(event)
            && self.author_match(event)
            && self.pubkey_tag_match(event)
            && self.event_tag_match(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::testvec::{event::*, subscription::*};
    use std::str::FromStr;

    #[test]
    fn serde_roundtrip() {
        let subs1: Subscription = serde_json::from_str(SUBS).unwrap();
        let ser_subs1 = serde_json::to_string(&subs1).unwrap();

        let subs2: Subscription = serde_json::from_str(&ser_subs1).unwrap();
        let ser_subs2 = serde_json::to_string(&subs2).unwrap();

        assert_eq!(subs1, subs2);
        assert_eq!(ser_subs1, ser_subs2);
    }

    #[test]
    fn subs_filtering() {
        let event = Event::from_str(VALID_EVENT).unwrap();
        // Check match by id
        let id_subs: Subscription = serde_json::from_str(ID_SUBS).unwrap();
        assert!(id_subs.interested_in_event(&event));

        // Check match by author
        let author_subs: Subscription = serde_json::from_str(AUTHORS_SUBS).unwrap();
        assert!(author_subs.interested_in_event(&event));

        // Check match by kind
        let kinds_subs: Subscription = serde_json::from_str(KINDS_SUBS).unwrap();
        assert!(kinds_subs.interested_in_event(&event));

        // Check match by since
        let since_subs: Subscription = serde_json::from_str(SINCE_SUBS).unwrap();
        assert!(since_subs.interested_in_event(&event));

        // Check match by until
        let until_subs: Subscription = serde_json::from_str(UNTIL_SUBS).unwrap();
        assert!(until_subs.interested_in_event(&event));

        // Check match by event tags
        let event_tags_subs: Subscription = serde_json::from_str(EVENT_TAGS_SUBS).unwrap();
        assert!(event_tags_subs.interested_in_event(&event));

        // Check match  by pubkey tags
        let pubkey_tags_subs: Subscription = serde_json::from_str(PUBKEY_TAGS_SUBS).unwrap();
        assert!(pubkey_tags_subs.interested_in_event(&event));
    }
}
