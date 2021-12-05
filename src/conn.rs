//use std::collections::HashMap;
use uuid::Uuid;

// state for a client connection
pub struct ClientConn {
    _client_id: Uuid,
    // current set of subscriptions
    //subscriptions: HashMap<String, Subscription>,
    // websocket
    //stream: WebSocketStream<TcpStream>,
    _max_subs: usize,
}

impl ClientConn {
    pub fn new() -> Self {
        let client_id = Uuid::new_v4();
        ClientConn {
            _client_id: client_id,
            _max_subs: 128,
        }
    }
}
