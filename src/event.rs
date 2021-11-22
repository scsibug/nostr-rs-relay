use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use serde_json::Result;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Event {
    id: u32,
    pubkey: u32,
    created_at: u64,
    kind: u8,
    tags: Vec<Vec<String>>,
    content: String,
    sig: u64,
}

// Goals:
// Roundtrip from JSON-string to Event, and back to string.
// Perform validation on an Event to ensure the id and signature are correct.

#[cfg(test)]
mod tests {
    use crate::event::Event;
    use serde_json::Result;
    fn simple_event() -> Event {
        super::Event {
            id: 0,
            pubkey: 0,
            created_at: 0,
            kind: 0,
            tags: vec![],
            content: "".to_owned(),
            sig: 0,
        }
    }

    #[test]
    fn event_creation() {
        // create an event
        let event = simple_event();
        assert_eq!(event.id, 0);
    }

    #[test]
    fn event_serialize() -> Result<()> {
        // serialize an event to JSON string
        let event = simple_event();
        let j = serde_json::to_string(&event)?;
        assert_eq!(j, "{\"id\":0,\"pubkey\":0,\"created_at\":0,\"kind\":0,\"tags\":[],\"content\":\"\",\"sig\":0}");
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
        assert_eq!(j, "{\"id\":0,\"pubkey\":0,\"created_at\":0,\"kind\":0,\"tags\":[[\"e\",\"xxxx\",\"wss://example.com\"],[\"p\",\"yyyyy\",\"wss://example.com:3033\"]],\"content\":\"\",\"sig\":0}");
        Ok(())
    }
}
