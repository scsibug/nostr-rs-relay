use crate::close::Close;
use crate::error::{Error, Result};
use crate::event::Event;
use crate::subscription::Subscription;
use log::{debug, info};
use std::collections::HashMap;
use uuid::Uuid;

// A protocol handler/helper.  Use one per client.
pub struct Proto {
    client_id: Uuid,
    // current set of subscriptions
    subscriptions: HashMap<String, Subscription>,
    max_subs: usize,
}

const MAX_SUBSCRIPTION_ID_LEN: usize = 256;

impl Proto {
    pub fn new() -> Self {
        let p = Proto {
            client_id: Uuid::new_v4(),
            subscriptions: HashMap::new(),
            max_subs: 128,
        };
        debug!("New client: {:?}", p.client_id);
        p
    }

    // TODO: figure out NOTICE handling for errors here
    pub fn process_message(&mut self, cmd: String) -> Result<()> {
        info!(
            "Processing message in proto for client: {:?}",
            self.client_id
        );
        let message = parse_cmd(cmd)?;
        info!("Parsed message: {:?}", message);
        match message {
            NostrRequest::EvReq(_) => {}
            NostrRequest::SubReq(sub) => self.subscribe(sub),
            NostrRequest::CloseReq(close) => self.unsubscribe(close),
        };
        Ok(())
    }

    pub fn subscribe(&mut self, s: Subscription) {
        // TODO: add NOTICE responses for error conditions.  At the
        // moment, we are silently dropping subscription requests that
        // aren't perfect.

        // check if the subscription key is reasonable.
        let k = s.get_id();
        let sub_id_len = k.len();
        if sub_id_len > MAX_SUBSCRIPTION_ID_LEN {
            info!("Dropping subscription with huge ({}) length", sub_id_len);
            return;
        }
        // check if an existing subscription exists.
        if self.subscriptions.contains_key(&k) {
            info!("Client requested a subscription with an already-existing key");
            return;
        }

        // check if there is room for another subscription.
        if self.subscriptions.len() >= self.max_subs {
            info!("Client has reached the maximum number of unique subscriptions");
            return;
        }
        // add subscription
        self.subscriptions.insert(k, s);
        info!(
            "Registered new subscription, currently have {} active subs",
            self.subscriptions.len()
        );
    }

    pub fn unsubscribe(&mut self, c: Close) {
        self.subscriptions.remove(&c.get_id());
        info!(
            "Removed subscription, currently have {} active subs",
            self.subscriptions.len()
        );
    }
}

// A raw message with the expected type
#[derive(PartialEq, Debug)]
pub enum NostrRawMessage {
    EvRaw(String),
    SubRaw(String),
    CloseRaw(String),
}

// A fully parsed request
#[derive(PartialEq, Debug)]
pub enum NostrRequest {
    EvReq(Event),
    SubReq(Subscription),
    CloseReq(Close),
}

// Wrap the message in the expected request type
fn msg_type_wrapper(msg: String) -> Result<NostrRawMessage> {
    // check prefix.
    if msg.starts_with(r#"["EVENT","#) {
        Ok(NostrRawMessage::EvRaw(msg))
    } else if msg.starts_with(r#"["REQ","#) {
        Ok(NostrRawMessage::SubRaw(msg))
    } else if msg.starts_with(r#"["CLOSE","#) {
        Ok(NostrRawMessage::CloseRaw(msg))
    } else {
        Err(Error::CommandNotFound)
    }
}

pub fn parse_cmd(msg: String) -> Result<NostrRequest> {
    // turn this raw string into a parsed request
    let typ = msg_type_wrapper(msg)?;
    match typ {
        NostrRawMessage::EvRaw(_) => Err(Error::EventParseFailed),
        NostrRawMessage::SubRaw(m) => Ok(NostrRequest::SubReq(Subscription::parse(&m)?)),
        NostrRawMessage::CloseRaw(m) => Ok(NostrRequest::CloseReq(Close::parse(&m)?)),
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
