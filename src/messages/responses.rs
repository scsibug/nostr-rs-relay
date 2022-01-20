use super::event::Event;
use serde::ser::SerializeSeq;
use serde::Serialize;

/// An Event Response message sent by the relay to client
#[derive(Debug, PartialEq, Clone)]
pub struct EventResp {
    subscription_id: String,
    event: Event,
}

impl Serialize for EventResp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element("EVENT")?;
        seq.serialize_element(&self.subscription_id)?;
        seq.serialize_element(&self.event)?;
        seq.end()
    }
}

impl EventResp {
    /// Create new Event response
    pub fn new(subscription_id: &str, event: &Event) -> Self {
        Self {
            subscription_id: subscription_id.to_owned(),
            event: event.clone(),
        }
    }
}

/// A Notice Response Message send to client from relay
#[derive(Debug, PartialEq, Clone)]
pub struct NoticeResp {
    message: String,
}

impl Serialize for NoticeResp {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let mut seq = serializer.serialize_seq(None)?;
        seq.serialize_element("NOTICE")?;
        seq.serialize_element(&self.message)?;
        seq.end()
    }
}

impl NoticeResp {
    /// Create new notice response
    pub fn new(message: &str) -> Self {
        Self {
            message: message.to_owned(),
        }
    }
}
