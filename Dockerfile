FROM docker.io/library/rust:1-bookworm as builder

RUN apt-get update && \
    apt-get install -y cmake protobuf-compiler && \
    rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-auditable

RUN cargo new --bin nostr-rs-relay
WORKDIR /nostr-rs-relay

COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock
RUN cargo auditable build --release --locked

RUN rm src/*.rs

COPY ./src ./src
COPY ./proto ./proto
COPY ./build.rs ./build.rs
COPY config.toml /usr/src/app/config.toml

RUN rm ./target/release/deps/nostr*relay*
RUN cargo auditable build --release --locked

FROM docker.io/library/debian:bookworm-slim

ARG APP=/usr/src/app
ARG APP_DATA=/usr/src/app/db

RUN apt-get update && \
    apt-get install -y ca-certificates tzdata sqlite3 libc6 && \
    rm -rf /var/lib/apt/lists/*

EXPOSE 7777

ENV TZ=Etc/UTC \
    RUST_LOG=info,nostr_rs_relay=info \
    APP_DATA=${APP_DATA}

RUN mkdir -p ${APP} && \
    mkdir -p ${APP_DATA}

COPY --from=builder /nostr-rs-relay/target/release/nostr-rs-relay ${APP}/nostr-rs-relay
COPY --from=builder /usr/src/app/config.toml ${APP}/config.toml

WORKDIR ${APP}

CMD ./nostr-rs-relay --db ${APP_DATA}