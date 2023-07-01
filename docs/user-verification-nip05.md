# Author Verification Design Document

The relay will use NIP-05 DNS-based author verification to limit which
authors can publish events to a relay.  This document describes how
this feature will operate.

## Considerations

DNS-based author verification is designed to be deployed in relays that
want to prevent spam, so there should be strong protections to prevent
unauthorized authors from persisting data.  This includes data needed to
verify new authors.

There should be protections in place to ensure the relay cannot be
used to spam or flood other webservers.  Additionally, there should be
protections against server-side request forgery (SSRF).

## Design Overview

### Concepts

All authors are initially "unverified".  Unverified authors that submit
appropriate `NIP-05` metadata events become "candidates" for
verification.  A candidate author becomes verified  when the relay
inspects a kind `0` metadata event for the author with a `nip05` field,
and follows the procedure in `NIP-05` to successfully associate the
author with an internet identifier.

The `NIP-05` procedure verifies an author for a fixed period of time,
configurable by the relay operator.  If this "verification expiration
time" (`verify_expiration`) is exceeded without being refreshed, they
are once again unverified.

Verified authors have their status regularly and automatically updated
through scheduled polling to their verified domain, this process is
"re-verification".  It is performed based on the configuration setting
`verify_update_frequency`, which defines how long the relay waits
between verification attempts (whether the result was success or
failure).

Authors may change their verification data (the internet identifier from
`NIP-05`) with a new metadata event, which then requires
re-verification.  Their old verification remains valid until
expiration.

Performing candidate author verification is a best-effort activity and
may be significantly rate-limited to prevent relays being used to
attack other hosts.  Candidate verification (untrusted authors) should
never impact re-verification (trusted authors).

## Operating Modes

The relay may operate in one of three modes.  "Disabled" performs no
validation activities, and will never permit or deny events based on
an author's NIP-05 metadata.  "Passive" performs NIP-05 validation,
but does not permit or deny events based on the validity or presence
of NIP-05 metadata.  "Enabled" will require current and valid NIP-05
metadata for any events to be persisted.  "Enabled" mode will
additionally consider domain whitelist/blacklist configuration data to
restrict which author's events are persisted.

## Design Details

### Data Storage

Verification is stored in a dedicated table.  This tracks:

* `nip05` identifier
* most recent verification timestamp
* most recent verification failure timestamp
* reference to the metadata event (used for tracking `created_at` and
  `pubkey`)

### Event Handling

All events are first validated to ensure the signature is valid.

Incoming events of kind _other_ than metadata (kind `0`) submitted by
clients will be evaluated as follows.

* If the event's author has a current verification, the event is
  persisted as normal.
* If the event's author has either no verification, or the
  verification is expired, the event is rejected.

If the event is a metadata event, we handle it differently.

We first determine the verification status of the event's pubkey.

* If the event author is unverified, AND the event contains a `nip05`
  key, we consider this a verification candidate.
* If the event author is unverified, AND the event does not contain a
  `nip05` key, this is not a candidate, and the event is dropped.

* If the event author is verified, AND the event contains a `nip05`
  key that is identical to the currently stored value, no special
  action is needed.
* If the event author is verified, AND the event contains a different
  `nip05` than was previously verified, with a more recent timestamp,
  we need to re-verify.
* If the event author is verified, AND the event is missing a `nip05`
  key, and the event timestamp is more recent than what was verified,
  we do nothing.  The current verification will be allowed to expire.

### Candidate Verification

When a candidate verification is requested, a rate limit will be
utilized.  If the rate limit is exceeded, new candidate verification
requests will be dropped.  In practice, this is implemented by a
size-limited channel that drops events that exceed a threshold.

Candidates are never persisted in the database.

### Re-Verification

Re-verification is straightforward when there has been no change to
the `nip05` key.  A new request to the `nip05` domain is performed,
and if successful, the verification timestamp is updated to the
current time.  If the request fails due to a timeout or server error,
the failure timestamp is updated instead.

When the the `nip05` key has changed and this event is more recent, we
will create a new verification record, and delete all other records
for the same name.

Regarding creating new records vs. updating: We never update the event
reference or `nip05` identifier in a verification record. Every update
either reset the last failure or last success timestamp.

### Determining Verification Status

In determining if an event is from a verified author, the following
procedure should be used:

Join the verification table with the event table, to provide
verification data alongside the event `created_at` and `pubkey`
metadata.  Find the most recent verification record for the author,
based on the `created_at` time.

Reject the record if the success timestamp is not within our
configured expiration time.

Reject records with disallowed domains, based on any whitelists or
blacklists in effect.

If a result remains, the author is treated as verified.

This does give a time window for authors transitioning their verified
status between domains.  There may be a period of time in which there
are multiple valid rows in the verification table for a given author.

### Cleaning Up Inactive Verifications

After a author verification has expired, we will continue to check for
it to become valid again.  After a configurable number of attempts, we
should simply forget it, and reclaim the space.

### Addition of Domain Whitelist/Blacklist

A set of whitelisted or blacklisted domains may be provided.  If both
are provided, only the whitelist is used.  In this context, domains
are either "allowed" (present on a whitelist and NOT present on a
blacklist), or "denied" (NOT present on a whitelist and present on a
blacklist).

The processes outlined so far are modified in the presence of these
options:

* Only authors with allowed domains can become candidates for
  verification.
* Verification status queries additionally filter out any denied
  domains.
* Re-verification processes only proceed with allowed domains.

### Integration

We have an existing database writer thread, which receives events and
attempts to persist them to disk.  Once validated and persisted, these
events are broadcast to all subscribers.

When verification is enabled, the writer must check to ensure a valid,
unexpired verification record exists for the author.  All metadata
events (regardless of verification status) are forwarded to a verifier
module.  If the verifier determines a new verification record is
needed, it is also responsible for persisting and broadcasting the
event, just as the database writer would have done.

## Threat Scenarios

Some of these mitigations are fully implemented, others are documented
simply to demonstrate a mitigation is possible.

### Domain Spamming

*Threat*: A author with a high-volume of events creates a metadata event
with a bogus domain, causing the relay to generate significant
unwanted traffic to a target.

*Mitigation*: Rate limiting for all candidate verification will limit
external requests to a reasonable amount.  Currently, this is a simple
delay that slows down the HTTP task.

### Denial of Service for Legitimate Authors

*Threat*: A author with a high-volume of events creates a metadata event
with a domain that is invalid for them, _but which is used by other
legitimate authors_.  This triggers rate-limiting against the legitimate
domain, and blocks authors from updating their own metadata.

*Mitigation*: Rate limiting should only apply to candidates, so any
existing verified authors have priority for re-verification.  New
authors will be affected, as we can not distinguish between the threat
and a legitimate author. _(Unimplemented)_

### Denial of Service by Consuming Storage

*Threat*: A author creates a high volume of random metadata events with
unique domains, in order to cause us to store large amounts of data
for to-be-verified authors.

*Mitigation*: No data is stored for candidate authors.  This makes it
harder for new authors to become verified, but is effective at
preventing this attack.

### Metadata Replay for Verified Author

*Threat*: Attacker replays out-of-date metadata event for a author, to
cause a verification to fail.

*Mitigation*: New metadata events have their signed timestamp compared
against the signed timestamp of the event that has most recently
verified them.  If the metadata event is older, it is discarded.

### Server-Side Request Forgery via Metadata

*Threat*: Attacker includes malicious data in the `nip05` event, which
is used to generate HTTP requests against potentially internal
resources.  Either leaking data, or invoking webservices beyond their
own privileges.

*Mitigation*: Consider detecting and dropping when the `nip05` field
is an IP address.  Allow the relay operator to utilize the `blacklist`
or `whitelist` to constrain hosts that will be contacted.  Most
importantly, the verification process is hardcoded to only make
requests to a known url path
(`.well-known/nostr.json?name=<LOCAL_NAME>`).  The `<LOCAL_NAME>`
component is restricted to a basic ASCII subset (preventing additional
URL components).
