# gRPC Extensions Design Document

The relay will be extensible through gRPC endpoints, definable in the
main configuration file.  These will allow external programs to host
logic for deciding things such as, should this event be persisted,
should this connection be allowed, and should this subscription
request be registered.  The primary goal is allow for relay operator
specific functionality that allows them to serve smaller communities
and reduce spam and abuse.

This will likely evolve substantially, the first goal is to get a
basic one-way service that lets an externalized program decide on
event persistance.  This does not represent the final state of gRPC
extensibility in `nostr-rs-relay`.

## Considerations

Write event latency must not be significantly affected.  However, the
primary reason we are implementing this is spam/abuse protection, so
we are willing to tolerate some increase in latency if that protects
us against outages!

The interface should provide enough information to make simple
decisions, without burdening the relay to do extra queries.  The
decision endpoint will be mostly responsible for maintaining state and
gathering additional details.

## Design Overview

A gRPC server may be defined in the `config.toml` file.  If it exists,
the relay will attempt to connect to it and send a message for each
`EVENT` command submitted by clients.  If a successful response is
returned indicating the event is permitted, the relay continues
processing the event as normal.  All existing whitelist, blacklist,
and `NIP-05` validation checks are still performed and MAY still
result in the event being rejected.  If a successful response is
returned indicated the decision is anything other than permit, then
the relay MUST reject the event, and return a command result to the
user (using `NIP-20`) indicating the event was blocked (optionally
providing a message).

In the event there is an error in the gRPC interface, event processing
proceeds as if gRPC was disabled (fail open).  This allows gRPC
servers to be deployed with minimal chance of causing a full relay
outage.

## Design Details

Currently one procedure call is supported, `EventAdmit`, in the
`Authorization` service.  It accepts the following data in order to
support authorization decisions:

- The event itself
- The client IP that submitted the event
- The client's HTTP origin header, if one exists
- The client's HTTP user agent header, if one exists
- The public key of the client, if `NIP-42` authentication was
  performed (not supported in the relay yet!)
- The `NIP-05` associated with the event's public key, if it is known
  to the relay

A server providing authorization decisions will return the following:

- A decision to permit or deny the event
- An optional message that explains why the event was denied, to be
  transmitted to the client

## Security Issues

There is little attempt to secure this interface, since it is intended
for use processes running on the same host.  It is recommended to
ensure that the gRPC server providing the API is not exposed to the
public Internet.  Authorization server implementations should have
their own security reviews performed.

A slow gRPC server could cause availability issues for event
processing, since this is performed on a single thread.  Avoid any
expensive or long-running processes that could result from submitted
events, since any client can initiate a gRPC call to the service.
