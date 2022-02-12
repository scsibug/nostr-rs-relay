//! Event parsing and validation
use crate::config;
use crate::error::Error::*;
use crate::error::Result;
use crate::nip05;
use bitcoin_hashes::{sha256, Hash};
use lazy_static::lazy_static;
use log::*;
use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::Value;
use serde_json::Number;
use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;
use std::time::SystemTime;

lazy_static! {
    pub static ref SECP: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
}

/// Event command in network format
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct EventCmd {
    cmd: String, // expecting static "EVENT"
    event: Event,
}

/// Event parsed
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Event {
    pub id: String,
    pub(crate) pubkey: String,
    pub(crate) created_at: u64,
    pub(crate) kind: u64,
    #[serde(deserialize_with = "tag_from_string")]
    // NOTE: array-of-arrays may need to be more general than a string container
    pub(crate) tags: Vec<Vec<String>>,
    pub(crate) content: String,
    pub(crate) sig: String,
    // Optimization for tag search, built on demand
    #[serde(skip)]
    pub(crate) tagidx: Option<HashMap<String, HashSet<String>>>,
}

/// Simple tag type for array of array of strings.
type Tag = Vec<Vec<String>>;

/// Deserializer that ensures we always have a [`Tag`].
fn tag_from_string<'de, D>(deserializer: D) -> Result<Tag, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(Vec::new))
}

/// Convert network event to parsed/validated event.
impl From<EventCmd> for Result<Event> {
    fn from(ec: EventCmd) -> Result<Event> {
        // ensure command is correct
        if ec.cmd != "EVENT" {
            Err(CommandUnknownError)
        } else if ec.event.is_valid() {
            let mut e = ec.event;
            e.build_index();
            Ok(e)
        } else {
            Err(EventInvalid)
        }
    }
}

/// Seconds since 1970
pub fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|x| x.as_secs())
        .unwrap_or(0)
}

impl Event {
    pub fn is_kind_metadata(&self) -> bool {
        self.kind == 0
    }

    /// Pull a NIP-05 Name out of the event, if one exists
    pub fn get_nip05_addr(&self) -> Option<nip05::Nip05Name> {
        if self.is_kind_metadata() {
            // very quick check if we should attempt to parse this json
            if self.content.contains("\"nip05\"") {
                // Parse into JSON
                let md_parsed: Value = serde_json::from_str(&self.content).ok()?;
                let md_map = md_parsed.as_object()?;
                let nip05_str = md_map.get("nip05")?.as_str()?;
                return nip05::Nip05Name::try_from(nip05_str).ok();
            }
        }
        None
    }

    /// Build an event tag index
    fn build_index(&mut self) {
        // if there are no tags; just leave the index as None
        if self.tags.is_empty() {
            return;
        }
        // otherwise, build an index
        let mut idx: HashMap<String, HashSet<String>> = HashMap::new();
        // iterate over tags that have at least 2 elements
        for t in self.tags.iter().filter(|x| x.len() > 1) {
            let tagname = t.get(0).unwrap();
            let tagval = t.get(1).unwrap();
            // ensure a vector exists for this tag
            if !idx.contains_key(tagname) {
                idx.insert(tagname.clone(), HashSet::new());
            }
            // get the tag vec and insert entry
            let tidx = idx.get_mut(tagname).expect("could not get tag vector");
            tidx.insert(tagval.clone());
        }
        // save the tag structure
        self.tagidx = Some(idx);
    }

    /// Create a short event identifier, suitable for logging.
    pub fn get_event_id_prefix(&self) -> String {
        self.id.chars().take(8).collect()
    }
    pub fn get_author_prefix(&self) -> String {
        self.pubkey.chars().take(8).collect()
    }

    /// Check if this event has a valid signature.
    fn is_valid(&self) -> bool {
        // TODO: return a Result with a reason for invalid events
        // don't bother to validate an event with a timestamp in the distant future.
        let config = config::SETTINGS.read().unwrap();
        let max_future_sec = config.options.reject_future_seconds;
        if let Some(allowable_future) = max_future_sec {
            let curr_time = unix_time();
            // calculate difference, plus how far future we allow
            if curr_time + (allowable_future as u64) < self.created_at {
                let delta = self.created_at - curr_time;
                debug!(
                    "Event is too far in the future ({} seconds), rejecting",
                    delta
                );
                return false;
            }
        }
        // validation is performed by:
        // * parsing JSON string into event fields
        // * create an array:
        // ** [0, pubkey-hex-string, created-at-num, kind-num, tags-array-of-arrays, content-string]
        // * serialize with no spaces/newlines
        let c_opt = self.to_canonical();
        if c_opt.is_none() {
            debug!("event could not be canonicalized");
            return false;
        }
        let c = c_opt.unwrap();
        // * compute the sha256sum.
        let digest: sha256::Hash = sha256::Hash::hash(c.as_bytes());
        let hex_digest = format!("{:x}", digest);
        // * ensure the id matches the computed sha256sum.
        if self.id != hex_digest {
            debug!("event id does not match digest");
            return false;
        }
        // * validate the message digest (sig) using the pubkey & computed sha256 message hash.

        let sig = schnorr::Signature::from_str(&self.sig).unwrap();
        if let Ok(msg) = secp256k1::Message::from_slice(digest.as_ref()) {
            if let Ok(pubkey) = XOnlyPublicKey::from_str(&self.pubkey) {
                let verify = SECP.verify_schnorr(&sig, &msg, &pubkey);
                matches!(verify, Ok(()))
            } else {
                debug!("Client sent malformed pubkey");
                false
            }
        } else {
            info!("Error converting digest to secp256k1 message");
            false
        }
    }

    /// Convert event to canonical representation for signing.
    fn to_canonical(&self) -> Option<String> {
        // create a JsonValue for each event element
        let mut c: Vec<Value> = vec![];
        // id must be set to 0
        let id = Number::from(0_u64);
        c.push(serde_json::Value::Number(id));
        // public key
        c.push(Value::String(self.pubkey.to_owned()));
        // creation time
        let created_at = Number::from(self.created_at);
        c.push(serde_json::Value::Number(created_at));
        // kind
        let kind = Number::from(self.kind);
        c.push(serde_json::Value::Number(kind));
        // tags
        c.push(self.tags_to_canonical());
        // content
        c.push(Value::String(self.content.to_owned()));
        serde_json::to_string(&Value::Array(c)).ok()
    }

    /// Convert tags to a canonical form for signing.
    fn tags_to_canonical(&self) -> Value {
        let mut tags = Vec::<Value>::new();
        // iterate over self tags,
        for t in self.tags.iter() {
            // each tag is a vec of strings
            let mut a = Vec::<Value>::new();
            for v in t.iter() {
                a.push(serde_json::Value::String(v.to_owned()));
            }
            tags.push(serde_json::Value::Array(a));
        }
        serde_json::Value::Array(tags)
    }

    /// Determine if the given tag and value set intersect with tags in this event.
    pub fn generic_tag_val_intersect(&self, tagname: &str, check: &HashSet<String>) -> bool {
        match &self.tagidx {
            Some(idx) => match idx.get(tagname) {
                Some(valset) => {
                    let common = valset.intersection(check);
                    common.count() > 0
                }
                None => false,
            },
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn simple_event() -> Event {
        Event {
            id: "0".to_owned(),
            pubkey: "0".to_owned(),
            created_at: 0,
            kind: 0,
            tags: vec![],
            content: "".to_owned(),
            sig: "0".to_owned(),
            tagidx: None,
        }
    }

    #[test]
    fn event_creation() {
        // create an event
        let event = simple_event();
        assert_eq!(event.id, "0");
    }

    #[test]
    fn event_serialize() -> Result<()> {
        // serialize an event to JSON string
        let event = simple_event();
        let j = serde_json::to_string(&event)?;
        assert_eq!(j, "{\"id\":\"0\",\"pubkey\":\"0\",\"created_at\":0,\"kind\":0,\"tags\":[],\"content\":\"\",\"sig\":\"0\"}");
        Ok(())
    }

    #[test]
    fn empty_event_tag_match() -> Result<()> {
        let event = simple_event();
        assert!(!event
            .generic_tag_val_intersect("e", &HashSet::from(["foo".to_owned(), "bar".to_owned()])));
        Ok(())
    }

    #[test]
    fn single_event_tag_match() -> Result<()> {
        let mut event = simple_event();
        event.tags = vec![vec!["e".to_owned(), "foo".to_owned()]];
        event.build_index();
        assert_eq!(
            event.generic_tag_val_intersect(
                "e",
                &HashSet::from(["foo".to_owned(), "bar".to_owned()])
            ),
            true
        );
        Ok(())
    }

    #[test]
    fn event_tags_serialize() -> Result<()> {
        // serialize an event with tags to JSON string
        let mut event = simple_event();
        event.tags = vec![
            vec![
                "e".to_owned(),
                "xxxx".to_owned(),
                "wss://example.com".to_owned(),
            ],
            vec![
                "p".to_owned(),
                "yyyyy".to_owned(),
                "wss://example.com:3033".to_owned(),
            ],
        ];
        let j = serde_json::to_string(&event)?;
        assert_eq!(j, "{\"id\":\"0\",\"pubkey\":\"0\",\"created_at\":0,\"kind\":0,\"tags\":[[\"e\",\"xxxx\",\"wss://example.com\"],[\"p\",\"yyyyy\",\"wss://example.com:3033\"]],\"content\":\"\",\"sig\":\"0\"}");
        Ok(())
    }

    #[test]
    fn event_deserialize() -> Result<()> {
        let raw_json = r#"{"id":"1384757da583e6129ce831c3d7afc775a33a090578f888dd0d010328ad047d0c","pubkey":"bbbd9711d357df4f4e498841fd796535c95c8e751fa35355008a911c41265fca","created_at":1612650459,"kind":1,"tags":null,"content":"hello world","sig":"59d0cc47ab566e81f72fe5f430bcfb9b3c688cb0093d1e6daa49201c00d28ecc3651468b7938642869ed98c0f1b262998e49a05a6ed056c0d92b193f4e93bc21"}"#;
        let e: Event = serde_json::from_str(raw_json)?;
        assert_eq!(e.kind, 1);
        assert_eq!(e.tags.len(), 0);
        Ok(())
    }

    #[test]
    fn event_canonical() {
        let e = Event {
            id: "999".to_owned(),
            pubkey: "012345".to_owned(),
            created_at: 501234,
            kind: 1,
            tags: vec![],
            content: "this is a test".to_owned(),
            sig: "abcde".to_owned(),
            tagidx: None,
        };
        let c = e.to_canonical();
        let expected = Some(r#"[0,"012345",501234,1,[],"this is a test"]"#.to_owned());
        assert_eq!(c, expected);
    }

    #[test]
    fn event_canonical_with_tags() {
        let e = Event {
            id: "999".to_owned(),
            pubkey: "012345".to_owned(),
            created_at: 501234,
            kind: 1,
            tags: vec![
                vec!["#e".to_owned(), "aoeu".to_owned()],
                vec![
                    "#p".to_owned(),
                    "aaaa".to_owned(),
                    "ws://example.com".to_owned(),
                ],
            ],
            content: "this is a test".to_owned(),
            sig: "abcde".to_owned(),
            tagidx: None,
        };
        let c = e.to_canonical();
        let expected_json = r###"[0,"012345",501234,1,[["#e","aoeu"],["#p","aaaa","ws://example.com"]],"this is a test"]"###;
        let expected = Some(expected_json.to_owned());
        assert_eq!(c, expected);
    }
}
