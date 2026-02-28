//! NIP-77 negentropy set reconciliation support
use crate::subscription::ReqFilter;
use serde_json::Value;

/// NIP-77 negentropy protocol message
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NegMessage {
    /// NEG-OPEN: initiate negentropy reconciliation
    Open {
        sub_id: String,
        filter: ReqFilter,
        msg_hex: String,
    },
    /// NEG-MSG: continue reconciliation round-trip
    Message {
        sub_id: String,
        msg_hex: String,
    },
    /// NEG-CLOSE: end negentropy session
    Close { sub_id: String },
}

/// Try to parse a JSON string as a NIP-77 negentropy message.
/// Returns `None` if the message is not a negentropy message.
pub fn parse_neg_message(msg: &str) -> Option<NegMessage> {
    let arr: Vec<Value> = serde_json::from_str(msg).ok()?;
    let cmd = arr.first()?.as_str()?;
    match cmd {
        "NEG-OPEN" => {
            let sub_id = arr.get(1)?.as_str()?.to_string();
            let filter_val = arr.get(2)?;
            let filter: ReqFilter = serde_json::from_value(filter_val.clone()).ok()?;
            let msg_hex = arr.get(3)?.as_str()?.to_string();
            Some(NegMessage::Open {
                sub_id,
                filter,
                msg_hex,
            })
        }
        "NEG-MSG" => {
            let sub_id = arr.get(1)?.as_str()?.to_string();
            let msg_hex = arr.get(2)?.as_str()?.to_string();
            Some(NegMessage::Message { sub_id, msg_hex })
        }
        "NEG-CLOSE" => {
            let sub_id = arr.get(1)?.as_str()?.to_string();
            Some(NegMessage::Close { sub_id })
        }
        _ => None,
    }
}

/// Format a NEG-MSG response as a JSON string
pub fn make_neg_msg(sub_id: &str, hex_payload: &str) -> String {
    serde_json::json!(["NEG-MSG", sub_id, hex_payload]).to_string()
}

/// Format a NEG-ERR response as a JSON string
pub fn make_neg_err(sub_id: &str, reason: &str) -> String {
    serde_json::json!(["NEG-ERR", sub_id, reason]).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_neg_open() {
        let msg = r#"["NEG-OPEN","sub1",{"kinds":[1]},"6100000200"]"#;
        match parse_neg_message(msg) {
            Some(NegMessage::Open {
                sub_id,
                filter,
                msg_hex,
            }) => {
                assert_eq!(sub_id, "sub1");
                assert_eq!(filter.kinds, Some(vec![1]));
                assert_eq!(msg_hex, "6100000200");
            }
            _ => panic!("expected NegMessage::Open"),
        }
    }

    #[test]
    fn parse_neg_msg() {
        let msg = r#"["NEG-MSG","sub1","abcdef"]"#;
        match parse_neg_message(msg) {
            Some(NegMessage::Message { sub_id, msg_hex }) => {
                assert_eq!(sub_id, "sub1");
                assert_eq!(msg_hex, "abcdef");
            }
            _ => panic!("expected NegMessage::Message"),
        }
    }

    #[test]
    fn parse_neg_close() {
        let msg = r#"["NEG-CLOSE","sub1"]"#;
        match parse_neg_message(msg) {
            Some(NegMessage::Close { sub_id }) => {
                assert_eq!(sub_id, "sub1");
            }
            _ => panic!("expected NegMessage::Close"),
        }
    }

    #[test]
    fn parse_non_neg_message() {
        let msg = r#"["REQ","sub1",{}]"#;
        assert!(parse_neg_message(msg).is_none());
    }

    #[test]
    fn parse_neg_open_missing_fields() {
        let msg = r#"["NEG-OPEN"]"#;
        assert!(parse_neg_message(msg).is_none());
    }

    #[test]
    fn format_neg_msg() {
        let result = make_neg_msg("sub1", "6100");
        let arr: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(arr[0], "NEG-MSG");
        assert_eq!(arr[1], "sub1");
        assert_eq!(arr[2], "6100");
    }

    #[test]
    fn format_neg_err() {
        let result = make_neg_err("sub1", "CLOSED: too many");
        let arr: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(arr[0], "NEG-ERR");
        assert_eq!(arr[1], "sub1");
        assert_eq!(arr[2], "CLOSED: too many");
    }

    #[test]
    fn neg_err_blocked_reason_format() {
        // strfry-compatible "blocked:" prefix for oversized result sets
        let result = make_neg_err("neg1", "blocked: too many query results");
        let arr: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(arr[2], "blocked: too many query results");
        assert!(arr[2].as_str().unwrap().starts_with("blocked:"));
    }

    #[test]
    fn neg_err_protocol_error_format() {
        let result = make_neg_err("neg1", "PROTOCOL-ERROR: invalid hex");
        let arr: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert!(arr[2].as_str().unwrap().starts_with("PROTOCOL-ERROR:"));
    }

    #[test]
    fn neg_open_reopen_same_sub_id_parses() {
        // Two NEG-OPEN with same sub_id should both parse successfully;
        // server must close existing session before processing the new one
        let msg1 = r#"["NEG-OPEN","same-id",{"kinds":[1]},"6100000200"]"#;
        let msg2 = r#"["NEG-OPEN","same-id",{"kinds":[0]},"6100000200"]"#;
        let parsed1 = parse_neg_message(msg1);
        let parsed2 = parse_neg_message(msg2);
        assert!(parsed1.is_some());
        assert!(parsed2.is_some());
        // They should parse as different filters
        if let (
            Some(NegMessage::Open { filter: f1, .. }),
            Some(NegMessage::Open { filter: f2, .. }),
        ) = (&parsed1, &parsed2) {
            assert_ne!(f1, f2);
        }
    }

    #[test]
    fn neg_open_strips_limit_from_filter() {
        // NEG-OPEN with a limit in the filter should parse; server strips limit
        let msg = r#"["NEG-OPEN","sub1",{"kinds":[1],"limit":100},"6100000200"]"#;
        match parse_neg_message(msg) {
            Some(NegMessage::Open { filter, .. }) => {
                // Parser preserves the limit; server is responsible for stripping it
                assert_eq!(filter.limit, Some(100));
            }
            _ => panic!("expected NegMessage::Open"),
        }
    }

    #[test]
    fn neg_session_lifecycle_messages_parse() {
        // Full lifecycle: OPEN -> MSG -> MSG -> CLOSE
        let open = r#"["NEG-OPEN","lifecycle",{"kinds":[1]},"6100000200"]"#;
        let msg1 = r#"["NEG-MSG","lifecycle","abcdef0123"]"#;
        let msg2 = r#"["NEG-MSG","lifecycle","456789"]"#;
        let close = r#"["NEG-CLOSE","lifecycle"]"#;

        assert!(matches!(parse_neg_message(open), Some(NegMessage::Open { .. })));
        assert!(matches!(parse_neg_message(msg1), Some(NegMessage::Message { .. })));
        assert!(matches!(parse_neg_message(msg2), Some(NegMessage::Message { .. })));
        assert!(matches!(parse_neg_message(close), Some(NegMessage::Close { .. })));
    }

    #[test]
    fn neg_msg_without_open_parses() {
        // NEG-MSG for a non-existent session should still parse;
        // server is responsible for returning NEG-ERR "CLOSED: session not found"
        let msg = r#"["NEG-MSG","no-such-session","abcdef"]"#;
        assert!(matches!(parse_neg_message(msg), Some(NegMessage::Message { .. })));
    }

    #[test]
    fn neg_open_empty_filter() {
        // Empty filter (match all events) should parse
        let msg = r#"["NEG-OPEN","sub1",{},"6100000200"]"#;
        assert!(matches!(parse_neg_message(msg), Some(NegMessage::Open { .. })));
    }

    #[test]
    fn neg_open_invalid_filter_rejects() {
        // Invalid filter (not an object) should fail to parse
        let msg = r#"["NEG-OPEN","sub1","not-a-filter","6100000200"]"#;
        assert!(parse_neg_message(msg).is_none());
    }

    #[test]
    fn neg_close_extra_fields_parses() {
        // NEG-CLOSE with extra fields should still parse (only first 2 elements matter)
        let msg = r#"["NEG-CLOSE","sub1","extra-ignored"]"#;
        assert!(matches!(parse_neg_message(msg), Some(NegMessage::Close { .. })));
    }
}
