# [nostr-rs-relay](https://git.sr.ht/~gheartsfield/nostr-rs-relay)

This is a [nostr](https://github.com/fiatjaf/nostr) relay, written in
Rust.  It currently supports the entire relay protocol, and has a
SQLite persistence layer.

The project master repository is available on
[sourcehut](https://sr.ht/~gheartsfield/nostr-rs-relay/), and is
mirrored on [GitHub](https://github.com/scsibug/nostr-rs-relay).

## Features

NIPs with a relay-specific implementation are listed here.

- [x] NIP-01: Core event model
- [x] NIP-01: Hide old metadata events
- [x] NIP-01: Id/Author prefix search (_experimental_)
- [x] NIP-02: Hide old contact list events
- [ ] NIP-03: OpenTimestamps
- [x] NIP-05: Mapping Nostr keys to DNS identifiers
- [x] NIP-09: Event deletion
- [x] NIP-11: Relay information document
- [x] NIP-12: Generic tag search (_experimental_)
- [x] NIP-15: End of stored events notice
- [x] NIP-16: Replaceable and ephemeral events
- [x] NIP-22: Event `created_at` limits (_future-dated events only_)

## Quick Start

The provided `Dockerfile` will compile and build the server
application.  Use a bind mount to store the SQLite database outside of
the container image, and map the container's 8080 port to a host port
(7000 in the example below).

```console
$ docker build -t nostr-rs-relay .

$ docker run -it -p 7000:8080 \
  --mount src=$(pwd)/data,target=/usr/src/app/db,type=bind nostr-rs-relay

[2021-12-31T19:58:31Z INFO  nostr_rs_relay] listening on: 0.0.0.0:8080
[2021-12-31T19:58:31Z INFO  nostr_rs_relay::db] opened database "/usr/src/app/db/nostr.db" for writing
[2021-12-31T19:58:31Z INFO  nostr_rs_relay::db] DB version = 2
```

Use a `nostr` client such as
[`noscl`](https://github.com/fiatjaf/noscl) to publish and query
events.

```console
$ noscl publish "hello world"
Sent to 'ws://localhost:8090'.
Seen it on 'ws://localhost:8090'.
$ noscl home
Text Note [81cf...2652] from 296a...9b92 5 seconds ago
  hello world
```

A pre-built container is also available on DockerHub:
https://hub.docker.com/r/scsibug/nostr-rs-relay

## Configuration

The sample [`config.toml`](config.toml) file demonstrates the
configuration available to the relay.  This file is optional, but may
be mounted into a docker container like so:

```console
$ docker run -it -p 7000:8080 \
  --mount src=$(pwd)/config.toml,target=/usr/src/app/config.toml,type=bind \
  --mount src=$(pwd)/data,target=/usr/src/app/db,type=bind \
  nostr-rs-relay
```

Options include rate-limiting, event size limits, and network address
settings.

## Reverse Proxy Configuration

For examples of putting the relay behind a reverse proxy (for TLS
termination, load balancing, and other features), see [Reverse
Proxy](reverse-proxy.md).

## Dev Channel

For development discussions, please feel free to use the [sourcehut
mailing list](https://lists.sr.ht/~gheartsfield/nostr-rs-relay-devel).
Or, drop by the [Nostr Telegram Channel](https://t.me/nostr_protocol).

To chat about `nostr-rs-relay` on `nostr` itself; visit our channel on [anigma](https://anigma.io/) or another client that supports [NIP-28](https://github.com/nostr-protocol/nips/blob/master/28.md) chats:
 * `2ad246a094fee48c6e455dd13d759d5f41b5a233120f5719d81ebc1935075194`

License
---
This project is MIT licensed.
