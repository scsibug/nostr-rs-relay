use anyhow::Result;
use std::thread;
use std::time::Duration;

mod common;

#[test]
fn startup() -> Result<()> {
    let relay = common::start_relay()?;
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
    Ok(())
}
