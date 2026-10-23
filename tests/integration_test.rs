use anyhow::Result;
use futures::SinkExt;
use futures::StreamExt;
use std::thread;
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tracing::info;
mod common;

#[tokio::test]
async fn start_and_stop() -> Result<()> {
    // this will be the common pattern for acquiring a new relay:
    // start a fresh relay, on a port to-be-provided back to us:
    let relay = common::start_relay()?;
    // wait for the relay's webserver to start up and deliver a page:
    common::wait_for_healthy_relay(&relay).await?;
    let port = relay.port;
    // just make sure we can startup and shut down.
    // if we send a shutdown message before the server is listening,
    // we will get a SendError.  Keep sending until someone is
    // listening.
    loop {
        let shutdown_res = relay.shutdown_tx.send(());
        match shutdown_res {
            Ok(()) => {
                break;
            }
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    // wait for relay to shutdown
    let thread_join = relay.handle.join();
    assert!(thread_join.is_ok());
    // assert that port is now available.
    assert!(common::port_is_available(port));
    Ok(())
}

#[tokio::test]
async fn relay_home_page() -> Result<()> {
    // get a relay and wait for startup...
    let relay = common::start_relay()?;
    common::wait_for_healthy_relay(&relay).await?;
    // tell relay to shutdown
    let _res = relay.shutdown_tx.send(());
    Ok(())
}

#[tokio::test]
async fn relay_info_document() -> Result<()> {
    let relay = common::start_relay()?;
    common::wait_for_healthy_relay(&relay).await?;
    let req = hyper::Request::builder()
        .uri(format!("http://127.0.0.1:{}/", relay.port))
        .header("Accept", "application/nostr+json")
        .body(hyper::Body::empty())?;
    let res = hyper::Client::new().request(req).await?;
    assert_eq!(res.status(), hyper::StatusCode::OK);
    // NIP-11: relays MUST accept CORS requests
    let headers = res.headers();
    assert_eq!(headers["content-type"], "application/nostr+json");
    assert_eq!(headers["access-control-allow-origin"], "*");
    assert!(headers.contains_key("access-control-allow-headers"));
    assert!(headers.contains_key("access-control-allow-methods"));
    let body = hyper::body::to_bytes(res.into_body()).await?;
    let info: serde_json::Value = serde_json::from_slice(&body)?;
    assert!(info["supported_nips"].as_array().is_some());
    assert_eq!(info["limitation"]["max_subscriptions"], 32);
    // pay-to-relay is disabled by default
    assert!(info.get("payments_url").is_none());
    assert!(info.get("fees").is_none());
    let _res = relay.shutdown_tx.send(());
    Ok(())
}

//#[tokio::test]
// Still inwork
#[allow(dead_code)]
async fn publish_test() -> Result<()> {
    // get a relay and wait for startup
    let relay = common::start_relay()?;
    common::wait_for_healthy_relay(&relay).await?;
    // open a non-secure websocket connection.
    let (mut ws, _res) = connect_async(format!("ws://localhost:{}", relay.port)).await?;
    // send a simple pre-made message
    let simple_event = r#"["EVENT", {"content": "hello world","created_at": 1691239763,
      "id":"f3ce6798d70e358213ebbeba4886bbdfacf1ecfd4f65ee5323ef5f404de32b86",
      "kind": 1,
      "pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
      "sig": "30ca29e8581eeee75bf838171dec818af5e6de2b74f5337de940f5cc91186534c0b20d6cf7ad1043a2c51dbd60b979447720a471d346322103c83f6cb66e4e98",
      "tags": []}]"#;
    ws.send(simple_event.into()).await?;
    // get response from server, confirm it is an array with first element "OK"
    let event_confirm = ws.next().await;
    ws.close(None).await?;
    info!("event confirmed: {:?}", event_confirm);
    // open a new connection, and wait for some time to get the event.
    let (mut sub_ws, _res) = connect_async(format!("ws://localhost:{}", relay.port)).await?;
    let event_sub = r#"["REQ", "simple", {}]"#;
    sub_ws.send(event_sub.into()).await?;
    // read from subscription
    let _ws_next = sub_ws.next().await;
    let _res = relay.shutdown_tx.send(());
    Ok(())
}
