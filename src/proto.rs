use crate::error::{Error, Result};
use crate::{event, request};
use log::{debug, info};
use uuid::Uuid;

// A protocol handler/helper.  Use one per client.
pub struct Proto {
    client_id: Uuid,
}

impl Proto {
    pub fn new() -> Self {
        let p = Proto {
            client_id: Uuid::new_v4(),
        };
        debug!("New client: {:?}", p.client_id);
        p
    }

    pub fn process_message(self: &Self, cmd: String) {
        info!(
            "Processing message in proto for client: {:?}",
            self.client_id
        );
        // check what kind of message
        info!("Parse result: {:?}", parse_type(cmd));
    }
}

// A raw message with the expected type
#[derive(PartialEq, Debug)]
pub enum NostrRawMessage {
    Event(String),
    Req(String),
    Close(String),
}

// A fully parsed request
#[derive(PartialEq, Debug)]
pub enum NostrRequest {
    Event(event::Event),
    Subscription(request::Subscription),
}

// Wrap the message in the expected request type
fn msg_type_wrapper(msg: String) -> Result<NostrRawMessage> {
    // check prefix.
    if msg.starts_with(r#"["EVENT","#) {
        Ok(NostrRawMessage::Event(msg))
    } else if msg.starts_with(r#"["REQ","#) {
        Ok(NostrRawMessage::Req(msg))
    } else if msg.starts_with(r#"["CLOSE","#) {
        Ok(NostrRawMessage::Close(msg))
    } else {
        Err(Error::CommandNotFound)
    }
}

pub fn parse_type(msg: String) -> Result<NostrRequest> {
    // turn this raw string into a parsed request
    let typ = msg_type_wrapper(msg)?;
    match typ {
        NostrRawMessage::Event(_) => Err(Error::EventParseFailed),
        NostrRawMessage::Req(m) => {
            let s = request::Subscription::parse(&m)?;
            Ok(NostrRequest::Subscription(s))
        }
        NostrRawMessage::Close(_) => Err(Error::CloseParseFailed),
    }
}

// Parse the request into a fully deserialized type

// The protocol-handling process looks something like:
// Receive a message (bytes).
// Determine the type.  We could do this with an untagged deserialization in serde.  Or we can peek at the prefix.
// Wrap the message string in the client request type (either Event, Req, Close)
// For Req/Close, we can fully parse these.

// For Event, we want to be more cautious.
// Before we admit an event, we should reject any duplicates.
// duplicates in the datastore will have already been sent out to interested subscribers.
// No point in verifying an event that we already have.

// Event pipeline looks like:
// Get message. (./)
// Verify it is an event. (./)
// Parse into string / number components from JSON.
// Perform validation, re-serialize (or can we re-use the original?)
// Publish to subscribers.
// Push to datastore.
