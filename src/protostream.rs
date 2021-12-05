use crate::error::{Error, Result};
use crate::event::EventCmd;
use crate::subscription::Subscription;
use core::pin::Pin;
use futures::sink::Sink;
use futures::stream::Stream;
use futures::task::Context;
use futures::task::Poll;
use log::*;
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::WebSocketStream;
use tungstenite::error::Error as WsError;
use tungstenite::protocol::Message;

// A Nostr message is either event, subscription, or close.
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
#[serde(untagged)]
pub enum NostrMessage {
    EventMsg(EventCmd),
    SubMsg(Subscription),
    CloseMsg,
}

// Either an event w/ subscription, or a notice
#[derive(Deserialize, Serialize, Clone, PartialEq, Debug)]
enum NostrResponse {
    Notice(String),
}

// A Nostr protocol stream is layered on top of a Websocket stream.
pub struct NostrStream {
    ws_stream: WebSocketStream<TcpStream>,
}

// given a websocket, return a protocol stream
//impl Stream<Item = Result<BasicMessage, BasicError>> + Sink<BasicResponse>
pub fn wrap_ws_in_nostr(ws: WebSocketStream<TcpStream>) -> NostrStream {
    return NostrStream { ws_stream: ws };
}

impl Stream for NostrStream {
    type Item = Result<NostrMessage>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        // convert Message to NostrMessage
        fn convert(msg: String) -> Result<NostrMessage> {
            debug!("Input message: {}", &msg);
            let parsed_res: Result<NostrMessage> = serde_json::from_str(&msg).map_err(|e| e.into());
            match parsed_res {
                Ok(m) => Ok(m),
                Err(e) => {
                    debug!("Proto parse error: {:?}", e);
                    Err(Error::ProtoParseError)
                }
            }
        }

        match Pin::new(&mut self.ws_stream).poll_next(cx) {
            Poll::Pending => Poll::Pending,         // not ready
            Poll::Ready(None) => Poll::Ready(None), // done
            Poll::Ready(Some(v)) => match v {
                Ok(Message::Text(vs)) => Poll::Ready(Some(convert(vs))), // convert message->basicmessage
                Ok(Message::Binary(_)) => Poll::Ready(Some(Err(Error::ProtoParseError))),
                Ok(Message::Pong(_)) | Ok(Message::Ping(_)) => Poll::Pending,
                Ok(Message::Close(_)) => Poll::Ready(None),
                Err(WsError::AlreadyClosed) | Err(WsError::ConnectionClosed) => Poll::Ready(None), // done
                Err(_) => Poll::Ready(Some(Err(Error::ConnError))),
            },
        }
    }
}

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
        let res_message = serde_json::to_string(&item).expect("Could convert message to string");
        match Pin::new(&mut self.ws_stream).start_send(Message::Text(res_message)) {
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
