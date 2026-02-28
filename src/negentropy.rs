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
