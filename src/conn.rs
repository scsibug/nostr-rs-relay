//! Client connection state
use crate::close::Close;
use crate::error::Error;
use crate::error::Result;

use crate::subscription::Subscription;
use std::collections::HashMap;
use tracing::{debug, trace};
use uuid::Uuid;

/// A subscription identifier has a maximum length
const MAX_SUBSCRIPTION_ID_LEN: usize = 256;

/// State for a client connection
pub struct ClientConn {
    /// Client IP (either from socket, or configured proxy header
    client_ip_addr: String,
    /// Unique client identifier generated at connection time
    client_id: Uuid,
    /// The current set of active client subscriptions
    subscriptions: HashMap<String, Subscription>,
    /// Per-connection maximum concurrent subscriptions
    max_subs: usize,
}

impl Default for ClientConn {
    fn default() -> Self {
        Self::new("unknown".to_owned())
    }
}

impl ClientConn {
    /// Create a new, empty connection state.
    #[must_use]
    pub fn new(client_ip_addr: String) -> Self {
        let client_id = Uuid::new_v4();
        ClientConn {
            client_ip_addr,
            client_id,
            subscriptions: HashMap::new(),
            max_subs: 32,
        }
    }

    #[must_use] pub fn subscriptions(&self) -> &HashMap<String, Subscription> {
        &self.subscriptions
    }

    /// Check if the given subscription already exists
    #[must_use] pub fn has_subscription(&self, sub: &Subscription) -> bool {
        self.subscriptions.values().any(|x| x == sub)
    }

    /// Get a short prefix of the client's unique identifier, suitable
    /// for logging.
    #[must_use]
    pub fn get_client_prefix(&self) -> String {
        self.client_id.to_string().chars().take(8).collect()
    }

    #[must_use]
    pub fn ip(&self) -> &str {
        &self.client_ip_addr
    }

    /// Add a new subscription for this connection.
    /// # Errors
    ///
    /// Will return `Err` if the client has too many subscriptions, or
    /// if the provided name is excessively long.
    pub fn subscribe(&mut self, s: Subscription) -> Result<()> {
        let k = s.get_id();
        let sub_id_len = k.len();
        // prevent arbitrarily long subscription identifiers from
        // being used.
        if sub_id_len > MAX_SUBSCRIPTION_ID_LEN {
            debug!(
                "ignoring sub request with excessive length: ({})",
                sub_id_len
            );
            return Err(Error::SubIdMaxLengthError);
        }
        // check if an existing subscription exists, and replace if so
        if self.subscriptions.contains_key(&k) {
            self.subscriptions.remove(&k);
            self.subscriptions.insert(k, s.clone());
            trace!(
                "replaced existing subscription (cid: {}, sub: {:?})",
                self.get_client_prefix(),
                s.get_id()
            );
            return Ok(());
        }

        // check if there is room for another subscription.
        if self.subscriptions.len() >= self.max_subs {
            return Err(Error::SubMaxExceededError);
        }
        // add subscription
        self.subscriptions.insert(k, s);
        trace!(
            "registered new subscription, currently have {} active subs (cid: {})",
            self.subscriptions.len(),
            self.get_client_prefix(),
        );
        Ok(())
    }

    /// Remove the subscription for this connection.
    pub fn unsubscribe(&mut self, c: &Close) {
        // TODO: return notice if subscription did not exist.
        self.subscriptions.remove(&c.id);
        trace!(
            "removed subscription, currently have {} active subs (cid: {})",
            self.subscriptions.len(),
            self.get_client_prefix(),
        );
    }
}
