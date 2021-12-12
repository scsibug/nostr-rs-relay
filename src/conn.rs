//! Client connection state
use crate::close::Close;
use crate::error::Result;
use crate::event::Event;
use crate::subscription::Subscription;
use log::*;
use std::collections::HashMap;
use uuid::Uuid;

/// A subscription identifier has a maximum length
const MAX_SUBSCRIPTION_ID_LEN: usize = 256;

/// State for a client connection
pub struct ClientConn {
    /// Unique client identifier generated at connection time
    client_id: Uuid,
    /// The current set of active client subscriptions
    subscriptions: HashMap<String, Subscription>,
    /// Per-connection maximum concurrent subscriptions
    max_subs: usize,
}

impl ClientConn {
    /// Create a new, empty connection state.
    pub fn new() -> Self {
        let client_id = Uuid::new_v4();
        ClientConn {
            client_id,
            subscriptions: HashMap::new(),
            max_subs: 128,
        }
    }

    /// Get a short prefix of the client's unique identifier, suitable
    /// for logging.
    pub fn get_client_prefix(&self) -> String {
        self.client_id.to_string().chars().take(8).collect()
    }

    /// Find the first subscription identifier that matches the event,
    /// if any do.
    pub fn get_matching_subscription(&self, e: &Event) -> Option<&str> {
        for (id, sub) in self.subscriptions.iter() {
            if sub.interested_in_event(e) {
                return Some(id);
            }
        }
        None
    }

    /// Add a new subscription for this connection.
    pub fn subscribe(&mut self, s: Subscription) -> Result<()> {
        let k = s.get_id();
        let sub_id_len = k.len();
        // prevent arbitrarily long subscription identifiers from
        // being used.
        if sub_id_len > MAX_SUBSCRIPTION_ID_LEN {
            info!("Dropping subscription with huge ({}) length", sub_id_len);
            return Ok(());
        }
        // check if an existing subscription exists, and replace if so
        if self.subscriptions.contains_key(&k) {
            self.subscriptions.remove(&k);
            self.subscriptions.insert(k, s);
            debug!("Replaced existing subscription");
            return Ok(());
        }

        // check if there is room for another subscription.
        if self.subscriptions.len() >= self.max_subs {
            info!("Client has reached the maximum number of unique subscriptions");
            return Ok(());
        }
        // add subscription
        self.subscriptions.insert(k, s);
        info!(
            "Registered new subscription, currently have {} active subs",
            self.subscriptions.len()
        );
        return Ok(());
    }

    /// Remove the subscription for this connection.
    pub fn unsubscribe(&mut self, c: Close) {
        // TODO: return notice if subscription did not exist.
        self.subscriptions.remove(&c.id);
        info!(
            "Removed subscription, currently have {} active subs",
            self.subscriptions.len()
        );
    }
}
