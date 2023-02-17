# Pay to Relay Design Document

The relay with use payment as a form of spam prevention. In order to post to the relay a user must pay a set rate. There is also the option to require a payment for each note posted to the relay. There is no cost to read from the relay.

## Configuration

Currently, [LNBits](https://github.com/lnbits/lnbits) is implemented as the payment processor.  LNBits exposes a simple API for creating invoices, to use this API create a wallet and on the right side find "API info" you will need to add the invoice/read key to this relays config file.

The below configuration will need to be added to config.toml
```
[pay_to_relay]
# Enable pay to relay
enabled = true
# The cost to be admitted to relay
admission_cost = 1000
# The cost in sats per post
cost_per_event = 0
# Url of lnbits api
node_url = "https://<IP of node>:5001/api/v1/payments"
# LNBits api secret
api_secret = "<LNbits api key>"
# Terms of service
terms_message = """This service ....
"""
# Whether or not new sign ups should be allowed 
sign_ups = true 
secret_key = "<nostr secret key to send dms>"
```

The LNBits instance must have a signed HTTPS a self signed certificate will not work.  

## Design Overview

### Concepts

All authors are initially not admitted to write to the relay.  There are two ways to gain access write to the relay. The first is by attempting to post the the relay, upon receiving an event from an author that is not admitted, the relay will send a direct message including the terms of service of the relay and a lighting invoice for the admission cost.  Once this invoice is payed the author can write to the relay. For this method to work the author must be reading from the relay. An author can also pay and accept the terms of service via a webpage `https://<relay-url>/join`.

## Design Details

Authors are stored in a dedicated table. This tracks:

* `pubkey`
* `is_admitted` whether on no the admission invoice has been paid, accepting the terms of service.
* `balance` the current balance in sats of the author, used if there is a cost per post
* `tos_accepted_at` the timestamp of when the author accepted the tos

Invoice information is stored in a dedicated table. This tracks:
* `payment_hash` the payment hash of the lighting invoice
* `pubkey` of the author the invoice is issued to
* `invoice` bolt11 invoice
* `amount` in sats
* `status` (Paid/Unpaid/Expired)
* `description`
* `created_at` timestamp of creation
* `confirmed_at` timestamp of payment

### Event Handling 

If "pay to relay" is enabled, all incoming events are evaluated to determine whether the author is on the relay's whitelist or if they have paid the admission fee and accepted the terms. If "pay per note" is enabled, there is an additional check to ensure that the author has enough balance, which is then reduced by the cost per note. If the author is on the whitelist, this balance check is not necessary.

### Integration

We have an existing database writer thread, which receives events and
attempts to persist them to disk.  Once validated and persisted, these
events are broadcast to all subscribers.

When "pay to relay" is enabled, the writer must check if the author is admitted to post. If the author is not admitted to post the event is forwarded to the payment module. Where an invoice is generated, persisted and broadcast as an direct message to the author.

### Threat Scenarios

Some of these mitigation's are fully implemented, others are documented
simply to demonstrate a mitigation is possible.

### Sign up Spamming

*Threat*: An attacker generates a large number of new pubkeys publishing to the relays. Causing a large number of new invoices to be created for each new pubkey.

*Mitigation*: Rate limit number of new sign ups

### Admitted Author Spamming 

*Threat*: An attacker gains write access by paying the admission fee, and then floods the relay with a large number of spam events.

*Mitigation*: The attacker's admission can be revoked and their admission fee will not be refunded. Enabling "cost per event" and increasing the admission cost can also discourage this type of behavior.

