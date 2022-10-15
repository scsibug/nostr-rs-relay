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
use std::str::FromStr;
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

impl FromStr for Field {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "kind" {
            Ok(Field::Kind)
        } else if value == "created_at" {
            Ok(Field::CreatedAt)
        } else {
            Err(Error::DelegationParseError)
        }
    }
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub enum Operator {
    LessThan,
    GreaterThan,
    Equals,
    NotEquals,
}
impl FromStr for Operator {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value == "<" {
            Ok(Operator::LessThan)
        } else if value == ">" {
            Ok(Operator::GreaterThan)
        } else if value == "=" {
            Ok(Operator::Equals)
        } else if value == "!" {
            Ok(Operator::NotEquals)
        } else {
            Err(Error::DelegationParseError)
        }
    }
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
    pub(crate) values: Vec<usize>,
}

fn str_to_condition(cs: &str) -> Option<Condition> {
    // a condition is a string (alphanum+underscore), an operator (<>=!), and values (num+comma)
    lazy_static! {
        static ref RE: Regex = Regex::new("([[:word:]]+)([<>=!]+)([,[[:digit:]]]*)").unwrap();
    }
    // match against the regex
    let caps = RE.captures(cs)?;
    let field = caps.get(1)?.as_str().parse::<Field>().ok()?;
    let operator = caps.get(2)?.as_str().parse::<Operator>().ok()?;
    // values are just comma separated numbers, but all must be parsed
    let rawvals = caps.get(3)?.as_str();
    let values = rawvals
        .split_terminator(',')
        .map(|n| n.parse::<usize>().ok())
        .collect::<Option<Vec<_>>>()?;
    // convert field string into Field
    Some(Condition {
        field,
        operator,
        values,
    })
}

/// Parse a condition query from a string slice
impl FromStr for ConditionQuery {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        // split the string with '&'
        let mut conditions = vec![];
        let condstrs = value.split_terminator('&');
        // parse each individual condition
        for c in condstrs {
            conditions.push(str_to_condition(c).ok_or(Error::DelegationParseError)?);
        }
        Ok(ConditionQuery { conditions })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // parse condition strings
    #[test]
    fn parse_empty() -> Result<()> {
        // given an empty condition query, produce an empty vector
        let empty_cq = ConditionQuery { conditions: vec![] };
        let parsed = "".parse::<ConditionQuery>()?;
        assert_eq!(parsed, empty_cq);
        Ok(())
    }

    // parse field 'kind'
    #[test]
    fn test_kind_field_parse() -> Result<()> {
        let field = "kind".parse::<Field>()?;
        assert_eq!(field, Field::Kind);
        Ok(())
    }
    // parse field 'created_at'
    #[test]
    fn test_created_at_field_parse() -> Result<()> {
        let field = "created_at".parse::<Field>()?;
        assert_eq!(field, Field::CreatedAt);
        Ok(())
    }
    // parse unknown field
    #[test]
    fn unknown_field_parse() {
        let field = "unk".parse::<Field>();
        assert!(field.is_err());
    }

    // parse a full conditional query with an empty array
    #[test]
    fn parse_kind_equals_empty() -> Result<()> {
        // given an empty condition query, produce an empty vector
        let kind_cq = ConditionQuery {
            conditions: vec![Condition {
                field: Field::Kind,
                operator: Operator::Equals,
                values: vec![],
            }],
        };
        let parsed = "kind=".parse::<ConditionQuery>()?;
        assert_eq!(parsed, kind_cq);
        Ok(())
    }
    // parse a full conditional query with a single value
    #[test]
    fn parse_kind_equals_singleval() -> Result<()> {
        // given an empty condition query, produce an empty vector
        let kind_cq = ConditionQuery {
            conditions: vec![Condition {
                field: Field::Kind,
                operator: Operator::Equals,
                values: vec![1],
            }],
        };
        let parsed = "kind=1".parse::<ConditionQuery>()?;
        assert_eq!(parsed, kind_cq);
        Ok(())
    }
    // parse a full conditional query with multiple values
    #[test]
    fn parse_kind_equals_multival() -> Result<()> {
        // given an empty condition query, produce an empty vector
        let kind_cq = ConditionQuery {
            conditions: vec![Condition {
                field: Field::Kind,
                operator: Operator::Equals,
                values: vec![1, 2, 4],
            }],
        };
        let parsed = "kind=1,2,4".parse::<ConditionQuery>()?;
        assert_eq!(parsed, kind_cq);
        Ok(())
    }
    // parse multiple conditions
    #[test]
    fn parse_multi_conditions() -> Result<()> {
        // given an empty condition query, produce an empty vector
        let cq = ConditionQuery {
            conditions: vec![
                Condition {
                    field: Field::Kind,
                    operator: Operator::GreaterThan,
                    values: vec![10000],
                },
                Condition {
                    field: Field::Kind,
                    operator: Operator::LessThan,
                    values: vec![20000],
                },
                Condition {
                    field: Field::Kind,
                    operator: Operator::NotEquals,
                    values: vec![10001],
                },
                Condition {
                    field: Field::CreatedAt,
                    operator: Operator::LessThan,
                    values: vec![1665867123],
                },
            ],
        };
        let parsed =
            "kind>10000&kind<20000&kind!10001&created_at<1665867123".parse::<ConditionQuery>()?;
        assert_eq!(parsed, cq);
        Ok(())
    }
}
