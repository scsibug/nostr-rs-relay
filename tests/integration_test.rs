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

//#[tokio::test]
// Still inwork
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

#[tokio::test]
async fn nip_114_flow_test() -> Result<()> {
    // Start a relay and wait for startup
    let relay = common::start_relay()?;
    common::wait_for_healthy_relay(&relay).await?;

    // Open a WebSocket connection to the relay
    let (mut ws, _res) = connect_async(format!("ws://localhost:{}", relay.port)).await?;

    let event_id = "f3ce6798d70e358213ebbeba4886bbdfacf1ecfd4f65ee5323ef5f404de32b86";

    // send a simple pre-made message
    let simple_event = r#"["EVENT", {"content": "hello world","created_at": 1691239763,
      "id":"f3ce6798d70e358213ebbeba4886bbdfacf1ecfd4f65ee5323ef5f404de32b86",
      "kind": 1,
      "pubkey": "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
      "sig": "30ca29e8581eeee75bf838171dec818af5e6de2b74f5337de940f5cc91186534c0b20d6cf7ad1043a2c51dbd60b979447720a471d346322103c83f6cb66e4e98",
      "tags": []}]"#;
    ws.send(simple_event.into()).await?;

    // wait a sec so the event is saved
    // but with this the event is persisted first, and response never sent?
    // thread::sleep(Duration::from_millis(1000));

    // Send a subscription request with ids_only set to true
    let ids_request = r#"["REQ", "sub1", {"kinds": [1], "ids_only": true}]"#; // Example filter
    ws.send(ids_request.into()).await?;


    let mut message_count = 0;
    // loop until we receive a message string that contains the event_id. no parsing yet, just quick check
    let mut have_msg;
    loop {
        have_msg = ws.next().await
            .ok_or_else(|| anyhow::Error::msg("Did not receive a response message"))??;
        info!("Received {:?}", have_msg);
        if have_msg.to_text()?.contains("HAVE") {
            break;
        }
        message_count += 1;
        if message_count > 5 {
            panic!("Did not receive a HAVE message");
        }
    }

    // Assuming the "HAVE" message is in the format ["HAVE", "sub1", ["event_id1", ...]]
    // Parse the "HAVE" message
    let have_msg_json: serde_json::Value = serde_json::from_str(&have_msg.to_text()?)?;
    assert!(have_msg_json.is_array(), "Response is not an array");
    let have_msg_array = have_msg_json.as_array().unwrap();

    // Check if "HAVE" message content is correct
    assert_eq!(have_msg_array[0].as_str().unwrap(), "HAVE", "Response does not start with 'HAVE'");
    assert_eq!(have_msg_array[1].as_str().unwrap(), "sub1", "HAVE message does not contain the correct subscription ID");
    let received_event_id = have_msg_array[2].as_str().unwrap();
    assert_eq!(received_event_id, event_id, "HAVE message does not contain the correct event ID");
    assert_eq!(have_msg_array.len(), 3, "HAVE message does not have 3 elements");

    // Request full event data for specific IDs
    let req_message = format!(r#"["REQ", "new_sub", {{"ids": ["{}"]}}]"#, received_event_id);
    ws.send(req_message.into()).await?;

        // Listen for full event data
    let event_data_msg = ws.next().await
        .ok_or_else(|| anyhow::Error::msg("Did not receive full event data"))??;
    info!("Received full event data: {:?}", event_data_msg);

    // Shutdown the relay
    let _res = relay.shutdown_tx.send(());
    let _join_handle = relay.handle.join();

    Ok(())
}


