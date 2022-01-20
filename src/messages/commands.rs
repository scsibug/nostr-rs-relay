use std::str::FromStr;

use crate::error::Result;
use serde::de::Unexpected;
use serde::{ser::SerializeSeq, Deserialize, Serialize};
use serde_json::Value;

use super::event::Event;

/// Close command parsed
#[derive(PartialEq, Debug, Clone)]
pub struct Close {
    /// The subscription identifier being closed.
    pub id: String,
}

impl Serialize for Close {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element("CLOSE")?;
        seq.serialize_element(&self.id)?;
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Close {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let received: Value = Deserialize::deserialize(deserializer)?;

        let items = received.as_array().ok_or(serde::de::Error::invalid_type(
            Unexpected::Other("Close message value"),
            &"a jason array",
        ))?;

        // Check length is always 2
        if items.len() != 2 {
            return Err(serde::de::Error::custom(
                "Invalid length of close message, expected 2",
            ));
        }

        // Try parsing rest of the stuffs
        match &items[0] {
            Value::String(flag) => {
                // Check if flag byte matches
                if flag != "CLOSE" {
                    return Err(serde::de::Error::invalid_type(
                        Unexpected::Other("closing message flag"),
                        &"CLOSE",
                    ));
                } else {
                    // Read the subscription id
                    Ok(Close {
                        id: serde_json::from_value(items[1].clone())
                            .map_err(|e| serde::de::Error::custom(e.to_string()))?,
                    })
                }
            }
            // For any other type raise error
            _ => Err(serde::de::Error::invalid_type(
                Unexpected::Other("flag type in close message"),
                &"a json string",
            )),
        }
    }
}

/// Event command in network format
#[derive(PartialEq, Debug, Clone)]
pub struct EventCmd {
    event: Event,
}

impl Serialize for EventCmd {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element("EVENT")?;
        seq.serialize_element(&self.event)?;
        seq.end()
    }
}

impl<'de> Deserialize<'de> for EventCmd {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let received: Value = Deserialize::deserialize(deserializer)?;

        // Check if json is an array
        let items = received.as_array().ok_or(serde::de::Error::invalid_type(
            Unexpected::Other("json type in event message"),
            &"a json array",
        ))?;

        // Check if message flag matches
        if items[0] != "EVENT" {
            return Err(serde::de::Error::invalid_value(
                Unexpected::Other("message flag"),
                &"EVENT",
            ));
        } else {
            // Try parsing the event
            // Validation of Event is also performed in `Event::from_str()`
            let event = Event::from_str(
                serde_json::to_string(&items[1])
                    .map_err(|e| serde::de::Error::custom(e.to_string()))?
                    .as_ref(),
            )
            .map_err(|e| serde::de::Error::custom(e.to_string()))?;
            Ok(Self { event })
        }
    }
}

impl From<EventCmd> for Event {
    fn from(ec: EventCmd) -> Self {
        ec.event
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn close_command() {
        let close = r#"["CLOSE","subscriptiuon id"]"#;
        let close1: Close = serde_json::from_str(close).unwrap();
        let ser_close1 = serde_json::to_string(&close1).unwrap();
        assert_eq!(ser_close1, close);
    }

    #[test]
    fn event_command() {
        let network_event_cmd = r#"
        [
            "EVENT",
            {
                "id": "5436cab31e64e4f2cbd6216a68d95369210174fa4a82e77d09184aa51806de60",
                "pubkey": "5ac9d737d7f18933967a065b007afe63e03ce9c83bd0da4c491c0810f597d2dd",
                "created_at": 1642540678,
                "kind": 2,
                "tags": [
                [
                    "e",
                    "5419ad84da0a358e474b8d58ed2f00d8ba42097481acf665f444d901a92758aa"
                ],
                [
                    "e",
                    "bdefa1da005259928d0ad0baed9c460945b4b82618c4551de6e95ee09ece25d2"
                ],
                [
                    "p",
                    "96d25b33044b45eee308a8278b99a2a76d26e28ddc1488db4ae7a64fba4750c9"
                ],
                [
                    "p",
                    "859d23503e69157eb3e22bc225687d33b6ab514ae53602e99d7d75b59393e62d"
                ],
                [
                    "e",
                    "4d7ec2e01be6ba5111a0a0ce821639885ea4cb8f4a652accd911dfb7d5151e17"
                ],
                [
                    "p",
                    "cfc5794db955d560b8aec1bd0f27d41d52e1d9d0b157057f7501ea3d30dca034"
                ]
                ],
                "content": "Some String Content",
                "sig": "e947a5c4a65eefd08292e8a8d995fb8b9e43a0f2ddeda57086ddecd0a9f84c2c2b59ae28e92838412268d1e4d091d4d4319661403a4641e18457e24fc7bda0f8"
            }
        ]
        "#;
        let event_cmd: EventCmd = serde_json::from_str(network_event_cmd).unwrap();
        let ser_event_cmd = serde_json::to_string(&event_cmd).unwrap();
        let event_cmd_2: EventCmd = serde_json::from_str(&ser_event_cmd).unwrap();
        assert_eq!(event_cmd, event_cmd_2);
    }
}
