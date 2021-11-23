use serde::{Deserialize, Deserializer, Serialize};
//use serde_json::json;
//use serde_json::Result;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct Event {
    #[serde(deserialize_with = "u32_from_string")]
    id: u32,
    #[serde(deserialize_with = "u32_from_string")]
    pubkey: u32,
    created_at: u64,
    kind: u8,
    #[serde(deserialize_with = "tag_from_string")]
    tags: Vec<Vec<String>>,
    content: String,
    #[serde(deserialize_with = "u64_from_string")]
    sig: u64,
}

type Tag = Vec<Vec<String>>;

// handle a default value (empty vec) for null tags
fn tag_from_string<'de, D>(deserializer: D) -> Result<Tag, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(|| vec![]))
}

fn u32_from_string<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let _s: String = Deserialize::deserialize(deserializer)?;
    Ok(0)
}

fn u64_from_string<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let _s: String = Deserialize::deserialize(deserializer)?;
    Ok(0)
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

    #[test]
    fn event_deserialize() -> Result<()> {
        let raw_json = r#"{"id":"1384757da583e6129ce831c3d7afc775a33a090578f888dd0d010328ad047d0c","pubkey":"bbbd9711d357df4f4e498841fd796535c95c8e751fa35355008a911c41265fca","created_at":1612650459,"kind":1,"tags":null,"content":"hello world","sig":"59d0cc47ab566e81f72fe5f430bcfb9b3c688cb0093d1e6daa49201c00d28ecc3651468b7938642869ed98c0f1b262998e49a05a6ed056c0d92b193f4e93bc21"}"#;

        // id: 1384757da583e6129ce831c3d7afc775a33a090578f888dd0d010328ad047d0c
        // pubkey: bbbd9711d357df4f4e498841fd796535c95c8e751fa35355008a911c41265fca",
        // created_at: 1612650459
        // kind :1,
        // tags":null,
        // "content":"hello world",
        // "sig":"59d0cc47ab566e81f72fe5f430bcfb9b3c688cb0093d1e6daa49201c00d28ecc3651468b7938642869ed98c0f1b262998e49a05a6ed056c0d92b193f4e93bc21"}]"#;

        let e: Event = serde_json::from_str(raw_json)?;
        // assert that the kind is 1
        assert_eq!(e.kind, 1);
        assert_eq!(e.tags.len(), 0);
        Ok(())
    }
}
