# [nostr-rs-relay](https://git.sr.ht/~gheartsfield/nostr-rs-relay)

This is a [nostr](https://github.com/fiatjaf/nostr) relay, written in
Rust.  It currently supports the entire relay protocol, and has a
SQLite persistence layer.

The project master repository is available on
[sourcehut](https://sr.ht/~gheartsfield/nostr-rs-relay/), and is
mirrored on [GitHub](https://github.com/scsibug/nostr-rs-relay).

## Quick Start

The provided `Dockerfile` will compile and build the server application.  Use a bind mount to store the SQLite database outside of the container image, and map the container's 8080 port to a host port (8090 in the example below).

```console
$ docker build -t nostr-rs-relay .
$ docker run -p 8090:8080 --mount src=$(pwd)/nostr_data,target=/usr/src/app/db,type=bind nostr-rs-relay
[2021-12-12T04:20:47Z INFO  nostr_rs_relay] Listening on: 0.0.0.0:8080
[2021-12-12T04:20:47Z INFO  nostr_rs_relay::db] Opened database for writing
[2021-12-12T04:20:47Z INFO  nostr_rs_relay::db] init completed
```

Use a `nostr` client such as [`noscl`](https://github.com/fiatjaf/noscl) to publish and query events.

```console
$ noscl publish "hello world"
Sent to 'ws://localhost:8090'.
Seen it on 'ws://localhost:8090'.
$ noscl home
Text Note [81cf...2652] from 296a...9b92 5 seconds ago
  hello world
```

License
---
This project is MIT licensed.
