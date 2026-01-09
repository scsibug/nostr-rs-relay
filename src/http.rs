use std::{collections::HashMap, sync::Arc};

use hyper::{body::to_bytes, Body, Method, Request};
use log::debug;
use tokio::sync::{mpsc, oneshot};
use tracing::error;

use crate::{
    config::Settings,
    db::{self, SubmittedEvent},
    event::EventWrapper,
    notice::Notice,
    repo::NostrRepo,
    server::{convert_to_msg, NostrMessage},
};

/// Event types that are allowed to be sent or requested via REST
const ALLOWED_EVENT_TYPES: &[u64; 3] = &[17, 57, 1059];

fn parse_query_params(query_string: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();

    if query_string.is_empty() {
        return params;
    }

    for pair in query_string.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            params.insert(key.to_string(), value.to_string());
        }
    }

    params
}

// TODO: more granular error handling
pub(crate) async fn handle_request(
    request: Request<Body>,
    repo: Arc<dyn NostrRepo>,
    event_tx: mpsc::Sender<SubmittedEvent>,
    config: &Settings,
) -> anyhow::Result<String> {
    let method = request.method().clone();
    match method {
        Method::GET => {
            let query_params = parse_query_params(request.uri().query().unwrap_or_default());
            let message = query_params
                .get("filter")
                .ok_or(anyhow::anyhow!("Filter not found"))?;
            if message.len() > config.ohttp.max_request_bytes {
                return Err(anyhow::anyhow!(
                    "Message too large, max: {}, got: {}",
                    config.ohttp.max_request_bytes,
                    message.len()
                ));
            }
            let message_string = String::from_utf8(hex::decode(message)?)?;
            let nostr_message = convert_to_msg(&message_string, None)?;
            match nostr_message {
                NostrMessage::SubMsg(sub) => {
                    // Create a channel for query results
                    let (query_tx, mut query_rx) = mpsc::channel::<db::QueryResult>(1000);
                    // Create a channel to abandon the query if needed. Currently not used.
                    let (_abandon_query_tx, abandon_query_rx) = oneshot::channel::<()>();

                    let repo_clone = repo.clone();
                    let sub_clone = sub.clone();
                    tokio::spawn(async move {
                        if let Err(e) = repo_clone
                            .query_subscription(
                                sub_clone,
                                "ohttp_client".to_string(),
                                query_tx,
                                abandon_query_rx,
                            )
                            .await
                        {
                            error!("OHTTP subscription query error: {:?}", e);
                        }
                    });
                    let mut events = Vec::new();
                    let timeout = tokio::time::Duration::from_secs(10);

                    loop {
                        tokio::select! {
                            // Receive query results
                            result = query_rx.recv() => {
                                match result {
                                    Some(query_result) => {
                                        // Add event to our response
                                        events.push(query_result.event);
                                    }
                                    None => {
                                        // Channel closed, query finished
                                        break;
                                    }
                                }
                            }
                            _ = tokio::time::sleep(timeout) => {
                                error!("Subscription timeout reached");
                                break;
                            }
                        }
                    }
                    // Send response as newline-delimited json
                    let response_data = events.join("\n");
                    Ok(response_data.to_string())
                }
                _ => {
                    Err(anyhow::anyhow!(
                        "Expected a subscription message for GET request"
                    ))
                }
            }
        }
        Method::POST => {
            let body = to_bytes(request.into_body()).await?;
            if body.len() > config.ohttp.max_request_bytes {
                return Err(anyhow::anyhow!(
                    "Request body too large, max: {}, got: {}",
                    config.ohttp.max_request_bytes,
                    body.len()
                ));
            }
            let body = String::from_utf8(body.to_vec())?;
            let nostr_message = convert_to_msg(&body, None)?;

            // posting an event, expecting an event event
            match nostr_message {
                NostrMessage::EventMsg(event_command) => {
                    let parsed: crate::error::Result<EventWrapper> = event_command.into();
                    match parsed {
                        Ok(EventWrapper::WrappedEvent(e)) => {
                            // Create a notice channel for OHTTP responses
                            let (notice_tx, mut notice_rx) =
                                tokio::sync::mpsc::channel::<Notice>(1);

                            let submit_event = SubmittedEvent {
                                event: e.clone(),
                                notice_tx,
                                source_ip: "ohttp".to_string(),
                                origin: None,
                                user_agent: None,
                                auth_pubkey: None,
                            };

                            if !ALLOWED_EVENT_TYPES.contains(&e.kind) {
                                return Err(anyhow::anyhow!("Event type not allowed"));
                            }

                            // Send to database writer
                            if let Err(e) = event_tx.send(submit_event).await {
                                return Err(anyhow::anyhow!(
                                    "Failed to send event to database: {:?}",
                                    e
                                ));
                            }

                            // Wait for processing result and log any notices
                            if let Some(notice) = notice_rx.recv().await {
                                if let Notice::Message(msg) = notice {
                                    debug!("Event processing message: {}", msg)
                                }
                                Ok(String::new())
                            } else {
                                Err(anyhow::anyhow!("Failed to send event to database"))
                            }
                        }
                        Ok(EventWrapper::WrappedAuth(_)) => {
                            Err(anyhow::anyhow!("AUTH events are not supported"))
                        }
                        Err(e) => {
                            Err(anyhow::anyhow!("Invalid event: {:?}", e))
                        }
                    }
                }

                _ => {
                    Err(anyhow::anyhow!(
                        "Expected an event message for POST request"
                    ))
                }
            }
        }
        _ => {
            Err(anyhow::anyhow!("Unsupported HTTP method"))
        }
    }
}
