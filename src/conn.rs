use crate::close::Close;
use crate::error::Result;
use crate::subscription::Subscription;
use log::*;
use std::collections::HashMap;
use uuid::Uuid;

// subscription identifiers must be reasonably sized.
const MAX_SUBSCRIPTION_ID_LEN: usize = 256;

// state for a client connection
pub struct ClientConn {
    _client_id: Uuid,
    // current set of subscriptions
    subscriptions: HashMap<String, Subscription>,
    // websocket
    //stream: WebSocketStream<TcpStream>,
    max_subs: usize,
}

impl ClientConn {
    pub fn new() -> Self {
        let client_id = Uuid::new_v4();
        ClientConn {
            _client_id: client_id,
            subscriptions: HashMap::new(),
            max_subs: 128,
        }
    }

    pub fn subscribe(&mut self, s: Subscription) -> Result<()> {
        let k = s.get_id();
        let sub_id_len = k.len();
        if sub_id_len > MAX_SUBSCRIPTION_ID_LEN {
            info!("Dropping subscription with huge ({}) length", sub_id_len);
            return Ok(());
        }
        // check if an existing subscription exists, and replace if so
        if self.subscriptions.contains_key(&k) {
            self.subscriptions.remove(&k);
            self.subscriptions.insert(k, s);
            info!("Replaced existing subscription");
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

    pub fn unsubscribe(&mut self, c: Close) {
        // TODO: return notice if subscription did not exist.
        self.subscriptions.remove(&c.get_id());
        info!(
            "Removed subscription, currently have {} active subs",
            self.subscriptions.len()
        );
    }
}
