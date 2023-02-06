//! Event parsing and validation
use crate::delegation::validate_delegation;
use crate::error::Error::{CommandUnknownError, EventCouldNotCanonicalize, EventInvalidId, EventInvalidSignature, EventMalformedPubkey};
use crate::error::Result;
use crate::nip05;
use crate::utils::unix_time;
use bitcoin_hashes::{sha256, Hash};
use lazy_static::lazy_static;
use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::value::Value;
use serde_json::Number;
use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;
use tracing::{debug, info};

lazy_static! {
    /// Secp256k1 verification instance.
    pub static ref SECP: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
}

/// Event command in network format.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct EventCmd {
    cmd: String, // expecting static "EVENT"
    event: Event,
}

impl EventCmd {
    #[must_use] pub fn event_id(&self) -> &str {
        &self.event.id
    }
}

/// Parsed nostr event.
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    #[serde(skip)]
    pub delegated_by: Option<String>,
    pub created_at: u64,
    pub kind: u64,
    #[serde(deserialize_with = "tag_from_string")]
    // NOTE: array-of-arrays may need to be more general than a string container
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
    // Optimization for tag search, built on demand.
    #[serde(skip)]
    pub tagidx: Option<HashMap<char, HashSet<String>>>,
}

/// Simple tag type for array of array of strings.
type Tag = Vec<Vec<String>>;

/// Deserializer that ensures we always have a [`Tag`].
fn tag_from_string<'de, D>(deserializer: D) -> Result<Tag, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_default())
}

/// Attempt to form a single-char tag name.
#[must_use] pub fn single_char_tagname(tagname: &str) -> Option<char> {
    // We return the tag character if and only if the tagname consists
    // of a single char.
    let mut tagnamechars = tagname.chars();
    let firstchar = tagnamechars.next();
    match firstchar {
        Some(_) => {
            // check second char
            if tagnamechars.next().is_none() {
                firstchar
            } else {
                None
            }
        }
        None => None,
    }
}

/// Convert network event to parsed/validated event.
impl From<EventCmd> for Result<Event> {
    fn from(ec: EventCmd) -> Result<Event> {
        // ensure command is correct
        if ec.cmd == "EVENT" {
            ec.event.validate().map(|_| {
                let mut e = ec.event;
                e.build_index();
                e.update_delegation();
                e
            })
        } else {
            Err(CommandUnknownError)
        }
    }
}

impl Event {
    #[cfg(test)]
    #[must_use] pub fn simple_event() -> Event {
        Event {
            id: "0".to_owned(),
            pubkey: "0".to_owned(),
            delegated_by: None,
            created_at: 0,
            kind: 0,
            tags: vec![],
            content: "".to_owned(),
            sig: "0".to_owned(),
            tagidx: None,
        }
    }

    #[must_use] pub fn is_kind_metadata(&self) -> bool {
        self.kind == 0
    }

    /// Should this event be persisted?
    #[must_use] pub fn is_ephemeral(&self) -> bool {
        self.kind >= 20000 && self.kind < 30000
    }

    /// Should this event be replaced with newer timestamps from same author?
    #[must_use] pub fn is_replaceable(&self) -> bool {
        self.kind == 0 || self.kind == 3 || self.kind == 41 || (self.kind >= 10000 && self.kind < 20000)
    }

    /// Should this event be replaced with newer timestamps from same author, for distinct `d` tag values?
    #[must_use] pub fn is_param_replaceable(&self) -> bool {
        self.kind >= 30000 && self.kind < 40000
    }

    /// What is the replaceable `d` tag value?

    /// Should this event be replaced with newer timestamps from same author, for distinct `d` tag values?
    #[must_use] pub fn distinct_param(&self) -> Option<String> {
        if self.is_param_replaceable() {
            let default = "".to_string();
            let dvals:Vec<&String> = self.tags
                .iter()
                .filter(|x| !x.is_empty())
                .filter(|x| x.get(0).unwrap() == "d")
                .map(|x| x.get(1).unwrap_or(&default)).take(1)
                .collect();
            let dval_first = dvals.get(0);
            match dval_first {
                Some(_) => {dval_first.map(|x| x.to_string())},
                None => Some(default)
            }
        } else {
            None
        }
    }

    /// Pull a NIP-05 Name out of the event, if one exists
    #[must_use] pub fn get_nip05_addr(&self) -> Option<nip05::Nip05Name> {
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

    // is this event delegated (properly)?
    // does the signature match, and are conditions valid?
    // if so, return an alternate author for the event
    #[must_use] pub fn delegated_author(&self) -> Option<String> {
        // is there a delegation tag?
        let delegation_tag: Vec<String> = self
            .tags
            .iter()
            .filter(|x| x.len() == 4)
            .filter(|x| x.get(0).unwrap() == "delegation")
            .take(1)
            .next()?.clone(); // get first tag

        //let delegation_tag = self.tag_values_by_name("delegation");
        // delegation tags should have exactly 3 elements after the name (pubkey, condition, sig)
        // the event is signed by the delagatee
        let delegatee = &self.pubkey;
        // the delegation tag references the claimed delagator
        let delegator: &str = delegation_tag.get(1)?;
        let querystr: &str = delegation_tag.get(2)?;
        let sig: &str = delegation_tag.get(3)?;

        // attempt to get a condition query; this requires the delegation to have a valid signature.
        if let Some(cond_query) = validate_delegation(delegator, delegatee, querystr, sig) {
            // The signature was valid, now we ensure the delegation
            // condition is valid for this event:
            if cond_query.allows_event(self) {
                // since this is allowed, we will provide the delegatee
                Some(delegator.into())
            } else {
                debug!("an event failed to satisfy delegation conditions");
                None
            }
        } else {
            debug!("event had had invalid delegation signature");
            None
        }
    }

    /// Update delegation status
    pub fn update_delegation(&mut self) {
        self.delegated_by = self.delegated_author();
    }
    /// Build an event tag index
    pub fn build_index(&mut self) {
        // if there are no tags; just leave the index as None
        if self.tags.is_empty() {
            return;
        }
        // otherwise, build an index
        let mut idx: HashMap<char, HashSet<String>> = HashMap::new();
        // iterate over tags that have at least 2 elements
        for t in self.tags.iter().filter(|x| x.len() > 1) {
            let tagname = t.get(0).unwrap();
            let tagnamechar_opt = single_char_tagname(tagname);
            if tagnamechar_opt.is_none() {
                continue;
            }
            let tagnamechar = tagnamechar_opt.unwrap();
            let tagval = t.get(1).unwrap();
            // ensure a vector exists for this tag
            idx.entry(tagnamechar).or_insert_with(HashSet::new);
            // get the tag vec and insert entry
            let idx_tag_vec = idx.get_mut(&tagnamechar).expect("could not get tag vector");
            idx_tag_vec.insert(tagval.clone());
        }
        // save the tag structure
        self.tagidx = Some(idx);
    }

    /// Create a short event identifier, suitable for logging.
    #[must_use] pub fn get_event_id_prefix(&self) -> String {
        self.id.chars().take(8).collect()
    }
    #[must_use] pub fn get_author_prefix(&self) -> String {
        self.pubkey.chars().take(8).collect()
    }

    /// Retrieve tag initial values across all tags matching the name
    #[must_use] pub fn tag_values_by_name(&self, tag_name: &str) -> Vec<String> {
        self.tags
            .iter()
            .filter(|x| x.len() > 1)
            .filter(|x| x.get(0).unwrap() == tag_name)
            .map(|x| x.get(1).unwrap().clone())
            .collect()
    }

    #[must_use] pub fn is_valid_timestamp(&self, reject_future_seconds: Option<usize>) -> bool {
        if let Some(allowable_future) = reject_future_seconds {
            let curr_time = unix_time();
            // calculate difference, plus how far future we allow
            if curr_time + (allowable_future as u64) < self.created_at {
                let delta = self.created_at - curr_time;
                debug!(
                    "event is too far in the future ({} seconds), rejecting",
                    delta
                );
                return false;
            }
        }
        true
    }

    /// Check if this event has a valid signature.
    pub fn validate(&self) -> Result<()> {
        // TODO: return a Result with a reason for invalid events
        // validation is performed by:
        // * parsing JSON string into event fields
        // * create an array:
        // ** [0, pubkey-hex-string, created-at-num, kind-num, tags-array-of-arrays, content-string]
        // * serialize with no spaces/newlines
        let c_opt = self.to_canonical();
        if c_opt.is_none() {
            debug!("could not canonicalize");
            return Err(EventCouldNotCanonicalize);
        }
        let c = c_opt.unwrap();
        // * compute the sha256sum.
        let digest: sha256::Hash = sha256::Hash::hash(c.as_bytes());
        let hex_digest = format!("{digest:x}");
        // * ensure the id matches the computed sha256sum.
        if self.id != hex_digest {
            debug!("event id does not match digest");
            return Err(EventInvalidId);
        }
        // * validate the message digest (sig) using the pubkey & computed sha256 message hash.
        let sig = schnorr::Signature::from_str(&self.sig).unwrap();
        if let Ok(msg) = secp256k1::Message::from_slice(digest.as_ref()) {
            if let Ok(pubkey) = XOnlyPublicKey::from_str(&self.pubkey) {
                SECP.verify_schnorr(&sig, &msg, &pubkey)
                    .map_err(|_| EventInvalidSignature)
            } else {
                debug!("client sent malformed pubkey");
                Err(EventMalformedPubkey)
            }
        } else {
            info!("error converting digest to secp256k1 message");
            Err(EventInvalidSignature)
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
        c.push(Value::String(self.pubkey.clone()));
        // creation time
        let created_at = Number::from(self.created_at);
        c.push(serde_json::Value::Number(created_at));
        // kind
        let kind = Number::from(self.kind);
        c.push(serde_json::Value::Number(kind));
        // tags
        c.push(self.tags_to_canonical());
        // content
        c.push(Value::String(self.content.clone()));
        serde_json::to_string(&Value::Array(c)).ok()
    }

    /// Convert tags to a canonical form for signing.
    fn tags_to_canonical(&self) -> Value {
        let mut tags = Vec::<Value>::new();
        // iterate over self tags,
        for t in &self.tags {
            // each tag is a vec of strings
            let mut a = Vec::<Value>::new();
            for v in t.iter() {
                a.push(serde_json::Value::String(v.clone()));
            }
            tags.push(serde_json::Value::Array(a));
        }
        serde_json::Value::Array(tags)
    }

    /// Determine if the given tag and value set intersect with tags in this event.
    #[must_use] pub fn generic_tag_val_intersect(&self, tagname: char, check: &HashSet<String>) -> bool {
        match &self.tagidx {
            // check if this is indexable tagname
            Some(idx) => match idx.get(&tagname) {
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

    #[test]
    fn event_creation() {
        // create an event
        let event = Event::simple_event();
        assert_eq!(event.id, "0");
    }

    #[test]
    fn event_serialize() -> Result<()> {
        // serialize an event to JSON string
        let event = Event::simple_event();
        let j = serde_json::to_string(&event)?;
        assert_eq!(j, "{\"id\":\"0\",\"pubkey\":\"0\",\"created_at\":0,\"kind\":0,\"tags\":[],\"content\":\"\",\"sig\":\"0\"}");
        Ok(())
    }

    #[test]
    fn empty_event_tag_match() {
        let event = Event::simple_event();
        assert!(!event
                .generic_tag_val_intersect('e', &HashSet::from(["foo".to_owned(), "bar".to_owned()])));
    }

    #[test]
    fn single_event_tag_match() {
        let mut event = Event::simple_event();
        event.tags = vec![vec!["e".to_owned(), "foo".to_owned()]];
        event.build_index();
        assert_eq!(
            event.generic_tag_val_intersect(
                'e',
                &HashSet::from(["foo".to_owned(), "bar".to_owned()])
            ),
            true
        );
    }

    #[test]
    fn event_tags_serialize() -> Result<()> {
        // serialize an event with tags to JSON string
        let mut event = Event::simple_event();
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
            delegated_by: None,
            created_at: 501_234,
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
    fn event_tag_select() {
        let e = Event {
            id: "999".to_owned(),
            pubkey: "012345".to_owned(),
            delegated_by: None,
            created_at: 501_234,
            kind: 1,
            tags: vec![
                vec!["j".to_owned(), "abc".to_owned()],
                vec!["e".to_owned(), "foo".to_owned()],
                vec!["e".to_owned(), "bar".to_owned()],
                vec!["e".to_owned(), "baz".to_owned()],
                vec![
                    "p".to_owned(),
                    "aaaa".to_owned(),
                    "ws://example.com".to_owned(),
                ],
            ],
            content: "this is a test".to_owned(),
            sig: "abcde".to_owned(),
            tagidx: None,
        };
        let v = e.tag_values_by_name("e");
        assert_eq!(v, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn event_no_tag_select() {
        let e = Event {
            id: "999".to_owned(),
            pubkey: "012345".to_owned(),
            delegated_by: None,
            created_at: 501_234,
            kind: 1,
            tags: vec![
                vec!["j".to_owned(), "abc".to_owned()],
                vec!["e".to_owned(), "foo".to_owned()],
                vec!["e".to_owned(), "baz".to_owned()],
                vec![
                    "p".to_owned(),
                    "aaaa".to_owned(),
                    "ws://example.com".to_owned(),
                ],
            ],
            content: "this is a test".to_owned(),
            sig: "abcde".to_owned(),
            tagidx: None,
        };
        let v = e.tag_values_by_name("x");
        // asking for tags that don't exist just returns zero-length vector
        assert_eq!(v.len(), 0);
    }

    #[test]
    fn event_canonical_with_tags() {
        let e = Event {
            id: "999".to_owned(),
            pubkey: "012345".to_owned(),
            delegated_by: None,
            created_at: 501_234,
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

    #[test]
    fn ephemeral_event() {
        let mut event = Event::simple_event();
        event.kind=20000;
        assert!(event.is_ephemeral());
        event.kind=29999;
        assert!(event.is_ephemeral());
        event.kind=30000;
        assert!(!event.is_ephemeral());
        event.kind=19999;
        assert!(!event.is_ephemeral());
    }

    #[test]
    fn replaceable_event() {
        let mut event = Event::simple_event();
        event.kind=0;
        assert!(event.is_replaceable());
        event.kind=3;
        assert!(event.is_replaceable());
        event.kind=10000;
        assert!(event.is_replaceable());
        event.kind=19999;
        assert!(event.is_replaceable());
        event.kind=20000;
        assert!(!event.is_replaceable());
    }

    #[test]
    fn param_replaceable_event() {
        let mut event = Event::simple_event();
        event.kind = 30000;
        assert!(event.is_param_replaceable());
        event.kind = 39999;
        assert!(event.is_param_replaceable());
        event.kind = 29999;
        assert!(!event.is_param_replaceable());
        event.kind = 40000;
        assert!(!event.is_param_replaceable());
    }

    #[test]
    fn param_replaceable_value_case_1() {
        // NIP case #1: "tags":[["d",""]]
        let mut event = Event::simple_event();
        event.kind = 30000;
        event.tags = vec![
            vec!["d".to_owned(), "".to_owned()]];
        assert_eq!(event.distinct_param(), Some("".to_string()));
    }

    #[test]
    fn param_replaceable_value_case_2() {
        // NIP case #2: "tags":[]: implicit d tag with empty value
        let mut event = Event::simple_event();
        event.kind = 30000;
        assert_eq!(event.distinct_param(), Some("".to_string()));
    }

    #[test]
    fn param_replaceable_value_case_3() {
        // NIP case #3: "tags":[["d"]]: implicit empty value ""
        let mut event = Event::simple_event();
        event.kind = 30000;
        event.tags = vec![
            vec!["d".to_owned()]];
        assert_eq!(event.distinct_param(), Some("".to_string()));
    }

    #[test]
    fn param_replaceable_value_case_4() {
        // NIP case #4: "tags":[["d",""],["d","not empty"]]: only first d tag is considered
        let mut event = Event::simple_event();
        event.kind = 30000;
        event.tags = vec![
            vec!["d".to_owned(), "".to_string()],
            vec!["d".to_owned(), "not empty".to_string()]
        ];
        assert_eq!(event.distinct_param(), Some("".to_string()));
    }

    #[test]
    fn param_replaceable_value_case_4b() {
        // Variation of #4 with
        // NIP case #4: "tags":[["d","not empty"],["d",""]]: only first d tag is considered
        let mut event = Event::simple_event();
        event.kind = 30000;
        event.tags = vec![
            vec!["d".to_owned(), "not empty".to_string()],
            vec!["d".to_owned(), "".to_string()]
        ];
        assert_eq!(event.distinct_param(), Some("not empty".to_string()));
    }

    #[test]
    fn param_replaceable_value_case_5() {
        // NIP case #5: "tags":[["d"],["d","some value"]]: only first d tag is considered
        let mut event = Event::simple_event();
        event.kind = 30000;
        event.tags = vec![
            vec!["d".to_owned()],
            vec!["d".to_owned(), "second value".to_string()],
            vec!["d".to_owned(), "third value".to_string()]
        ];
        assert_eq!(event.distinct_param(), Some("".to_string()));
    }

    #[test]
    fn param_replaceable_value_case_6() {
        // NIP case #6: "tags":[["e"]]: same as no tags
        let mut event = Event::simple_event();
        event.kind = 30000;
        event.tags = vec![
            vec!["e".to_owned()],
        ];
        assert_eq!(event.distinct_param(), Some("".to_string()));
    }

}
