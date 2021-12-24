//! Subscription and filter parsing
use crate::error::Result;
use crate::event::Event;
use serde::{Deserialize, Deserializer, Serialize};

/// Subscription identifier and set of request filters
#[derive(Serialize, PartialEq, Debug, Clone)]
pub struct Subscription {
    pub id: String,
    pub filters: Vec<ReqFilter>,
}

/// Filter for requests
///
/// Corresponds to client-provided subscription request elements.  Any
/// element can be present if it should be used in filtering, or
/// absent ([`None`]) if it should be ignored.
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ReqFilter {
    /// Event hash
    pub id: Option<String>,
    /// Event kind
    pub kind: Option<u64>,
    /// Referenced event hash
    #[serde(rename = "#e")]
    pub event: Option<String>,
    /// Referenced public key for a petname
    #[serde(rename = "#p")]
    pub pubkey: Option<String>,
    /// Events published after this time
    pub since: Option<u64>,
    /// Events published before this time
    pub until: Option<u64>,
    /// List of author public keys
    pub authors: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for Subscription {
    /// Custom deserializer for subscriptions, which have a more
    /// complex structure than the other message types.
    fn deserialize<D>(deserializer: D) -> Result<Subscription, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut v: serde_json::Value = Deserialize::deserialize(deserializer)?;
        // this shoud be a 3-or-more element array.
        // verify the first element is a String, REQ
        // get the subscription from the second element.
        // convert each of the remaining objects into filters

        // check for array
        let va = v
            .as_array_mut()
            .ok_or_else(|| serde::de::Error::custom("not array"))?;

        // check length
        if va.len() < 3 {
            return Err(serde::de::Error::custom("not enough fields"));
        }
        let mut i = va.iter_mut();
        // get command ("REQ") and ensure it is a string
        let req_cmd_str: serde_json::Value = i.next().unwrap().take();
        let req = req_cmd_str
            .as_str()
            .ok_or_else(|| serde::de::Error::custom("first element of request was not a string"))?;
        if req != "REQ" {
            return Err(serde::de::Error::custom("missing REQ command"));
        }

        // ensure sub id is a string
        let sub_id_str: serde_json::Value = i.next().unwrap().take();
        let sub_id = sub_id_str
            .as_str()
            .ok_or_else(|| serde::de::Error::custom("missing subscription id"))?;

        let mut filters = vec![];
        for fv in i {
            let f: ReqFilter = serde_json::from_value(fv.take())
                .map_err(|_| serde::de::Error::custom("could not parse filter"))?;
            filters.push(f);
        }
        Ok(Subscription {
            id: sub_id.to_owned(),
            filters,
        })
    }
}

impl Subscription {
    /// Get a copy of the subscription identifier.
    pub fn get_id(&self) -> String {
        self.id.clone()
    }
    /// Determine if this subscription matches a given [`Event`].  Any
    /// individual filter match is sufficient.
    pub fn interested_in_event(&self, event: &Event) -> bool {
        for f in self.filters.iter() {
            if f.interested_in_event(event) {
                return true;
            }
        }
        false
    }
}

impl ReqFilter {
    /// Check for a match within the authors list.
    // TODO: Ambiguity; what if the array is empty?  Should we
    // consider that the same as null?
    fn authors_match(&self, event: &Event) -> bool {
        self.authors
            .as_ref()
            .map(|vs| vs.contains(&event.pubkey.to_owned()))
            .unwrap_or(true)
    }
    /// Check if this filter either matches, or does not care about the event tags.
    fn event_match(&self, event: &Event) -> bool {
        self.event
            .as_ref()
            .map(|t| event.event_tag_match(t))
            .unwrap_or(true)
    }

    /// Check if this filter either matches, or does not care about
    /// the pubkey/petname tags.
    fn pubkey_match(&self, event: &Event) -> bool {
        self.pubkey
            .as_ref()
            .map(|t| event.pubkey_tag_match(t))
            .unwrap_or(true)
    }

    /// Check if this filter either matches, or does not care about the kind.
    fn kind_match(&self, kind: u64) -> bool {
        self.kind.map(|v| v == kind).unwrap_or(true)
    }

    /// Determine if all populated fields in this filter match the provided event.
    pub fn interested_in_event(&self, event: &Event) -> bool {
        self.id.as_ref().map(|v| v == &event.id).unwrap_or(true)
            && self.since.map(|t| event.created_at > t).unwrap_or(true)
            && self.kind_match(event.kind)
            && self.authors_match(event)
            && self.pubkey_match(event)
            && self.event_match(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_request_parse() -> Result<()> {
        let raw_json = "[\"REQ\",\"some-id\",{}]";
        let s: Subscription = serde_json::from_str(raw_json)?;
        assert_eq!(s.id, "some-id");
        assert_eq!(s.filters.len(), 1);
        assert_eq!(s.filters.get(0).unwrap().authors, None);
        Ok(())
    }

    #[test]
    fn incorrect_header() {
        let raw_json = "[\"REQUEST\",\"some-id\",\"{}\"]";
        assert!(serde_json::from_str::<Subscription>(raw_json).is_err());
    }

    #[test]
    fn req_missing_filters() {
        let raw_json = "[\"REQ\",\"some-id\"]";
        assert!(serde_json::from_str::<Subscription>(raw_json).is_err());
    }

    #[test]
    fn invalid_filter() {
        // unrecognized field in filter
        let raw_json = "[\"REQ\",\"some-id\",{\"foo\": 3}]";
        assert!(serde_json::from_str::<Subscription>(raw_json).is_err());
    }

    #[test]
    fn author_filter() -> Result<()> {
        let raw_json = "[\"REQ\",\"some-id\",{\"author\": \"test-author-id\"}]";
        let s: Subscription = serde_json::from_str(raw_json)?;
        assert_eq!(s.id, "some-id");
        assert_eq!(s.filters.len(), 1);
        let first_filter = s.filters.get(0).unwrap();
        assert_eq!(first_filter.author, Some("test-author-id".to_owned()));
        Ok(())
    }

    #[test]
    fn interest_id_nomatch() -> Result<()> {
        // subscription with a filter for ID
        let s: Subscription = serde_json::from_str(r#"["REQ","xyz",{"id":"abc"}]"#)?;
        let e = Event {
            id: "abcde".to_owned(),
            pubkey: "".to_owned(),
            created_at: 0,
            kind: 0,
            tags: Vec::new(),
            content: "".to_owned(),
            sig: "".to_owned(),
        };
        assert_eq!(s.interested_in_event(&e), false);
        Ok(())
    }

    #[test]
    fn interest_time_and_id() -> Result<()> {
        // subscription with a filter for ID and time
        let s: Subscription = serde_json::from_str(r#"["REQ","xyz",{"id":"abc", "since": 1000}]"#)?;
        let e = Event {
            id: "abc".to_owned(),
            pubkey: "".to_owned(),
            created_at: 50,
            kind: 0,
            tags: Vec::new(),
            content: "".to_owned(),
            sig: "".to_owned(),
        };
        assert_eq!(s.interested_in_event(&e), false);
        Ok(())
    }

    #[test]
    fn interest_time_and_id2() -> Result<()> {
        // subscription with a filter for ID and time
        let s: Subscription = serde_json::from_str(r#"["REQ","xyz",{"id":"abc", "since": 1000}]"#)?;
        let e = Event {
            id: "abc".to_owned(),
            pubkey: "".to_owned(),
            created_at: 1001,
            kind: 0,
            tags: Vec::new(),
            content: "".to_owned(),
            sig: "".to_owned(),
        };
        assert_eq!(s.interested_in_event(&e), true);
        Ok(())
    }

    #[test]
    fn interest_id() -> Result<()> {
        // subscription with a filter for ID
        let s: Subscription = serde_json::from_str(r#"["REQ","xyz",{"id":"abc"}]"#)?;
        let e = Event {
            id: "abc".to_owned(),
            pubkey: "".to_owned(),
            created_at: 0,
            kind: 0,
            tags: Vec::new(),
            content: "".to_owned(),
            sig: "".to_owned(),
        };
        assert_eq!(s.interested_in_event(&e), true);
        Ok(())
    }

    #[test]
    fn authors_single() -> Result<()> {
        // subscription with a filter for ID
        let s: Subscription = serde_json::from_str(r#"["REQ","xyz",{"authors":["abc"]}]"#)?;
        let e = Event {
            id: "123".to_owned(),
            pubkey: "abc".to_owned(),
            created_at: 0,
            kind: 0,
            tags: Vec::new(),
            content: "".to_owned(),
            sig: "".to_owned(),
        };
        assert_eq!(s.interested_in_event(&e), true);
        Ok(())
    }
    #[test]

    fn authors_multi_pubkey() -> Result<()> {
        // check for any of a set of authors, against the pubkey
        let s: Subscription = serde_json::from_str(r#"["REQ","xyz",{"authors":["abc", "bcd"]}]"#)?;
        let e = Event {
            id: "123".to_owned(),
            pubkey: "bcd".to_owned(),
            created_at: 0,
            kind: 0,
            tags: Vec::new(),
            content: "".to_owned(),
            sig: "".to_owned(),
        };
        assert_eq!(s.interested_in_event(&e), true);
        Ok(())
    }

    #[test]
    fn authors_multi_no_match() -> Result<()> {
        // check for any of a set of authors, against the pubkey
        let s: Subscription = serde_json::from_str(r#"["REQ","xyz",{"authors":["abc", "bcd"]}]"#)?;
        let e = Event {
            id: "123".to_owned(),
            pubkey: "xyz".to_owned(),
            created_at: 0,
            kind: 0,
            tags: Vec::new(),
            content: "".to_owned(),
            sig: "".to_owned(),
        };
        assert_eq!(s.interested_in_event(&e), false);
        Ok(())
    }
}
