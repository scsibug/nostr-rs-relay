use crate::error::{Error, Result};
use serde::{Deserialize, Deserializer, Serialize};
//use serde_json::json;
//use serde_json::Result;

// Container for a request filter
#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
#[serde(transparent)]
pub struct ReqCmd {
    cmds: Vec<String>,
}

#[derive(PartialEq, Debug, Clone)]
pub struct Subscription {
    id: String,
    filters: Vec<ReqFilter>,
}

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct ReqFilter {
    id: Option<String>,
    author: Option<String>,
    kind: Option<u8>,
    #[serde(rename = "e#")]
    event: Option<String>,
    #[serde(rename = "p#")]
    pubkey: Option<String>,
    since: Option<u64>,
    authors: Option<Vec<String>>,
}

impl<'de> Deserialize<'de> for Subscription {
    fn deserialize<D>(deserializer: D) -> Result<Subscription, D::Error>
    where
        D: Deserializer<'de>,
    {
        let r: ReqCmd = Deserialize::deserialize(deserializer)?;
        let elems = r.cmds;
        // ensure we have at least 3 fields
        if elems.len() < 3 {
            Err(serde::de::Error::custom("not enough fields"))
        } else {
            // divide into req/sub-id vector, and filter vector
            let (header, filter_strs) = elems.split_at(2);
            let req_cmd = header.get(0).unwrap();
            let sub_id = header.get(1).unwrap();
            if req_cmd != "REQ" {
                return Err(serde::de::Error::custom("missing REQ command"));
            }
            let mut filters = vec![];
            for e in filter_strs.iter() {
                let des_res = serde_json::from_str::<ReqFilter>(e);
                match des_res {
                    Ok(f) => filters.push(f),
                    Err(_) => return Err(serde::de::Error::custom("could not parse filter")),
                }
            }
            Ok(Subscription {
                id: sub_id.to_owned(),
                filters,
            })
        }
    }
}

// impl Subscription {
//     pub fn parse(json: &str) -> Result<Subscription> {
//         use serde to parse the ReqCmd, and then extract elements
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    // fn simple_req() -> Event {
    //     super::Event {
    //         id: 0,
    //         pubkey: 0,
    //         created_at: 0,
    //         kind: 0,
    //         tags: vec![],
    //         content: "".to_owned(),
    //         sig: 0,
    //     }
    // }

    #[test]
    fn empty_request_parse() -> Result<()> {
        let raw_json = "[\"REQ\",\"some-id\",\"{}\"]";
        let s: Subscription = serde_json::from_str(raw_json)?;
        assert_eq!(s.id, "some-id");
        assert_eq!(s.filters.len(), 1);
        assert_eq!(s.filters.get(0).unwrap().author, None);
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
        let raw_json = "[\"REQ\",\"some-id\",\"{\"foo\": 3}\"]";
        assert!(serde_json::from_str::<Subscription>(raw_json).is_err());
    }

    // #[test]
    // fn author_filter() -> Result<()> {
    //     let raw_json = "[\"REQ\",\"some-id\",\"{\"author\": \"test-author-id\"}\"]";
    //     let s: Subscription = serde_json::from_str(raw_json)?;
    //     assert_eq!(s.id, "some-id");
    //     assert_eq!(s.filters.len(), 1);
    //     Ok(())
    // }
}
