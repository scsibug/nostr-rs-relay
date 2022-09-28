use anyhow::Result;

use std::thread;
use std::time::Duration;

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
