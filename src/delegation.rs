//! Event parsing and validation
use crate::error::Error;
use crate::error::Result;
//use crate::utils::unix_time;
//use bitcoin_hashes::{sha256, Hash};
use lazy_static::lazy_static;
use regex::Regex;
//use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
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

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct ConditionQuery {
    pub(crate) conditions: Vec<Condition>,
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

fn str_to_condition(cs: &str) -> Option<Condition> {
    // a condition is a string (alphanum+underscore), an operator (<>=!), and values (num+comma)
    lazy_static! {
        static ref RE: Regex = Regex::new("([[:word:]])([<>=!]+)([,[[:digit:]]]+))").unwrap();
    }
    // match against the regex
    let caps = RE.captures(cs)?;
    let _field =caps.get(0)?;
    Some(Condition {field: Field::Kind, operator: Operator::LessThan, values: vec![]})
}



/// Parse a condition query from a string slice
impl TryFrom<&str> for ConditionQuery {
    type Error = Error;
    fn try_from(value: &str) -> Result<Self, Self::Error> {
	// split the string with '&'
	let conds = value.split('&');
	// parse each individual condition
	for c in conds.into_iter() {
	    str_to_condition(c).ok_or(Error::DelegationParseError)?;
	}
	Ok(ConditionQuery{conditions: vec![]})

    }


}

#[cfg(test)]
mod tests {
    use super::*;

    // parse condition strings
    #[test]
    fn parse_empty() {
	// given an empty condition query, produce an empty vector
        assert_eq!(Delegation::from(""), vec![]);
    }
}
