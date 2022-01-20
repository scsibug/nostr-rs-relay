//! Nostr protocol layered over WebSocket
use crate::config;
use crate::error::{Error, Result};
use crate::messages::{Close, Event, EventCmd, Subscription};
use core::pin::Pin;
use futures::sink::Sink;
use futures::stream::Stream;
use futures::task::Context;
use futures::task::Poll;
use hyper::upgrade::Upgraded;
use log::*;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::WebSocketStream;
use tungstenite::error::Error as WsError;
use tungstenite::protocol::Message;

use super::messages::{EventResp, NoticeResp};

/// Nostr protocol messages from a client
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
#[serde(untagged)]
pub enum NostrMessage {
    /// An `EVENT` message
    Event(EventCmd),
    /// A `REQ` message
    Req(Subscription),
    /// A `CLOSE` message
    Close(Close),
}

/// Nostr protocol messages from a relay/server
#[derive(Serialize, Clone, PartialEq, Debug)]
#[serde(untagged)]
pub enum NostrResponse {
    /// A `NOTICE` response
    Notice(NoticeResp),
    /// An `EVENT` response, composed of the subscription identifier,
    /// and serialized event JSON
    Event(EventResp),
}

impl NostrResponse {
    pub fn new_notice(msg: &str) -> Self {
        Self::Notice(NoticeResp::new(msg))
    }

    pub fn new_event(subs_id: &str, event: &Event) -> Self {
        Self::Event(EventResp::new(subs_id, event))
    }
}

/// A Nostr protocol stream is layered on top of a Websocket stream.
pub struct NostrStream {
    ws_stream: WebSocketStream<Upgraded>,
}

/// Given a websocket, return a protocol stream wrapper.
pub fn wrap_ws_in_nostr(ws: WebSocketStream<Upgraded>) -> NostrStream {
    NostrStream { ws_stream: ws }
}

/// Implement the [`Stream`] interface to produce Nostr messages.
impl Stream for NostrStream {
    type Item = Result<NostrMessage>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // get the configuration
        /// Convert Message to NostrMessage
        fn convert(msg: String) -> Result<NostrMessage> {
            let config = config::SETTINGS.read().unwrap();
            let parsed_res: Result<NostrMessage> = serde_json::from_str(&msg).map_err(|e| e.into());
            match parsed_res {
                Ok(m) => {
                    if let NostrMessage::Event(_) = m {
                        if let Some(max_size) = config.limits.max_event_bytes {
                            // check length, ensure that some max size is set.
                            if msg.len() > max_size && max_size > 0 {
                                return Err(Error::EventMaxLengthError(msg.len()));
                            }
                        }
                    }
                    Ok(m)
                }
                Err(e) => {
                    debug!("proto parse error: {:?}", e);
                    debug!("parse error on message: {}", msg.trim());
                    Err(Error::ProtoParseError)
                }
            }
        }
        match Pin::new(&mut self.ws_stream).poll_next(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(v)) => match v {
                Ok(Message::Text(vs)) => Poll::Ready(Some(convert(vs))),
                Ok(Message::Binary(_)) => Poll::Ready(Some(Err(Error::ProtoParseError))),
                Ok(Message::Pong(_)) | Ok(Message::Ping(_)) => Poll::Pending,
                Ok(Message::Close(_)) => Poll::Ready(None),
                Err(WsError::AlreadyClosed) | Err(WsError::ConnectionClosed) => Poll::Ready(None),
                Err(_) => Poll::Ready(Some(Err(Error::ConnError))),
            },
        }
    }
}

/// Implement the [`Sink`] interface to produce Nostr responses.
impl Sink<NostrResponse> for NostrStream {
    type Error = Error;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        // map the error type
        match Pin::new(&mut self.ws_stream).poll_ready(cx) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(())),
            Poll::Ready(Err(_)) => Poll::Ready(Err(Error::ConnWriteError)),
            Poll::Pending => Poll::Pending,
        }
    }

    fn start_send(mut self: Pin<&mut Self>, item: NostrResponse) -> Result<(), Self::Error> {
        // TODO: do real escaping for these - at least on NOTICE,
        // which surely has some problems if arbitrary text is sent.
        let send_str = serde_json::to_string(&item)?;
        match Pin::new(&mut self.ws_stream).start_send(Message::Text(send_str)) {
            Ok(()) => Ok(()),
            Err(_) => Err(Error::ConnWriteError),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}
