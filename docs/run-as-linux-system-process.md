# Run as a linux system process

Docker makes it easy to spin up and down environments but it's also possible to run `nostr-rs-relay` as a systemd linux process.
This guide assumes you're on a Linux machine and that Rust is already installed.

## Instructions

### Build nostr-rs-relay from source
Start by building the application from source. Here is how to do that:
1. `git clone https://github.com/scsibug/nostr-rs-relay.git`
2. `cd nostr-rs-relay`
3. `cargo build --release`

### Place the files where they belong
We want to place the nostr-rs-relay binary and the config.toml file where they belong. While still in the root level of the nostr-rs-relay folder you cloned in last step, run the following commands:
1. `sudo cp target/release/nostr-rs-relay /usr/local/bin/`
2. `sudo mkdir /etc/nostr-rs-relay`
2. `sudo cp config.toml /etc/nostr-rs-relay`

### Create the Systemd service file
We need to create a new Systemd service file. These files are placed in the `/etc/systemd/system/` folder where you will find many other services running.

1. `sudo vim /etc/systemd/system/nostr-rs-relay.service`
2. Paste in the contents of [this service file](../contrib/nostr-rs-relay.service). Remember to replace the `User` value with your own username.
3. Save the file and exit your text editor


### Run the service
To get the service running, we need to reload the systemd daemon and enable the service.

1. `sudo systemctl daemon-reload`
2. `sudo systemctl start nostr-rs-relay.service`
3. `sudo systemctl status nostr-rs-relay.service`


### Tips

#### Logs
The application will write logs to the journal. To read it, execute `sudo journalctl -f -u nostr-rs-relay`
