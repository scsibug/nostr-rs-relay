# Reverse Proxy Setup Guide

It is recommended to run `nostr-rs-relay` behind a reverse proxy such
as `haproxy` or `nginx` to provide TLS termination.  A simple example
of an `haproxy` configuration is documented here.

## Minimal HAProxy Configuration

Assumptions:

* HAProxy version is `2.4.10` or greater (older versions not tested).
* Hostname for the relay is `relay.example.com`.
* Your relay should be available over wss://relay.example.com
* Your (NIP-11) relay info page should be available on https://relay.example.com
* SSL certificate is located in `/etc/certs/example.com.pem`.
* Relay is running on port 8080.
* Limit connections to 400 concurrent.
* HSTS (HTTP Strict Transport Security) is desired.
* Only TLS 1.2 or greater is allowed.

```
global
    ssl-default-bind-ciphersuites TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:TLS_CHACHA20_POLY1305_SHA256
    ssl-default-bind-options prefer-client-ciphers no-sslv3 no-tlsv10 no-tlsv11 no-tls-tickets

frontend fe_prod
    mode    http
    bind    :443 ssl crt /etc/certs/example.com.pem alpn h2,http/1.1
    bind    :80
    http-request set-header X-Forwarded-Proto https if { ssl_fc }
    redirect scheme https code 301 if !{ ssl_fc }
    acl host_relay hdr(host) -i relay.example.com
    use_backend relay if host_relay
    # HSTS (1 year)
    http-response set-header Strict-Transport-Security max-age=31536000

backend relay
    mode http
    option tcp-check
    default-server maxconn 400 check inter 20s fastinter 1s
    server relay 127.0.0.1:8080
```

### Notes

You may experience WebSocket connection problems with Firefox if
HTTP/2 is enabled, for older versions of HAProxy (2.3.x).  Either
disable HTTP/2 (`h2`), or upgrade HAProxy.
