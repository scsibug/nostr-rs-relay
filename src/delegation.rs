//! Event parsing and validation
use crate::error::Error;
use crate::error::Result;
use crate::event::Event;
use bitcoin_hashes::{sha256, Hash};
use lazy_static::lazy_static;
use regex::Regex;
use secp256k1::{schnorr, Secp256k1, VerifyOnly, XOnlyPublicKey};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use tracing::{debug, info};

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

lazy_static! {
    /// Secp256k1 verification instance.
    pub static ref SECP: Secp256k1<VerifyOnly> = Secp256k1::verification_only();
}

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
    pub conditions: Vec<Condition>,
}

impl ConditionQuery {
    #[must_use] pub fn allows_event(&self, event: &Event) -> bool {
        // check each condition, to ensure that the event complies
        // with the restriction.
        for c in &self.conditions {
            if !c.allows_event(event) {
                // any failing conditions invalidates the delegation
                // on this event
                return false;
            }
        }
        // delegation was permitted unconditionally, or all conditions
        // were true
        true
    }
}

// Verify that the delegator approved the delegation; return a ConditionQuery if so.
#[must_use] pub fn validate_delegation(
    delegator: &str,
    delegatee: &str,
    cond_query: &str,
    sigstr: &str,
) -> Option<ConditionQuery> {
    // form the token
    let tok = format!("nostr:delegation:{delegatee}:{cond_query}");
    // form SHA256 hash
    let digest: sha256::Hash = sha256::Hash::hash(tok.as_bytes());
    let sig = schnorr::Signature::from_str(sigstr).unwrap();
    if let Ok(msg) = secp256k1::Message::from_slice(digest.as_ref()) {
        if let Ok(pubkey) = XOnlyPublicKey::from_str(delegator) {
            let verify = SECP.verify_schnorr(&sig, &msg, &pubkey);
            if verify.is_ok() {
                // return the parsed condition query
                cond_query.parse::<ConditionQuery>().ok()
            } else {
                debug!("client sent an delegation signature that did not validate");
                None
            }
        } else {
            debug!("client sent malformed delegation pubkey");
            None
        }
    } else {
        info!("error converting delegation digest to secp256k1 message");
        None
    }
}

/// Parsed delegation condition
/// see <https://github.com/nostr-protocol/nips/pull/28#pullrequestreview-1084903800>
/// An example complex condition would be:  `kind=1,2,3&created_at<1665265999`
#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct Condition {
    pub field: Field,
    pub operator: Operator,
    pub values: Vec<u64>,
}

impl Condition {
    /// Check if this condition allows the given event to be delegated
    #[must_use] pub fn allows_event(&self, event: &Event) -> bool {
        // determine what the right-hand side of the operator is
        let resolved_field = match &self.field {
            Field::Kind => event.kind,
            Field::CreatedAt => event.created_at,
        };
        match &self.operator {
            Operator::LessThan => {
                // the less-than operator is only valid for single values.
                if self.values.len() == 1 {
                    if let Some(v) = self.values.first() {
                        return resolved_field < *v;
                    }
                }
            }
            Operator::GreaterThan => {
                // the greater-than operator is only valid for single values.
                if self.values.len() == 1 {
                    if let Some(v) = self.values.first() {
                        return resolved_field > *v;
                    }
                }
            }
            Operator::Equals => {
                // equals is interpreted as "must be equal to at least one provided value"
                return self.values.iter().any(|&x| resolved_field == x);
            }
            Operator::NotEquals => {
                // not-equals is interpreted as "must not be equal to any provided value"
                // this is the one case where an empty list of values could be allowed; even though it is a pointless restriction.
                return self.values.iter().all(|&x| resolved_field != x);
            }
        }
        false
    }
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
        .map(|n| n.parse::<u64>().ok())
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
                    values: vec![1_665_867_123],
                },
            ],
        };
        let parsed =
            "kind>10000&kind<20000&kind!10001&created_at<1665867123".parse::<ConditionQuery>()?;
        assert_eq!(parsed, cq);
        Ok(())
    }
    // Check for condition logic on event w/ empty values
    #[test]
    fn condition_with_empty_values() {
        let mut c = Condition {
            field: Field::Kind,
            operator: Operator::GreaterThan,
            values: vec![],
        };
        let e = Event::simple_event();
        assert!(!c.allows_event(&e));
        c.operator = Operator::LessThan;
        assert!(!c.allows_event(&e));
        c.operator = Operator::Equals;
        assert!(!c.allows_event(&e));
        // Not Equals applied to an empty list *is* allowed
        // (pointless, but logically valid).
        c.operator = Operator::NotEquals;
        assert!(c.allows_event(&e));
    }

    // Check for condition logic on event w/ single value
    #[test]
    fn condition_kind_gt_event_single() {
        let c = Condition {
            field: Field::Kind,
            operator: Operator::GreaterThan,
            values: vec![10],
        };
        let mut e = Event::simple_event();
        // kind is not greater than 10, not allowed
        e.kind = 1;
        assert!(!c.allows_event(&e));
        // kind is greater than 10, allowed
        e.kind = 100;
        assert!(c.allows_event(&e));
        // kind is 10, not allowed
        e.kind = 10;
        assert!(!c.allows_event(&e));
    }
    // Check for condition logic on event w/ multi values
    #[test]
    fn condition_with_multi_values() {
        let mut c = Condition {
            field: Field::Kind,
            operator: Operator::Equals,
            values: vec![0, 10, 20],
        };
        let mut e = Event::simple_event();
        // Allow if event kind is in list for Equals
        e.kind = 10;
        assert!(c.allows_event(&e));
        // Deny if event kind is not in list for Equals
        e.kind = 11;
        assert!(!c.allows_event(&e));
        // Deny if event kind is in list for NotEquals
        e.kind = 10;
        c.operator = Operator::NotEquals;
        assert!(!c.allows_event(&e));
        // Allow if event kind is not in list for NotEquals
        e.kind = 99;
        c.operator = Operator::NotEquals;
        assert!(c.allows_event(&e));
        // Always deny if GreaterThan/LessThan for a list
        c.operator = Operator::LessThan;
        assert!(!c.allows_event(&e));
        c.operator = Operator::GreaterThan;
        assert!(!c.allows_event(&e));
    }
}
