# Reverse Proxy Setup Guide

It is recommended to run `nostr-rs-relay` behind a reverse proxy such
as `haproxy`, `nginx` or `traefik` to provide TLS termination.  Simple examples
for `haproxy`, `nginx` and `traefik` configurations are documented here.

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
    timeout connect 5s
    timeout client 50s
    timeout server 50s
    timeout tunnel 1h
    timeout client-fin 30s
    option tcp-check
    default-server maxconn 400 check inter 20s fastinter 1s
    server relay 127.0.0.1:8080
```

### HAProxy Notes

You may experience WebSocket connection problems with Firefox if
HTTP/2 is enabled, for older versions of HAProxy (2.3.x).  Either
disable HTTP/2 (`h2`), or upgrade HAProxy.

## Bare-bones Nginx Configuration

Assumptions:

* `Nginx` version is `1.18.0` (other versions not tested).
* Hostname for the relay is `relay.example.com`.
* SSL certificate and key are located at `/etc/letsencrypt/live/relay.example.com/`.
* Relay is running on port `8080`.

```
http {
    server {
        listen 443 ssl;
        server_name relay.example.com;
        ssl_certificate /etc/letsencrypt/live/relay.example.com/fullchain.pem;
        ssl_certificate_key /etc/letsencrypt/live/relay.example.com/privkey.pem;
        ssl_protocols TLSv1.3 TLSv1.2;
        ssl_prefer_server_ciphers on;
        ssl_ecdh_curve secp521r1:secp384r1;
        ssl_ciphers EECDH+AESGCM:EECDH+AES256;

        # Optional Diffie-Helmann parameters
        # Generate with openssl dhparam -out /etc/ssl/certs/dhparam.pem 4096
        #ssl_dhparam /etc/ssl/certs/dhparam.pem;

        ssl_session_cache shared:TLS:2m;
        ssl_buffer_size 4k;

        # OCSP stapling
        ssl_stapling on;
        ssl_stapling_verify on;
        resolver 1.1.1.1 1.0.0.1 [2606:4700:4700::1111] [2606:4700:4700::1001]; # Cloudflare

        # Set HSTS to 365 days
        add_header Strict-Transport-Security 'max-age=31536000; includeSubDomains; preload' always;
        keepalive_timeout 70;

        location / {
            proxy_pass http://localhost:8080;
            proxy_http_version 1.1;
            proxy_read_timeout 1d;
            proxy_send_timeout 1d;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection "Upgrade";
            proxy_set_header Host $host;
        }
    }
}
```

### Nginx Notes

The above configuration was tested on `nginx` `1.18.0` on `Ubuntu` `20.04` and `22.04`

For help installing `nginx` on `Ubuntu`, see [this guide](https://www.digitalocean.com/community/tutorials/how-to-install-nginx-on-ubuntu-20-04).

For guidance on using `letsencrypt` to obtain a cert on `Ubuntu`, including an `nginx` plugin, see [this post](https://www.digitalocean.com/community/tutorials/how-to-secure-nginx-with-let-s-encrypt-on-ubuntu-20-04).


## Example Traefik Configuration

Assumptions:

* `Traefik` version is `2.9` (other versions not tested).
* `Traefik` is used for provisioning of Let's Encrypt certificates.
* `Traefik` is running in `Docker`, using `docker compose` and labels for the static configuration. An equivalent setup useing a Traefik config file is possible too (but not covered here).
* Strict Transport Security is enabled.
* Hostname for the relay is `relay.example.com`, email adres for ACME certificates provider is `name@example.com`.
* ipv6 is enabled, a viable private ipv6 subnet is specified in the example below. 
* Relay is running on port `8080`.

```
version: '3'

networks:
  nostr:
    enable_ipv6: true
    ipam:
      config:
        - subnet: fd00:db8:a::/64
          gateway: fd00:db8:a::1

services:
  traefik:
    image: traefik:v2.9
    networks:
      nostr:
    command:
      - "--log.level=ERROR"
      # letsencrypt configuration
      - "--certificatesResolvers.http.acme.email==name@example.com"
      - "--certificatesResolvers.http.acme.storage=/certs/acme.json"
      - "--certificatesResolvers.http.acme.httpChallenge.entryPoint=http"
      # define entrypoints
      - "--entryPoints.http.address=:80"
      - "--entryPoints.http.http.redirections.entryPoint.to=https"
      - "--entryPoints.http.http.redirections.entryPoint.scheme=https"
      - "--entryPoints.https.address=:443"
      - "--entryPoints.https.forwardedHeaders.insecure=true"
      - "--entryPoints.https.proxyProtocol.insecure=true"
      # docker provider (get configuration from container labels)
      - "--providers.docker.endpoint=unix:///var/run/docker.sock"
      - "--providers.docker.exposedByDefault=false"
      - "--providers.file.directory=/config"
      - "--providers.file.watch=true"
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - "/var/run/docker.sock:/var/run/docker.sock:ro"
      - "$(pwd)/traefik/certs:/certs"
      - "$(pwd)/traefik/config:/config"
    logging:
      driver: "local"
    restart: always

  # example nostr config. only labels: section is relevant for Traefik config
  nostr:
   image: nostr-rs-relay:latest
   container_name: nostr-relay
   networks:
     nostr:
   restart: always
   user: 100:100
   volumes:
   - '$(pwd)/nostr/data:/usr/src/app/db:Z'
   - '$(pwd)/nostr/config/config.toml:/usr/src/app/config.toml:ro,Z'
   labels:
     - "traefik.enable=true"
     - "traefik.http.routers.nostr.entrypoints=https"
     - "traefik.http.routers.nostr.rule=Host(`relay.example.com`)"
     - "traefik.http.routers.nostr.tls.certresolver=http"
     - "traefik.http.routers.nostr.service=nostr"
     - "traefik.http.services.nostr.loadbalancer.server.port=8080"
     - "traefik.http.services.nostr.loadbalancer.passHostHeader=true"
     - "traefik.http.middlewares.nostr.headers.sslredirect=true"
     - "traefik.http.middlewares.nostr.headers.stsincludesubdomains=true"
     - "traefik.http.middlewares.nostr.headers.stspreload=true"
     - "traefik.http.middlewares.nostr.headers.stsseconds=63072000"
     - "traefik.http.routers.nostr.middlewares=nostr"
```

### Traefik Notes

Traefik will take care of the provisioning and renewal of certificates. In case of an ipv4-only relay, simply detele the `enable_ipv6:` and `ipam:` entries in the `networks:` section of the docker-compose file.