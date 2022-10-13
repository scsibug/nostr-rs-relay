//! Event parsing and validation
//use crate::error::Error::*;
//use crate::error::Result;
//use crate::utils::unix_time;
//use bitcoin_hashes::{sha256, Hash};
//use lazy_static::lazy_static;
//use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserializer, Deserialize, Serialize};
//use serde_json::value::Value;
//use serde_json::Number;
//use std::collections::HashMap;
//use std::collections::HashSet;
//use std::str::FromStr;
//use tracing::{debug, info};

// This handles everything related to delegation, in particular the
// condition/rune parsing and logic.

// Conditions are poorly specified, so we will implement the minimum
// necessary for now.

// fields MUST be either "kind" or "created_at".
// operators supported are ">", "<", "=", "!".
// no operations on 'content' are supported.

// this allows constraints for:
// valid date ranges (valid from X->Y dates).
// specific kinds (publish kind=1,5)
// kind ranges (publish ephemeral events, kind>19999&kind<30001)

// for more complex scenarios (allow delegatee to publish ephemeral
// AND replacement events), it may be necessary to generate and use
// different condition strings, since we do not support grouping or
// "OR" logic.

// using serde_urldecode, we can get a serde data format from the
// condition string.  We will then map that with a deserializer that
// maps to a ConditionQuery.


#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub enum Field {
    Kind,
    CreatedAt,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub enum Operator {
    LessThan,
    GreaterThan,
    Equals,
    NotEquals,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
/// Values are the data associated with a restriction.  For now, these can only be a single numbers.
pub enum Value {
    Number(u64),
}

#[derive(Serialize, PartialEq, Eq, Debug, Clone)]
pub struct ConditionQuery {
    pub(crate) conditions: Vec<Condition>,
}

impl <'de> Deserialize<'de> for ConditionQuery {
    fn deserialize<D>(d: D) -> Result<ConditionQuery, D::Error>
    where
        D: Deserializer<'de>,
    {
	// recv'd is an array of pairs.
	let _recvd: Value = d.deserialize_seq
	let cond_list = recvd.as_object().ok_or_else(|| {
            serde::de::Error::invalid_type(
                Unexpected::Other("reqfilter is not an object"),
                &"a json object",
            )
        })?;

	// all valid conditions will be put into this vec
	let conditions : Vec<Condition> = vec![];
	// loop through the parsed value, and identify if they are
	// known attributes.  unknown attributes will trigger failure,
	// since we can't respect those restrictions.
	Ok(ConditionQuery{conditions})
    }

}

/// Parsed delegation statement
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct Delegation {
    pub(crate) pubkey: String,
    pub(crate) condition_query: ConditionQuery,
    pub(crate) signature: String,
}

/// Parsed delegation condition
/// see https://github.com/nostr-protocol/nips/pull/28#pullrequestreview-1084903800
/// An example complex condition would be:  kind=1,2,3&created_at<1665265999
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct Condition {
    pub(crate) field: Field,
    pub(crate) operator: Operator,
    pub(crate) values: Vec<Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // parse condition strings
    #[test]
    fn parse_empty() {
	// given an empty condition query, produce an empty vector
        assert_eq!(Delegation::from(""), vec![]);
	assert_eq!(serde_urlencoded::from_str::<Vec<(String, String)>>(""), vec![]);
    }
}
